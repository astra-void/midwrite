//! Lowering of TypeScript functions to the concurrency IR of
//! `midwrite-model` (Phase 6, stage G).
//!
//! This is a *separate, opt-in* frontend: `midwrite check` never calls it.
//! It lowers one named function (inlining helpers up to a depth bound) into
//! the small operation language the model checker explores: Drizzle
//! reads/writes/transactions, configured external effects, `Promise.all`
//! concurrency, local computation and control flow. Everything outside that
//! subset becomes `Op::Unknown` (statements) or `Expr::Unknown`
//! (expressions) with a precise detail and source reference. Unknown
//! behavior is never dropped and never turned into a no-op.
//!
//! The supported subset is documented in `docs/phase6/CONCURRENT-IR.md`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, ArrowFunctionBody, BindingPattern, ChainElement,
    ClassElement, Declaration, ExportDefaultDeclarationKind, Expression, FormalParameters,
    Function, FunctionBody, ImportDeclarationSpecifier, MethodDefinitionKind, ModuleExportName,
    ObjectPropertyKind, Program, Statement, VariableDeclarator,
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};

use crate::semantic::{Root, member_path as callee_text, property_key as static_key, unwind};
use oxc_syntax::operator::{AssignmentOperator, BinaryOperator, LogicalOperator, UnaryOperator};

use midwrite_core::config::Config;
use midwrite_core::location::LineIndex;
use midwrite_model::ir::{
    Conn, Isolation, Join, JoinKind, LockMode, OnConflict, Op, Origin, ProjItem, SourceRef, Step,
    Stmt, StmtKind,
};
use midwrite_model::provider::{
    ActorProvider, EntryInfo, ExternalBinding, ExtractRequest, Extracted, InvocationContract,
    TableBinding, TransactionRetryBinding, WrapperFacts,
};
use midwrite_model::value::{BinOp, Expr, Value};

use crate::context::{ProjectContext, Resolved};
use crate::handles::{HandleFact, Provenance, Routing};
use crate::identity;
use crate::resolve::{Resolution, absolute};
use crate::wrapper;

/// Phase 14: the JavaScript value domain (`ExtractRequest::js_values`).
mod js_values;
/// Phase 16: bounded loops, caught errors, request bodies.
mod loops;

/// Lowers functions of a TypeScript project to the concurrency IR.
#[derive(Clone, Debug)]
pub struct TsActorProvider {
    root: PathBuf,
    receivers: Vec<String>,
    /// Phase 12: one [`ProjectContext`] per project root, shared by every
    /// extraction made through this provider (and its clones).
    contexts: Arc<Mutex<HashMap<PathBuf, Arc<ProjectContext>>>>,
}

impl TsActorProvider {
    /// A provider for the project at `root`. Database receiver names are
    /// read from the project's configuration file (`midwrite.json`, or a
    /// legacy name) when it exists (plain identifiers only); the default is
    /// `db`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let mut receivers = vec!["db".to_owned()];
        if let Some(found) = midwrite_core::config::discover_config(&root)
            && let Ok(config) = Config::load(&found.path)
        {
            let names: Vec<String> = config
                .database_receivers
                .iter()
                .map(|rule| rule.name().to_owned())
                .filter(|name| {
                    name.chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
                })
                .collect();
            if !names.is_empty() {
                receivers = names;
            }
        }
        Self {
            root,
            receivers,
            contexts: Arc::default(),
        }
    }

    /// The shared analysis context of the project at `root`, built on first
    /// use.
    pub fn context(&self, root: &Path) -> Arc<ProjectContext> {
        let key = absolute(root);
        let mut contexts = self.contexts.lock().expect("contexts lock");
        contexts
            .entry(key.clone())
            .or_insert_with(|| Arc::new(ProjectContext::new(&key)))
            .clone()
    }

    /// Override the identifiers treated as the root database handle.
    pub fn with_receivers(mut self, receivers: Vec<String>) -> Self {
        self.receivers = receivers;
        self
    }
}

impl ActorProvider for TsActorProvider {
    fn resolve_reference(&self, root: &Path, file: &str, local: &str) -> Option<(String, String)> {
        let root = if root.as_os_str().is_empty() {
            self.root.as_path()
        } else {
            root
        };
        let ctx = self.context(root);
        let index = ctx.index_of(&absolute(&root.join(file)))?;
        match ctx.resolve_local(index, local).ok()? {
            Resolved::Binding { file, local } => Some((ctx.rel(&ctx.files[file]), local)),
            Resolved::Namespace { .. } => None,
        }
    }

    fn extract_schema(
        &self,
        root: &Path,
        file: &str,
    ) -> Result<midwrite_model::schema_facts::ExtractedSchema, String> {
        let root = if root.as_os_str().is_empty() {
            self.root.as_path()
        } else {
            root
        };
        let mut extracted = crate::schema::extract_schema(root, file)?;
        // Phase 12: `$defaultFn(helper)` with `helper` imported from a
        // project module: classify the helper's own definition.
        let ctx = self.context(root);
        let here = absolute(&root.join(file));
        for table in &mut extracted.tables {
            for column in &mut table.columns {
                let Some(generation) = &mut column.generation else {
                    continue;
                };
                let Some((specifier, name)) = generation.callback_import.clone() else {
                    continue;
                };
                let resolved = ctx.follow(&here, &specifier).and_then(|target| {
                    match ctx.resolve_export(target, &name)? {
                        Resolved::Binding { file, local } => Ok((file, local)),
                        Resolved::Namespace { .. } => Err(format!("`{name}` is a namespace")),
                    }
                });
                let outcome = resolved.and_then(|(target, local)| {
                    let abs = ctx.files[target].clone();
                    let text = ctx.source(&abs)?;
                    let allocator = Allocator::default();
                    let program = load(&allocator, &abs, &text)?;
                    crate::generators::exported_callback(&program, &local)
                        .map(|g| (g, format!("{}#{local}", ctx.rel(&abs))))
                });
                match outcome {
                    Ok((g, at)) => {
                        generation.class =
                            midwrite_model::schema_facts::GeneratorClass::ApplicationGenerated;
                        generation.generator = Some(g.to_owned());
                        generation.detail = format!("{} (the helper `{at}`)", generation.detail);
                    }
                    Err(why) => {
                        generation.detail = format!("{}: {why}", generation.detail);
                    }
                }
            }
        }
        Ok(extracted)
    }

    fn extract(&self, request: &ExtractRequest) -> Result<Extracted, String> {
        let root = if request.root.as_os_str().is_empty() {
            self.root.clone()
        } else {
            request.root.clone()
        };
        let selector = request
            .selector
            .as_deref()
            .map(identity::Selector::parse)
            .transpose()?;
        let file = selector
            .as_ref()
            .map_or_else(|| request.file.clone(), |s| s.module.clone());
        let main_abs = absolute(&root.join(&file));
        let mut ctx = self.context(&root);
        if ctx.index_of(&main_abs).is_none() {
            // A file outside the discovered modules (excluded by the
            // project's configuration): a private context that includes it.
            let mut files = ctx.files.clone();
            files.push(main_abs.clone());
            ctx = Arc::new(ProjectContext::with_files(ctx.root.clone(), files));
        }
        let source = ctx
            .source(&main_abs)
            .map_err(|_| format!("cannot read `{file}`"))?;
        let allocator = Allocator::default();
        let parsed = load(&allocator, &main_abs, &source).map_err(|e| format!("`{file}`: {e}"))?;
        let module = Module::new(&main_abs, file.clone(), &source, &parsed);
        let found = selector
            .as_ref()
            .map(|sel| identity::resolve(&parsed, &module.lines, sel))
            .transpose()?;
        let info = match &found {
            Some(found) => fn_info_of(found),
            None => find_function(&module, &request.function).ok_or_else(|| {
                format!(
                    "function `{}` was not found in `{}`",
                    request.function, request.file
                )
            })?,
        };

        let mut lowerer = Lowerer {
            request,
            receivers: &self.receivers,
            root: ctx.root.clone(),
            ctx: ctx.clone(),
            codes: vec![Vec::new()],
            cur: 0,
            frames: Vec::new(),
            counter: 0,
            tx_open: 0,
            assumptions: Vec::new(),
            js_facts: Default::default(),
            qualify: false,
            param_types: HashMap::new(),
            pending_this: None,
            prov: Provenance::new(ctx.clone(), &request.replica_factories),
            replicating: None,
            alias_hops: 0,
            cached_calls: HashSet::new(),
            pending_scopes: None,
            const_decl: false,
            pred_helpers: HashMap::new(),
            p16: Default::default(),
        };
        for error in lowerer.prov.errors.clone() {
            lowerer.assume(format!("declared replica factory not applied: {error}"));
        }
        let entry = match (&found, &selector) {
            (Some(found), Some(selector)) => {
                Some(lowerer.entry_info(&module, found, selector, request.contract.clone())?)
            }
            _ => None,
        };
        let params = lowerer.lower_main(&module, &info, request);
        Ok(Extracted {
            params,
            codes: lowerer.codes,
            assumptions: lowerer.assumptions,
            entry,
        })
    }
}

/// The name `midwrite check` gives a selected function: its binding for a
/// plain `#binding` selector, `Class.method` for methods, else the
/// position-based `<anonymous@line:column>`. Linking uses the anchor; the
/// name is for display.
fn static_name(found: &identity::Found<'_>, selector: &identity::Selector) -> String {
    if let identity::FnNode::Function(f) = found.node
        && let Some(id) = &f.id
    {
        return id.name.as_str().to_owned();
    }
    match &selector.target {
        identity::Target::Path { binding, steps } => match steps.last() {
            None => binding.clone(),
            Some(identity::Step::Method(m) | identity::Step::Static(m)) if steps.len() == 1 => {
                format!("{binding}.{m}")
            }
            _ => identity::anonymous_name(found.line, found.column),
        },
        identity::Target::Anchor { .. } => identity::anonymous_name(found.line, found.column),
    }
}

fn fn_info_of<'a>(found: &identity::Found<'a>) -> FnInfo<'a> {
    let name = match found.node {
        identity::FnNode::Function(f) => f.id.as_ref().map(|id| id.name.as_str().to_owned()),
        identity::FnNode::Arrow(_) => None,
    }
    .unwrap_or_else(|| identity::anonymous_name(found.line, found.column));
    FnInfo {
        name,
        is_async: found.node.is_async(),
        params: found.node.params(),
        body: match found.node.body() {
            Ok(b) => FnBody::Block(b),
            Err(e) => FnBody::Expr(e),
        },
        span: found.span,
    }
}

fn load<'a>(allocator: &'a Allocator, path: &Path, source: &'a str) -> Result<Program<'a>, String> {
    let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::ts());
    let parsed = Parser::new(allocator, source, source_type).parse();
    if let Some(error) = parsed.diagnostics.errors().next() {
        return Err(format!("parse error: {}", error.message));
    }
    if parsed.fatal_error {
        return Err("parse error: fatal".to_owned());
    }
    Ok(parsed.program)
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

// ---------------------------------------------------------------------------
// Modules and functions
// ---------------------------------------------------------------------------

struct Module<'a> {
    abs: PathBuf,
    /// Path shown in source references (relative to the project root).
    rel: String,
    source: &'a str,
    lines: LineIndex<'a>,
    program: &'a Program<'a>,
}

impl<'a> Module<'a> {
    fn new(abs: &Path, rel: String, source: &'a str, program: &'a Program<'a>) -> Self {
        Self {
            abs: abs.to_path_buf(),
            rel,
            source,
            lines: LineIndex::new(source),
            program,
        }
    }
}

enum FnBody<'a> {
    Block(&'a FunctionBody<'a>),
    Expr(&'a Expression<'a>),
}

struct FnInfo<'a> {
    name: String,
    is_async: bool,
    params: &'a FormalParameters<'a>,
    body: FnBody<'a>,
    span: Span,
}

fn info_of_function<'a>(name: &str, function: &'a Function<'a>) -> Option<FnInfo<'a>> {
    let body = function.body.as_deref()?;
    Some(FnInfo {
        name: name.to_owned(),
        is_async: function.r#async,
        params: &function.params,
        body: FnBody::Block(body),
        span: function.span,
    })
}

fn info_of_expression<'a>(name: &str, expression: &'a Expression<'a>) -> Option<FnInfo<'a>> {
    match expression.get_inner_expression() {
        Expression::ArrowFunctionExpression(arrow) => info_of_arrow(name, arrow),
        Expression::FunctionExpression(function) => info_of_function(name, function),
        _ => None,
    }
}

fn info_of_arrow<'a>(
    name: &str,
    arrow: &'a oxc_ast::ast::ArrowFunctionExpression<'a>,
) -> Option<FnInfo<'a>> {
    Some(FnInfo {
        name: name.to_owned(),
        is_async: arrow.r#async,
        params: &arrow.params,
        body: match &arrow.body {
            ArrowFunctionBody::FunctionBody(body) => FnBody::Block(body),
            other => FnBody::Expr(other.as_expression()?),
        },
        span: arrow.span,
    })
}

/// Phase 14: the function node (arrow function or function expression)
/// whose span is exactly `span`, with the program's lifetime.
fn closure_at<'a>(program: &'a Program<'a>, span: Span, name: &str) -> Option<FnInfo<'a>> {
    let semantic = oxc_semantic::SemanticBuilder::new()
        .with_build_nodes(true)
        .build(program)
        .semantic;
    for node in semantic.nodes().iter() {
        match node.kind() {
            oxc_ast::AstKind::ArrowFunctionExpression(a) if a.span == span => {
                return info_of_arrow(name, a);
            }
            oxc_ast::AstKind::Function(f) if f.span == span => {
                return info_of_function(name, f);
            }
            _ => {}
        }
    }
    None
}

fn declaration_info<'a>(name: &str, declaration: &'a Declaration<'a>) -> Option<FnInfo<'a>> {
    match declaration {
        Declaration::FunctionDeclaration(function) => {
            let id = function.id.as_ref()?;
            (id.name.as_str() == name)
                .then(|| info_of_function(name, function))
                .flatten()
        }
        Declaration::VariableDeclaration(variables) => {
            for declarator in &variables.declarations {
                if let BindingPattern::BindingIdentifier(id) = &declarator.id
                    && id.name.as_str() == name
                    && let Some(init) = &declarator.init
                {
                    return info_of_expression(name, init);
                }
            }
            None
        }
        _ => None,
    }
}

/// A function declared at the top level of the module (exported or not).
fn find_function<'a>(module: &Module<'a>, name: &str) -> Option<FnInfo<'a>> {
    let program: &'a Program<'a> = module.program;
    for statement in &program.body {
        let found = match statement {
            Statement::FunctionDeclaration(function) => function
                .id
                .as_ref()
                .filter(|id| id.name.as_str() == name)
                .and_then(|_| info_of_function(name, function)),
            Statement::VariableDeclaration(variables) => {
                variables.declarations.iter().find_map(|declarator| {
                    if let BindingPattern::BindingIdentifier(id) = &declarator.id
                        && id.name.as_str() == name
                    {
                        declarator
                            .init
                            .as_ref()
                            .and_then(|init| info_of_expression(name, init))
                    } else {
                        None
                    }
                })
            }
            Statement::ExportDeclaration(export) => declaration_info(name, &export.declaration),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// The function exported from the module under `export_name`.
fn find_export<'a>(module: &Module<'a>, export_name: &str) -> Result<FnInfo<'a>, String> {
    let program: &'a Program<'a> = module.program;
    for statement in &program.body {
        match statement {
            Statement::ExportDeclaration(export) => {
                if let Some(info) = declaration_info(export_name, &export.declaration) {
                    return Ok(info);
                }
            }
            Statement::ExportFromDeclaration(export) => {
                for specifier in &export.specifiers {
                    if specifier.exported.name().as_str() == export_name {
                        return Err(format!(
                            "`{export_name}` is a re-export, which is not followed"
                        ));
                    }
                }
            }
            Statement::ExportNamedDeclaration(export) => {
                for specifier in &export.specifiers {
                    if specifier.exported.name().as_str() == export_name
                        && let ModuleExportName::IdentifierReference(local) = &specifier.local
                        && let Some(info) = find_function(module, local.name.as_str())
                    {
                        return Ok(info);
                    }
                }
            }
            Statement::ExportDefaultDeclaration(export) if export_name == "default" => {
                match &export.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                        if let Some(info) = info_of_function("default", function) {
                            return Ok(info);
                        }
                    }
                    other => {
                        if let Some(expression) = other.as_expression() {
                            if let Expression::Identifier(id) = expression.get_inner_expression()
                                && let Some(info) = find_function(module, id.name.as_str())
                            {
                                return Ok(info);
                            }
                            if let Some(info) = info_of_expression("default", expression) {
                                return Ok(info);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Err(format!(
        "no exported function `{export_name}` in `{}`",
        module.rel
    ))
}

/// Phase 12: the initializer of a top-level `const name = ...` (exported or
/// not).
/// Phase 16: the strings of the `const` array `name` denotes in `m` (a
/// module-level `const NAME = ["a", "b"] as const`, here or imported through
/// the project's resolver; spreads of such arrays included), or `None`.
fn const_string_array(
    ctx: &ProjectContext,
    m: &Module<'_>,
    name: &str,
    depth: u32,
) -> Option<Vec<String>> {
    fn strings(
        ctx: &ProjectContext,
        m: &Module<'_>,
        init: &Expression<'_>,
        depth: u32,
    ) -> Option<Vec<String>> {
        let Expression::ArrayExpression(items) = init.get_inner_expression() else {
            return None;
        };
        let mut out = Vec::new();
        for item in &items.elements {
            match item {
                oxc_ast::ast::ArrayExpressionElement::SpreadElement(s) => {
                    let Expression::Identifier(id) = s.argument.get_inner_expression() else {
                        return None;
                    };
                    out.extend(const_string_array(ctx, m, id.name.as_str(), depth + 1)?);
                }
                _ => match item.as_expression().map(|e| e.get_inner_expression()) {
                    Some(Expression::StringLiteral(s)) => out.push(s.value.as_str()?.to_owned()),
                    _ => return None,
                },
            }
        }
        Some(out)
    }
    if depth > 4 {
        return None;
    }
    if let Some(init) = const_initializer(m.program, name) {
        return strings(ctx, m, init, depth);
    }
    let file = ctx.index_of(&m.abs)?;
    let Ok(Resolved::Binding { file, local }) = ctx.resolve_local(file, name) else {
        return None;
    };
    let abs = ctx.files[file].clone();
    let text = ctx.source(&abs).ok()?;
    let allocator = Allocator::default();
    let program = load(&allocator, &abs, &text).ok()?;
    let module = Module::new(&abs, ctx.rel(&abs), &text, &program);
    let init = const_initializer(module.program, &local)?;
    strings(ctx, &module, init, depth)
}

fn const_initializer<'a>(program: &'a Program<'a>, name: &str) -> Option<&'a Expression<'a>> {
    for statement in &program.body {
        let declaration = match statement {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            other => other.as_declaration(),
        };
        if let Some(Declaration::VariableDeclaration(v)) = declaration
            && v.kind.is_const()
        {
            for d in &v.declarations {
                if let BindingPattern::BindingIdentifier(id) = &d.id
                    && id.name.as_str() == name
                {
                    return d.init.as_ref();
                }
            }
        }
    }
    None
}

/// Phase 13: `const name = other` at the top level (exported or not): a
/// function alias (`export const canManage = isAdmin`). Only a plain
/// identifier initializer of a `const`; a call (`wrap(f)`) is not an alias.
fn alias_target(program: &Program<'_>, name: &str) -> Option<String> {
    match const_initializer(program, name)?.get_inner_expression() {
        Expression::Identifier(id) if id.name.as_str() != name => Some(id.name.as_str().to_owned()),
        _ => None,
    }
}

/// Phase 13: the import specifier a value is built from: `z.object({...})
/// .strict()` whose chain starts at an imported identifier (named, default
/// or namespace import). The caller checks that it is a package.
fn package_chain_root(module: &Module<'_>, init: &Expression<'_>) -> Option<String> {
    let mut e = init.get_inner_expression();
    loop {
        e = match e {
            Expression::CallExpression(c) => c.callee.get_inner_expression(),
            Expression::StaticMemberExpression(m) => m.object.get_inner_expression(),
            Expression::Identifier(id) => {
                return match find_import(module, id.name.as_str())? {
                    ImportRef::Named { source, .. }
                    | ImportRef::Default { source }
                    | ImportRef::Namespace { source } => Some(source),
                };
            }
            _ => return None,
        };
    }
}

/// Phase 13: `const name = cache(<function>)` with `cache` imported from
/// `react` (a verified built-in higher-order contract, `crate::packages`):
/// the wrapped function.
fn react_cache_fn<'a>(module: &Module<'a>, name: &str) -> Option<FnInfo<'a>> {
    let program: &'a Program<'a> = module.program;
    let Expression::CallExpression(call) = const_initializer(program, name)?.get_inner_expression()
    else {
        return None;
    };
    let Expression::Identifier(callee) = call.callee.get_inner_expression() else {
        return None;
    };
    match find_import(module, callee.name.as_str())? {
        ImportRef::Named {
            source,
            name: imported,
        } if crate::packages::is_react_cache(&source, &imported) => {}
        _ => return None,
    }
    if call.arguments.len() != 1 {
        return None;
    }
    info_of_expression(name, call.arguments[0].as_expression()?)
}

/// Phase 12: whether the module's text writes a property of `name`
/// (`name.p = ...`, `name[...] = ...`, `delete name.p`, `Object.assign(name,
/// ...)`). Conservative: any such text disables method resolution on it.
fn mutates(text: &str, name: &str) -> bool {
    let bytes = text.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
    if text.contains(&format!("delete {name}.")) || text.contains(&format!("Object.assign({name}"))
    {
        return true;
    }
    let mut start = 0;
    while let Some(found) = text[start..].find(name) {
        let at = start + found;
        start = at + name.len();
        if at > 0 && (is_ident(bytes[at - 1]) || bytes[at - 1] == b'.') {
            continue;
        }
        let rest = &text[at + name.len()..];
        let after = if let Some(r) = rest.strip_prefix('.') {
            let len = r.bytes().take_while(|b| is_ident(*b)).count();
            &r[len..]
        } else if rest.starts_with('[') {
            match rest.find(']') {
                Some(close) => &rest[close + 1..],
                None => continue,
            }
        } else {
            continue;
        };
        let after = after.trim_start();
        if after.starts_with('=') && !after.starts_with("==") && !after.starts_with("=>") {
            return true;
        }
    }
    false
}

/// Phase 12: a class declared at the top level of the module.
fn find_class<'a>(program: &'a Program<'a>, name: &str) -> Option<&'a oxc_ast::ast::Class<'a>> {
    for statement in &program.body {
        let declaration = match statement {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            other => other.as_declaration(),
        };
        if let Some(Declaration::ClassDeclaration(c)) = declaration
            && c.id.as_ref().is_some_and(|id| id.name.as_str() == name)
        {
            return Some(c);
        }
        if let Statement::ExportDefaultDeclaration(e) = statement
            && let ExportDefaultDeclarationKind::ClassDeclaration(c) = &e.declaration
            && c.id.as_ref().is_some_and(|id| id.name.as_str() == name)
        {
            return Some(c);
        }
    }
    None
}

/// Phase 12: an instance method (or arrow-function field) of a class.
fn class_method<'a>(program: &'a Program<'a>, class: &str, method: &str) -> Option<FnInfo<'a>> {
    let c = find_class(program, class)?;
    let label = format!("{class}.{method}");
    for element in &c.body.body {
        match element {
            ClassElement::MethodDefinition(def)
                if def.kind == MethodDefinitionKind::Method
                    && !def.r#static
                    && !def.computed
                    && property_key_name(&def.key).as_deref() == Some(method) =>
            {
                return info_of_function(&label, &def.value);
            }
            ClassElement::PropertyDefinition(def)
                if !def.r#static
                    && !def.computed
                    && property_key_name(&def.key).as_deref() == Some(method) =>
            {
                return info_of_expression(&label, def.value.as_ref()?);
            }
            _ => {}
        }
    }
    None
}

/// Phase 12: a method of a `const` object literal: `m() {}`, `m: async ()
/// => {}`, `m: function () {}`, or `m: f` / shorthand `m` of a function
/// declared in the module.
fn object_method<'a>(module: &Module<'a>, object: &str, method: &str) -> Option<FnInfo<'a>> {
    let Expression::ObjectExpression(o) =
        const_initializer(module.program, object)?.get_inner_expression()
    else {
        return None;
    };
    let label = format!("{object}.{method}");
    let mut found = None;
    for property in &o.properties {
        let ObjectPropertyKind::ObjectProperty(p) = property else {
            // A spread may redefine the method.
            return None;
        };
        if p.computed || property_key_name(&p.key).as_deref() != Some(method) {
            continue;
        }
        found = match p.value.get_inner_expression() {
            Expression::Identifier(id) => find_function(module, id.name.as_str()),
            value => info_of_expression(&label, value),
        };
    }
    found
}

fn property_key_name(key: &oxc_ast::ast::PropertyKey<'_>) -> Option<String> {
    match key {
        oxc_ast::ast::PropertyKey::StaticIdentifier(id) => Some(id.name.as_str().to_owned()),
        oxc_ast::ast::PropertyKey::StringLiteral(s) => s.value.as_str().map(str::to_owned),
        _ => None,
    }
}

/// Phase 12: the function a resolved export names: the top-level binding
/// `local` of the declaring module (`default` for a default export).
fn find_resolved<'a>(module: &Module<'a>, local: &str) -> Result<FnInfo<'a>, String> {
    if local == "default" {
        return find_export(module, local);
    }
    find_function(module, local).ok_or_else(|| {
        format!(
            "`{local}` in `{}` is not a function declaration",
            module.rel
        )
    })
}

/// How a name used as a callee is imported.
enum ImportRef {
    Named { source: String, name: String },
    Default { source: String },
    Namespace { source: String },
}

fn find_import(module: &Module<'_>, local: &str) -> Option<ImportRef> {
    for statement in &module.program.body {
        if let Statement::ImportDeclaration(import) = statement {
            let source = import.source.value.as_str().unwrap_or_default().to_owned();
            for specifier in import.specifiers.iter().flatten() {
                match specifier {
                    ImportDeclarationSpecifier::ImportSpecifier(s)
                        if s.local.name.as_str() == local =>
                    {
                        return Some(ImportRef::Named {
                            source,
                            name: s.imported.name().as_str().to_owned(),
                        });
                    }
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(s)
                        if s.local.name.as_str() == local =>
                    {
                        return Some(ImportRef::Default { source });
                    }
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(s)
                        if s.local.name.as_str() == local =>
                    {
                        return Some(ImportRef::Namespace { source });
                    }
                    _ => {}
                }
            }
        }
    }
    None
}

/// `const NAME = <literal>` at the top level of the module.
fn module_constant(module: &Module<'_>, name: &str) -> Option<Expr> {
    fn check(variables: &oxc_ast::ast::VariableDeclaration<'_>, name: &str) -> Option<Expr> {
        variables.declarations.iter().find_map(|declarator| {
            if let BindingPattern::BindingIdentifier(id) = &declarator.id
                && id.name.as_str() == name
                && variables.kind.is_const()
            {
                literal(declarator.init.as_ref()?)
            } else {
                None
            }
        })
    }
    for statement in &module.program.body {
        let found = match statement {
            Statement::VariableDeclaration(variables) => check(variables, name),
            Statement::ExportDeclaration(export) => match &export.declaration {
                Declaration::VariableDeclaration(variables) => check(variables, name),
                _ => None,
            },
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Phase 14: `const name = [<literal>, ...]` (optionally `as const`) at the
/// top level: its value, and whether it is readonly by its type. A plain
/// array qualifies only when it is not exported and the module never uses
/// it other than by reading it (a mutating method, an assignment or passing
/// it on would make it a shared mutable object).
fn module_list_constant(module: &Module<'_>, name: &str) -> Option<(Expr, bool)> {
    fn elements(array: &oxc_ast::ast::ArrayExpression<'_>) -> Option<Vec<Value>> {
        array
            .elements
            .iter()
            .map(|e| match e.as_expression().and_then(literal) {
                Some(Expr::Lit(v)) => Some(v),
                _ => None,
            })
            .collect()
    }
    let found = |variables: &oxc_ast::ast::VariableDeclaration<'_>| {
        variables.declarations.iter().find_map(|d| {
            let BindingPattern::BindingIdentifier(id) = &d.id else {
                return None;
            };
            if id.name.as_str() != name || !variables.kind.is_const() {
                return None;
            }
            match d.init.as_ref()? {
                Expression::TSAsExpression(as_)
                    if as_.type_annotation.is_const_type_reference() =>
                {
                    match as_.expression.get_inner_expression() {
                        Expression::ArrayExpression(a) => Some((elements(a)?, true)),
                        _ => None,
                    }
                }
                Expression::ArrayExpression(a) => Some((elements(a)?, false)),
                _ => None,
            }
        })
    };
    let mut result = None;
    for statement in &module.program.body {
        match statement {
            Statement::VariableDeclaration(variables) => {
                if let Some(r) = found(variables) {
                    result = Some((r, false));
                }
            }
            Statement::ExportDeclaration(export) => {
                if let Declaration::VariableDeclaration(variables) = &export.declaration
                    && let Some(r) = found(variables)
                {
                    result = Some((r, true));
                }
            }
            _ => {}
        }
    }
    let ((items, readonly), exported) = result?;
    if !readonly && (exported || !read_only_uses(module.source, name)) {
        return None;
    }
    Some((Expr::Lit(Value::List(items)), readonly))
}

/// Phase 14: every use of `name` in `text` reads it: followed by a reading
/// member (`.includes(`, `.length`, `[`...) or iterated by `for … of`.
fn read_only_uses(text: &str, name: &str) -> bool {
    const READS: &[&str] = &[
        "includes",
        "indexOf",
        "length",
        "map",
        "filter",
        "find",
        "findIndex",
        "some",
        "every",
        "join",
        "at",
    ];
    let bytes = text.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
    let mut start = 0;
    let mut first = true;
    while let Some(found) = text[start..].find(name) {
        let at = start + found;
        start = at + name.len();
        if (at > 0 && (is_ident(bytes[at - 1]) || bytes[at - 1] == b'.'))
            || bytes.get(at + name.len()).copied().is_some_and(is_ident)
        {
            continue;
        }
        if first {
            // The declaration itself.
            first = false;
            continue;
        }
        let rest = &text[at + name.len()..];
        let before = text[..at].trim_end();
        let ok = match rest.strip_prefix('.') {
            Some(r) => READS.iter().any(|m| {
                r.starts_with(m) && !r[m.len()..].starts_with(|c: char| c.is_alphanumeric())
            }),
            None if rest.starts_with('[') => rest.find(']').is_some_and(|close| {
                let after = rest[close + 1..].trim_start();
                !(after.starts_with('=') && !after.starts_with("==") && !after.starts_with("=>"))
            }),
            None => before.ends_with(" of"),
        };
        if !ok {
            return false;
        }
    }
    true
}

/// `name.member` for a top-level `enum name { ... }` (string or numeric
/// members) or `const name = { member: <literal> }` (optionally `as const`)
/// that is never reassigned (a `const`).
fn enum_member_in(module: &Module<'_>, name: &str, member: &str) -> Option<Expr> {
    use oxc_ast::ast::{TSEnumDeclaration, TSEnumMemberName};
    fn from_enum(e: &TSEnumDeclaration<'_>, member: &str) -> Option<Expr> {
        let mut next: Option<i64> = Some(0);
        for m in &e.body.members {
            let key = match &m.id {
                TSEnumMemberName::Identifier(id) => id.name.as_str().to_owned(),
                TSEnumMemberName::String(s) => s.value.as_str().unwrap_or_default().to_owned(),
                _ => return None,
            };
            let value = match &m.initializer {
                Some(init) => literal(init)?,
                None => Expr::lit(next?),
            };
            next = match &value {
                Expr::Lit(Value::Int(n)) => n.checked_add(1),
                _ => None,
            };
            if key == member {
                return Some(value);
            }
        }
        None
    }
    for statement in &module.program.body {
        let declaration = match statement {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            other => other.as_declaration(),
        };
        match declaration {
            Some(Declaration::TSEnumDeclaration(e)) if e.id.name.as_str() == name => {
                return from_enum(e, member);
            }
            Some(Declaration::VariableDeclaration(v)) if v.kind.is_const() => {
                for d in &v.declarations {
                    if let (BindingPattern::BindingIdentifier(id), Some(init)) = (&d.id, &d.init)
                        && id.name.as_str() == name
                        && let Expression::ObjectExpression(o) = init.get_inner_expression()
                    {
                        return o.properties.iter().find_map(|p| match p {
                            ObjectPropertyKind::ObjectProperty(p)
                                if !p.computed && static_key(&p.key).as_deref() == Some(member) =>
                            {
                                literal(&p.value)
                            }
                            _ => None,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn literal(expression: &Expression<'_>) -> Option<Expr> {
    match expression.get_inner_expression() {
        Expression::NumericLiteral(n) => integer(n.value).map(Expr::lit),
        Expression::StringLiteral(s) => Some(Expr::Lit(Value::Str(
            s.value.as_str().unwrap_or_default().to_owned(),
        ))),
        Expression::BooleanLiteral(b) => Some(Expr::lit(b.value)),
        Expression::NullLiteral(_) => Some(Expr::Lit(Value::Null)),
        _ => None,
    }
}

fn integer(value: f64) -> Option<i64> {
    (value.fract() == 0.0 && value.abs() < 9.0e15).then_some(value as i64)
}

// ---------------------------------------------------------------------------
// Lowering state
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Binding {
    Local(String),
    Tx,
    Db,
    /// A row array from a mutation with `returning()`: only truthiness of
    /// its first element (`row`) is modeled, as `count > 0`.
    Presence(String),
    /// An object whose listed fields are locals (`rows.length`,
    /// `{ n: count() }` rows).
    Virtual(BTreeMap<String, String>),
    /// An array of at most one row (`select ... limit(1)`, a single-row
    /// `insert ... returning()`): `length` and `[0]` are modeled exactly.
    Rows {
        row: String,
        length: String,
    },
    /// A `const` object literal whose properties are expressions evaluated
    /// at its declaration (Phase 7; `values(obj)`, `obj.key`).
    Object(Vec<(String, Expr)>),
    /// A caught exception: its properties (driver messages, SQLSTATE
    /// wrappers) are not modeled; only its truthiness is.
    Opaque,
    /// Phase 12: an instance of an analyzed class (`new UserRepository(tx)`)
    /// whose methods are inlined with this `this`.
    Instance(Box<ThisBinding>),
    /// Phase 13: a replicated database (`withReplicas`): writes and
    /// transactions use the primary, reads a replica
    /// (`crate::handles`).
    Replicated(Box<Routing>),
    /// Phase 13: a name used as a database (a configured receiver) whose
    /// value is not a recognized database, in a project that builds a
    /// replicated one: its operations are not bound.
    UnknownDb(String),
    /// Phase 14: a zod schema passed to an inlined function (its
    /// `parse`/`safeParse` are modeled by `crate::zod`), or why it is not
    /// modeled.
    Zod(Box<Result<crate::zod::Validated, String>>),
    /// Phase 14: a function value (an arrow function or function
    /// expression passed to an inlined project function). Calling it
    /// inlines its body where the call happens, with the bindings of its
    /// definition (`docs/phase14/SOURCE-COVERAGE.md`).
    Closure(Box<Closure>),
    /// Phase 16: a Drizzle predicate held in a local (`const p = and(eq(..),
    /// ..)`), lowered where it is declared: its JavaScript operands are
    /// locals assigned there (Drizzle captures their values when `eq()` is
    /// called), its columns stay columns. `.where(p)` and `and(p, ..)` use
    /// it (`docs/phase16/DRIZZLE-PREDICATES.md`).
    Pred(Box<HeldPred>),
    /// Phase 16: a caught error a handler inspects: the local holding the
    /// error object (`docs/phase16/LOOP-SEMANTICS.md`).
    Caught(String),
    /// Phase 16: a route handler's `Request` parameter: `await
    /// request.json()` is the local (the actor's argument) holding the
    /// parsed body (`docs/phase16/FRAMEWORK-REQUESTS.md`).
    Request(String),
}

/// Phase 16: a Drizzle predicate held in a local ([`Binding::Pred`]).
#[derive(Clone, Debug)]
struct HeldPred {
    /// The predicate, or an `Expr::Unknown` saying why the declaration is
    /// not modeled.
    expr: Expr,
    /// When the local may hold `undefined` (`c ? eq(..) : undefined`): a
    /// boolean expression over locals, true when it holds the predicate.
    /// Drizzle's `and`/`or`/`where` drop an `undefined` operand.
    defined: Option<Expr>,
    /// Where it was declared (`file:line:column`).
    at: String,
}

/// Phase 16: the Drizzle operators a predicate is built from (as
/// [`Lowerer::pred_in`] lowers them).
const PRED_OPS: [&str; 13] = [
    "eq",
    "ne",
    "gt",
    "gte",
    "lt",
    "lte",
    "and",
    "or",
    "not",
    "isNull",
    "isNotNull",
    "inArray",
    "notInArray",
];

/// Phase 16: `undefined` or `void x` (what Drizzle's `and`/`or` drop; `null`
/// is not dropped).
fn undefined_like(e: &Expression<'_>) -> bool {
    let e = e.get_inner_expression();
    is_nullish(e) && !matches!(e, Expression::NullLiteral(_))
        || matches!(e, Expression::UnaryExpression(u) if u.operator == UnaryOperator::Void)
}

/// Phase 16: whether a `select(...)` projection is `{ name: count() }`.
fn is_count_projection(argument: &Argument<'_>) -> bool {
    matches!(argument.as_expression().map(Expression::get_inner_expression),
        Some(Expression::ObjectExpression(object))
            if object.properties.len() == 1
                && matches!(&object.properties[0], ObjectPropertyKind::ObjectProperty(p)
                    if matches!(p.value.get_inner_expression(), Expression::CallExpression(c)
                        if c.arguments.is_empty()
                            && matches!(c.callee.get_inner_expression(), Expression::Identifier(i) if i.name.as_str() == "count"))))
}

/// Phase 16: the expression a non-async function returns when its body is
/// that one `return` (or an arrow's expression body).
fn returned_expr<'a>(info: &FnInfo<'a>) -> Option<&'a Expression<'a>> {
    if info.is_async {
        return None;
    }
    match &info.body {
        FnBody::Expr(e) => Some(e),
        FnBody::Block(body) => match body.statements.as_slice() {
            [Statement::ReturnStatement(r)] => r.argument.as_ref(),
            _ => None,
        },
    }
}

/// Phase 16: whether a lowered predicate reads a clock placeholder (bound per
/// statement execution, not where a held predicate was built).
fn reads_clock(e: &Expr) -> bool {
    fn value(v: &Value) -> bool {
        match v {
            Value::Gen(g) => {
                midwrite_model::db::is_clock(g) && g.actor == midwrite_model::db::CLOCK_PLACEHOLDER
            }
            Value::Row(r) => r.values().any(value),
            Value::List(items) => items.iter().any(value),
            _ => false,
        }
    }
    match e {
        Expr::Lit(v) => value(v),
        Expr::Field(x, _) | Expr::Not(x) | Expr::IsNull(x) => reads_clock(x),
        Expr::Bin(_, a, b) => reads_clock(a) || reads_clock(b),
        Expr::Object(fields) => fields.iter().any(|(_, x)| reads_clock(x)),
        Expr::Array(xs) | Expr::Call { args: xs, .. } => xs.iter().any(reads_clock),
        Expr::Each { list, body, .. } => reads_clock(list) || reads_clock(body),
        Expr::Local(_) | Expr::Col(_) | Expr::Unknown(_) => false,
    }
}

/// Phase 16: whether the module's text assigns `name` more than once
/// (`name = ...` beyond its declaration, compound assignments, `++`/`--`).
/// Conservative: a same-named variable anywhere in the module counts.
fn reassigned(text: &str, name: &str) -> bool {
    let bytes = text.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
    let mut assignments = 0;
    let mut start = 0;
    while let Some(found) = text[start..].find(name) {
        let at = start + found;
        start = at + name.len();
        if at > 0 && (is_ident(bytes[at - 1]) || bytes[at - 1] == b'.') {
            continue;
        }
        if bytes.get(at + name.len()).is_some_and(|b| is_ident(*b)) {
            continue;
        }
        let before = text[..at].trim_end();
        if before.ends_with("++") || before.ends_with("--") {
            return true;
        }
        let after = text[at + name.len()..].trim_start();
        if after.starts_with("++") || after.starts_with("--") {
            return true;
        }
        let compound = [
            "+=", "-=", "*=", "/=", "%=", "??=", "||=", "&&=", "|=", "&=",
        ];
        if compound.iter().any(|op| after.starts_with(op)) {
            return true;
        }
        if after.starts_with('=') && !after.starts_with("==") && !after.starts_with("=>") {
            assignments += 1;
        }
    }
    assignments > 1
}

/// Phase 14: a closure: where its function node is, and the scopes it
/// closed over. Captured `let`/`const` bindings name the same model locals
/// as in the defining function, so assignments after the capture are seen
/// (JavaScript closures capture variables, not values).
#[derive(Clone, Debug)]
struct Closure {
    abs: PathBuf,
    rel: String,
    span: Span,
    name: String,
    scopes: Vec<HashMap<String, Binding>>,
    this: Option<ThisBinding>,
}

/// Phase 12: whose methods `this.m()` calls inside an inlined method.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Owner {
    /// A never-mutated `const` object literal of the module.
    Object(String),
    /// A class declared in the module (no `extends`).
    Class(String),
}

/// Phase 12: what `this` denotes inside an inlined method: the fields the
/// constructor stored (a database or transaction handle, or a value), and
/// the object or class whose other methods `this.m()` calls.
#[derive(Clone, Debug)]
struct ThisBinding {
    fields: HashMap<String, Binding>,
    owner: (PathBuf, Owner),
}

struct Ret {
    result: String,
    patches: Vec<(usize, usize)>,
    main: bool,
}

struct Frame {
    function: String,
    file: String,
    prefix: String,
    scopes: Vec<HashMap<String, Binding>>,
    rets: Vec<Ret>,
    used: HashSet<String>,
    key: (PathBuf, String),
    /// `try` blocks entered and not left in this frame (a `return` from an
    /// inlined helper leaves them).
    tries: u32,
    /// Enclosing `switch`/loop targets: pending `break` jumps, pending
    /// `continue` jumps (`None` for a `switch`), and the `try` depth at
    /// entry (jumping out leaves inner `try` blocks).
    breaks: Vec<JumpTarget>,
    /// Phase 12: `this` inside an inlined method of a repository object or
    /// class instance.
    this: Option<ThisBinding>,
}

/// Pending jumps (code vector, index) to an enclosing `switch` or loop:
/// `break`s, `continue`s (`None` for a `switch`), and the `try` depth at
/// entry.
type JumpTarget = (Vec<(usize, usize)>, Option<Vec<(usize, usize)>>, u32);

#[derive(Clone, Copy)]
struct Cx {
    /// `T.col` denotes a column.
    col_ok: bool,
    /// Evaluated only conditionally (right operand of `&&`/`||`): forms
    /// that must be hoisted before the statement are not exact here.
    cond: bool,
    /// Only the truthiness of the value is used.
    truthy: bool,
}

const TOP: Cx = Cx {
    col_ok: false,
    cond: false,
    truthy: false,
};

/// What the caller does with the value of a call.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Want {
    Discard,
    /// `const x = await ...`
    Scalar,
    /// `const [x] = await ...`
    Single,
}

enum Res {
    Unit,
    Value(Expr),
    Row(String),
    /// Number of rows a mutation with `returning()` affected.
    Affected(String),
    Aggregate {
        field: String,
        local: String,
    },
    /// An array of at most one row.
    Rows {
        row: String,
        length: String,
    },
    /// Phase 14: an object whose listed properties are locals
    /// (`schema.safeParse(x)`: `success`, `data`).
    Fields(BTreeMap<String, String>),
}

enum ArgVal {
    Handle(Binding),
    Expr(Expr),
}

struct Lowerer<'r> {
    request: &'r ExtractRequest,
    receivers: &'r [String],
    root: PathBuf,
    /// Phase 12: the shared project context (files, resolver, sources,
    /// module summaries).
    ctx: Arc<ProjectContext>,
    codes: Vec<Vec<Step>>,
    cur: usize,
    frames: Vec<Frame>,
    counter: u32,
    tx_open: u32,
    assumptions: Vec<String>,
    /// Phase 14: facts of the JavaScript value lowering.
    js_facts: js_values::JsFacts,
    /// Column references are qualified (`table.column`): set while lowering
    /// a joined query.
    qualify: bool,
    /// Phase 12: declared types (as written) of the entry's parameters that
    /// are not database handles, to name the blocker when one is used as a
    /// Drizzle receiver.
    param_types: HashMap<String, (String, String)>,
    /// Phase 12: the `this` the next inlined function gets.
    pending_this: Option<ThisBinding>,
    /// Phase 13: database handle provenance (declared replica factories).
    prov: Provenance,
    /// Phase 13: why a handle known only by a plain drizzle type may be a
    /// replicated database (the project builds one), computed once.
    replicating: Option<Option<String>>,
    /// Phase 13: function aliases followed across modules (bounded).
    alias_hops: u32,
    /// Phase 13: React `cache`d functions already called by the actor.
    cached_calls: HashSet<(PathBuf, String)>,
    /// Phase 14: the scopes the next inlined function closed over.
    pending_scopes: Option<Vec<HashMap<String, Binding>>>,
    /// Phase 16: the declaration being lowered is a `const`.
    const_decl: bool,
    /// Phase 16: imported functions already examined as predicate helpers
    /// (module, export) → whether their body is `return <predicate>`.
    pred_helpers: HashMap<(PathBuf, String), bool>,
    /// Phase 16: loop unrolling, caught errors and request state.
    p16: loops::Phase16,
}

struct ChainStep<'a, 'b> {
    path: Vec<String>,
    method: String,
    args: &'b [Argument<'a>],
}

impl<'r> Lowerer<'r> {
    // -- infrastructure ----------------------------------------------------

    fn frame(&self) -> &Frame {
        self.frames.last().expect("a frame is active")
    }

    fn frame_mut(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("a frame is active")
    }

    fn assume(&mut self, text: impl Into<String>) {
        let text = text.into();
        if !self.assumptions.contains(&text) {
            self.assumptions.push(text);
        }
    }

    fn src(&self, module: &Module<'_>, span: Span) -> SourceRef {
        let (line, column) = module.lines.position(span.start);
        let (end_line, _) = module
            .lines
            .position(span.end.saturating_sub(1).max(span.start));
        // Spans made by offset arithmetic (`end - 1`) can fall inside a
        // multi-byte character: widen to character boundaries (Phase 11; a
        // non-ASCII comment or string used to panic here).
        let source = module.source;
        let mut end = (span.end as usize).min(source.len());
        while !source.is_char_boundary(end) {
            end += 1;
        }
        let mut start = (span.start as usize).min(end);
        while !source.is_char_boundary(start) {
            start -= 1;
        }
        let mut text: String = source[start..end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if text.chars().count() > 80 {
            text = text.chars().take(77).collect::<String>() + "...";
        }
        let frame = self.frame();
        let chain: Vec<String> = self.frames[..self.frames.len() - 1]
            .iter()
            .map(|f| f.function.clone())
            .collect();
        SourceRef {
            origin: Origin::Extracted,
            file: module.rel.clone(),
            line,
            column,
            end_line,
            function: frame.function.clone(),
            call_chain: chain,
            effect_id: None,
            diagnostics: Vec::new(),
            text,
        }
    }

    fn emit(&mut self, module: &Module<'_>, span: Span, op: Op) -> (usize, usize) {
        if let Op::Let { local, expr } = &op {
            self.p16.note_let(local, expr);
        }
        let step = Step {
            op,
            src: self.src(module, span),
        };
        self.codes[self.cur].push(step);
        (self.cur, self.codes[self.cur].len() - 1)
    }

    fn patch(&mut self, at: (usize, usize), target: usize) {
        match &mut self.codes[at.0][at.1].op {
            Op::JumpIf { target: t, .. } | Op::Jump { target: t } => *t = target,
            Op::Try { handler: t, .. } => *t = target,
            _ => unreachable!("patching a non-jump"),
        }
    }

    fn here(&self) -> usize {
        self.codes[self.cur].len()
    }

    fn tmp(&mut self, base: &str) -> String {
        self.counter += 1;
        let prefix = self.frame().prefix.clone();
        format!("{prefix}_{base}{}", self.counter)
    }

    fn fresh(&mut self, name: &str) -> String {
        let frame = self.frame_mut();
        let mut candidate = format!("{}{name}", frame.prefix);
        let mut n = 1;
        while frame.used.contains(&candidate) {
            candidate = format!("{}{name}#{n}", frame.prefix);
            n += 1;
        }
        frame.used.insert(candidate.clone());
        candidate
    }

    fn declare(&mut self, name: &str, binding: Binding) {
        self.frame_mut()
            .scopes
            .last_mut()
            .expect("a scope is open")
            .insert(name.to_owned(), binding);
    }

    fn lookup(&self, name: &str) -> Option<Binding> {
        self.frame()
            .scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn handle_of(&mut self, name: &str) -> Option<Binding> {
        match self.lookup(name) {
            Some(
                b @ (Binding::Tx | Binding::Db | Binding::Replicated(_) | Binding::UnknownDb(_)),
            ) => Some(b),
            Some(_) => None,
            None => {
                let file = self.frames.last().map(|f| f.key.0.clone())?;
                self.module_handle(&file, name)
            }
        }
    }

    /// Phase 13: whether the project builds a replicated database (then a
    /// handle known only by a plain drizzle type, or only by its receiver
    /// name, may be a replicated one).
    fn replicating(&mut self) -> Option<String> {
        if self.replicating.is_none() {
            self.replicating = Some(self.prov.replicating());
        }
        self.replicating.clone().flatten()
    }

    /// Phase 13: the binding of a handle known by provenance.
    fn bind_fact(&mut self, fact: HandleFact, written: &str) -> Option<Binding> {
        match fact {
            HandleFact::Primary { by_type: false, .. } => Some(Binding::Db),
            HandleFact::Primary {
                by_type: true,
                origin,
            } => match self.replicating() {
                Some(why) => Some(Binding::Replicated(Box::new(Routing::ambiguous(format!(
                    "`{written}` is known only as {origin}, and the project builds a replicated database ({why})"
                ))))),
                None => Some(Binding::Db),
            },
            HandleFact::Replicated(routing) => {
                self.assume(format!(
                    "`{written}` is a replicated database: {}; {}",
                    routing.origin,
                    routing.contract()
                ));
                Some(Binding::Replicated(Box::new(routing)))
            }
            HandleFact::Unknown(_) => None,
        }
    }

    /// Phase 13: a module-level name of `file` (declared there or imported)
    /// used as a database: its value provenance; a configured receiver name
    /// (`db`) that cannot be resolved keeps the Phase 7 meaning (the root
    /// database), unless the project builds a replicated database.
    fn module_handle(&mut self, file: &Path, name: &str) -> Option<Binding> {
        let index = self.ctx.index_of(file);
        let fact = match index {
            Some(index) => self.prov.value(index, name),
            None => HandleFact::Unknown("the module is not analyzed".to_owned()),
        };
        let why = match &fact {
            HandleFact::Unknown(why) => why.clone(),
            _ => String::new(),
        };
        if let Some(binding) = self.bind_fact(fact, name) {
            return Some(binding);
        }
        if !self.receivers.iter().any(|r| r == name) {
            return None;
        }
        match self.replicating() {
            None => Some(Binding::Db),
            Some(replicating) => Some(Binding::UnknownDb(format!(
                "`{name}` is a database receiver by name only ({why}), and the project builds a replicated database ({replicating}): an unrecognized database is neither the primary nor a replicated one"
            ))),
        }
    }

    /// Phase 13: a database handle written as an expression: a handle
    /// local, `this.field`, `x.$primary` / `x.usePrimaryOnly()` of a
    /// replicated database, a database factory call (`getDb()`, `await
    /// connectDb()`).
    fn handle_expr(&mut self, m: &Module<'_>, e: &Expression<'_>) -> Option<Binding> {
        let e = match e.get_inner_expression() {
            Expression::AwaitExpression(a) => a.argument.get_inner_expression(),
            other => other,
        };
        match e {
            Expression::Identifier(id) => self.handle_of(id.name.as_str()),
            Expression::StaticMemberExpression(member) => {
                if let Some(h) = self.this_handle(e) {
                    return Some(h);
                }
                let property = member.property.name.as_str();
                match self.handle_expr(m, &member.object)? {
                    Binding::Replicated(r)
                        if r.primary_properties.iter().any(|p| p == property) =>
                    {
                        Some(Binding::Db)
                    }
                    _ => None,
                }
            }
            Expression::CallExpression(c) => match c.callee.get_inner_expression() {
                Expression::Identifier(f) => {
                    self.factory_handle(m, f.name.as_str(), c.arguments.is_empty())
                }
                Expression::StaticMemberExpression(member) if c.arguments.is_empty() => {
                    let method = member.property.name.as_str();
                    match self.handle_expr(m, &member.object)? {
                        Binding::Replicated(r)
                            if r.primary_only_methods.iter().any(|p| p == method) =>
                        {
                            Some(Binding::Db)
                        }
                        _ => None,
                    }
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Phase 13: `f(...)` of a project function returning a database: a
    /// zero-argument function with a declared drizzle database return type
    /// (Phase 12), or one whose result's provenance is known.
    fn factory_handle(&mut self, m: &Module<'_>, name: &str, no_args: bool) -> Option<Binding> {
        if self.lookup(name).is_some() {
            return None;
        }
        if no_args && self.db_factory(m, name) {
            return self.bind_fact(
                HandleFact::Primary {
                    origin: format!("the declared return type of `{name}`"),
                    by_type: true,
                },
                &format!("{name}()"),
            );
        }
        let file = self.ctx.index_of(&m.abs)?;
        let fact = self.prov.call_result(file, name);
        self.bind_fact(fact, &format!("{name}()"))
    }

    /// Phase 12: whether `name`, called with no arguments in `m`, returns a
    /// root Drizzle database (see [`ProjectContext::returns_drizzle_database`]).
    fn db_factory(&self, m: &Module<'_>, name: &str) -> bool {
        let Some(file) = self.ctx.index_of(&m.abs) else {
            return false;
        };
        match self.ctx.resolve_local(file, name) {
            Ok(Resolved::Binding { file, local }) => {
                self.ctx.returns_drizzle_database(file, &local)
            }
            _ => false,
        }
    }

    /// The table an identifier written in module `m` denotes. Phase 12:
    /// when the scenario's tables carry their declarations, the identifier
    /// must resolve (through imports and re-exports, whatever its local
    /// name) to one of them; a local variable or another declaration with
    /// the same name is not a table. Otherwise (Phase 7–11 scenarios),
    /// tables are bound by identifier name, as before.
    fn table_ref(&self, m: &Module<'_>, identifier: &str) -> Option<&'r TableBinding> {
        if !self.request.tables.iter().any(|t| t.source.is_some()) {
            return self.table(identifier);
        }
        if self.lookup(identifier).is_some() {
            return None;
        }
        let file = self.ctx.index_of(&m.abs)?;
        let Ok(Resolved::Binding { file, local }) = self.ctx.resolve_local(file, identifier) else {
            return None;
        };
        let module = self.ctx.rel(&self.ctx.files[file]);
        self.request.tables.iter().find(|t| {
            t.source
                .as_ref()
                .is_some_and(|(f, b)| *f == module && *b == local)
        })
    }

    /// `db.query.<key>`: with declarations, only a table whose declaring
    /// module the querying module reaches through its imports (the schema
    /// object passed to `drizzle(...)` is imported somewhere on that path).
    fn query_table(&self, m: &Module<'_>, key: &str) -> Option<&'r TableBinding> {
        let table = self.table(key)?;
        let Some((module, _)) = &table.source else {
            return Some(table);
        };
        let from = self.ctx.index_of(&m.abs)?;
        let target = self.ctx.index_of(&absolute(&self.root.join(module)))?;
        self.ctx.closure(from).contains(&target).then_some(table)
    }

    /// A table by name: the identifier (Phase 7–11), or, for `db.query.<key>`
    /// with declarations, the one table whose binding is `key` (the keys of
    /// Drizzle's relational API are the schema's export names).
    fn table(&self, identifier: &str) -> Option<&'r TableBinding> {
        if self.request.tables.iter().any(|t| t.source.is_some()) {
            let mut found = self
                .request
                .tables
                .iter()
                .filter(|t| t.source.as_ref().is_some_and(|(_, b)| b == identifier));
            let first = found.next()?;
            return found.next().is_none().then_some(first);
        }
        self.request
            .tables
            .iter()
            .find(|t| t.identifier == identifier)
    }

    fn unknown_op(&mut self, module: &Module<'_>, span: Span, detail: impl Into<String>) -> Res {
        let detail = detail.into();
        self.emit(
            module,
            span,
            Op::Unknown {
                detail: detail.clone(),
                defines: Vec::new(),
            },
        );
        Res::Value(Expr::Unknown(detail))
    }

    /// Lower into a new child code vector (ends with `Return`).
    fn with_child<R>(
        &mut self,
        m: &Module<'_>,
        span: Span,
        f: impl FnOnce(&mut Self) -> R,
    ) -> (usize, R) {
        let child = self.codes.len();
        self.codes.push(Vec::new());
        let saved = self.cur;
        self.cur = child;
        let result = f(self);
        let end = self.src(m, span);
        self.codes[child].push(Step {
            op: Op::Return,
            src: end,
        });
        self.cur = saved;
        (child, result)
    }

    // -- selected entries (Phase 7) ----------------------------------------

    /// Facts about the selected entry and the wrapper it is passed to, and
    /// how the scenario's declared contract relates to them.
    fn entry_info(
        &mut self,
        m: &Module<'_>,
        found: &identity::Found<'_>,
        selector: &identity::Selector,
        contract: Option<InvocationContract>,
    ) -> Result<EntryInfo, String> {
        let wrapper = found
            .via_call
            .map(|(callee, index)| self.wrapper_facts(m, callee, index));
        let declared = contract.as_ref().and_then(|c| c.invocation.clone());
        if let Some(d) = &declared
            && !matches!(d.as_str(), "awaited" | "fire-and-forget")
        {
            return Err(format!(
                "entry.invocation `{d}`: expected `awaited` or `fire-and-forget`"
            ));
        }
        let status = match &wrapper {
            None => "not-needed",
            Some(w) => {
                let source_mode = match w.invocation.as_str() {
                    "returned-function-awaits"
                    | "returned-function-returns-promise"
                    | "invoked-during-wrap" => Some("awaited"),
                    "returned-function-fire-and-forget" | "invoked-during-wrap-unawaited" => {
                        Some("fire-and-forget")
                    }
                    "not-invoked" => {
                        return Err(format!(
                            "`{}` never invokes the selected callback ({}): there is nothing to model",
                            w.wrapper, w.detail
                        ));
                    }
                    _ => None,
                };
                match (source_mode, &declared) {
                    (Some(s), Some(d)) if s != d => {
                        return Err(format!(
                            "entry.invocation `{d}` contradicts the wrapper's source: `{}` {} ({})",
                            w.wrapper,
                            w.invocation,
                            w.defined_at.as_deref().unwrap_or("")
                        ));
                    }
                    (Some(_), _) if w.conditional && contract.is_none() => {
                        return Err(format!(
                            "`{}` invokes the callback only when its guards pass ({}); declare the conditions you assume in `entry.preconditions`",
                            w.wrapper,
                            w.guards.join("; ")
                        ));
                    }
                    (Some(_), Some(_)) => "consistent",
                    (Some(_), None) if contract.is_some() => "consistent",
                    (Some(_), None) => "absent",
                    (None, Some(_)) => "required",
                    (None, None) => {
                        return Err(format!(
                            "how `{}` invokes the callback is unknown ({}); declare `entry.invocation` (and `entry.preconditions`) if you know it",
                            w.wrapper, w.detail
                        ));
                    }
                }
            }
        };
        if let Some(w) = &wrapper {
            self.assume(format!(
                "entry: the callback {} of `{}` is modeled from its own body; the wrapper's code around the call is not part of the actor",
                selector, w.wrapper
            ));
        }
        if let Some(c) = &contract {
            for p in &c.preconditions {
                self.assume(format!("declared precondition (scenario, not source): {p}"));
            }
            if let Some(i) = &c.invocation {
                self.assume(format!("declared invocation (scenario, not source): {i}"));
            }
        }
        Ok(EntryInfo {
            selector: selector.to_string(),
            anchor: format!("{}@{}:{}", m.rel, found.line, found.column),
            file: m.rel.clone(),
            end_line: m.lines.position(found.span.end.saturating_sub(1)).0,
            static_name: static_name(found, selector),
            wrapper,
            contract,
            contract_status: status.to_owned(),
        })
    }

    fn wrapper_facts(
        &mut self,
        m: &Module<'_>,
        callee: &Expression<'_>,
        index: usize,
    ) -> WrapperFacts {
        let label = callee_text(callee).unwrap_or_else(|| "<expression>".to_owned());
        let analyze = |info: &FnInfo<'_>, module: &Module<'_>| -> WrapperFacts {
            match &info.body {
                FnBody::Block(body) => wrapper::analyze(
                    &label,
                    info.params,
                    body,
                    index,
                    &wrapper::Source {
                        rel: &module.rel,
                        text: module.source,
                        lines: &module.lines,
                    },
                    info.span,
                ),
                FnBody::Expr(_) => wrapper::unknown(
                    &label,
                    "a wrapper with a concise arrow body is not analyzed",
                ),
            }
        };
        match self.resolve_callee(m, callee) {
            Callee::Method { label, .. } => wrapper::unknown(
                &label,
                "a method of a repository object or class used as a wrapper is not analyzed",
            ),
            Callee::Local(info) | Callee::Cached(info) => analyze(&info, m),
            Callee::Imported {
                abs, rel, export, ..
            } => {
                let Ok(source) = self.ctx.source(&abs) else {
                    return wrapper::unknown(&label, format!("cannot read `{rel}`"));
                };
                let allocator = Allocator::default();
                let program = match load(&allocator, &abs, &source) {
                    Ok(p) => p,
                    Err(e) => return wrapper::unknown(&label, format!("`{rel}`: {e}")),
                };
                let module = Module::new(&abs, rel.clone(), &source, &program);
                match find_resolved(&module, &export) {
                    Ok(info) => analyze(&info, &module),
                    Err(why) => wrapper::unknown(&label, why),
                }
            }
            Callee::Closure(c) => wrapper::unknown(
                &label,
                format!("a closure (`{}`) used as a wrapper is not analyzed", c.name),
            ),
            Callee::Unresolvable(why) => wrapper::unknown(&label, why),
        }
    }

    // -- entry --------------------------------------------------------------

    fn lower_main(
        &mut self,
        module: &Module<'_>,
        info: &FnInfo<'_>,
        _request: &ExtractRequest,
    ) -> Vec<String> {
        self.frames.push(Frame {
            function: info.name.clone(),
            file: module.rel.clone(),
            prefix: String::new(),
            scopes: vec![HashMap::new()],
            rets: vec![Ret {
                result: String::new(),
                patches: Vec::new(),
                main: true,
            }],
            used: HashSet::new(),
            key: (module.abs.clone(), info.name.clone()),
            tries: 0,
            breaks: Vec::new(),
            this: None,
        });
        let mut params = Vec::new();
        for (index, item) in info.params.items.iter().enumerate() {
            if let BindingPattern::BindingIdentifier(id) = &item.pattern {
                let name = id.name.as_str().to_owned();
                self.frame_mut().used.insert(name.clone());
                // Phase 12: a parameter declared with a Drizzle database type
                // (`db: Database` where `Database` is a drizzle-orm
                // `...Database` type) is the root database handle.
                // Phase 16: a route handler's `request: Request`.
                if let Some(annotation) = &item.type_annotation
                    && let Some(t) = crate::context::reference_name(&annotation.type_annotation)
                    && self.request_param(module, &t)
                {
                    self.declare(&name, Binding::Request(name.clone()));
                    params.push(name);
                    continue;
                }
                if let Some(annotation) = &item.type_annotation {
                    let file = self.ctx.index_of(&module.abs);
                    let span = annotation.type_annotation.span();
                    let written = module
                        .source
                        .get(span.start as usize..span.end as usize)
                        .unwrap_or_default()
                        .to_owned();
                    // Phase 13: the type's provenance (computed types,
                    // imported aliases, `PgWithReplicas`, ...).
                    let fact = match crate::context::reference_name(&annotation.type_annotation) {
                        Some(t) if file.is_some_and(|f| self.ctx.is_database_type(f, &t)) => {
                            HandleFact::Primary {
                                origin: format!("the drizzle-orm database type `{t}`"),
                                by_type: true,
                            }
                        }
                        _ => match file {
                            Some(f) => {
                                self.prov
                                    .annotation(f, module.program, &annotation.type_annotation)
                            }
                            None => HandleFact::Unknown("the module is not analyzed".to_owned()),
                        },
                    };
                    let why = match &fact {
                        HandleFact::Unknown(why) => why.clone(),
                        _ => String::new(),
                    };
                    let replicated = matches!(fact, HandleFact::Replicated(_));
                    if let Some(binding) = self.bind_fact(fact, &format!("{name}: {written}")) {
                        if replicated {
                            self.assume(format!(
                                "parameter `{name}: {written}` is taken to hold the database its type is computed from (type provenance; casts and other values of the same type are not tracked)"
                            ));
                        }
                        self.declare(&name, binding);
                        params.push(name);
                        continue;
                    }
                    self.param_types.insert(name.clone(), (written, why));
                }
                self.declare(&name, Binding::Local(name.clone()));
                params.push(name);
            } else {
                let name = format!("_param{index}");
                self.frame_mut().used.insert(name.clone());
                params.push(name.clone());
                match &item.pattern {
                    BindingPattern::ObjectPattern(object) => {
                        self.destructure(module, object, Expr::Local(name), item.span);
                    }
                    _ => {
                        self.unknown_op(
                            module,
                            item.span,
                            "only `{ a, b: c }` parameter destructuring is supported",
                        );
                    }
                }
            }
        }
        if info.params.rest.is_some() {
            self.unknown_op(
                module,
                info.params.span,
                "rest parameters are not supported",
            );
        }
        self.body(module, info);
        let span = match &info.body {
            FnBody::Block(b) => b.span,
            FnBody::Expr(e) => e.span(),
        };
        self.emit(
            module,
            Span::new(span.end.saturating_sub(1), span.end),
            Op::Return,
        );
        params
    }

    fn body(&mut self, module: &Module<'_>, info: &FnInfo<'_>) {
        match &info.body {
            FnBody::Block(block) => {
                for statement in &block.statements {
                    self.statement(module, statement);
                }
            }
            FnBody::Expr(expression) => {
                self.return_value(module, Some(expression), expression.span());
            }
        }
    }

    /// `{ a, b: c }` bound from `base` (a parameter): each property is read
    /// once, as JavaScript does (reading a property of `null` throws).
    fn destructure(
        &mut self,
        m: &Module<'_>,
        object: &oxc_ast::ast::ObjectPattern<'_>,
        base: Expr,
        span: Span,
    ) {
        if object.rest.is_some() {
            self.unknown_op(m, span, "object rest patterns are not supported");
            return;
        }
        for property in &object.properties {
            let (Some(key), BindingPattern::BindingIdentifier(id)) = (
                static_key(&property.key).filter(|_| !property.computed),
                &property.value,
            ) else {
                self.unknown_op(
                    m,
                    property.span,
                    "only `{ a, b: c }` destructuring without defaults or nesting is supported",
                );
                continue;
            };
            let local = self.fresh(id.name.as_str());
            let expr = self.read_prop(base.clone(), &key);
            self.emit(
                m,
                property.span,
                Op::Let {
                    local: local.clone(),
                    expr,
                },
            );
            self.declare(id.name.as_str(), Binding::Local(local));
        }
    }

    // -- statements -------------------------------------------------------

    fn statement(&mut self, m: &Module<'_>, statement: &Statement<'_>) {
        match statement {
            Statement::ExpressionStatement(s) => {
                self.expression_statement(m, &s.expression, s.span)
            }
            Statement::VariableDeclaration(d) => {
                for declarator in &d.declarations {
                    self.const_decl = d.kind == oxc_ast::ast::VariableDeclarationKind::Const;
                    self.declarator(m, declarator);
                    self.const_decl = false;
                }
            }
            Statement::IfStatement(s) => {
                let cond = self.expr(
                    m,
                    &s.test,
                    Cx {
                        truthy: true,
                        ..TOP
                    },
                );
                let jump = self.emit(
                    m,
                    s.test.span(),
                    Op::JumpIf {
                        cond: Expr::not(cond),
                        target: usize::MAX,
                    },
                );
                self.nested(m, &s.consequent);
                match &s.alternate {
                    Some(alternate) => {
                        let end_jump = self.emit(m, s.span, Op::Jump { target: usize::MAX });
                        let else_start = self.here();
                        self.patch(jump, else_start);
                        self.nested(m, alternate);
                        let end = self.here();
                        self.patch(end_jump, end);
                    }
                    None => {
                        let end = self.here();
                        self.patch(jump, end);
                    }
                }
            }
            Statement::BlockStatement(block) => {
                self.frame_mut().scopes.push(HashMap::new());
                for s in &block.body {
                    self.statement(m, s);
                }
                self.frame_mut().scopes.pop();
            }
            Statement::ReturnStatement(s) => {
                self.return_value(m, s.argument.as_ref(), s.span);
            }
            Statement::ThrowStatement(s) => {
                // Phase 16: re-throws and inspected `Error`s keep the error.
                if !self.structured_throw(m, s) {
                    let message = throw_message(&s.argument);
                    self.emit(m, s.span, Op::Throw { message });
                }
            }
            Statement::TryStatement(t) => self.try_statement(m, t),
            Statement::SwitchStatement(sw) => self.switch_statement(m, sw),
            Statement::ForOfStatement(f) => self.for_of(m, f),
            // Phase 16: bounded loops (JavaScript values only).
            Statement::ForStatement(f) if self.for_statement(m, f) => {}
            Statement::WhileStatement(w) if self.while_statement(m, w) => {}
            Statement::DoWhileStatement(d) if self.do_while_statement(m, d) => {}
            Statement::BreakStatement(b)
                if b.label.is_none() && !self.frame().breaks.is_empty() =>
            {
                self.jump_out(m, b.span, false);
            }
            Statement::ContinueStatement(c)
                if c.label.is_none()
                    && self
                        .frame()
                        .breaks
                        .iter()
                        .rev()
                        .find_map(|(_, c, _)| c.as_ref())
                        .is_some() =>
            {
                self.jump_out(m, c.span, true);
            }
            Statement::EmptyStatement(_)
            | Statement::FunctionDeclaration(_)
            | Statement::TSTypeAliasDeclaration(_)
            | Statement::TSInterfaceDeclaration(_) => {}
            other => {
                let what = match other {
                    Statement::ForStatement(_)
                    | Statement::ForInStatement(_)
                    | Statement::ForOfStatement(_) => "`for` loops",
                    Statement::WhileStatement(_) | Statement::DoWhileStatement(_) => {
                        "`while` loops"
                    }
                    Statement::SwitchStatement(_) => "`switch` statements",
                    Statement::BreakStatement(_) | Statement::ContinueStatement(_) => {
                        "`break`/`continue`"
                    }
                    Statement::LabeledStatement(_) => "labeled statements",
                    Statement::ClassDeclaration(_) => "class declarations",
                    _ => "this statement",
                };
                self.unknown_op(
                    m,
                    other.span(),
                    format!("{what} are not part of the supported concurrency subset"),
                );
            }
        }
    }

    /// `try { B } catch (e) { C } finally { F }` (Phase 7). A failure in
    /// `B` (a throw, a rejected await, a database error) resumes at the
    /// handler; `F` runs on the normal path and, re-throwing afterwards, on
    /// the exceptional paths. A crash runs nothing. Not modeled: `try`
    /// inside a transaction callback (PostgreSQL's aborted-transaction
    /// state), exits through a `finally`, and inspecting the caught error.
    fn try_statement(&mut self, m: &Module<'_>, t: &oxc_ast::ast::TryStatement<'_>) {
        if self.tx_open > 0 {
            self.unknown_op(
                m,
                t.span,
                "`try` inside a transaction callback is not modeled (after a database error PostgreSQL's transaction is aborted)",
            );
            return;
        }
        let jumps_out = |statements: &[Statement<'_>]| statements.iter().any(has_jump);
        if t.finalizer.is_some()
            && (jumps_out(&t.block.body)
                || t.handler.as_ref().is_some_and(|h| jumps_out(&h.body.body)))
        {
            self.unknown_op(
                m,
                t.span,
                "`return`/`break`/`continue` inside a `try` with `finally` is not modeled",
            );
            return;
        }
        let rethrow = (t.handler.is_none()).then(|| self.tmp("err"));
        // Phase 16: a handler that uses the caught error (other than to
        // re-throw it) receives the error object.
        let inspected = self.js()
            && t.handler.as_ref().is_some_and(|h| {
                matches!(&h.param, Some(p) if matches!(&p.pattern,
                    BindingPattern::BindingIdentifier(id) if loops::catch_inspects(&h.body.body, id.name.as_str())))
            });
        let caught = inspected.then(|| self.tmp("caught"));
        let shape = if inspected {
            self.p16.caught_seen = true;
            Some(self.caught_shape(m))
        } else if self.p16.inspecting > 0 && rethrow.is_some() {
            // A `finally` inside an inspected block re-throws the error
            // object unchanged.
            Some(self.caught_shape(m))
        } else {
            None
        };
        let enter = self.emit(
            m,
            t.block.span,
            Op::Try {
                handler: usize::MAX,
                error: rethrow.clone().or_else(|| caught.clone()),
                inspect: shape,
            },
        );
        self.frame_mut().tries += 1;
        self.frame_mut().scopes.push(HashMap::new());
        if inspected {
            self.p16.inspecting += 1;
        }
        for statement in &t.block.body {
            self.statement(m, statement);
        }
        if inspected {
            self.p16.inspecting -= 1;
        }
        self.frame_mut().scopes.pop();
        self.frame_mut().tries -= 1;
        self.emit(m, t.block.span, Op::EndTry);
        let finally = |this: &mut Self| {
            if let Some(f) = &t.finalizer {
                this.frame_mut().scopes.push(HashMap::new());
                for statement in &f.body {
                    this.statement(m, statement);
                }
                this.frame_mut().scopes.pop();
            }
        };
        finally(self);
        let mut ends = vec![self.emit(m, t.span, Op::Jump { target: usize::MAX })];
        let handler_pc = self.here();
        self.patch(enter, handler_pc);
        match (&t.handler, &rethrow) {
            (Some(catch), _) => {
                // The catch body may throw: run `finally`, then re-throw.
                let caught_after = t.finalizer.is_some().then(|| self.tmp("err"));
                let inner = caught_after.as_ref().map(|err| {
                    self.emit(
                        m,
                        catch.span,
                        Op::Try {
                            handler: usize::MAX,
                            error: Some(err.clone()),
                            inspect: shape,
                        },
                    )
                });
                if inner.is_some() {
                    self.frame_mut().tries += 1;
                }
                self.frame_mut().scopes.push(HashMap::new());
                if let Some(param) = &catch.param {
                    match &param.pattern {
                        BindingPattern::BindingIdentifier(id) => match &caught {
                            Some(local) => {
                                self.declare(id.name.as_str(), Binding::Caught(local.clone()));
                            }
                            None => self.declare(id.name.as_str(), Binding::Opaque),
                        },
                        _ => {
                            self.unknown_op(
                                m,
                                catch.span,
                                "a destructured catch parameter is not modeled",
                            );
                        }
                    }
                }
                for statement in &catch.body.body {
                    self.statement(m, statement);
                }
                self.frame_mut().scopes.pop();
                if let (Some(inner), Some(err)) = (inner, caught_after) {
                    self.frame_mut().tries -= 1;
                    self.emit(m, catch.span, Op::EndTry);
                    finally(self);
                    ends.push(self.emit(m, t.span, Op::Jump { target: usize::MAX }));
                    let pc = self.here();
                    self.patch(inner, pc);
                    finally(self);
                    self.emit(m, t.span, Op::Rethrow { local: err });
                }
            }
            (None, Some(err)) => {
                finally(self);
                self.emit(m, t.span, Op::Rethrow { local: err.clone() });
            }
            (None, None) => unreachable!("a try statement has a catch or a finally"),
        }
        let end = self.here();
        for jump in ends {
            self.patch(jump, end);
        }
    }

    /// `break` (or `continue`) to the innermost enclosing target.
    fn jump_out(&mut self, m: &Module<'_>, span: Span, cont: bool) {
        let tries = self.frame().tries;
        let index = if cont {
            self.frame()
                .breaks
                .iter()
                .rposition(|(_, c, _)| c.is_some())
        } else {
            self.frame().breaks.len().checked_sub(1)
        };
        let Some(index) = index else {
            self.unknown_op(m, span, "`break`/`continue` without a supported target");
            return;
        };
        let depth = self.frame().breaks[index].2;
        for _ in depth..tries {
            self.emit(m, span, Op::EndTry);
        }
        let jump = self.emit(m, span, Op::Jump { target: usize::MAX });
        let target = &mut self.frame_mut().breaks[index];
        if cont {
            target.1.as_mut().expect("a loop target").push(jump);
        } else {
            target.0.push(jump);
        }
    }

    /// `switch (d) { case a: ... default: ... }` with JavaScript's strict
    /// equality and fall-through. Case tests must be free of side effects
    /// (literals, variables, property reads): JavaScript evaluates them
    /// lazily, the model up front.
    fn switch_statement(&mut self, m: &Module<'_>, sw: &oxc_ast::ast::SwitchStatement<'_>) {
        let pure = |e: &Expression<'_>| {
            matches!(
                e.get_inner_expression(),
                Expression::NumericLiteral(_)
                    | Expression::StringLiteral(_)
                    | Expression::BooleanLiteral(_)
                    | Expression::NullLiteral(_)
                    | Expression::Identifier(_)
                    | Expression::StaticMemberExpression(_)
            )
        };
        if sw
            .cases
            .iter()
            .any(|c| c.test.as_ref().is_some_and(|t| !pure(t)))
        {
            self.unknown_op(
                m,
                sw.span,
                "a `case` test with possible side effects is not modeled",
            );
            return;
        }
        let discriminant = self.expr(m, &sw.discriminant, TOP);
        let tmp = self.tmp("switch");
        self.emit(
            m,
            sw.discriminant.span(),
            Op::Let {
                local: tmp.clone(),
                expr: discriminant,
            },
        );
        let mut entries = Vec::new();
        for case in &sw.cases {
            if let Some(test) = &case.test {
                let value = self.expr(m, test, TOP);
                entries.push(Some(self.emit(
                    m,
                    test.span(),
                    Op::JumpIf {
                        cond: Expr::bin(BinOp::Eq, Expr::Local(tmp.clone()), value),
                        target: usize::MAX,
                    },
                )));
            } else {
                entries.push(None);
            }
        }
        let no_match = self.emit(m, sw.span, Op::Jump { target: usize::MAX });
        let tries = self.frame().tries;
        self.frame_mut().breaks.push((Vec::new(), None, tries));
        self.frame_mut().scopes.push(HashMap::new());
        let mut default_pc = None;
        for (case, entry) in sw.cases.iter().zip(entries) {
            let pc = self.here();
            match entry {
                Some(jump) => self.patch(jump, pc),
                None => default_pc = Some(pc),
            }
            for statement in &case.consequent {
                self.statement(m, statement);
            }
        }
        self.frame_mut().scopes.pop();
        let (breaks, _, _) = self.frame_mut().breaks.pop().expect("pushed above");
        let end = self.here();
        self.patch(no_match, default_pc.unwrap_or(end));
        for jump in breaks {
            self.patch(jump, end);
        }
    }

    /// `for (const x of [a, b, c]) { ... }`: unrolled over the literal's
    /// elements (evaluated first, as JavaScript does). Iterating anything
    /// else has an unknown number of iterations and is not modeled.
    fn for_of(&mut self, m: &Module<'_>, f: &oxc_ast::ast::ForOfStatement<'_>) {
        use oxc_ast::ast::ForStatementLeft;
        let array = match f.right.get_inner_expression() {
            Expression::ArrayExpression(array) => Some(array),
            _ => None,
        };
        let name = match &f.left {
            ForStatementLeft::VariableDeclaration(d) if d.declarations.len() == 1 && !f.r#await => {
                match &d.declarations[0].id {
                    BindingPattern::BindingIdentifier(id) => id.name.as_str().to_owned(),
                    _ => {
                        self.unknown_op(
                            m,
                            f.span,
                            "destructuring in a `for...of` head is not modeled",
                        );
                        return;
                    }
                }
            }
            _ => {
                self.unknown_op(m, f.span, "this `for...of` head is not modeled");
                return;
            }
        };
        // Phase 14: a loop over a list value (JavaScript values only).
        let Some(array) = array else {
            if !self.for_of_value(m, f, &name) {
                self.unknown_op(
                    m,
                    f.span,
                    "a loop over a collection of unknown length is not modeled (only `for (const x of [literal, ...])`)",
                );
            }
            return;
        };
        let mut elements = Vec::new();
        for element in &array.elements {
            match element.as_expression() {
                Some(e) => elements.push((self.expr(m, e, TOP), element.span())),
                None => {
                    self.unknown_op(m, element.span(), "a spread or hole in a `for...of` array");
                    return;
                }
            }
        }
        let tries = self.frame().tries;
        self.frame_mut()
            .breaks
            .push((Vec::new(), Some(Vec::new()), tries));
        // Phase 16: the loop's classification (JavaScript values only).
        self.for_of_completed(
            m,
            f.span,
            &format!("an array literal of {} element(s)", elements.len()),
        );
        for (value, span) in elements {
            self.frame_mut().scopes.push(HashMap::new());
            let local = self.fresh(&name);
            self.emit(
                m,
                span,
                Op::Let {
                    local: local.clone(),
                    expr: value,
                },
            );
            self.declare(&name, Binding::Local(local));
            self.statement(m, &f.body);
            self.frame_mut().scopes.pop();
            // `continue` goes to the next iteration.
            let next = self.here();
            let continues = std::mem::take(
                self.frame_mut()
                    .breaks
                    .last_mut()
                    .expect("pushed above")
                    .1
                    .as_mut()
                    .expect("a loop target"),
            );
            for jump in continues {
                self.patch(jump, next);
            }
        }
        let (breaks, _, _) = self.frame_mut().breaks.pop().expect("pushed above");
        let end = self.here();
        for jump in breaks {
            self.patch(jump, end);
        }
    }

    fn nested(&mut self, m: &Module<'_>, statement: &Statement<'_>) {
        self.frame_mut().scopes.push(HashMap::new());
        self.statement(m, statement);
        self.frame_mut().scopes.pop();
    }

    fn return_value(&mut self, m: &Module<'_>, argument: Option<&Expression<'_>>, span: Span) {
        let value = match argument {
            None => self.undef(),
            Some(arg) => match arg.get_inner_expression() {
                Expression::AwaitExpression(a) => {
                    let res = self.call_value(m, &a.argument, true, Want::Scalar, a.span);
                    self.res_expr(res)
                }
                Expression::CallExpression(call) => {
                    // Returning a promise from an async function is awaited
                    // by the caller.
                    let res = self.call_value(m, arg, true, Want::Scalar, call.span);
                    self.res_expr(res)
                }
                _ => self.expr(m, arg, TOP),
            },
        };
        let ret = self.frame().rets.last().map(|r| (r.main, r.result.clone()));
        let (main, result) = ret.expect("a return target exists");
        if main {
            // Phase 13: what the entry returns, when an invariant reads it
            // (an expression not understood stays `Expr::Unknown`).
            if self.request.return_values && self.cur == 0 {
                self.emit(m, span, Op::ReturnValue { expr: value });
            }
            self.emit(m, span, Op::Return);
        } else {
            // Returning from an inlined helper leaves its open `try` blocks.
            for _ in 0..self.frame().tries {
                self.emit(m, span, Op::EndTry);
            }
            self.emit(
                m,
                span,
                Op::Let {
                    local: result,
                    expr: value,
                },
            );
            let jump = self.emit(m, span, Op::Jump { target: usize::MAX });
            self.frame_mut()
                .rets
                .last_mut()
                .expect("a return target exists")
                .patches
                .push(jump);
        }
    }

    fn res_expr(&mut self, res: Res) -> Expr {
        match res {
            Res::Unit => Expr::Lit(Value::Null),
            Res::Value(e) => e,
            Res::Row(l) => Expr::Local(l),
            Res::Affected(_) | Res::Aggregate { .. } | Res::Rows { .. } => {
                Expr::Unknown("an array of rows used as a value".to_owned())
            }
            Res::Fields(_) => Expr::Unknown(
                "a validation result is only modeled through its properties".to_owned(),
            ),
        }
    }

    fn expression_statement(&mut self, m: &Module<'_>, expression: &Expression<'_>, span: Span) {
        // Phase 16: `x++`, `x += k` on locals (JavaScript values only).
        if self.update_statement(m, expression, span) {
            return;
        }
        match expression.get_inner_expression() {
            Expression::AwaitExpression(a) => {
                self.call_value(m, &a.argument, true, Want::Discard, span);
            }
            Expression::CallExpression(_) => {
                self.call_value(m, expression, false, Want::Discard, span);
            }
            Expression::AssignmentExpression(assign)
                if assign.operator == AssignmentOperator::Assign =>
            {
                use oxc_ast::ast::AssignmentTarget;
                if let AssignmentTarget::AssignmentTargetIdentifier(id) = &assign.left
                    && let Some(Binding::Local(local)) = self.lookup(id.name.as_str())
                {
                    let value = self.value_of(m, &assign.right);
                    self.emit(m, span, Op::Let { local, expr: value });
                } else {
                    self.unknown_op(m, span, "assignment target is not a local variable");
                }
            }
            _ => {
                self.unknown_op(
                    m,
                    span,
                    "expression statement is not part of the supported concurrency subset",
                );
            }
        }
    }

    /// The value of an initializer or right-hand side, hoisting awaited
    /// calls.
    fn value_of(&mut self, m: &Module<'_>, init: &Expression<'_>) -> Expr {
        self.expr(m, init, TOP)
    }

    fn declarator(&mut self, m: &Module<'_>, d: &VariableDeclarator<'_>) {
        let Some(init) = &d.init else {
            // `let x;`
            if let BindingPattern::BindingIdentifier(id) = &d.id {
                let local = self.fresh(id.name.as_str());
                self.declare(id.name.as_str(), Binding::Local(local.clone()));
                let expr = self.undef();
                self.emit(m, d.span, Op::Let { local, expr });
            } else {
                self.unknown_op(m, d.span, "destructuring declaration without initializer");
            }
            return;
        };
        let inner = init.get_inner_expression();
        let (call, awaited): (Option<&Expression<'_>>, bool) = match inner {
            Expression::AwaitExpression(a) => (Some(&a.argument), true),
            Expression::CallExpression(_) => (Some(inner), false),
            _ => (None, false),
        };
        match &d.id {
            BindingPattern::BindingIdentifier(id) => {
                let name = id.name.as_str();
                // Phase 16: `const p = and(eq(..), ..)` (a Drizzle predicate,
                // an alias of one, a conditional choosing one, or a call of a
                // project helper returning one): lowered here, used by
                // `where(p)`.
                if self.predicate_shaped(m, inner) {
                    let held = self.held_predicate(m, d, name, inner);
                    self.declare(name, Binding::Pred(held));
                    return;
                }
                // Phase 12: `const db = getDb()` (or `await getDb()`) of a
                // database factory binds a root database handle. Phase 13:
                // also `db.$primary`, `db.usePrimaryOnly()` of a replicated
                // database and calls of functions whose result is a known
                // database (`await connectDb()`).
                if matches!(
                    inner,
                    Expression::CallExpression(_)
                        | Expression::AwaitExpression(_)
                        | Expression::StaticMemberExpression(_)
                ) && let Some(handle) = self.handle_expr(m, inner)
                {
                    self.declare(name, handle);
                    return;
                }
                // Phase 14: `const m = new Map()` / `new Set()`.
                if let Expression::NewExpression(new) = inner
                    && self.declare_collection(m, d, name, id.span, new).is_some()
                {
                    return;
                }
                // Phase 12: `const repo = new Repository(tx)` of an analyzed
                // class.
                if let Expression::NewExpression(new) = inner {
                    match self.new_instance(m, new) {
                        Ok(this) => {
                            self.declare(name, Binding::Instance(Box::new(this)));
                        }
                        Err(why) => {
                            self.unknown_op(m, new.span, format!("`new`: {why}"));
                            self.declare(name, Binding::Opaque);
                        }
                    }
                    return;
                }
                if let Some(call) = call {
                    let res = self.call_value(m, call, awaited, Want::Scalar, init.span());
                    self.bind_result(m, &d.id, res, d.span, false);
                    self.note_const_string(m, d.span, name);
                } else if let Expression::Identifier(src) = inner
                    && self.handle_of(src.name.as_str()).is_some()
                {
                    let handle = self.handle_of(src.name.as_str()).expect("checked");
                    self.declare(name, handle);
                } else if let Expression::ObjectExpression(object) = inner {
                    // Properties are evaluated now, in source order.
                    let mut fields = Vec::new();
                    for property in &object.properties {
                        match property {
                            ObjectPropertyKind::ObjectProperty(p)
                                if !p.computed
                                    && !p.method
                                    && p.kind == oxc_ast::ast::PropertyKind::Init =>
                            {
                                let Some(key) = static_key(&p.key) else {
                                    self.unknown_op(m, p.span, "object literal key is not static");
                                    continue;
                                };
                                let value = self.expr(m, &p.value, TOP);
                                fields.retain(|(k, _): &(String, Expr)| *k != key);
                                fields.push((key, value));
                            }
                            other => {
                                self.unknown_op(
                                    m,
                                    other.span(),
                                    "spread, computed, method or accessor property in an object literal",
                                );
                            }
                        }
                    }
                    self.declare(name, Binding::Object(fields));
                } else {
                    let value = self.value_of(m, init);
                    let local = self.fresh(name);
                    self.emit(
                        m,
                        d.span,
                        Op::Let {
                            local: local.clone(),
                            expr: value,
                        },
                    );
                    self.declare(name, Binding::Local(local));
                    self.note_const_string(m, d.span, name);
                }
            }
            BindingPattern::ArrayPattern(array) => {
                // Phase 14: `const [a, b] = list` (JavaScript values; the
                // Phase 7 `const [row] = await query` form is kept).
                let legacy = array.rest.is_none()
                    && array.elements.len() == 1
                    && array.elements[0].is_some()
                    && awaited
                    && call.is_some_and(|c| !self.js() || self.database_call(m, c));
                if self.js() && !legacy {
                    let value = match call {
                        Some(call) => self.call_value(m, call, awaited, Want::Scalar, init.span()),
                        None => Res::Value(self.value_of(m, init)),
                    };
                    self.destructure_list(m, array, value, d.span);
                    return;
                }
                if array.rest.is_some() || array.elements.len() != 1 || array.elements[0].is_none()
                {
                    self.unknown_op(
                        m,
                        d.span,
                        "array destructuring is only supported as `const [row] = await ...`",
                    );
                    return;
                }
                let Some(call) = call.filter(|_| awaited) else {
                    self.unknown_op(
                        m,
                        d.span,
                        "array destructuring is only supported for an awaited query",
                    );
                    return;
                };
                let res = self.call_value(m, call, true, Want::Single, init.span());
                let element = array.elements[0].as_ref().expect("checked");
                self.bind_result(m, element, res, d.span, true);
            }
            BindingPattern::AssignmentPattern(_) => {
                self.unknown_op(
                    m,
                    d.span,
                    "default values in declarations are not supported",
                );
            }
            BindingPattern::ObjectPattern(object) => {
                if object.rest.is_some() {
                    self.unknown_op(m, d.span, "object rest patterns are not supported");
                    return;
                }
                let base = if let Some(call) = call {
                    let res = self.call_value(m, call, awaited, Want::Scalar, init.span());
                    self.res_expr(res)
                } else {
                    self.value_of(m, init)
                };
                for property in &object.properties {
                    let (Some(key), BindingPattern::BindingIdentifier(id)) = (
                        static_key(&property.key).filter(|_| !property.computed),
                        &property.value,
                    ) else {
                        self.unknown_op(m, property.span, "unsupported destructuring property");
                        continue;
                    };
                    let local = self.fresh(id.name.as_str());
                    let expr = self.read_prop(base.clone(), &key);
                    self.emit(
                        m,
                        property.span,
                        Op::Let {
                            local: local.clone(),
                            expr,
                        },
                    );
                    self.declare(id.name.as_str(), Binding::Local(local));
                }
            }
        }
    }

    fn bind_result(
        &mut self,
        m: &Module<'_>,
        pattern: &BindingPattern<'_>,
        res: Res,
        span: Span,
        single: bool,
    ) {
        match pattern {
            BindingPattern::BindingIdentifier(id) => {
                let name = id.name.as_str();
                match res {
                    Res::Row(local) => self.declare(name, Binding::Local(local)),
                    Res::Rows { row, length } if single => {
                        self.declare(name, Binding::Local(row));
                        let _ = length;
                    }
                    Res::Rows { row, length } => self.declare(name, Binding::Rows { row, length }),
                    Res::Affected(count) if single => {
                        // `const [row] = await ...returning()`: only the
                        // truthiness of `row` (`count > 0`) is modeled.
                        self.declare(name, Binding::Presence(count));
                    }
                    Res::Affected(count) => {
                        let mut fields = BTreeMap::new();
                        fields.insert("length".to_owned(), count);
                        self.declare(name, Binding::Virtual(fields));
                    }
                    Res::Aggregate { field, local } => {
                        let mut fields = BTreeMap::new();
                        fields.insert(field, local);
                        self.declare(name, Binding::Virtual(fields));
                    }
                    Res::Fields(fields) => self.declare(name, Binding::Virtual(fields)),
                    other => {
                        let value = match other {
                            Res::Unit => Expr::Lit(Value::Null),
                            Res::Value(e) => e,
                            _ => unreachable!(),
                        };
                        let local = self.fresh(name);
                        self.emit(
                            m,
                            span,
                            Op::Let {
                                local: local.clone(),
                                expr: value,
                            },
                        );
                        self.declare(name, Binding::Local(local));
                    }
                }
            }
            BindingPattern::ObjectPattern(object) => match res {
                // Phase 14: `const { a } = schema.parse(x)` and other
                // values: each property read once, in order.
                Res::Value(e) => {
                    let tmp = self.tmp("obj");
                    self.emit(
                        m,
                        span,
                        Op::Let {
                            local: tmp.clone(),
                            expr: e,
                        },
                    );
                    self.destructure(m, object, Expr::Local(tmp), span);
                }
                Res::Aggregate { field, local } => {
                    for property in &object.properties {
                        match (static_key(&property.key), &property.value) {
                            (Some(key), BindingPattern::BindingIdentifier(id)) if key == field => {
                                self.declare(id.name.as_str(), Binding::Local(local.clone()));
                            }
                            _ => {
                                self.unknown_op(
                                    m,
                                    property.span,
                                    "only the aggregate column can be destructured",
                                );
                            }
                        }
                    }
                }
                _ => {
                    self.unknown_op(
                        m,
                        span,
                        "object destructuring of this result is not supported",
                    );
                }
            },
            BindingPattern::ArrayPattern(_) | BindingPattern::AssignmentPattern(_) => {
                self.unknown_op(m, span, "nested destructuring of a result is not supported");
            }
        }
    }

    // -- expressions ------------------------------------------------------

    fn expr(&mut self, m: &Module<'_>, e: &Expression<'_>, cx: Cx) -> Expr {
        let e = e.get_inner_expression();
        // Phase 16: `++x` in a value, `new Response(...)`.
        if self.js() {
            match e {
                Expression::UpdateExpression(u) => return self.update_value(m, u, cx),
                Expression::NewExpression(new) => {
                    if let Some(value) = self.new_response(m, new) {
                        return value;
                    }
                }
                _ => {}
            }
        }
        // Phase 14: JavaScript values (inactive without `js_values`).
        if let Some(value) = self.js_expr(m, e, cx) {
            return value;
        }
        match e {
            Expression::NumericLiteral(n) => match integer(n.value) {
                Some(v) => Expr::lit(v),
                None => Expr::Unknown(format!("non-integer number {}", n.value)),
            },
            Expression::StringLiteral(s) => {
                Expr::Lit(Value::Str(s.value.as_str().unwrap_or_default().to_owned()))
            }
            Expression::BooleanLiteral(b) => Expr::lit(b.value),
            Expression::NullLiteral(_) => Expr::Lit(Value::Null),
            Expression::Identifier(id) => self.identifier(m, id.name.as_str(), cx),
            Expression::StaticMemberExpression(member) => self.member(
                m,
                &member.object,
                member.property.name.as_str(),
                member.optional,
                cx,
            ),
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::StaticMemberExpression(member) => self.member(
                    m,
                    &member.object,
                    member.property.name.as_str(),
                    member.optional,
                    cx,
                ),
                _ => Expr::Unknown("optional chain form is not supported".to_owned()),
            },
            Expression::UnaryExpression(u) => match u.operator {
                UnaryOperator::LogicalNot => {
                    let inner = self.expr(
                        m,
                        &u.argument,
                        Cx {
                            truthy: true,
                            col_ok: cx.col_ok,
                            cond: cx.cond,
                        },
                    );
                    Expr::not(inner)
                }
                UnaryOperator::UnaryNegation => match literal(&u.argument) {
                    Some(Expr::Lit(Value::Int(n))) => Expr::lit(-n),
                    _ => {
                        let inner = self.expr(m, &u.argument, cx);
                        Expr::bin(BinOp::Sub, Expr::lit(0), inner)
                    }
                },
                other => Expr::Unknown(format!("unary operator `{}`", other.as_str())),
            },
            Expression::BinaryExpression(b) => self.binary(m, b, cx),
            Expression::LogicalExpression(l)
                if !cx.col_ok && (!cx.truthy || l.operator == LogicalOperator::Coalesce) =>
            {
                // In value position `a || b` is `a` or `b`, not a boolean:
                // evaluate `a`, then `b` only when JavaScript would.
                self.logical_value(m, l, cx)
            }
            Expression::LogicalExpression(l) => {
                let op = match l.operator {
                    LogicalOperator::And => BinOp::And,
                    LogicalOperator::Or => BinOp::Or,
                    LogicalOperator::Coalesce => {
                        return Expr::Unknown("`??` in a query is not supported".to_owned());
                    }
                };
                let left = self.expr(m, &l.left, cx);
                let right = self.expr(m, &l.right, Cx { cond: true, ..cx });
                Expr::bin(op, left, right)
            }
            Expression::ConditionalExpression(c) => self.conditional(m, c, cx),
            Expression::ObjectExpression(object) if !cx.col_ok => {
                let mut fields: Vec<(String, Expr)> = Vec::new();
                for property in &object.properties {
                    match property {
                        ObjectPropertyKind::ObjectProperty(p)
                            if !p.computed
                                && !p.method
                                && p.kind == oxc_ast::ast::PropertyKind::Init =>
                        {
                            let Some(key) = static_key(&p.key) else {
                                return Expr::Unknown(
                                    "object literal key is not static".to_owned(),
                                );
                            };
                            let value = self.expr(
                                m,
                                &p.value,
                                Cx {
                                    truthy: false,
                                    ..cx
                                },
                            );
                            fields.retain(|(k, _)| *k != key);
                            fields.push((key, value));
                        }
                        _ => {
                            return Expr::Unknown(
                                "spread, computed, method or accessor property in an object literal".to_owned(),
                            );
                        }
                    }
                }
                Expr::Object(fields)
            }
            Expression::ComputedMemberExpression(cm) => {
                let index = match cm.expression.get_inner_expression() {
                    Expression::NumericLiteral(n) => integer(n.value),
                    _ => None,
                };
                match (cm.object.get_inner_expression(), index) {
                    (Expression::Identifier(id), Some(i)) => match self.lookup(id.name.as_str()) {
                        Some(Binding::Rows { row, .. }) if i == 0 => Expr::Local(row),
                        // An array of at most one row has nothing at 1, 2, ...
                        Some(Binding::Rows { .. }) if i > 0 => Expr::Lit(Value::Null),
                        _ => Expr::Unknown("computed member access".to_owned()),
                    },
                    _ => Expr::Unknown("computed member access".to_owned()),
                }
            }
            // Phase 13: a template literal without interpolation is a string.
            Expression::TemplateLiteral(t) if t.expressions.is_empty() && t.quasis.len() == 1 => {
                match t.quasis[0].value.cooked.as_ref().and_then(|c| c.as_str()) {
                    Some(text) => Expr::Lit(Value::Str(text.to_owned())),
                    None => Expr::Unknown(
                        "expression of kind `template literal` is not supported: an invalid escape"
                            .to_owned(),
                    ),
                }
            }
            Expression::TemplateLiteral(_) => Expr::Unknown(
                "expression of kind `template literal` is not supported: interpolation builds a string, and string concatenation is not modeled"
                    .to_owned(),
            ),
            Expression::AwaitExpression(a) => {
                if cx.cond {
                    return Expr::Unknown(
                        "`await` inside a conditionally evaluated operand cannot be hoisted exactly"
                            .to_owned(),
                    );
                }
                let res = self.call_value(m, &a.argument, true, Want::Scalar, a.span);
                self.res_expr(res)
            }
            Expression::CallExpression(call) => {
                if cx.cond {
                    return Expr::Unknown(
                        "call inside a conditionally evaluated operand cannot be hoisted exactly"
                            .to_owned(),
                    );
                }
                // Synchronous helpers can be inlined in expression position.
                let res = self.call_value(m, e, false, Want::Scalar, call.span);
                self.res_expr(res)
            }
            other => Expr::Unknown(format!(
                "expression of kind `{}` is not supported",
                expression_kind(other)
            )),
        }
    }

    fn identifier(&mut self, m: &Module<'_>, name: &str, cx: Cx) -> Expr {
        if name == "undefined" {
            return self.undef();
        }
        match self.lookup(name) {
            Some(Binding::Local(local)) => Expr::Local(local),
            Some(Binding::Presence(count)) => {
                if cx.truthy {
                    Expr::bin(BinOp::Gt, Expr::Local(count), Expr::lit(0))
                } else {
                    Expr::Unknown(format!(
                        "`{name}` (a row returned by a mutation) is only modeled in truthiness tests"
                    ))
                }
            }
            Some(Binding::Tx | Binding::Db | Binding::Replicated(_) | Binding::UnknownDb(_)) => {
                Expr::Unknown(format!("database handle `{name}` used as a value"))
            }
            Some(Binding::Virtual(_)) => Expr::Unknown(format!(
                "`{name}` is only modeled through its listed fields"
            )),
            // An array is truthy whether or not it is empty.
            Some(Binding::Rows { .. }) if cx.truthy => Expr::lit(true),
            Some(Binding::Rows { .. }) => Expr::Unknown(format!(
                "`{name}` (an array of rows) is only modeled through `length` and `[0]`"
            )),
            Some(Binding::Instance(_)) if cx.truthy => Expr::lit(true),
            Some(Binding::Instance(_)) => Expr::Unknown(format!(
                "`{name}` (an instance of an analyzed class) is not a value"
            )),
            Some(Binding::Opaque) if cx.truthy => Expr::lit(true),
            Some(Binding::Opaque) => Expr::Unknown(format!(
                "the caught error `{name}` is not modeled (only that it was thrown)"
            )),
            // Phase 16: an error object is truthy; as a value it is the
            // object (whose unknown properties stay unknown).
            Some(Binding::Caught(_)) if cx.truthy => Expr::lit(true),
            Some(Binding::Caught(local)) => Expr::Local(local),
            Some(Binding::Request(_)) if cx.truthy => Expr::lit(true),
            Some(Binding::Request(_)) => Expr::Unknown(format!(
                "the request `{name}` is only modeled through `await {name}.json()`"
            )),
            Some(Binding::Zod(_)) if cx.truthy => Expr::lit(true),
            Some(Binding::Zod(_)) => Expr::Unknown(format!(
                "the zod schema `{name}` is only modeled through `parse`/`safeParse`"
            )),
            // Phase 14: a function value is truthy; it is otherwise only
            // modeled when called.
            Some(Binding::Closure(_)) if cx.truthy => Expr::lit(true),
            Some(Binding::Closure(c)) => Expr::Unknown(format!(
                "the function `{name}` ({}) is only modeled when it is called",
                c.name
            )),
            Some(Binding::Object(_)) if cx.truthy => Expr::lit(true),
            Some(Binding::Object(_)) => Expr::Unknown(format!(
                "the object `{name}` is only modeled through its properties"
            )),
            // Phase 16: a Drizzle predicate (an SQL object) is truthy; its
            // value is only used by the queries it is passed to.
            Some(Binding::Pred(_)) if cx.truthy => Expr::lit(true),
            Some(Binding::Pred(_)) => Expr::Unknown(format!(
                "the Drizzle predicate `{name}` is only modeled where a query uses it (`where`, `and`, `or`, `not`)"
            )),
            None => {
                if let Some(constant) = module_constant(m, name) {
                    return constant;
                }
                // Phase 14: a module-level array of literals (JavaScript
                // value semantics only, so Phase 13 extraction is unchanged).
                if self.request.js_values
                    && let Some((list, readonly)) = module_list_constant(m, name)
                {
                    if readonly {
                        self.assume(format!(
                            "`{name}` is `as const` (a readonly tuple): it is not mutated (casts are not tracked)"
                        ));
                    }
                    return list;
                }
                let _ = cx;
                if self.table_ref(m, name).is_some() {
                    return Expr::Unknown(format!("table `{name}` used as a value"));
                }
                Expr::Unknown(format!("unresolved identifier `{name}`"))
            }
        }
    }

    fn member(
        &mut self,
        m: &Module<'_>,
        object: &Expression<'_>,
        property: &str,
        optional: bool,
        cx: Cx,
    ) -> Expr {
        let object = object.get_inner_expression();
        if let Expression::Identifier(id) = object {
            let name = id.name.as_str();
            match self.lookup(name) {
                Some(Binding::Virtual(fields)) => {
                    return match fields.get(property) {
                        Some(local) => Expr::Local(local.clone()),
                        None => Expr::Unknown(format!("`{name}.{property}` is not modeled")),
                    };
                }
                Some(Binding::Presence(_)) => {
                    return Expr::Unknown(format!(
                        "`{name}.{property}`: fields of a returned row are not modeled"
                    ));
                }
                Some(Binding::Opaque) => {
                    return Expr::Unknown(format!(
                        "`{name}.{property}`: the caught error's properties are not modeled"
                    ));
                }
                // Phase 16: a property of an error object (never nullish, so
                // `e?.p` is `e.p`).
                Some(Binding::Caught(local)) => {
                    if property.starts_with('$') {
                        return Expr::Unknown(format!(
                            "`{name}.{property}`: this property of a caught error is not modeled"
                        ));
                    }
                    return self.read_prop(Expr::Local(local), property);
                }
                Some(Binding::Request(_)) => {
                    return Expr::Unknown(format!(
                        "`{name}.{property}`: only `await {name}.json()` of a request is modeled"
                    ));
                }
                Some(Binding::Rows { length, .. }) => {
                    return if property == "length" {
                        Expr::Local(length)
                    } else {
                        Expr::Unknown(format!("`{name}.{property}` on an array of rows"))
                    };
                }
                Some(Binding::Object(fields)) => {
                    // A missing property of an object literal is `undefined`.
                    return fields
                        .iter()
                        .find(|(k, _)| k == property)
                        .map_or_else(|| self.undef(), |(_, e)| e.clone());
                }
                None => {
                    if let Some(table) = self.table_ref(m, name) {
                        if !cx.col_ok {
                            return Expr::Unknown(format!(
                                "column reference `{name}.{property}` outside a query"
                            ));
                        }
                        if !table.columns.iter().any(|c| c == property) {
                            return Expr::Unknown(format!(
                                "`{property}` is not a column of table `{}` (property names must equal column names)",
                                table.table
                            ));
                        }
                        if self.qualify {
                            return Expr::Col(format!("{}.{property}", table.table));
                        }
                        return Expr::Col(property.to_owned());
                    }
                    if let Some(value) = self.enum_member(m, name, property) {
                        return value;
                    }
                }
                _ => {}
            }
        }
        if optional {
            // `a?.b`: exact only for a plain local base, hoisted as
            // `t = null; if a != null { t = a.b }`.
            if cx.cond {
                return Expr::Unknown(
                    "`?.` inside a conditionally evaluated operand cannot be hoisted exactly"
                        .to_owned(),
                );
            }
            let base = match object {
                Expression::Identifier(id) => match self.lookup(id.name.as_str()) {
                    Some(Binding::Local(local)) => Expr::Local(local),
                    _ => return Expr::Unknown("`?.` on a non-local base".to_owned()),
                },
                // Phase 16: `e.cause?.code` on a caught error: the base (a
                // property read) is evaluated first, into a local.
                _ if self.js() && self.caught_root(object) => {
                    let value = self.expr(
                        m,
                        object,
                        Cx {
                            truthy: false,
                            ..cx
                        },
                    );
                    let tmp = self.tmp("base");
                    self.emit(
                        m,
                        object.span(),
                        Op::Let {
                            local: tmp.clone(),
                            expr: value,
                        },
                    );
                    Expr::Local(tmp)
                }
                _ => return Expr::Unknown("`?.` on a non-identifier base".to_owned()),
            };
            let tmp = self.tmp("opt");
            let span = object.span();
            let undefined = self.undef();
            self.emit(
                m,
                span,
                Op::Let {
                    local: tmp.clone(),
                    expr: undefined,
                },
            );
            let cond = self.nullish(base.clone());
            let jump = self.emit(
                m,
                span,
                Op::JumpIf {
                    cond,
                    target: usize::MAX,
                },
            );
            let expr = self.read_prop(base, property);
            self.emit(
                m,
                span,
                Op::Let {
                    local: tmp.clone(),
                    expr,
                },
            );
            let end = self.here();
            self.patch(jump, end);
            return Expr::Local(tmp);
        }
        // Phase 14: `a?.b.c` short-circuits the whole chain when `a` is
        // nullish; only a single optional link is modeled.
        if self.js() && has_optional_link(object) {
            return Expr::Unknown(
                "an optional chain with a link after `?.` is not modeled".to_owned(),
            );
        }
        let base = self.expr(
            m,
            object,
            Cx {
                truthy: false,
                ..cx
            },
        );
        self.read_prop(base, property)
    }

    /// `a || b`, `a && b`, `a ?? b` whose *value* is used: hoisted as
    /// `t = a; if <short-circuit> skip; t = b`, so `b` (and any call in it)
    /// is evaluated exactly when JavaScript evaluates it.
    fn logical_value(
        &mut self,
        m: &Module<'_>,
        l: &oxc_ast::ast::LogicalExpression<'_>,
        cx: Cx,
    ) -> Expr {
        if cx.cond {
            return Expr::Unknown(
                "a value-producing `||`/`&&`/`??` inside a conditionally evaluated operand"
                    .to_owned(),
            );
        }
        let left = self.expr(
            m,
            &l.left,
            Cx {
                truthy: false,
                ..cx
            },
        );
        let tmp = self.tmp("logic");
        self.emit(
            m,
            l.left.span(),
            Op::Let {
                local: tmp.clone(),
                expr: left,
            },
        );
        let skip = match l.operator {
            LogicalOperator::Or => Expr::Local(tmp.clone()),
            LogicalOperator::And => Expr::not(Expr::Local(tmp.clone())),
            LogicalOperator::Coalesce => Expr::not(self.nullish(Expr::Local(tmp.clone()))),
        };
        let jump = self.emit(
            m,
            l.span,
            Op::JumpIf {
                cond: skip,
                target: usize::MAX,
            },
        );
        let right = self.expr(
            m,
            &l.right,
            Cx {
                truthy: false,
                ..cx
            },
        );
        self.emit(
            m,
            l.right.span(),
            Op::Let {
                local: tmp.clone(),
                expr: right,
            },
        );
        let end = self.here();
        self.patch(jump, end);
        Expr::Local(tmp)
    }

    /// `E.MEMBER` of a TypeScript `enum` (or a `const` object literal of
    /// literals) declared in this module or imported from an analyzed one.
    fn enum_member(&mut self, m: &Module<'_>, name: &str, member: &str) -> Option<Expr> {
        if let Some(v) = enum_member_in(m, name, member) {
            return Some(v);
        }
        let (source, export) = match find_import(m, name)? {
            ImportRef::Named { source, name } => (source, name),
            _ => return None,
        };
        let Resolution::File(index) = self.ctx.resolve(&m.abs, &source) else {
            return None;
        };
        let abs = self.ctx.files[index].clone();
        let text = self.ctx.source(&abs).ok()?;
        let allocator = Allocator::default();
        let program = load(&allocator, &abs, &text).ok()?;
        let module = Module::new(&abs, relative(&self.root, &abs), &text, &program);
        enum_member_in(&module, &export, member)
    }

    fn binary(&mut self, m: &Module<'_>, b: &oxc_ast::ast::BinaryExpression<'_>, cx: Cx) -> Expr {
        use BinaryOperator as B;
        // Phase 16: `e instanceof Error` of a caught error.
        if let Some(value) = self.caught_instanceof(m, b) {
            return value;
        }
        let op = match b.operator {
            B::StrictEquality | B::Equality => BinOp::Eq,
            B::StrictInequality | B::Inequality => BinOp::Ne,
            B::LessThan => BinOp::Lt,
            B::LessEqualThan => BinOp::Le,
            B::GreaterThan => BinOp::Gt,
            B::GreaterEqualThan => BinOp::Ge,
            B::Addition => BinOp::Add,
            B::Subtraction => BinOp::Sub,
            B::Multiplication => BinOp::Mul,
            other => {
                return Expr::Unknown(format!("binary operator `{}`", other.as_str()));
            }
        };
        // `row === undefined` / `row == null` on a returned-row binding.
        if matches!(op, BinOp::Eq | BinOp::Ne) {
            for (a, c) in [(&b.left, &b.right), (&b.right, &b.left)] {
                if let Expression::Identifier(id) = a.get_inner_expression()
                    && let Some(Binding::Presence(count)) = self.lookup(id.name.as_str())
                    && is_nullish(c)
                {
                    let present = Expr::bin(BinOp::Gt, Expr::Local(count), Expr::lit(0));
                    return if op == BinOp::Eq {
                        Expr::not(present)
                    } else {
                        present
                    };
                }
            }
        }
        // Phase 14: loose equality and string concatenation.
        if let Some(value) = self.js_binary(m, b, cx) {
            return value;
        }
        let left = self.expr(
            m,
            &b.left,
            Cx {
                truthy: false,
                ..cx
            },
        );
        let right = self.expr(
            m,
            &b.right,
            Cx {
                truthy: false,
                ..cx
            },
        );
        Expr::bin(op, left, right)
    }

    fn conditional(
        &mut self,
        m: &Module<'_>,
        c: &oxc_ast::ast::ConditionalExpression<'_>,
        cx: Cx,
    ) -> Expr {
        if cx.cond {
            return Expr::Unknown(
                "conditional expression inside a conditionally evaluated operand".to_owned(),
            );
        }
        let test = self.expr(m, &c.test, Cx { truthy: true, ..cx });
        let tmp = self.tmp("cond");
        let jump = self.emit(
            m,
            c.test.span(),
            Op::JumpIf {
                cond: Expr::not(test),
                target: usize::MAX,
            },
        );
        let then_value = self.expr(m, &c.consequent, cx);
        self.emit(
            m,
            c.consequent.span(),
            Op::Let {
                local: tmp.clone(),
                expr: then_value,
            },
        );
        let end_jump = self.emit(m, c.span, Op::Jump { target: usize::MAX });
        let else_start = self.here();
        self.patch(jump, else_start);
        let else_value = self.expr(m, &c.alternate, cx);
        self.emit(
            m,
            c.alternate.span(),
            Op::Let {
                local: tmp.clone(),
                expr: else_value,
            },
        );
        let end = self.here();
        self.patch(end_jump, end);
        Expr::Local(tmp)
    }

    // -- calls --------------------------------------------------------------

    fn call_value(
        &mut self,
        m: &Module<'_>,
        e: &Expression<'_>,
        awaited: bool,
        want: Want,
        span: Span,
    ) -> Res {
        let e = e.get_inner_expression();
        let Expression::CallExpression(call) = e else {
            return self.unknown_op(
                m,
                span,
                "`await` of something other than a direct call is not supported",
            );
        };
        // Phase 14: built-in methods on values (inactive without
        // `js_values`).
        if let Some(res) = self.value_method(m, call, want, call.span) {
            return res;
        }
        // Phase 16: `await request.json()`, `Response.json(...)`.
        if let Some(res) = self.http_call(m, call, awaited) {
            return res;
        }
        let text = callee_text(&call.callee);
        if text.as_deref() == Some("Promise.all") {
            return self.promise_all(m, call, awaited, want);
        }
        if let Some((root, steps)) = flatten(e)
            && steps[0].path.is_empty()
            && matches!(
                steps[0].method.as_str(),
                "select" | "insert" | "update" | "delete" | "transaction" | "execute"
            )
            && self.frames.len() == 1
            && let Some((ty, why)) = self.param_types.get(&root).cloned()
        {
            return self.unknown_op(
                m,
                call.span,
                if why.is_empty() {
                    format!(
                        "database handle of an unresolved type: parameter `{root}: {ty}` is not a drizzle-orm database type (its operations are not bound)"
                    )
                } else {
                    format!(
                        "database handle of an unresolved type: parameter `{root}: {ty}` is not a drizzle-orm database type (its operations are not bound): {why}"
                    )
                },
            );
        }
        if let Some((root, mut steps)) = flatten(e)
            && root == "this"
            && let Some(field) = steps[0].path.first().cloned()
            && let Some(handle) = self.this_field_handle(&field)
        {
            steps[0].path.remove(0);
            let root = format!("this.{field}");
            return self.database(m, call.span, &root, handle, &steps, awaited, want);
        }
        if let Some((root, steps)) = flatten(e)
            && root != "this"
            && let Some(handle) = match root.strip_suffix("()") {
                Some(factory) => self.factory_handle(m, factory, true),
                None => self.handle_of(&root),
            }
        {
            return self.database(m, call.span, &root, handle, &steps, awaited, want);
        }
        if let Some(text) = &text
            && let Some(binding) = self
                .request
                .externals
                .iter()
                .find(|b| &b.callee == text)
                .cloned()
        {
            return self.external(m, call, &binding, awaited);
        }
        if let Some(text) = &text
            && let Some(binding) = self
                .request
                .transaction_retries
                .iter()
                .find(|b| &b.callee == text)
                .cloned()
        {
            if !awaited {
                return self.unknown_op(
                    m,
                    call.span,
                    format!("`{text}(...)` is not awaited: the transaction is not supported"),
                );
            }
            let step = ChainStep {
                path: Vec::new(),
                method: "transaction".to_owned(),
                args: &call.arguments,
            };
            return self.transaction(m, call.span, &Binding::Db, &step, want, Some(&binding));
        }
        // Phase 14: `schema.parse(x)` / `schema.safeParse(x)` of a zod
        // schema the extractor can read.
        if let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression()
            && matches!(member.property.name.as_str(), "parse" | "safeParse")
            && let Some(schema) = self.zod_schema(m, &member.object)
        {
            let safe = member.property.name == "safeParse";
            // Phase 16: the Phase 14 subset keeps its explicit lowering
            // (zod 3 semantics, verified identical in zod 4 by the
            // differential matrix) unless the schema was built with zod 4;
            // everything else is validated with the version's semantics.
            let schema = schema.and_then(|v| {
                if v.schema.is_phase14() && v.version != Ok(crate::zod::Version::V4) {
                    // The object type test needs the version (zod 4 takes a
                    // Map or Set for an object): unknown → both.
                    let version = v.version.unwrap_or(crate::zod::Version::Either);
                    return Ok((true, version, v.schema));
                }
                let version = v.version?;
                self.zod_static_facts(version, &v.schema)?;
                Ok((false, version, v.schema))
            });
            return match schema {
                Ok((phase14, version, schema)) => {
                    match call.arguments.first().and_then(|a| a.as_expression()) {
                        Some(input) if call.arguments.len() == 1 => {
                            let input = self.expr(m, input, TOP);
                            match phase14 {
                                true => self.zod_parse(m, call.span, version, &schema, input, safe),
                                false => self
                                    .zod_parse_checked(m, call.span, version, &schema, input, safe),
                            }
                        }
                        _ => self.unknown_op(
                            m,
                            call.span,
                            "a zod `parse` with other than one argument",
                        ),
                    }
                }
                Err(why) => {
                    let detail = match self.package_contract(m, &call.callee) {
                        Some(contract) => format!("{contract}; the schema is not modeled: {why}"),
                        None => format!("zod schema not modeled: {why}"),
                    };
                    self.unknown_op(m, call.span, detail)
                }
            };
        }
        match self.resolve_callee(m, &call.callee) {
            Callee::Method {
                abs,
                rel,
                method,
                this,
                label,
            } => self.inline_method(m, &abs, &rel, &method, this, &label, call, awaited, want),
            Callee::Local(info) => self.inline(m, &info, m, call, awaited, want),
            Callee::Cached(info) => match self.first_cached_call(m, &info.name) {
                Ok(()) => self.inline(m, &info, m, call, awaited, want),
                Err(why) => self.unknown_op(m, call.span, why),
            },
            Callee::Imported {
                abs,
                rel,
                export,
                label,
            } => self.inline_imported(m, &abs, &rel, &export, &label, call, awaited, want),
            Callee::Closure(c) => self.call_closure(m, &c, call, awaited, want),
            Callee::Unresolvable(reason) => {
                // Phase 13: a package call covered by a verified contract
                // stays unknown, with the contract named; Phase 14: unless
                // its result is a fresh random value (a generation event).
                if let Some((contract, detail)) = self.package_call(m, &call.callee) {
                    if contract.modeled == crate::packages::Modeled::FreshValue
                        && !contract.is_async
                    {
                        let local = self.tmp("gen");
                        self.emit(
                            m,
                            call.span,
                            Op::Generate {
                                local: local.clone(),
                                source: text.clone().unwrap_or_default(),
                            },
                        );
                        self.assume(format!(
                            "generated-values-distinct: `{}` returns a fresh value, distinct from every other value ({})",
                            text.as_deref().unwrap_or("<call>"),
                            contract.source
                        ));
                        return Res::Value(Expr::Local(local));
                    }
                    return self.unknown_op(m, call.span, detail);
                }
                self.unknown_op(
                    m,
                    call.span,
                    format!(
                        "call to `{}`: {reason}",
                        text.as_deref().unwrap_or("<expression>")
                    ),
                )
            }
        }
    }

    /// Phase 13: the contract detail of a package call (`crate::packages`):
    /// an export of a package (`hash` of `bcryptjs`, `bcrypt.hash` of a
    /// default or namespace import) or a method of a module-level value
    /// built by a package (`schema.safeParse` of a zod schema).
    fn package_contract(&mut self, m: &Module<'_>, callee: &Expression<'_>) -> Option<String> {
        self.package_call(m, callee).map(|(_, detail)| detail)
    }

    /// Phase 14: the contract of a package call and its unknown-operation
    /// detail.
    fn package_call(
        &mut self,
        m: &Module<'_>,
        callee: &Expression<'_>,
    ) -> Option<(&'static crate::packages::Contract, String)> {
        match callee.get_inner_expression() {
            Expression::Identifier(id) => {
                let name = id.name.as_str();
                if self.frames.is_empty() || self.lookup(name).is_some() {
                    return None;
                }
                let ImportRef::Named {
                    source,
                    name: export,
                } = find_import(m, name)?
                else {
                    return None;
                };
                let c = crate::packages::export_contract(&source, &export)?;
                if !matches!(self.ctx.resolve(&m.abs, &source), Resolution::Package) {
                    return None;
                }
                Some((c, crate::packages::describe(c, name, &source)))
            }
            Expression::StaticMemberExpression(member) => {
                let Expression::Identifier(object) = member.object.get_inner_expression() else {
                    return None;
                };
                let object = object.name.as_str();
                let method = member.property.name.as_str();
                if self.frames.is_empty() || self.lookup(object).is_some() {
                    return None;
                }
                let written = format!("{object}.{method}");
                match find_import(m, object) {
                    Some(ImportRef::Default { source } | ImportRef::Namespace { source }) => {
                        let c = crate::packages::export_contract(&source, method)?;
                        if !matches!(self.ctx.resolve(&m.abs, &source), Resolution::Package) {
                            return None;
                        }
                        Some((c, crate::packages::describe(c, &written, &source)))
                    }
                    _ => {
                        let file = self.ctx.index_of(&m.abs)?;
                        let Ok(Resolved::Binding { file, local }) =
                            self.ctx.resolve_local(file, object)
                        else {
                            return None;
                        };
                        let abs = self.ctx.files[file].clone();
                        let text = self.ctx.source(&abs).ok()?;
                        let allocator = Allocator::default();
                        let program = load(&allocator, &abs, &text).ok()?;
                        let module = Module::new(&abs, self.ctx.rel(&abs), &text, &program);
                        let init = const_initializer(&program, &local)?;
                        let source = package_chain_root(&module, init)?;
                        let c = crate::packages::method_contract(&source, method)?;
                        if !matches!(self.ctx.resolve(&abs, &source), Resolution::Package) {
                            return None;
                        }
                        Some((c, crate::packages::describe(c, &written, &source)))
                    }
                }
            }
            _ => None,
        }
    }

    fn promise_all(
        &mut self,
        m: &Module<'_>,
        call: &oxc_ast::ast::CallExpression<'_>,
        awaited: bool,
        want: Want,
    ) -> Res {
        if !awaited || want != Want::Discard {
            return self.unknown_op(
                m,
                call.span,
                "`Promise.all` is only supported as `await Promise.all([...])` whose result is unused",
            );
        }
        let array = match call.arguments.first().and_then(Argument::as_expression) {
            Some(expr) => match expr.get_inner_expression() {
                Expression::ArrayExpression(array) => Some(array),
                _ => None,
            },
            None => None,
        };
        let Some(array) = array else {
            return self.unknown_op(
                m,
                call.span,
                "`Promise.all` argument is not an array literal",
            );
        };
        let mut children = Vec::new();
        for element in &array.elements {
            let expression = match element {
                ArrayExpressionElement::SpreadElement(_) | ArrayExpressionElement::Elision(_) => {
                    None
                }
                other => other.as_expression(),
            };
            let (child, ()) = self.with_child(m, element.span(), |this| match expression {
                Some(expression) => {
                    let inner = match expression.get_inner_expression() {
                        Expression::AwaitExpression(a) => &a.argument,
                        other => other,
                    };
                    this.call_value(m, inner, true, Want::Discard, element.span());
                }
                None => {
                    this.unknown_op(m, element.span(), "spread or hole in `Promise.all` array");
                }
            });
            children.push(child);
        }
        self.emit(
            m,
            call.span,
            Op::Spawn {
                children,
                detached: false,
            },
        );
        self.emit(m, call.span, Op::Join);
        Res::Unit
    }

    fn external(
        &mut self,
        m: &Module<'_>,
        call: &oxc_ast::ast::CallExpression<'_>,
        binding: &ExternalBinding,
        awaited: bool,
    ) -> Res {
        let mut payload = Vec::new();
        let mut found_object = false;
        for argument in &call.arguments {
            if let Some(expression) = argument.as_expression()
                && let Expression::ObjectExpression(object) = expression.get_inner_expression()
            {
                found_object = true;
                for property in &object.properties {
                    match property {
                        ObjectPropertyKind::ObjectProperty(p) if !p.computed => {
                            if let Some(key) = static_key(&p.key) {
                                let value = self.expr(m, &p.value, TOP);
                                payload.push((key, value));
                            }
                        }
                        _ => self.assume(format!(
                            "payload of `{}`: a spread or computed property is not modeled",
                            binding.callee
                        )),
                    }
                }
                break;
            }
        }
        if !found_object && !call.arguments.is_empty() {
            self.assume(format!(
                "payload of `{}` is not an object literal and is modeled as empty",
                binding.callee
            ));
        }
        let key = binding.key_argument.and_then(|index| {
            call.arguments
                .get(index)
                .and_then(Argument::as_expression)
                .map(|e| self.expr(m, e, TOP))
        });
        let op = Op::External {
            channel: binding.channel.clone(),
            payload,
            key,
        };
        if awaited {
            self.emit(m, call.span, op);
        } else {
            let (child, ()) = self.with_child(m, call.span, |this| {
                this.emit(m, call.span, op);
            });
            self.emit(
                m,
                call.span,
                Op::Spawn {
                    children: vec![child],
                    detached: true,
                },
            );
        }
        Res::Unit
    }

    // -- helper inlining ----------------------------------------------------

    fn resolve_callee<'a>(&mut self, m: &Module<'a>, callee: &Expression<'a>) -> Callee<'a> {
        let callee = callee.get_inner_expression();
        match callee {
            Expression::Identifier(id) => {
                let name = id.name.as_str();
                // Phase 14: a local binding shadows module functions.
                if let Some(Binding::Closure(c)) =
                    self.frames.last().and_then(|_| self.lookup(name))
                {
                    return Callee::Closure(c);
                }
                if let Some(info) = find_function(m, name) {
                    return Callee::Local(info);
                }
                // Phase 13: `const name = other` in this module: the alias
                // is resolved as `other` (bounded: aliases of aliases).
                let mut name = name.to_owned();
                for _ in 0..8 {
                    if find_import(m, &name).is_some() {
                        break;
                    }
                    let Some(target) = alias_target(m.program, &name) else {
                        break;
                    };
                    if let Some(info) = find_function(m, &target) {
                        self.assume(format!(
                            "`{}` is an alias of the function `{target}` (`const {} = {target}`)",
                            id.name, id.name
                        ));
                        return Callee::Local(info);
                    }
                    name = target;
                }
                let name = name.as_str();
                if let Some(info) = react_cache_fn(m, name) {
                    return Callee::Cached(info);
                }
                match find_import(m, name) {
                    Some(ImportRef::Named { source, name: export }) => {
                        self.callee_import(m, &source, export, name)
                    }
                    Some(ImportRef::Default { source }) => {
                        self.callee_import(m, &source, "default".to_owned(), name)
                    }
                    Some(ImportRef::Namespace { .. }) => {
                        Callee::Unresolvable("a namespace import is not callable".to_owned())
                    }
                    None => Callee::Unresolvable(
                        "not a function declared in this module or imported from an analyzed module"
                            .to_owned(),
                    ),
                }
            }
            Expression::StaticMemberExpression(member) => {
                if let Expression::Identifier(object) = member.object.get_inner_expression()
                    && let Some(ImportRef::Namespace { source }) =
                        find_import(m, object.name.as_str())
                {
                    let label = format!("{}.{}", object.name, member.property.name);
                    return self.callee_import(
                        m,
                        &source,
                        member.property.name.as_str().to_owned(),
                        &label,
                    );
                }
                // Phase 12: repository objects and class instances.
                let method = member.property.name.as_str();
                match member.object.get_inner_expression() {
                    Expression::Identifier(object) => {
                        let name = object.name.as_str();
                        // Wrapper analysis resolves callees before the
                        // entry's frame exists: no locals then.
                        let local = if self.frames.is_empty() {
                            None
                        } else {
                            self.lookup(name)
                        };
                        match local {
                            Some(Binding::Instance(this)) => self.method_callee(*this, method, name),
                            Some(_) => Callee::Unresolvable(
                                "method calls are not resolved (on a local value)".to_owned(),
                            ),
                            None => self.module_receiver(m, name, method),
                        }
                    }
                    Expression::ThisExpression(_) => match self.frames.last().and_then(|f| f.this.clone()) {
                        Some(this) => self.method_callee(this, method, "this"),
                        None => Callee::Unresolvable(
                            "method calls are not resolved (`this` outside an analyzed object or class)"
                                .to_owned(),
                        ),
                    },
                    _ => Callee::Unresolvable("method calls are not resolved".to_owned()),
                }
            }
            _ => Callee::Unresolvable("the callee is not a plain function reference".to_owned()),
        }
    }

    // -- Phase 12: repository objects and class instances -------------------

    fn this_field_handle(&self, field: &str) -> Option<Binding> {
        match self.frames.last()?.this.as_ref()?.fields.get(field)? {
            b @ (Binding::Db | Binding::Tx | Binding::Replicated(_) | Binding::UnknownDb(_)) => {
                Some(b.clone())
            }
            _ => None,
        }
    }

    /// `this.field` where the field holds a database or transaction handle.
    fn this_handle(&self, e: &Expression<'_>) -> Option<Binding> {
        let Expression::StaticMemberExpression(member) = e else {
            return None;
        };
        if !matches!(
            member.object.get_inner_expression(),
            Expression::ThisExpression(_)
        ) {
            return None;
        }
        self.this_field_handle(member.property.name.as_str())
    }

    fn method_callee<'a>(&self, this: ThisBinding, method: &str, written: &str) -> Callee<'a> {
        let abs = this.owner.0.clone();
        let rel = relative(&self.root, &abs);
        Callee::Method {
            abs,
            rel,
            method: method.to_owned(),
            label: format!("{written}.{method}"),
            this,
        }
    }

    /// `name.method(...)` where `name` is (imported from) a module-level
    /// `const name = { ... }` object literal that the module never mutates,
    /// or `const name = new C(...)` of an analyzed class.
    fn module_receiver<'a>(&mut self, m: &Module<'_>, name: &str, method: &str) -> Callee<'a> {
        let unresolved =
            |why: String| Callee::Unresolvable(format!("method calls are not resolved: {why}"));
        let Some(file) = self.ctx.index_of(&m.abs) else {
            return unresolved("the module is not analyzed".to_owned());
        };
        let (file, local) = match self.ctx.resolve_local(file, name) {
            Ok(Resolved::Binding { file, local }) => (file, local),
            Ok(Resolved::Namespace { .. }) => {
                return unresolved(format!("`{name}` is a namespace"));
            }
            Err(why) => return unresolved(why),
        };
        let abs = self.ctx.files[file].clone();
        let Ok(text) = self.ctx.source(&abs) else {
            return unresolved(format!("cannot read `{}`", self.ctx.rel(&abs)));
        };
        let allocator = Allocator::default();
        let Ok(program) = load(&allocator, &abs, &text) else {
            return unresolved(format!("`{}` does not parse", self.ctx.rel(&abs)));
        };
        let Some(init) = const_initializer(&program, &local) else {
            return unresolved(format!(
                "`{name}` is not a module-level `const` object or instance"
            ));
        };
        match init.get_inner_expression() {
            Expression::ObjectExpression(_) => {
                if mutates(&text, &local) {
                    return unresolved(format!("the object `{local}` is modified in its module"));
                }
                let this = ThisBinding {
                    fields: HashMap::new(),
                    owner: (abs, Owner::Object(local)),
                };
                self.method_callee(this, method, name)
            }
            Expression::NewExpression(new) => {
                let module = Module::new(&abs, self.ctx.rel(&abs), &text, &program);
                // Constructor arguments of a module-level instance: handles
                // only (the module's database receivers, or a factory call).
                let mut args: Vec<Option<Binding>> = Vec::new();
                for a in &new.arguments {
                    let handle = match a.as_expression().map(Expression::get_inner_expression) {
                        Some(Expression::Identifier(id)) => {
                            self.module_handle(&abs, id.name.as_str())
                        }
                        Some(Expression::CallExpression(c)) => {
                            match c.callee.get_inner_expression() {
                                Expression::Identifier(f) => self.factory_handle(
                                    &module,
                                    f.name.as_str(),
                                    c.arguments.is_empty(),
                                ),
                                _ => None,
                            }
                        }
                        _ => None,
                    };
                    args.push(handle);
                }
                match self.class_this(&module, &new.callee, args) {
                    Ok(this) => self.method_callee(this, method, name),
                    Err(why) => unresolved(why),
                }
            }
            _ => unresolved(format!(
                "`{name}` is neither an object literal nor an instance"
            )),
        }
    }

    /// `new C(args)` in a function being lowered: handles stay handles,
    /// other arguments are evaluated now.
    fn new_instance(
        &mut self,
        m: &Module<'_>,
        new: &oxc_ast::ast::NewExpression<'_>,
    ) -> Result<ThisBinding, String> {
        let mut args = Vec::new();
        for a in &new.arguments {
            let Some(e) = a.as_expression() else {
                return Err("spread argument".to_owned());
            };
            if let Some(h) = self.handle_expr(m, e) {
                args.push(Some(h));
            } else {
                let value = self.expr(m, e, TOP);
                let local = self.tmp("field");
                self.emit(
                    m,
                    a.span(),
                    Op::Let {
                        local: local.clone(),
                        expr: value,
                    },
                );
                args.push(Some(Binding::Local(local)));
            }
        }
        self.class_this(m, &new.callee, args)
    }

    /// The `this` of an instance of the class `callee` (resolved in `m`)
    /// constructed with `args`: constructor parameter properties and
    /// `this.f = param` statements. A class with `extends`, or a constructor
    /// that does anything else, is not analyzed.
    fn class_this(
        &self,
        m: &Module<'_>,
        callee: &Expression<'_>,
        args: Vec<Option<Binding>>,
    ) -> Result<ThisBinding, String> {
        let Expression::Identifier(class) = callee.get_inner_expression() else {
            return Err("the class is not a plain identifier".to_owned());
        };
        let file = self
            .ctx
            .index_of(&m.abs)
            .ok_or("the module is not analyzed")?;
        let (file, local) = match self.ctx.resolve_local(file, class.name.as_str())? {
            Resolved::Binding { file, local } => (file, local),
            Resolved::Namespace { .. } => return Err("a namespace is not a class".to_owned()),
        };
        let abs = self.ctx.files[file].clone();
        let text = self.ctx.source(&abs)?;
        let allocator = Allocator::default();
        let program = load(&allocator, &abs, &text)?;
        let class_node = find_class(&program, &local)
            .ok_or_else(|| format!("`{local}` is not a class declared in its module"))?;
        if class_node.heritage.is_some() {
            return Err(format!(
                "`{local}` extends another class (inherited methods are not resolved)"
            ));
        }
        let mut fields = HashMap::new();
        for element in &class_node.body.body {
            if let ClassElement::MethodDefinition(def) = element
                && def.kind == MethodDefinitionKind::Constructor
            {
                let params = &def.value.params;
                let index_of = |name: &str| {
                    params.items.iter().position(|p| {
                        matches!(&p.pattern, BindingPattern::BindingIdentifier(id) if id.name.as_str() == name)
                    })
                };
                for (i, p) in params.items.iter().enumerate() {
                    if (p.accessibility.is_some() || p.readonly)
                        && let BindingPattern::BindingIdentifier(id) = &p.pattern
                        && let Some(Some(b)) = args.get(i)
                    {
                        fields.insert(id.name.as_str().to_owned(), b.clone());
                    }
                }
                for statement in def.value.body.iter().flat_map(|b| b.statements.iter()) {
                    let Statement::ExpressionStatement(es) = statement else {
                        return Err(format!(
                            "the constructor of `{local}` does more than store its parameters"
                        ));
                    };
                    let Expression::AssignmentExpression(assign) =
                        es.expression.get_inner_expression()
                    else {
                        return Err(format!(
                            "the constructor of `{local}` does more than store its parameters"
                        ));
                    };
                    let target = match &assign.left {
                        oxc_ast::ast::AssignmentTarget::StaticMemberExpression(t)
                            if matches!(
                                t.object.get_inner_expression(),
                                Expression::ThisExpression(_)
                            ) =>
                        {
                            t.property.name.as_str().to_owned()
                        }
                        _ => {
                            return Err(format!(
                                "the constructor of `{local}` assigns something other than `this.field`"
                            ));
                        }
                    };
                    let Expression::Identifier(source) = assign.right.get_inner_expression() else {
                        return Err(format!(
                            "the constructor of `{local}` stores a computed value in `this.{target}`"
                        ));
                    };
                    if let Some(i) = index_of(source.name.as_str())
                        && let Some(Some(b)) = args.get(i)
                    {
                        fields.insert(target, b.clone());
                    }
                }
            }
            if let ClassElement::PropertyDefinition(def) = element
                && !def.r#static
                && let Some(value) = &def.value
                && !matches!(
                    value.get_inner_expression(),
                    Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_)
                )
            {
                return Err(format!(
                    "`{local}` initializes a field with a value (not analyzed)"
                ));
            }
        }
        Ok(ThisBinding {
            fields,
            owner: (abs, Owner::Class(local)),
        })
    }

    /// Inline `method` of the object or class `this.owner`.
    #[allow(clippy::too_many_arguments)]
    fn inline_method(
        &mut self,
        caller: &Module<'_>,
        abs: &Path,
        rel: &str,
        method: &str,
        this: ThisBinding,
        label: &str,
        call: &oxc_ast::ast::CallExpression<'_>,
        awaited: bool,
        want: Want,
    ) -> Res {
        let source = match self.ctx.source(abs) {
            Ok(s) => s,
            Err(e) => return self.unknown_op(caller, call.span, format!("call to `{label}`: {e}")),
        };
        let allocator = Allocator::default();
        let program = match load(&allocator, abs, &source) {
            Ok(p) => p,
            Err(e) => {
                return self.unknown_op(
                    caller,
                    call.span,
                    format!("call to `{label}`: `{rel}`: {e}"),
                );
            }
        };
        let module = Module::new(abs, rel.to_owned(), &source, &program);
        let info = match &this.owner.1 {
            Owner::Object(object) => object_method(&module, object, method),
            Owner::Class(class) => class_method(&program, class, method),
        };
        let Some(info) = info else {
            return self.unknown_op(
                caller,
                call.span,
                format!("method calls are not resolved: `{label}` is not a method of an analyzed object or class"),
            );
        };
        self.pending_this = Some(this);
        let result = self.inline(caller, &info, &module, call, awaited, want);
        self.pending_this = None;
        result
    }

    /// Phase 13: a call of a React `cache`d function. The first call of an
    /// actor is taken to run the wrapped function (an assumption: no earlier
    /// call in the same request); a later call returns the memoized result,
    /// which is not modeled.
    fn first_cached_call(&mut self, module: &Module<'_>, name: &str) -> Result<(), String> {
        let key = (module.abs.clone(), name.to_owned());
        if !self.cached_calls.insert(key) {
            return Err(format!(
                "a repeated call of `{name}` (React `cache`) returns the memoized result of an earlier call, which is not modeled"
            ));
        }
        self.assume(format!(
            "`{name}` of `{}` is React `cache(fn)` ({}): the actor's first call is taken to run `fn` (no earlier call with the same arguments in the same request)",
            module.rel,
            crate::packages::REACT_CACHE_CONTRACT
        ));
        Ok(())
    }

    fn callee_import<'a>(
        &mut self,
        m: &Module<'_>,
        source: &str,
        export: String,
        label: &str,
    ) -> Callee<'a> {
        match self.ctx.resolve(&m.abs, source) {
            Resolution::File(index) => {
                // Phase 12: follow re-exports (`export { x } from`, `export
                // *`) to the module that declares the binding.
                let (index, local) = match self.ctx.resolve_export(index, &export) {
                    Ok(Resolved::Binding { file, local }) => (file, local),
                    Ok(Resolved::Namespace { .. }) => {
                        return Callee::Unresolvable(format!(
                            "`{label}` is a namespace object, not a function"
                        ));
                    }
                    Err(why) => return Callee::Unresolvable(why),
                };
                let abs = self.ctx.files[index].clone();
                let rel = relative(&self.root, &abs);
                Callee::Imported {
                    abs,
                    rel,
                    export: local,
                    label: label.to_owned(),
                }
            }
            Resolution::Package => Callee::Unresolvable(format!(
                "imported from the package `{source}`, which is not analyzed"
            )),
            Resolution::NotFound(why) => Callee::Unresolvable(why),
            Resolution::Ambiguous(c) => Callee::Unresolvable(format!(
                "the import `{source}` is ambiguous: {}",
                c.join(", ")
            )),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn inline_imported(
        &mut self,
        caller: &Module<'_>,
        abs: &Path,
        rel: &str,
        export: &str,
        label: &str,
        call: &oxc_ast::ast::CallExpression<'_>,
        awaited: bool,
        want: Want,
    ) -> Res {
        let source = match self.ctx.source(abs) {
            Ok(s) => s,
            Err(e) => {
                return self.unknown_op(caller, call.span, format!("call to `{label}`: {e}"));
            }
        };
        let allocator = Allocator::default();
        let program = match load(&allocator, abs, &source) {
            Ok(p) => p,
            Err(e) => {
                return self.unknown_op(
                    caller,
                    call.span,
                    format!("call to `{label}`: `{rel}`: {e}"),
                );
            }
        };
        let module = Module::new(abs, rel.to_owned(), &source, &program);
        let info = match find_resolved(&module, export) {
            Ok(info) => info,
            Err(why) => {
                // Phase 13: `export const f = cache(async () => ...)` of
                // React: the wrapped function runs on the first call.
                if let Some(info) = react_cache_fn(&module, export) {
                    return match self.first_cached_call(&module, export) {
                        Ok(()) => self.inline(caller, &info, &module, call, awaited, want),
                        Err(why) => self.unknown_op(caller, call.span, why),
                    };
                }
                // Phase 13: `export const a = b`: an alias of a function of
                // this module or of one it imports.
                if self.alias_hops < 8
                    && let Some(mut target) = alias_target(module.program, export)
                {
                    // Aliases of aliases within the module (bounded).
                    for _ in 0..8 {
                        if find_function(&module, &target).is_some()
                            || find_import(&module, &target).is_some()
                        {
                            break;
                        }
                        match alias_target(module.program, &target) {
                            Some(next) => target = next,
                            None => break,
                        }
                    }
                    if let Some(info) = find_function(&module, &target) {
                        self.assume(format!(
                            "`{label}` is an alias of the function `{target}` of `{rel}`"
                        ));
                        return self.inline(caller, &info, &module, call, awaited, want);
                    }
                    let callee = match find_import(&module, &target) {
                        Some(ImportRef::Named { source, name }) => {
                            self.callee_import(&module, &source, name, label)
                        }
                        Some(ImportRef::Default { source }) => {
                            self.callee_import(&module, &source, "default".to_owned(), label)
                        }
                        _ => Callee::Unresolvable(why.clone()),
                    };
                    if let Callee::Imported {
                        abs: target_abs,
                        rel: target_rel,
                        export: target_export,
                        ..
                    } = callee
                    {
                        self.alias_hops += 1;
                        let result = self.inline_imported(
                            caller,
                            &target_abs,
                            &target_rel,
                            &target_export,
                            label,
                            call,
                            awaited,
                            want,
                        );
                        self.alias_hops -= 1;
                        return result;
                    }
                }
                return self.unknown_op(caller, call.span, format!("call to `{label}`: {why}"));
            }
        };
        self.inline(caller, &info, &module, call, awaited, want)
    }

    /// Inline `info` (declared in `callee_module`) at `call` (written in
    /// `caller`).
    /// Phase 14: the zod schema `e` denotes (a parameter bound to one, a
    /// module-level or imported `const` built from zod, or a builder chain
    /// written in place); `None` when `e` is not a zod schema at all.
    fn zod_schema(
        &mut self,
        m: &Module<'_>,
        e: &Expression<'_>,
    ) -> Option<Result<crate::zod::Validated, String>> {
        struct InModule<'m, 'a> {
            module: &'m Module<'a>,
            /// Phase 16: the project root (where the version lookup stops).
            root: &'m Path,
            /// Phase 16: imported `const` arrays (`z.enum(KINDS)`).
            ctx: &'m ProjectContext,
        }
        impl<'a> crate::zod::Names<'a> for InModule<'_, 'a> {
            fn zod_import(&self, name: &str) -> Option<String> {
                match find_import(self.module, name)? {
                    ImportRef::Named { source, name } if name == "z" => Some(source),
                    ImportRef::Namespace { source } | ImportRef::Default { source } => Some(source),
                    ImportRef::Named { .. } => None,
                }
                .filter(|s| crate::packages::is_zod(s))
            }
            fn version(&self, specifier: &str) -> Result<crate::zod::Version, String> {
                crate::zod::resolve_version(&self.module.abs, self.root, specifier)
            }
            fn constant(&self, name: &str) -> Option<&'a Expression<'a>> {
                const_initializer(self.module.program, name)
            }
            fn string_array(&self, name: &str) -> Option<Vec<String>> {
                const_string_array(self.ctx, self.module, name, 0)
            }
        }
        let root = self.ctx.root.clone();
        let ctx = self.ctx.clone();
        let is_zod_chain = |module: &Module<'_>, init: &Expression<'_>| {
            package_chain_root(module, init).is_some_and(|s| crate::packages::is_zod(&s))
        };
        match e.get_inner_expression() {
            Expression::Identifier(id) => {
                let name = id.name.as_str();
                match self.frames.last().and_then(|_| self.lookup(name)) {
                    Some(Binding::Zod(schema)) => return Some(*schema),
                    Some(_) => return None,
                    None => {}
                }
                if let Some(init) = const_initializer(m.program, name) {
                    return is_zod_chain(m, init).then(|| {
                        crate::zod::interpret(
                            init,
                            &InModule {
                                module: m,
                                root: &root,
                                ctx: &ctx,
                            },
                        )
                    });
                }
                // An imported schema.
                let file = self.ctx.index_of(&m.abs)?;
                let Ok(Resolved::Binding { file, local }) = self.ctx.resolve_local(file, name)
                else {
                    return None;
                };
                let abs = self.ctx.files[file].clone();
                let text = self.ctx.source(&abs).ok()?;
                let allocator = Allocator::default();
                let program = load(&allocator, &abs, &text).ok()?;
                let module = Module::new(&abs, self.ctx.rel(&abs), &text, &program);
                let init = const_initializer(module.program, &local)?;
                is_zod_chain(&module, init).then(|| {
                    crate::zod::interpret(
                        init,
                        &InModule {
                            module: &module,
                            root: &root,
                            ctx: &ctx,
                        },
                    )
                })
            }
            Expression::CallExpression(_) if is_zod_chain(m, e) => {
                let _ = e;
                None
            }
            _ => None,
        }
    }

    /// Phase 14: `schema.parse(input)` (`safe`: `safeParse`): the
    /// validation as explicit steps (type tests, the string checks and
    /// rewrites in declaration order, the object's keys), then the parsed
    /// value. `parse` throws when a check fails; `safeParse` yields
    /// `{ success, data }` (its `error` is not modeled).
    fn zod_parse(
        &mut self,
        m: &Module<'_>,
        span: Span,
        version: crate::zod::Version,
        schema: &crate::zod::Schema,
        input: Expr,
        safe: bool,
    ) -> Res {
        let value = self.tmp("zin");
        self.emit(
            m,
            span,
            Op::Let {
                local: value.clone(),
                expr: input,
            },
        );
        let (success, data) = (self.tmp("zok"), self.tmp("zout"));
        if safe {
            self.emit(
                m,
                span,
                Op::Let {
                    local: data.clone(),
                    expr: Expr::Lit(Value::Null),
                },
            );
        }
        let mut fails = Vec::new();
        let out = self.zod_validate(m, span, version, schema, Expr::Local(value), &mut fails);
        self.emit(
            m,
            span,
            Op::Let {
                local: data.clone(),
                expr: out,
            },
        );
        if safe {
            self.emit(
                m,
                span,
                Op::Let {
                    local: success.clone(),
                    expr: Expr::lit(true),
                },
            );
        }
        let done = self.emit(m, span, Op::Jump { target: 0 });
        let fail = self.here();
        for at in fails {
            self.patch(at, fail);
        }
        if safe {
            self.emit(
                m,
                span,
                Op::Let {
                    local: success.clone(),
                    expr: Expr::lit(false),
                },
            );
        } else {
            self.emit(
                m,
                span,
                Op::Throw {
                    message: "ZodError: the input does not match the schema".to_owned(),
                },
            );
        }
        let end = self.here();
        self.patch(done, end);
        if safe {
            let mut fields = BTreeMap::new();
            fields.insert("success".to_owned(), success);
            fields.insert("data".to_owned(), data);
            Res::Fields(fields)
        } else {
            Res::Value(Expr::Local(data))
        }
    }

    fn zod_validate(
        &mut self,
        m: &Module<'_>,
        span: Span,
        version: crate::zod::Version,
        schema: &crate::zod::Schema,
        value: Expr,
        fails: &mut Vec<(usize, usize)>,
    ) -> Expr {
        use crate::zod::{NumCheck, Schema, StrCheck};
        let type_of = |v: &Expr, t: &str| {
            Expr::eq(
                Expr::Call {
                    func: midwrite_model::value::Builtin::TypeOf,
                    args: vec![v.clone()],
                },
                Expr::lit(t),
            )
        };
        let call = |func, args: Vec<Expr>| Expr::Call { func, args };
        let mut require = |this: &mut Self, ok: Expr| {
            fails.push(this.emit(
                m,
                span,
                Op::JumpIf {
                    cond: Expr::not(ok),
                    target: 0,
                },
            ));
        };
        use midwrite_model::value::Builtin as B;
        match schema {
            Schema::Str(checks) => {
                require(self, type_of(&value, "string"));
                let mut cur = value;
                for check in checks {
                    let len = || call(B::Length, vec![cur.clone()]);
                    match check {
                        StrCheck::Min(n) => {
                            require(self, Expr::bin(BinOp::Ge, len(), Expr::lit(*n)))
                        }
                        StrCheck::Max(n) => {
                            require(self, Expr::bin(BinOp::Le, len(), Expr::lit(*n)))
                        }
                        StrCheck::Length(n) => require(self, Expr::eq(len(), Expr::lit(*n))),
                        StrCheck::StartsWith(t) => require(
                            self,
                            call(B::StartsWith, vec![cur.clone(), Expr::lit(t.as_str())]),
                        ),
                        StrCheck::EndsWith(t) => require(
                            self,
                            call(B::EndsWith, vec![cur.clone(), Expr::lit(t.as_str())]),
                        ),
                        StrCheck::Includes(t) => require(
                            self,
                            call(B::Includes, vec![cur.clone(), Expr::lit(t.as_str())]),
                        ),
                        StrCheck::Trim | StrCheck::Lower | StrCheck::Upper => {
                            let func = match check {
                                StrCheck::Trim => B::Trim,
                                StrCheck::Lower => B::ToLowerCase,
                                _ => B::ToUpperCase,
                            };
                            let next = self.tmp("zstr");
                            self.emit(
                                m,
                                span,
                                Op::Let {
                                    local: next.clone(),
                                    expr: call(func, vec![cur]),
                                },
                            );
                            cur = Expr::Local(next);
                        }
                        // Phase 16: lowered by `zod_parse_checked` only.
                        StrCheck::Uuid | StrCheck::Email | StrCheck::Regex { .. } => require(
                            self,
                            Expr::Unknown(
                                "a zod string format outside the Phase 14 lowering".to_owned(),
                            ),
                        ),
                    }
                }
                cur
            }
            Schema::Num(checks) => {
                require(self, type_of(&value, "number"));
                for check in checks {
                    let (op, n) = match check {
                        NumCheck::Ge(n) => (BinOp::Ge, n),
                        NumCheck::Gt(n) => (BinOp::Gt, n),
                        NumCheck::Le(n) => (BinOp::Le, n),
                        NumCheck::Lt(n) => (BinOp::Lt, n),
                        NumCheck::Int => {
                            require(
                                self,
                                Expr::Unknown(
                                    "zod 4 `.int()` outside the Phase 14 lowering".to_owned(),
                                ),
                            );
                            continue;
                        }
                    };
                    require(self, Expr::bin(op, value.clone(), Expr::lit(*n)));
                }
                value
            }
            Schema::Bool => {
                require(self, type_of(&value, "boolean"));
                value
            }
            Schema::Literal(v) => {
                require(self, Expr::eq(value.clone(), Expr::Lit(v.clone())));
                value
            }
            Schema::Enum(items) => {
                require(self, type_of(&value, "string"));
                let any = items
                    .iter()
                    .map(|i| Expr::eq(value.clone(), Expr::lit(i.as_str())))
                    .reduce(|a, b| Expr::bin(BinOp::Or, a, b))
                    .unwrap_or(Expr::lit(false));
                require(self, any);
                value
            }
            Schema::Object(fields) => {
                // Phase 16: the exact object test of the version (`z.object({})`
                // accepts plain objects only; zod 4 also takes a Map or Set,
                // which the model refuses). Phase 14 tested `typeof` and
                // assumed the input was not an array; the differential
                // matrix showed arrays, Maps and Sets were then accepted.
                require(
                    self,
                    call(
                        B::ZodCheck,
                        vec![
                            Expr::Lit(midwrite_model::zodfn::to_value(
                                version,
                                &Schema::Object(Vec::new()),
                            )),
                            value.clone(),
                        ],
                    ),
                );
                let mut out = Vec::new();
                for (key, sub) in fields {
                    let field = self.tmp("zfield");
                    self.emit(
                        m,
                        span,
                        Op::Let {
                            local: field.clone(),
                            expr: Expr::field(value.clone(), key),
                        },
                    );
                    let parsed =
                        self.zod_validate(m, span, version, sub, Expr::Local(field), fails);
                    out.push((key.clone(), parsed));
                }
                Expr::Object(out)
            }
            // Phase 16: lowered by `zod_parse_checked` only.
            _ => {
                require(
                    self,
                    Expr::Unknown("a zod schema outside the Phase 14 lowering".to_owned()),
                );
                value
            }
        }
    }

    /// Phase 16: the static facts the output of `schema` needs (what a
    /// wrapper yields for `undefined`/`null`, what an object yields for an
    /// absent key), or why they are not modeled (zod 3 and zod 4 differ and
    /// the version is unknown, ...). Checked before anything is emitted.
    fn zod_static_facts(
        &self,
        version: crate::zod::Version,
        schema: &crate::zod::Schema,
    ) -> Result<(), String> {
        use crate::zod::Schema;
        use midwrite_model::zodfn;
        match schema {
            Schema::Optional(inner) | Schema::Nullable(inner) | Schema::Default(inner, _) => {
                let special = if matches!(schema, Schema::Nullable(_)) {
                    Value::Null
                } else {
                    Value::undefined()
                };
                zodfn::parse(version, schema, &special)
                    .map_err(|why| format!("`{}`: {why}", zodfn::render(schema)))?;
                self.zod_static_facts(version, inner)
            }
            Schema::Object(fields) | Schema::StrictObject(fields) | Schema::LooseObject(fields) => {
                for (key, field) in fields {
                    if zodfn::PROTOTYPE_KEYS.contains(&key.as_str()) {
                        return Err(format!(
                            "the zod object key `{key}` (a property of Object.prototype, read through the prototype by zod)"
                        ));
                    }
                    zodfn::absent(version, field).map_err(|why| {
                        format!("an absent `{key}` (`{}`): {why}", zodfn::render(field))
                    })?;
                    self.zod_static_facts(version, field)?;
                }
                Ok(())
            }
            Schema::Array(item, _) => self.zod_static_facts(version, item),
            Schema::Str(_)
            | Schema::Num(_)
            | Schema::Bool
            | Schema::Literal(_)
            | Schema::Enum(_) => Ok(()),
        }
    }

    /// Phase 16: `schema.parse(input)` (`safe`: `safeParse`) of a schema
    /// outside the Phase 14 subset or built with zod 4. The validation is
    /// one `ZodCheck` of the whole schema with the version's semantics
    /// (`midwrite_model::zodfn`); the parsed value is then built from the
    /// input as explicit steps (string transforms, defaults, the keys an
    /// object keeps, the unknown keys a passthrough object copies), so the
    /// values the code goes on to use stay traceable to the input's paths.
    /// `parse` throws when the check fails; `safeParse` yields `{ success,
    /// data }` with `data` undefined on failure (its `error` is not
    /// modeled).
    fn zod_parse_checked(
        &mut self,
        m: &Module<'_>,
        span: Span,
        version: crate::zod::Version,
        schema: &crate::zod::Schema,
        input: Expr,
        safe: bool,
    ) -> Res {
        use midwrite_model::value::Builtin as B;
        let value = self.tmp("zin");
        self.emit(
            m,
            span,
            Op::Let {
                local: value.clone(),
                expr: input,
            },
        );
        let (ok, success, data) = (self.tmp("zok"), self.tmp("zok"), self.tmp("zout"));
        self.emit(
            m,
            span,
            Op::Let {
                local: ok.clone(),
                expr: Expr::call(
                    B::ZodCheck,
                    vec![
                        Expr::Lit(midwrite_model::zodfn::to_value(version, schema)),
                        Expr::Local(value.clone()),
                    ],
                ),
            },
        );
        if safe {
            self.emit(
                m,
                span,
                Op::Let {
                    local: data.clone(),
                    expr: Expr::undefined(),
                },
            );
        }
        let fail = self.emit(
            m,
            span,
            Op::JumpIf {
                cond: Expr::not(Expr::Local(ok)),
                target: 0,
            },
        );
        let out = self.zod_output(m, span, version, schema, Expr::Local(value));
        self.emit(
            m,
            span,
            Op::Let {
                local: data.clone(),
                expr: out,
            },
        );
        if safe {
            self.emit(
                m,
                span,
                Op::Let {
                    local: success.clone(),
                    expr: Expr::lit(true),
                },
            );
        }
        let done = self.emit(m, span, Op::Jump { target: 0 });
        let failed = self.here();
        self.patch(fail, failed);
        if safe {
            self.emit(
                m,
                span,
                Op::Let {
                    local: success.clone(),
                    expr: Expr::lit(false),
                },
            );
        } else {
            self.emit(
                m,
                span,
                Op::Throw {
                    message: "ZodError: the input does not match the schema".to_owned(),
                },
            );
        }
        let end = self.here();
        self.patch(done, end);
        if safe {
            let mut fields = BTreeMap::new();
            fields.insert("success".to_owned(), success);
            fields.insert("data".to_owned(), data);
            Res::Fields(fields)
        } else {
            Res::Value(Expr::Local(data))
        }
    }

    /// Phase 16: the parsed value of `x`, which `schema` accepts (the
    /// `ZodCheck` before it passed). Facts were checked by
    /// [`Self::zod_static_facts`].
    fn zod_output(
        &mut self,
        m: &Module<'_>,
        span: Span,
        version: crate::zod::Version,
        schema: &crate::zod::Schema,
        x: Expr,
    ) -> Expr {
        use crate::zod::{Schema, StrCheck};
        use midwrite_model::value::Builtin as B;
        use midwrite_model::zodfn::{self, Absent, Unknown};
        let jump_unless = |this: &mut Self, cond: Expr| {
            this.emit(
                m,
                span,
                Op::JumpIf {
                    cond: Expr::not(cond),
                    target: 0,
                },
            )
        };
        match schema {
            Schema::Str(checks) => {
                let mut cur = x;
                for c in checks {
                    let func = match c {
                        StrCheck::Trim => B::Trim,
                        StrCheck::Lower => B::ToLowerCase,
                        StrCheck::Upper => B::ToUpperCase,
                        _ => continue,
                    };
                    cur = Expr::call(func, vec![cur]);
                }
                cur
            }
            Schema::Num(_) | Schema::Bool | Schema::Literal(_) | Schema::Enum(_) => x,
            // Elements rebuilt (objects, transforms, defaults): the array's
            // parsed value, computed whole by the zod semantics.
            Schema::Array(item, _) if !item.output_is_input() => Expr::call(
                B::ZodData,
                vec![Expr::Lit(zodfn::to_value(version, schema)), x],
            ),
            Schema::Array(..) => x,
            Schema::Optional(inner) | Schema::Nullable(inner) | Schema::Default(inner, _) => {
                let special = if matches!(schema, Schema::Nullable(_)) {
                    Value::Null
                } else {
                    Value::undefined()
                };
                // What the wrapper yields for its special value (a failure:
                // the check rejected it, the value is never used).
                let at_special = match zodfn::parse(version, schema, &special) {
                    Ok(Some(v)) => v,
                    _ => Value::undefined(),
                };
                if inner.output_is_input() && at_special == special {
                    return x;
                }
                let o = self.tmp("zval");
                self.emit(
                    m,
                    span,
                    Op::Let {
                        local: o.clone(),
                        expr: Expr::Lit(at_special),
                    },
                );
                let skip = self.emit(
                    m,
                    span,
                    Op::JumpIf {
                        cond: Expr::eq(x.clone(), Expr::Lit(special)),
                        target: 0,
                    },
                );
                let inner_out = self.zod_output(m, span, version, inner, x);
                self.emit(
                    m,
                    span,
                    Op::Let {
                        local: o.clone(),
                        expr: inner_out,
                    },
                );
                let end = self.here();
                self.patch(skip, end);
                Expr::Local(o)
            }
            Schema::Object(_) | Schema::StrictObject(_) | Schema::LooseObject(_) => {
                let Some((fields, unknown)) = schema.object() else {
                    return x;
                };
                let property = |k: &str| Expr::call(B::At, vec![x.clone(), Expr::lit(k)]);
                let has = |k: &str| Expr::call(B::HasOwn, vec![x.clone(), Expr::lit(k)]);
                let mut pairs = Vec::new();
                let mut omitted = Vec::new();
                for (key, field) in fields {
                    match zodfn::absent(version, field) {
                        Ok(Absent::Omit) => omitted.push((key, field)),
                        Ok(Absent::Set(u)) => {
                            let f = self.tmp("zfield");
                            self.emit(
                                m,
                                span,
                                Op::Let {
                                    local: f.clone(),
                                    expr: Expr::Lit(u),
                                },
                            );
                            let skip = jump_unless(self, has(key));
                            let v = self.zod_output(m, span, version, field, property(key));
                            self.emit(
                                m,
                                span,
                                Op::Let {
                                    local: f.clone(),
                                    expr: v,
                                },
                            );
                            let end = self.here();
                            self.patch(skip, end);
                            pairs.push((key.clone(), Expr::Local(f)));
                        }
                        // Required (or not modeled, refused before).
                        _ => {
                            let v = self.zod_output(m, span, version, field, property(key));
                            pairs.push((key.clone(), v));
                        }
                    }
                }
                let o = self.tmp("zobj");
                self.emit(
                    m,
                    span,
                    Op::Let {
                        local: o.clone(),
                        expr: Expr::Object(pairs),
                    },
                );
                for (key, field) in omitted {
                    let skip = jump_unless(self, has(key));
                    let v = self.zod_output(m, span, version, field, property(key));
                    self.emit(
                        m,
                        span,
                        Op::Let {
                            local: o.clone(),
                            expr: Expr::call(
                                B::ObjectAssign,
                                vec![Expr::Local(o.clone()), Expr::Object(vec![(key.clone(), v)])],
                            ),
                        },
                    );
                    let end = self.here();
                    self.patch(skip, end);
                }
                if unknown == Unknown::Passthrough {
                    let keys = Expr::Lit(Value::List(
                        fields.iter().map(|(k, _)| Value::str(k.as_str())).collect(),
                    ));
                    let mut rest = Expr::call(B::OmitKeys, vec![x.clone(), keys]);
                    // zod 3 drops unknown keys whose value is undefined
                    // (with an unknown version, `ZodCheck` refuses inputs
                    // where the versions' outputs differ).
                    if version != crate::zod::Version::V4 {
                        rest = Expr::call(B::DefinedEntries, vec![rest]);
                    }
                    self.emit(
                        m,
                        span,
                        Op::Let {
                            local: o.clone(),
                            expr: Expr::call(B::ObjectAssign, vec![rest, Expr::Local(o.clone())]),
                        },
                    );
                }
                Expr::Local(o)
            }
        }
    }

    /// Phase 14: an arrow function or function expression as a value: a
    /// closure over the current scopes.
    fn closure_value(&mut self, m: &Module<'_>, e: &Expression<'_>) -> Option<Binding> {
        let span = match e.get_inner_expression() {
            Expression::ArrowFunctionExpression(a) => a.span,
            Expression::FunctionExpression(f) if f.body.is_some() => f.span,
            _ => return None,
        };
        let at = self.src(m, span);
        Some(Binding::Closure(Box::new(Closure {
            abs: m.abs.clone(),
            rel: m.rel.clone(),
            span,
            name: format!("<closure@{}:{}>", at.line, at.column),
            scopes: self.frame().scopes.clone(),
            this: self.frame().this.clone(),
        })))
    }

    /// Phase 14: a call of a closure: its body is inlined here, in the
    /// scopes of its definition (a callback runs when, and as often as, the
    /// code that received it calls it; nothing is assumed about callbacks
    /// that are never called).
    fn call_closure(
        &mut self,
        caller: &Module<'_>,
        c: &Closure,
        call: &oxc_ast::ast::CallExpression<'_>,
        awaited: bool,
        want: Want,
    ) -> Res {
        let source = match self.ctx.source(&c.abs) {
            Ok(s) => s,
            Err(e) => {
                return self.unknown_op(caller, call.span, format!("call of `{}`: {e}", c.name));
            }
        };
        let allocator = Allocator::default();
        let program = match load(&allocator, &c.abs, &source) {
            Ok(p) => p,
            Err(e) => {
                return self.unknown_op(caller, call.span, format!("call of `{}`: {e}", c.name));
            }
        };
        let module = Module::new(&c.abs, c.rel.clone(), &source, &program);
        let Some(info) = closure_at(module.program, c.span, &c.name) else {
            return self.unknown_op(
                caller,
                call.span,
                format!("call of `{}`: the function node was not found", c.name),
            );
        };
        self.pending_this = c.this.clone();
        self.pending_scopes = Some(c.scopes.clone());
        self.inline(caller, &info, &module, call, awaited, want)
    }

    fn inline(
        &mut self,
        caller: &Module<'_>,
        info: &FnInfo<'_>,
        callee_module: &Module<'_>,
        call: &oxc_ast::ast::CallExpression<'_>,
        awaited: bool,
        want: Want,
    ) -> Res {
        // Phase 12: `this` of an inlined method (set by `inline_method`).
        let receiver = self.pending_this.take();
        // Phase 14: the scopes a closure closed over (set by `call_closure`).
        let captured = self.pending_scopes.take();
        let key = (callee_module.abs.clone(), info.name.clone());
        if self.frames.iter().any(|f| f.key == key) {
            return self.unknown_op(
                caller,
                call.span,
                format!("recursive call of `{}` is not inlined", info.name),
            );
        }
        if self.frames.len() > self.request.max_inline_depth {
            return self.unknown_op(
                caller,
                call.span,
                format!(
                    "call of `{}` exceeds the inline depth limit ({})",
                    info.name, self.request.max_inline_depth
                ),
            );
        }
        if want == Want::Single {
            return self.unknown_op(
                caller,
                call.span,
                format!("array destructuring of the result of `{}`", info.name),
            );
        }
        if !awaited && info.is_async && want == Want::Discard {
            // Fire-and-forget: a detached task running the helper.
            let (child, ()) = self.with_child(caller, call.span, move |this| {
                this.inline_here(
                    caller,
                    info,
                    callee_module,
                    call,
                    Want::Discard,
                    receiver,
                    captured,
                );
            });
            self.emit(
                caller,
                call.span,
                Op::Spawn {
                    children: vec![child],
                    detached: true,
                },
            );
            return Res::Unit;
        }
        if !awaited && info.is_async {
            return self.unknown_op(
                caller,
                call.span,
                format!(
                    "the promise returned by `{}` is used without `await`",
                    info.name
                ),
            );
        }
        self.inline_here(caller, info, callee_module, call, want, receiver, captured)
    }

    #[allow(clippy::too_many_arguments)]
    fn inline_here(
        &mut self,
        caller: &Module<'_>,
        info: &FnInfo<'_>,
        callee_module: &Module<'_>,
        call: &oxc_ast::ast::CallExpression<'_>,
        want: Want,
        this: Option<ThisBinding>,
        captured: Option<Vec<HashMap<String, Binding>>>,
    ) -> Res {
        // Evaluate the arguments in the caller's frame.
        let mut args: Vec<ArgVal> = Vec::new();
        for argument in &call.arguments {
            match argument.as_expression() {
                Some(expression) => {
                    if let Some(closure) = self.closure_value(caller, expression) {
                        args.push(ArgVal::Handle(closure));
                    } else if let Expression::Identifier(id) = expression.get_inner_expression()
                        && let Some(caught @ Binding::Caught(_)) = self.lookup(id.name.as_str())
                    {
                        // Phase 16: a caught error passed to a helper stays
                        // an error object there.
                        args.push(ArgVal::Handle(caught));
                    } else if let Some(schema) = self.zod_schema(caller, expression) {
                        args.push(ArgVal::Handle(Binding::Zod(Box::new(schema))));
                    } else if let Some(handle) = self.handle_expr(caller, expression) {
                        args.push(ArgVal::Handle(handle));
                    } else {
                        args.push(ArgVal::Expr(self.expr(caller, expression, TOP)));
                    }
                }
                None => args.push(ArgVal::Expr(Expr::Unknown(
                    "spread argument is not supported".to_owned(),
                ))),
            }
        }
        self.counter += 1;
        let prefix = format!("h{}$", self.counter);
        let result = format!("{prefix}ret");
        let mut used = HashSet::new();
        used.insert(result.clone());
        // The return slot belongs to the call site: its source is the call
        // in the caller's module (Phase 12; before, the caller's span was
        // resolved in the callee's file, which named an unrelated line).
        let undefined = self.undef();
        self.emit(
            caller,
            call.span,
            Op::Let {
                local: result.clone(),
                expr: undefined,
            },
        );
        // A closure's body sees the scopes it closed over, then its own.
        let mut scopes = captured.unwrap_or_default();
        scopes.push(HashMap::new());
        self.frames.push(Frame {
            function: info.name.clone(),
            file: callee_module.rel.clone(),
            prefix,
            scopes,
            rets: vec![Ret {
                result: result.clone(),
                patches: Vec::new(),
                main: false,
            }],
            used,
            key: (callee_module.abs.clone(), info.name.clone()),
            tries: 0,
            breaks: Vec::new(),
            this,
        });
        let mut args = args.into_iter();
        for (index, item) in info.params.items.iter().enumerate() {
            let supplied = args.next();
            let BindingPattern::BindingIdentifier(id) = &item.pattern else {
                match (&item.pattern, supplied) {
                    (BindingPattern::ObjectPattern(object), Some(ArgVal::Expr(e))) => {
                        let tmp = self.tmp("arg");
                        self.emit(
                            callee_module,
                            item.span,
                            Op::Let {
                                local: tmp.clone(),
                                expr: e,
                            },
                        );
                        self.destructure(callee_module, object, Expr::Local(tmp), item.span);
                    }
                    _ => {
                        self.unknown_op(
                            callee_module,
                            item.span,
                            "only `{ a, b: c }` destructuring of a value argument is supported",
                        );
                    }
                }
                continue;
            };
            let name = id.name.as_str();
            match supplied {
                Some(ArgVal::Handle(handle)) => self.declare(name, handle),
                other => {
                    let value = match other {
                        Some(ArgVal::Expr(e)) => e,
                        _ => match &item.initializer {
                            Some(default) => self.expr(callee_module, default, TOP),
                            None => self.undef(),
                        },
                    };
                    let local = self.fresh(name);
                    self.emit(
                        callee_module,
                        item.span,
                        Op::Let {
                            local: local.clone(),
                            expr: value,
                        },
                    );
                    self.declare(name, Binding::Local(local));
                }
            }
            let _ = index;
        }
        if info.params.rest.is_some() {
            self.unknown_op(
                callee_module,
                info.params.span,
                "rest parameters are not supported",
            );
        }
        self.body(callee_module, info);
        let frame = self.frames.pop().expect("frame pushed above");
        let end = self.here();
        for patch in frame.rets.into_iter().flat_map(|r| r.patches) {
            self.patch(patch, end);
        }
        let _ = frame.file;
        match want {
            Want::Discard => Res::Unit,
            _ => Res::Value(Expr::Local(result)),
        }
    }

    // -- database -----------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn database(
        &mut self,
        m: &Module<'_>,
        span: Span,
        root: &str,
        handle: Binding,
        steps: &[ChainStep<'_, '_>],
        awaited: bool,
        want: Want,
    ) -> Res {
        // Phase 13: a replicated database routes each operation to the
        // primary or to a replica.
        if let Binding::UnknownDb(why) = &handle {
            return self.unknown_op(
                m,
                span,
                format!("database handle of an unresolved type: {why}"),
            );
        }
        let mut handle = handle;
        let mut steps = steps;
        let mut replica_read = false;
        if let Binding::Replicated(routing) = &handle {
            let routing = routing.as_ref().clone();
            match self.route(&routing, steps) {
                Route::Primary(skip, path_skip) => {
                    handle = Binding::Db;
                    steps = &steps[skip..];
                    if path_skip {
                        // `db.$primary.<...>`: the property is the first path
                        // segment of the first step.
                        let first = &steps[0];
                        let rest = ChainStep {
                            path: first.path[1..].to_vec(),
                            method: first.method.clone(),
                            args: first.args,
                        };
                        let mut owned = vec![rest];
                        owned.extend(steps[1..].iter().map(|s| ChainStep {
                            path: s.path.clone(),
                            method: s.method.clone(),
                            args: s.args,
                        }));
                        return self.database(m, span, root, Binding::Db, &owned, awaited, want);
                    }
                }
                Route::Replica => {
                    let written = format!(
                        "`{root}.{}{}(...)`",
                        steps[0]
                            .path
                            .iter()
                            .map(|p| format!("{p}."))
                            .collect::<String>(),
                        steps[0].method
                    );
                    if routing.ambiguous {
                        return self.unknown_op(
                            m,
                            span,
                            format!(
                                "read through a read replica or the primary (the handle's routing is ambiguous): {written}: {}",
                                routing.origin
                            ),
                        );
                    }
                    if !self.request.replicas {
                        return self.unknown_op(
                            m,
                            span,
                            format!(
                                "read through a read replica (replica modeling not enabled): {written} of a replicated database ({})",
                                routing.origin
                            ),
                        );
                    }
                    replica_read = true;
                }
                Route::Unknown(why) => {
                    return self.unknown_op(
                        m,
                        span,
                        format!("database handle of a replicated database: {why}"),
                    );
                }
            }
        }
        let first = &steps[0];
        if first.method == "transaction" && first.path.is_empty() && steps.len() == 1 {
            if !awaited {
                return self.unknown_op(
                    m,
                    span,
                    "a transaction that is not awaited is not supported",
                );
            }
            return self.transaction(m, span, &handle, first, want, None);
        }
        if !awaited {
            return self.unknown_op(
                m,
                span,
                format!(
                    "Drizzle query on `{root}` is not awaited: the builder is lazy and is not executed"
                ),
            );
        }
        let conn = match handle {
            Binding::Tx => Conn::Tx,
            _ if replica_read => Conn::Replica,
            _ => Conn::Pool,
        };
        if first.method == "execute" && first.path.is_empty() {
            return self.unknown_op(m, span, "raw SQL via `execute` is not parsed");
        }
        if first.method == "findFirst" && first.path.len() == 2 && first.path[0] == "query" {
            return self.find_first(m, span, conn, first, steps.len(), want);
        }
        if !first.path.is_empty() {
            return self.unknown_op(
                m,
                span,
                format!(
                    "`{root}.{}.{}` is not supported",
                    first.path.join("."),
                    first.method
                ),
            );
        }
        match first.method.as_str() {
            "select" => self.select(m, span, conn, steps, want),
            "update" => self.mutation(m, span, conn, steps, want, Mutation::Update),
            "insert" => self.mutation(m, span, conn, steps, want, Mutation::Insert),
            "delete" => self.mutation(m, span, conn, steps, want, Mutation::Delete),
            other => self.unknown_op(
                m,
                span,
                format!("`{root}.{other}(...)` is not part of the supported Drizzle subset"),
            ),
        }
    }

    /// Phase 13: where an operation through a replicated database runs.
    fn route(&self, routing: &Routing, steps: &[ChainStep<'_, '_>]) -> Route {
        let first = &steps[0];
        if let Some(p) = first.path.first()
            && routing.primary_properties.iter().any(|x| x == p)
        {
            return Route::Primary(0, true);
        }
        if first.path.is_empty()
            && first.args.is_empty()
            && steps.len() > 1
            && routing.primary_only_methods.contains(&first.method)
        {
            return Route::Primary(1, false);
        }
        if first.path.is_empty() && crate::handles::PRIMARY_METHODS.contains(&first.method.as_str())
        {
            return Route::Primary(0, false);
        }
        if first.path.is_empty() && crate::handles::REPLICA_METHODS.contains(&first.method.as_str())
        {
            return Route::Replica;
        }
        if first.path.first().map(String::as_str) == Some("query") {
            return Route::Replica;
        }
        let written = first
            .path
            .iter()
            .map(|p| format!("{p}."))
            .collect::<String>()
            + &first.method;
        Route::Unknown(format!(
            "the routing of `{written}` is not part of the {} contract ({})",
            if routing.declared.is_some() {
                "declared replica factory's"
            } else {
                "drizzle-orm `withReplicas`"
            },
            routing.origin
        ))
    }

    fn transaction(
        &mut self,
        m: &Module<'_>,
        span: Span,
        handle: &Binding,
        step: &ChainStep<'_, '_>,
        want: Want,
        helper: Option<&TransactionRetryBinding>,
    ) -> Res {
        if !matches!(handle, Binding::Db) {
            return self.unknown_op(
                m,
                span,
                "nested transaction (savepoint) on a transaction handle is not supported",
            );
        }
        if self.tx_open > 0 {
            return self.unknown_op(
                m,
                span,
                "a transaction opened while another is open is not supported",
            );
        }
        if want == Want::Single {
            return self.unknown_op(m, span, "array destructuring of a transaction result");
        }
        let callback_index = helper.map_or(0, |h| h.callback_argument);
        let options_index = helper.map_or(Some(1), |h| h.options_argument);
        let Some(callback) = step
            .args
            .get(callback_index)
            .and_then(Argument::as_expression)
        else {
            return self.unknown_op(m, span, "transaction without a callback");
        };
        let Some(info) = info_of_expression("<transaction>", callback) else {
            return self.unknown_op(
                m,
                span,
                "the transaction callback is not an inline function",
            );
        };
        if info.params.items.len() > 1 || info.params.rest.is_some() {
            return self.unknown_op(m, span, "unsupported transaction callback parameters");
        }
        let mut isolation = None;
        let mut attempts = helper.map_or(1, |h| h.attempts);
        if let Some(options) = options_index
            .and_then(|i| step.args.get(i))
            .and_then(Argument::as_expression)
        {
            let Expression::ObjectExpression(object) = options.get_inner_expression() else {
                return self.unknown_op(m, span, "transaction options are not an object literal");
            };
            for property in &object.properties {
                let ObjectPropertyKind::ObjectProperty(p) = property else {
                    return self.unknown_op(m, span, "spread in transaction options");
                };
                match (static_key(&p.key).as_deref(), literal(&p.value)) {
                    (Some("isolationLevel"), Some(Expr::Lit(Value::Str(level)))) => {
                        match Isolation::parse(&level) {
                            Some(level) => isolation = Some(level),
                            None => {
                                return self.unknown_op(
                                    m,
                                    span,
                                    format!("unknown isolation level `{level}`"),
                                );
                            }
                        }
                    }
                    (Some("attempts"), Some(Expr::Lit(Value::Int(n)))) if helper.is_some() => {
                        match u32::try_from(n) {
                            Ok(n) if (1..=10).contains(&n) => attempts = n,
                            _ => {
                                return self.unknown_op(
                                    m,
                                    span,
                                    format!("{n} attempts is outside the supported 1..=10"),
                                );
                            }
                        }
                    }
                    (Some(key), _) => {
                        return self.unknown_op(
                            m,
                            span,
                            format!("transaction option `{key}` is not supported"),
                        );
                    }
                    _ => {
                        return self.unknown_op(m, span, "unsupported transaction option");
                    }
                }
            }
        }
        if let Some(h) = helper {
            self.assume(format!(
                "declared contract: `{}` runs its callback in a new transaction and re-runs it from BEGIN after SQLSTATE 40001/40P01, at most {attempts} time(s) in total, then rethrows; the helper's own code is not analyzed (source verification runs it)",
                h.callee
            ));
        }
        self.emit(
            m,
            span,
            Op::Begin {
                isolation,
                retry: attempts - 1,
            },
        );
        self.tx_open += 1;
        self.frame_mut().scopes.push(HashMap::new());
        if let Some(item) = info.params.items.first() {
            if let BindingPattern::BindingIdentifier(id) = &item.pattern {
                self.declare(id.name.as_str(), Binding::Tx);
            } else {
                self.unknown_op(m, item.span, "destructured transaction handle");
            }
        }
        let result = self.tmp("tx");
        self.frame_mut().used.insert(result.clone());
        self.frame_mut().rets.push(Ret {
            result: result.clone(),
            patches: Vec::new(),
            main: false,
        });
        self.emit(
            m,
            span,
            Op::Let {
                local: result.clone(),
                expr: Expr::Lit(Value::Null),
            },
        );
        self.body(m, &info);
        let ret = self.frame_mut().rets.pop().expect("pushed above");
        let end = self.here();
        for patch in ret.patches {
            self.patch(patch, end);
        }
        self.frame_mut().scopes.pop();
        self.tx_open -= 1;
        self.emit(m, span, Op::Commit);
        match want {
            Want::Discard => Res::Unit,
            _ => Res::Value(Expr::Local(result)),
        }
    }

    fn table_arg(
        &mut self,
        m: &Module<'_>,
        arg: Option<&Argument<'_>>,
    ) -> Result<&'r TableBinding, String> {
        let Some(expression) = arg.and_then(Argument::as_expression) else {
            return Err("missing table argument".to_owned());
        };
        let Expression::Identifier(id) = expression.get_inner_expression() else {
            return Err("the table is not a plain identifier".to_owned());
        };
        self.table_ref(m, id.name.as_str()).ok_or_else(|| {
            format!(
                "table identifier `{}` is not bound to a table of the scenario",
                id.name
            )
        })
    }

    fn pred(&mut self, m: &Module<'_>, e: &Expression<'_>) -> Expr {
        // `where(undefined)` (and an `and()` of nothing) selects every row.
        match self.pred_in(m, e, Some(true), false) {
            Ok((expr, _)) => expr,
            Err(why) => Expr::Unknown(why),
        }
    }

    // -- Phase 16: predicates held in locals --------------------------------

    /// Phase 16: whether `e` is a Drizzle predicate a local can hold: an
    /// operator call (`eq`, `and`, ... imported from `drizzle-orm`), a local
    /// already holding one, a conditional choosing between two (or between
    /// one and `undefined`), or a call of a project function whose body is
    /// `return <predicate>`.
    fn predicate_shaped(&mut self, m: &Module<'_>, e: &Expression<'_>) -> bool {
        self.shaped_in(m, e, 0, false)
    }

    /// `helper`: `e` is a helper's returned expression (its identifiers are
    /// the helper's, not the caller's).
    fn shaped_in(&mut self, m: &Module<'_>, e: &Expression<'_>, depth: u32, helper: bool) -> bool {
        if depth > 4 || self.frames.is_empty() {
            return false;
        }
        match e.get_inner_expression() {
            Expression::Identifier(id) => {
                !helper && matches!(self.lookup(id.name.as_str()), Some(Binding::Pred(_)))
            }
            Expression::ConditionalExpression(c) => {
                let a = self.shaped_in(m, &c.consequent, depth + 1, helper);
                let b = self.shaped_in(m, &c.alternate, depth + 1, helper);
                (a || undefined_like(&c.consequent))
                    && (b || undefined_like(&c.alternate))
                    && (a || b)
            }
            Expression::CallExpression(call) => {
                let Expression::Identifier(id) = call.callee.get_inner_expression() else {
                    return false;
                };
                let name = id.name.as_str();
                if !helper && self.lookup(name).is_some() {
                    return false;
                }
                if PRED_OPS.contains(&name) {
                    return matches!(find_import(m, name),
                        Some(ImportRef::Named { source, name: imported })
                            if imported == name
                                && (source == "drizzle-orm" || source.starts_with("drizzle-orm/")));
                }
                self.helper_returns_predicate(m, name, depth)
            }
            _ => false,
        }
    }

    /// Phase 16: whether `name` (a function of the module or imported from
    /// an analyzed one) is a non-async function whose body is `return
    /// <predicate>`.
    fn helper_returns_predicate(&mut self, m: &Module<'_>, name: &str, depth: u32) -> bool {
        if let Some(info) = find_function(m, name) {
            return returned_expr(&info).is_some_and(|ret| self.shaped_in(m, ret, depth + 1, true));
        }
        let Some(ImportRef::Named {
            source,
            name: export,
        }) = find_import(m, name)
        else {
            return false;
        };
        if source.starts_with("drizzle-orm") {
            return false;
        }
        let Callee::Imported {
            abs, rel, export, ..
        } = self.callee_import(m, &source, export, name)
        else {
            return false;
        };
        // Each imported function is parsed for this question once.
        let key = (abs.clone(), export.clone());
        if let Some(known) = self.pred_helpers.get(&key) {
            return *known;
        }
        let shaped = (|| {
            let text = self.ctx.source(&abs).ok()?;
            let allocator = Allocator::default();
            let program = load(&allocator, &abs, &text).ok()?;
            let module = Module::new(&abs, rel, &text, &program);
            let info = find_resolved(&module, &export).ok()?;
            let ret = returned_expr(&info)?;
            Some(self.shaped_in(&module, ret, depth + 1, true))
        })()
        .unwrap_or(false);
        self.pred_helpers.insert(key, shaped);
        shaped
    }

    /// Phase 16: the predicate held by the declaration `d` of `name`,
    /// lowered where it is declared.
    fn held_predicate(
        &mut self,
        m: &Module<'_>,
        d: &VariableDeclarator<'_>,
        name: &str,
        init: &Expression<'_>,
    ) -> Box<HeldPred> {
        let at = self.src(m, d.span).label();
        if !self.const_decl && reassigned(m.source, name) {
            return Box::new(HeldPred {
                expr: Expr::Unknown(format!(
                    "the predicate `{name}` is held in a variable that is assigned again (only a `const`, or a `let` assigned once, holding a predicate is modeled)"
                )),
                defined: None,
                at,
            });
        }
        let mut held = self.lower_held(m, init);
        held.at = at;
        Box::new(held)
    }

    fn lower_held(&mut self, m: &Module<'_>, e: &Expression<'_>) -> HeldPred {
        let unknown = |why: String| HeldPred {
            expr: Expr::Unknown(why),
            defined: None,
            at: String::new(),
        };
        let e = e.get_inner_expression();
        if let Expression::Identifier(id) = e
            && let Some(Binding::Pred(held)) = self.lookup(id.name.as_str())
        {
            // An alias: the same predicate (its operands were captured at
            // its own declaration).
            return *held;
        }
        if let Expression::ConditionalExpression(c) = e
            && (undefined_like(&c.consequent) || undefined_like(&c.alternate))
        {
            return self.optional_held(m, c);
        }
        if let Expression::CallExpression(call) = e
            && let Expression::Identifier(id) = call.callee.get_inner_expression()
            && !PRED_OPS.contains(&id.name.as_str())
        {
            return self.predicate_helper(m, call).unwrap_or_else(unknown);
        }
        match self.pred_in(m, e, None, false) {
            Ok((expr, _)) => self.capture(m, e.span(), expr, None),
            Err(why) => unknown(why),
        }
    }

    /// Phase 16: the JavaScript operands of a held predicate, assigned to
    /// locals now (Drizzle builds the SQL object when `eq()` is called).
    fn capture(
        &mut self,
        m: &Module<'_>,
        span: Span,
        expr: Expr,
        defined: Option<Expr>,
    ) -> HeldPred {
        let refuse = |why: String| HeldPred {
            expr: Expr::Unknown(why),
            defined: None,
            at: String::new(),
        };
        if let Some(why) = expr.unknown_detail() {
            return refuse(why.to_owned());
        }
        if reads_clock(&expr) {
            return refuse(
                "a clock reading (`new Date()`, ``sql`now()` ``) in a predicate held in a local is not modeled"
                    .to_owned(),
            );
        }
        let mut hoisted = Vec::new();
        let expr = self.hoist_operands(expr, &mut hoisted);
        for (local, value) in hoisted {
            self.emit(m, span, Op::Let { local, expr: value });
        }
        HeldPred {
            expr,
            defined,
            at: String::new(),
        }
    }

    /// Phase 16: `c ? pred : undefined` (or `c ? undefined : pred`): the
    /// predicate's operands are evaluated only when the test chooses it;
    /// `defined` records which way the test went.
    fn optional_held(
        &mut self,
        m: &Module<'_>,
        c: &oxc_ast::ast::ConditionalExpression<'_>,
    ) -> HeldPred {
        let refuse = |why: String| HeldPred {
            expr: Expr::Unknown(why),
            defined: None,
            at: String::new(),
        };
        let then_pred = !undefined_like(&c.consequent);
        let branch = if then_pred {
            &c.consequent
        } else {
            &c.alternate
        };
        let test = self.expr(
            m,
            &c.test,
            Cx {
                truthy: true,
                ..TOP
            },
        );
        if let Some(why) = test.unknown_detail() {
            return refuse(why.to_owned());
        }
        let which = self.tmp("which");
        self.emit(
            m,
            c.test.span(),
            Op::Let {
                local: which.clone(),
                expr: Expr::not(Expr::not(test)),
            },
        );
        let (pred, _) = match self.pred_in(m, branch, None, true) {
            Ok(p) => p,
            Err(why) => return refuse(why),
        };
        if let Some(why) = pred.unknown_detail() {
            return refuse(why.to_owned());
        }
        if reads_clock(&pred) {
            return refuse(
                "a clock reading (`new Date()`, ``sql`now()` ``) in a predicate held in a local is not modeled"
                    .to_owned(),
            );
        }
        let mut hoisted = Vec::new();
        let pred = self.hoist_operands(pred, &mut hoisted);
        for (local, _) in &hoisted {
            self.emit(
                m,
                c.span,
                Op::Let {
                    local: local.clone(),
                    expr: Expr::Lit(Value::Null),
                },
            );
        }
        let chosen = if then_pred {
            Expr::Local(which.clone())
        } else {
            Expr::not(Expr::Local(which.clone()))
        };
        let skip = self.emit(
            m,
            c.test.span(),
            Op::JumpIf {
                cond: Expr::not(chosen.clone()),
                target: usize::MAX,
            },
        );
        for (local, value) in hoisted {
            self.emit(m, branch.span(), Op::Let { local, expr: value });
        }
        let end = self.here();
        self.patch(skip, end);
        HeldPred {
            expr: pred,
            defined: Some(chosen),
            at: String::new(),
        }
    }

    /// Phase 16: a held predicate used where `pred_in` lowers one (`ident`
    /// as there: what an `undefined` operand stands for).
    fn use_held(
        &mut self,
        name: &str,
        held: &HeldPred,
        ident: Option<bool>,
    ) -> Result<(Expr, bool), String> {
        if let Some(why) = held.expr.unknown_detail() {
            return Err(why.to_owned());
        }
        self.assume(format!(
            "the Drizzle predicate `{name}` ({}) is built where it is declared: its operands keep the values they had there",
            held.at
        ));
        match (&held.defined, ident) {
            (None, _) => Ok((held.expr.clone(), false)),
            // With a boolean `defined`, `(defined AND p) OR (NOT defined AND
            // v)` is `p` or `v` in three-valued logic, NULL included.
            (Some(defined), Some(v)) => Ok((
                Expr::bin(
                    BinOp::Or,
                    Expr::bin(BinOp::And, defined.clone(), held.expr.clone()),
                    Expr::bin(BinOp::And, Expr::not(defined.clone()), Expr::lit(v)),
                ),
                true,
            )),
            (Some(_), None) => Err(format!(
                "the predicate `{name}` may be `undefined` here, which is not modeled"
            )),
        }
    }

    /// Phase 16: a call of a project function whose body is `return
    /// <predicate>`: its arguments are evaluated at the call, its returned
    /// predicate lowered with its parameters bound to them.
    fn predicate_helper(
        &mut self,
        m: &Module<'_>,
        call: &oxc_ast::ast::CallExpression<'_>,
    ) -> Result<HeldPred, String> {
        let Expression::Identifier(id) = call.callee.get_inner_expression() else {
            return Err("the predicate is not a call of a named function".to_owned());
        };
        let name = id.name.as_str();
        let unsupported = || {
            format!(
                "predicate operator `{name}` is not supported (supported: eq ne gt gte lt lte and or not isNull isNotNull inArray notInArray, and project functions whose body is `return <predicate>`)"
            )
        };
        if let Some(info) = find_function(m, name) {
            return self
                .helper_pred(m, &info, m, call)
                .unwrap_or_else(|| Err(unsupported()));
        }
        let Some(ImportRef::Named {
            source,
            name: export,
        }) = find_import(m, name)
        else {
            return Err(unsupported());
        };
        if source.starts_with("drizzle-orm") {
            return Err(unsupported());
        }
        let Callee::Imported {
            abs, rel, export, ..
        } = self.callee_import(m, &source, export, name)
        else {
            return Err(unsupported());
        };
        let text = self
            .ctx
            .source(&abs)
            .map_err(|e| format!("call to `{name}`: {e}"))?;
        let allocator = Allocator::default();
        let program =
            load(&allocator, &abs, &text).map_err(|e| format!("call to `{name}`: {e}"))?;
        let module = Module::new(&abs, rel, &text, &program);
        let info =
            find_resolved(&module, &export).map_err(|why| format!("call to `{name}`: {why}"))?;
        self.helper_pred(m, &info, &module, call)
            .unwrap_or_else(|| Err(unsupported()))
    }

    /// `None` when `info` is not a predicate helper.
    fn helper_pred(
        &mut self,
        caller: &Module<'_>,
        info: &FnInfo<'_>,
        callee_module: &Module<'_>,
        call: &oxc_ast::ast::CallExpression<'_>,
    ) -> Option<Result<HeldPred, String>> {
        let ret = returned_expr(info)?;
        if !self.shaped_in(callee_module, ret, 1, true) {
            return None;
        }
        let key = (callee_module.abs.clone(), info.name.clone());
        if self.frames.iter().any(|f| f.key == key) {
            return Some(Err(format!(
                "recursive call of `{}` is not inlined",
                info.name
            )));
        }
        if self.frames.len() > self.request.max_inline_depth {
            return Some(Err(format!(
                "call of `{}` exceeds the inline depth limit ({})",
                info.name, self.request.max_inline_depth
            )));
        }
        // The arguments, in the caller's frame.
        let mut args = Vec::new();
        for argument in &call.arguments {
            let Some(e) = argument.as_expression() else {
                return Some(Err("spread argument in a predicate helper call".to_owned()));
            };
            match e.get_inner_expression() {
                Expression::Identifier(x)
                    if matches!(self.lookup(x.name.as_str()), Some(Binding::Pred(_))) =>
                {
                    args.push(ArgVal::Handle(
                        self.lookup(x.name.as_str()).expect("checked"),
                    ));
                }
                _ => args.push(ArgVal::Expr(self.expr(caller, e, TOP))),
            }
        }
        self.counter += 1;
        self.frames.push(Frame {
            function: info.name.clone(),
            file: callee_module.rel.clone(),
            prefix: format!("h{}$", self.counter),
            scopes: vec![HashMap::new()],
            rets: Vec::new(),
            used: HashSet::new(),
            key,
            tries: 0,
            breaks: Vec::new(),
            this: None,
        });
        let mut args = args.into_iter();
        let mut bad = None;
        for item in &info.params.items {
            let BindingPattern::BindingIdentifier(id) = &item.pattern else {
                bad = Some("a destructured parameter of a predicate helper".to_owned());
                break;
            };
            match args.next() {
                Some(ArgVal::Handle(h)) => self.declare(id.name.as_str(), h),
                other => {
                    let value = match other {
                        Some(ArgVal::Expr(e)) => e,
                        _ => match &item.initializer {
                            Some(default) => self.expr(callee_module, default, TOP),
                            None => self.undef(),
                        },
                    };
                    let local = self.fresh(id.name.as_str());
                    self.emit(
                        callee_module,
                        item.span,
                        Op::Let {
                            local: local.clone(),
                            expr: value,
                        },
                    );
                    self.declare(id.name.as_str(), Binding::Local(local));
                }
            }
        }
        if info.params.rest.is_some() {
            bad = Some("rest parameters of a predicate helper".to_owned());
        }
        let held = match bad {
            None => self.lower_held(callee_module, ret),
            Some(why) => HeldPred {
                expr: Expr::Unknown(why),
                defined: None,
                at: String::new(),
            },
        };
        self.frames.pop();
        let at = format!(
            "returned by `{}` at {}",
            info.name,
            self.src(caller, call.span).label()
        );
        Some(Ok(HeldPred { at, ..held }))
    }

    /// Phase 14: a Drizzle predicate. `ident` is what an operand that is
    /// `undefined` at run time stands for where it occurs: Drizzle's `and()`
    /// and `or()` drop `undefined` operands (`true` resp. `false` in
    /// three-valued logic when another operand remains), `where(undefined)`
    /// selects every row (`true`); `None` where `undefined` is not modeled.
    /// The flag says whether the predicate may be `undefined` at run time.
    fn pred_in(
        &mut self,
        m: &Module<'_>,
        e: &Expression<'_>,
        ident: Option<bool>,
        cond: bool,
    ) -> Result<(Expr, bool), String> {
        let cx = Cx {
            col_ok: true,
            cond,
            ..TOP
        };
        let e = e.get_inner_expression();
        if is_nullish(e) && !matches!(e, Expression::NullLiteral(_))
            || matches!(e, Expression::UnaryExpression(u) if u.operator == UnaryOperator::Void)
        {
            return match ident {
                Some(v) => Ok((Expr::lit(v), true)),
                None => Err("an `undefined` predicate here is not modeled".to_owned()),
            };
        }
        // Phase 16: a predicate held in a local (`const p = eq(..)`).
        if let Expression::Identifier(id) = e {
            let name = id.name.as_str();
            return match self.lookup(name) {
                Some(Binding::Pred(held)) => self.use_held(name, &held, ident),
                _ => Err(format!(
                    "`{name}` is not a Drizzle predicate the model resolves (a `const` holding `eq(..)`, `and(..)`, ... is)"
                )),
            };
        }
        // Phase 14: `test ? predA : predB` (for example `parentId === null ?
        // isNull(t.parentId) : eq(t.parentId, parentId)`): the test is a
        // JavaScript condition evaluated before the statement is built.
        if let Expression::ConditionalExpression(c) = e {
            if cond {
                return Err(
                    "a conditional predicate inside a conditional predicate is not modeled"
                        .to_owned(),
                );
            }
            let test = self.expr(
                m,
                &c.test,
                Cx {
                    truthy: true,
                    ..TOP
                },
            );
            if let Some(why) = test.unknown_detail() {
                return Err(why.to_owned());
            }
            let (then_pred, d1) = self.pred_in(m, &c.consequent, ident, true)?;
            let (else_pred, d2) = self.pred_in(m, &c.alternate, ident, true)?;
            let which = self.tmp("which");
            self.emit(
                m,
                c.test.span(),
                Op::Let {
                    local: which.clone(),
                    expr: Expr::not(Expr::not(test)),
                },
            );
            // JavaScript evaluates the operands of the chosen branch only:
            // they are computed into locals under the test (the other
            // branch's are `null`, and its predicate is not selected).
            let mut hoisted_then = Vec::new();
            let mut hoisted_else = Vec::new();
            let then_pred = self.hoist_operands(then_pred, &mut hoisted_then);
            let else_pred = self.hoist_operands(else_pred, &mut hoisted_else);
            for (local, _) in hoisted_then.iter().chain(&hoisted_else) {
                self.emit(
                    m,
                    c.span,
                    Op::Let {
                        local: local.clone(),
                        expr: Expr::Lit(Value::Null),
                    },
                );
            }
            let to_else = self.emit(
                m,
                c.test.span(),
                Op::JumpIf {
                    cond: Expr::not(Expr::Local(which.clone())),
                    target: usize::MAX,
                },
            );
            for (local, value) in hoisted_then {
                self.emit(m, c.consequent.span(), Op::Let { local, expr: value });
            }
            let to_end = self.emit(m, c.span, Op::Jump { target: usize::MAX });
            let else_start = self.here();
            self.patch(to_else, else_start);
            for (local, value) in hoisted_else {
                self.emit(m, c.alternate.span(), Op::Let { local, expr: value });
            }
            let end = self.here();
            self.patch(to_end, end);
            // With a boolean `which`, `(which AND a) OR (NOT which AND b)` is
            // `a` or `b` in three-valued logic, NULL included.
            let picked = Expr::bin(
                BinOp::Or,
                Expr::bin(BinOp::And, Expr::Local(which.clone()), then_pred),
                Expr::bin(BinOp::And, Expr::not(Expr::Local(which)), else_pred),
            );
            return Ok((picked, d1 || d2));
        }
        let Expression::CallExpression(call) = e else {
            return Err("predicate is not a Drizzle operator call".to_owned());
        };
        let Expression::Identifier(op) = call.callee.get_inner_expression() else {
            return Err("predicate operator is not a plain identifier".to_owned());
        };
        let args: Vec<&Expression<'_>> = call
            .arguments
            .iter()
            .filter_map(Argument::as_expression)
            .collect();
        if args.len() != call.arguments.len() {
            return Err("spread argument in a predicate".to_owned());
        }
        let name = op.name.as_str();
        let binary = |this: &mut Self, op: BinOp| -> Result<(Expr, bool), String> {
            if args.len() != 2 {
                return Err(format!("`{name}` expects two arguments"));
            }
            // Phase 14: a JSON value (Drizzle binds it as JSON text) is not
            // compared by the model.
            if args.iter().any(|a| {
                matches!(
                    a.get_inner_expression(),
                    Expression::ObjectExpression(_) | Expression::ArrayExpression(_)
                )
            }) {
                return Err(format!(
                    "`{name}` with an object or array operand (a JSON comparison) is not modeled"
                ));
            }
            let a = this.statement_value(m, args[0], cx);
            let b = this.statement_value(m, args[1], cx);
            Ok((Expr::bin(op, a, b), false))
        };
        match name {
            "eq" => binary(self, BinOp::Eq),
            "ne" => binary(self, BinOp::Ne),
            "gt" => binary(self, BinOp::Gt),
            "gte" => binary(self, BinOp::Ge),
            "lt" => binary(self, BinOp::Lt),
            "lte" => binary(self, BinOp::Le),
            "and" | "or" => {
                let own = name == "and";
                let op = if own { BinOp::And } else { BinOp::Or };
                let mut parts = Vec::new();
                let mut droppable = true;
                for arg in &args {
                    let (p, d) = self.pred_in(m, arg, Some(own), cond)?;
                    droppable &= d;
                    parts.push(p);
                }
                // Every operand may be dropped: the call itself may be
                // `undefined`, which stands for `ident` where it occurs;
                // the operands' substitution must agree with it.
                if droppable && ident != Some(own) {
                    return Err(format!(
                        "`{name}(...)` whose every operand may be `undefined` is not modeled here"
                    ));
                }
                let expr = parts
                    .into_iter()
                    .reduce(|acc, next| Expr::bin(op, acc, next))
                    .unwrap_or_else(|| Expr::lit(own));
                Ok((expr, droppable))
            }
            "not" if args.len() == 1 => {
                let (inner, _) = self.pred_in(m, args[0], None, cond)?;
                Ok((Expr::not(inner), false))
            }
            "isNull" | "isNotNull" if args.len() == 1 => {
                let inner = self.statement_value(m, args[0], cx);
                let is_null = Expr::IsNull(Box::new(inner));
                Ok((
                    if name == "isNull" {
                        is_null
                    } else {
                        Expr::not(is_null)
                    },
                    false,
                ))
            }
            // Phase 14: `inArray(column, values)` / `notInArray(...)`
            // (`sqlfn`): an array literal or a local array.
            "inArray" | "notInArray" if args.len() == 2 => {
                let tested = self.statement_value(m, args[0], cx);
                let list = match args[1].get_inner_expression() {
                    Expression::ArrayExpression(a) => {
                        let mut items = Vec::new();
                        for el in &a.elements {
                            let Some(x) = el.as_expression() else {
                                return Err(format!(
                                    "`{name}` over an array with a spread or a hole"
                                ));
                            };
                            items.push(self.statement_value(m, x, cx));
                        }
                        Expr::Array(items)
                    }
                    other @ (Expression::Identifier(_) | Expression::StaticMemberExpression(_)) => {
                        self.expr(m, other, cx)
                    }
                    _ => {
                        return Err(format!(
                            "`{name}` over a subquery or a computed list is not modeled (an array literal or a local array is)"
                        ));
                    }
                };
                let call = Expr::Call {
                    func: midwrite_model::value::Builtin::InList,
                    args: vec![tested, list],
                };
                Ok((
                    if name == "inArray" {
                        call
                    } else {
                        Expr::not(call)
                    },
                    false,
                ))
            }
            // Phase 16: a project function whose body is `return
            // <predicate>` (evaluated where it is called).
            other
                if self.lookup(other).is_none()
                    && (find_function(m, other).is_some()
                        || matches!(find_import(m, other),
                            Some(ImportRef::Named { source, .. }) if !source.starts_with("drizzle-orm"))) =>
            {
                if cond {
                    return Err(format!(
                        "a call of `{other}` inside a conditional predicate is not modeled"
                    ));
                }
                let held = self.predicate_helper(m, call)?;
                self.use_held(other, &held, ident)
            }
            other => Err(format!(
                "predicate operator `{other}` is not supported (supported: eq ne gt gte lt lte and or not isNull isNotNull inArray notInArray)"
            )),
        }
    }

    /// Phase 14: the operands of a predicate that do not read the row
    /// (JavaScript values), replaced by fresh locals (`out` gets the
    /// assignments to emit); literals stay.
    fn hoist_operands(&mut self, e: Expr, out: &mut Vec<(String, Expr)>) -> Expr {
        match e {
            Expr::Lit(_) | Expr::Col(_) => e,
            other if !other.mentions_column() => {
                let local = self.tmp("operand");
                out.push((local.clone(), other));
                Expr::Local(local)
            }
            Expr::Not(x) => Expr::Not(Box::new(self.hoist_operands(*x, out))),
            Expr::IsNull(x) => Expr::IsNull(Box::new(self.hoist_operands(*x, out))),
            Expr::Bin(op, a, b) => {
                let a = self.hoist_operands(*a, out);
                let b = self.hoist_operands(*b, out);
                Expr::bin(op, a, b)
            }
            Expr::Array(items) => Expr::Array(
                items
                    .into_iter()
                    .map(|x| self.hoist_operands(x, out))
                    .collect(),
            ),
            Expr::Call { func, args } => Expr::Call {
                func,
                args: args
                    .into_iter()
                    .map(|x| self.hoist_operands(x, out))
                    .collect(),
            },
            other => other,
        }
    }

    /// Phase 14: a value written in a statement's arguments (an operand of
    /// a predicate, a value of `values`/`set`): a clock read there is one
    /// reading per execution of the statement ([`Self::clock_value`]).
    fn statement_value(&mut self, m: &Module<'_>, e: &Expression<'_>, cx: Cx) -> Expr {
        match self.clock_value(e) {
            Some(clock) => clock,
            None => self.expr(m, e, cx),
        }
    }

    /// Phase 14: `new Date()` (an application clock reading) or
    /// ``sql`now()` `` / ``sql`current_timestamp` `` (the transaction's
    /// start time) written where a statement reads it: a placeholder the
    /// engine replaces when the statement starts (one application reading
    /// per occurrence and execution; `now()` is the transaction's). Exposed
    /// for `expr` only through [`Self::clock_in_statement`].
    fn clock_value(&self, e: &Expression<'_>) -> Option<Expr> {
        match e.get_inner_expression() {
            Expression::NewExpression(new)
                if new.arguments.is_empty()
                    && matches!(new.callee.get_inner_expression(),
                        Expression::Identifier(id) if id.name.as_str() == "Date")
                    && self.lookup("Date").is_none() =>
            {
                Some(Expr::Lit(midwrite_model::db::clock_placeholder(
                    new.span.start,
                )))
            }
            Expression::TaggedTemplateExpression(t)
                if matches!(t.tag.get_inner_expression(),
                    Expression::Identifier(id) if id.name.as_str() == "sql")
                    && t.quasi.expressions.is_empty() =>
            {
                let text: String = t
                    .quasi
                    .quasis
                    .iter()
                    .map(|q| q.value.raw.as_str())
                    .collect::<String>()
                    .trim()
                    .to_ascii_lowercase();
                matches!(text.as_str(), "now()" | "current_timestamp")
                    .then(|| Expr::Lit(midwrite_model::db::now_placeholder()))
            }
            _ => None,
        }
    }

    /// Phase 14 helper for the lead's `expr`: the clock placeholder of `new
    /// Date()` / ``sql`now()` `` when the expression is evaluated as part of
    /// a database statement's arguments (it is bound per statement
    /// execution). Not for a value kept in a local and used later: that is
    /// one reading shared by its uses, which a placeholder does not express.
    #[allow(dead_code)]
    pub(crate) fn clock_in_statement(&self, e: &Expression<'_>) -> Option<Expr> {
        self.clock_value(e)
    }

    fn where_clause(&mut self, m: &Module<'_>, step: Option<&ChainStep<'_, '_>>) -> Expr {
        match step {
            None => Expr::lit(true),
            Some(step) => match step.args.first().and_then(Argument::as_expression) {
                Some(e) if step.args.len() == 1 => self.pred(m, e),
                _ => Expr::Unknown("`where` expects exactly one predicate".to_owned()),
            },
        }
    }

    /// Emit `op`, or an Unknown op if any expression of it is unknown.
    fn emit_db(
        &mut self,
        m: &Module<'_>,
        span: Span,
        conn: Conn,
        kind: StmtKind,
        exprs: &[&Expr],
    ) -> Option<()> {
        if let Some(detail) = exprs.iter().find_map(|e| e.unknown_detail()) {
            let detail = detail.to_owned();
            self.emit(
                m,
                span,
                Op::Unknown {
                    detail,
                    defines: Vec::new(),
                },
            );
            return None;
        }
        self.emit(
            m,
            span,
            Op::Db {
                stmt: Stmt { conn, kind },
            },
        );
        Some(())
    }

    fn find_first(
        &mut self,
        m: &Module<'_>,
        span: Span,
        conn: Conn,
        step: &ChainStep<'_, '_>,
        step_count: usize,
        want: Want,
    ) -> Res {
        if want == Want::Single || step_count != 1 {
            return self.unknown_op(m, span, "unsupported use of `query.<table>.findFirst`");
        }
        let Some(table) = self.query_table(m, &step.path[1]) else {
            return self.unknown_op(
                m,
                span,
                format!(
                    "table identifier `{}` is not bound to a table of the scenario",
                    step.path[1]
                ),
            );
        };
        let mut pred = Expr::lit(true);
        let mut ordered = false;
        if let Some(argument) = step.args.first() {
            let Some(Expression::ObjectExpression(options)) = argument
                .as_expression()
                .map(Expression::get_inner_expression)
            else {
                return self.unknown_op(m, span, "findFirst options are not an object literal");
            };
            for property in &options.properties {
                let ObjectPropertyKind::ObjectProperty(p) = property else {
                    return self.unknown_op(m, span, "spread in findFirst options");
                };
                match static_key(&p.key).as_deref() {
                    Some("where") => pred = self.pred(m, &p.value),
                    Some("orderBy") => ordered = true,
                    other => {
                        return self.unknown_op(
                            m,
                            span,
                            format!(
                                "findFirst option `{}` is not supported",
                                other.unwrap_or("<computed>")
                            ),
                        );
                    }
                }
            }
        }
        self.assume("relational query `findFirst` assumed to be `SELECT ... LIMIT 1`");
        self.assume(if ordered {
            "orderBy is not modeled: rows are taken in primary-key order"
        } else {
            "no orderBy: the first row in primary-key order is taken"
        });
        let into = self.tmp("row");
        let kind = StmtKind::Select {
            table: table.table.clone(),
            pred: pred.clone(),
            lock: LockMode::None,
            into: into.clone(),
            joins: Vec::new(),
            projection: None,
            many: None,
        };
        match self.emit_db(m, span, conn, kind, &[&pred]) {
            Some(()) => Res::Row(into),
            None => Res::Value(Expr::Unknown("findFirst predicate".to_owned())),
        }
    }

    fn select(
        &mut self,
        m: &Module<'_>,
        span: Span,
        conn: Conn,
        steps: &[ChainStep<'_, '_>],
        want: Want,
    ) -> Res {
        const DISCARDED: &str = "a select result is only supported as `const [row] = await db.select()...`, `const rows = await db.select()...limit(1)` (or a count)";
        // Phase 16: with JavaScript values, an awaited locking select whose
        // result is not used still locks its rows (the multi-row mode).
        if want == Want::Discard && !self.js() {
            return self.unknown_op(m, span, DISCARDED);
        }
        let mut table: Option<&TableBinding> = None;
        let mut joins: Vec<(JoinKind, &TableBinding, &ChainStep<'_, '_>)> = Vec::new();
        let mut where_step = None;
        let mut limit: Option<i64> = None;
        let mut lock = LockMode::None;
        let mut ordered = false;
        let mut order_step: Option<&ChainStep<'_, '_>> = None;
        for step in &steps[1..] {
            if !step.path.is_empty() {
                return self.unknown_op(m, span, "unsupported select chain");
            }
            match step.method.as_str() {
                "from" => match self.table_arg(m, step.args.first()) {
                    Ok(t) if table.is_none() => table = Some(t),
                    Ok(_) => return self.unknown_op(m, span, "more than one `from`"),
                    Err(why) => return self.unknown_op(m, span, why),
                },
                "leftJoin" | "innerJoin" if table.is_some() && where_step.is_none() => {
                    let kind = if step.method == "leftJoin" {
                        JoinKind::Left
                    } else {
                        JoinKind::Inner
                    };
                    match self.table_arg(m, step.args.first()) {
                        Ok(t)
                            if joins.iter().all(|(_, j, _)| j.table != t.table)
                                && table.is_some_and(|b| b.table != t.table) =>
                        {
                            joins.push((kind, t, step));
                        }
                        Ok(t) => {
                            return self.unknown_op(
                                m,
                                span,
                                format!(
                                    "table `{}` joined twice (aliases are not supported)",
                                    t.table
                                ),
                            );
                        }
                        Err(why) => return self.unknown_op(m, span, why),
                    }
                }
                "rightJoin" | "fullJoin" | "crossJoin" | "leftJoinLateral" | "innerJoinLateral" => {
                    return self.unknown_op(
                        m,
                        span,
                        format!("`{}` is not supported (only `innerJoin`/`leftJoin` on column equality)", step.method),
                    );
                }
                "where" if where_step.is_none() => where_step = Some(step),
                "limit" => {
                    limit = step
                        .args
                        .first()
                        .and_then(Argument::as_expression)
                        .and_then(literal)
                        .and_then(|e| match e {
                            Expr::Lit(Value::Int(n)) => Some(n),
                            _ => None,
                        });
                    if limit.is_none() {
                        return self.unknown_op(m, span, "`limit` is not an integer literal");
                    }
                }
                "orderBy" => {
                    ordered = true;
                    order_step = Some(step);
                }
                "for" => {
                    let mode = step
                        .args
                        .first()
                        .and_then(Argument::as_expression)
                        .and_then(literal);
                    // Phase 13: the four PostgreSQL row-lock strengths
                    // (`docs/phase13/ROW-LOCK-COMPATIBILITY.md`); SKIP LOCKED
                    // and NOWAIT only with `update`.
                    let strength = match &mode {
                        Some(Expr::Lit(Value::Str(s))) => match s.as_str() {
                            "update" => Some(LockMode::ForUpdate),
                            "no key update" => Some(LockMode::ForNoKeyUpdate),
                            "share" => Some(LockMode::ForShare),
                            "key share" => Some(LockMode::ForKeyShare),
                            _ => None,
                        },
                        _ => None,
                    };
                    let Some(strength) = strength else {
                        return self.unknown_op(
                            m,
                            span,
                            "row locking other than `.for(\"update\" | \"no key update\" | \"share\" | \"key share\")` is not supported",
                        );
                    };
                    if conn == Conn::Replica {
                        // Phase 13: PostgreSQL rejects row locks on a hot
                        // standby; the statement would fail, not lock.
                        return self.unknown_op(
                            m,
                            span,
                            "read through a read replica with a row lock (`.for(\"update\")`): a hot standby rejects row locks",
                        );
                    }
                    lock = strength;
                    if let Some(options) = step.args.get(1).and_then(Argument::as_expression) {
                        if strength != LockMode::ForUpdate {
                            return self.unknown_op(
                                m,
                                span,
                                "`skipLocked`/`noWait` are modeled with `.for(\"update\")` only",
                            );
                        }
                        let Expression::ObjectExpression(object) = options.get_inner_expression()
                        else {
                            return self.unknown_op(m, span, "`for` options are not an object");
                        };
                        for property in &object.properties {
                            let ObjectPropertyKind::ObjectProperty(p) = property else {
                                return self.unknown_op(m, span, "spread in `for` options");
                            };
                            let truthy = literal(&p.value) == Some(Expr::lit(true));
                            match (static_key(&p.key).as_deref(), truthy) {
                                (Some("skipLocked"), true) => lock = LockMode::SkipLocked,
                                (Some("noWait"), true) => lock = LockMode::NoWait,
                                _ => {
                                    return self.unknown_op(m, span, "unsupported `for` option");
                                }
                            }
                        }
                    }
                }
                other => {
                    return self.unknown_op(
                        m,
                        span,
                        format!("select method `{other}` is not supported"),
                    );
                }
            }
        }
        let Some(table) = table else {
            return self.unknown_op(m, span, "select without `from`");
        };
        let joined = !joins.is_empty();
        // Phase 16: the multi-row mode (JavaScript values: the result is an
        // array of rows): a lock without `.limit(1)` (every matching row is
        // locked, one at a time in primary-key order), a result bound to a
        // variable without `.limit(1)`, or an awaited select whose result is
        // not used.
        let counting = steps[0].args.first().is_some_and(is_count_projection);
        let many = self.js()
            && !joined
            && !counting
            && ((lock != LockMode::None && limit != Some(1))
                || want == Want::Discard
                || (want == Want::Scalar && limit != Some(1)));
        if want == Want::Discard && !many {
            return self.unknown_op(m, span, DISCARDED);
        }
        // `.orderBy(t.column)` / `.orderBy(asc(t.column))`: the loader
        // accepts it when the column is the primary key (the model's order).
        let mut order_by = None;
        if many && let Some(step) = order_step {
            let column = match step
                .args
                .iter()
                .map(Argument::as_expression)
                .collect::<Vec<_>>()
                .as_slice()
            {
                [Some(e)] => {
                    let e = match e.get_inner_expression() {
                        Expression::CallExpression(c)
                            if c.arguments.len() == 1
                                && matches!(c.callee.get_inner_expression(),
                                    Expression::Identifier(i) if i.name.as_str() == "asc"
                                        && matches!(find_import(m, "asc"),
                                            Some(ImportRef::Named { source, .. }) if source.starts_with("drizzle-orm"))) =>
                        {
                            c.arguments[0].as_expression()
                        }
                        other => Some(other),
                    };
                    match e.map(Expression::get_inner_expression) {
                        Some(Expression::StaticMemberExpression(member)) => {
                            match member.object.get_inner_expression() {
                                Expression::Identifier(id)
                                    if self.table(id.name.as_str()).is_some_and(|t| {
                                        t.table == table.table
                                            && t.columns
                                                .iter()
                                                .any(|c| c == member.property.name.as_str())
                                    }) =>
                                {
                                    Some(member.property.name.as_str().to_owned())
                                }
                                _ => None,
                            }
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            let Some(column) = column else {
                return self.unknown_op(
                    m,
                    span,
                    "`orderBy` on a multi-row select is modeled only as `.orderBy(t.<primary key>)` (ascending)",
                );
            };
            order_by = Some(column);
        }
        let many_limit = match limit {
            _ if !many => None,
            None => None,
            Some(n) => match u32::try_from(n) {
                Ok(n) => Some(n),
                Err(_) => return self.unknown_op(m, span, "a negative `limit` is not modeled"),
            },
        };
        if !many && lock != LockMode::None && limit != Some(1) {
            return self.unknown_op(
                m,
                span,
                "row locking is only supported with `.limit(1)` (a multi-row lock is not modeled)",
            );
        }
        if lock != LockMode::None && joined {
            return self.unknown_op(m, span, "row locking on a join is not modeled");
        }
        // `const rows = await db.select()...limit(1)`: `rows.length` and
        // `rows[0]` are modeled (exact: 0 or 1 row). Without `.limit(1)` the
        // array could hold any number of rows, which the model does not
        // represent.
        if !many && want == Want::Scalar && limit != Some(1) && steps[0].args.is_empty() {
            return self.unknown_op(
                m,
                span,
                "a select result bound to a variable is only supported with `.limit(1)` (its `length` and `[0]` are modeled)",
            );
        }
        // Projection.
        let in_query = |t: &str| t == table.table || joins.iter().any(|(_, j, _)| j.table == t);
        let mut aggregate: Option<String> = None;
        let mut projection: Option<Vec<(String, ProjItem)>> = None;
        if let Some(argument) = steps[0].args.first() {
            let Some(Expression::ObjectExpression(object)) = argument
                .as_expression()
                .map(Expression::get_inner_expression)
            else {
                return self.unknown_op(m, span, "select projection is not an object literal");
            };
            let is_count = object.properties.len() == 1
                && matches!(&object.properties[0], ObjectPropertyKind::ObjectProperty(p)
                    if matches!(p.value.get_inner_expression(), Expression::CallExpression(c)
                        if c.arguments.is_empty()
                            && matches!(c.callee.get_inner_expression(), Expression::Identifier(i) if i.name.as_str() == "count")));
            if is_count && !joined {
                if let ObjectPropertyKind::ObjectProperty(p) = &object.properties[0] {
                    aggregate = static_key(&p.key);
                }
                if aggregate.is_none() {
                    return self.unknown_op(m, span, "computed key in a count projection");
                }
            } else {
                let mut items = Vec::new();
                for property in &object.properties {
                    let ObjectPropertyKind::ObjectProperty(p) = property else {
                        return self.unknown_op(m, span, "spread in a select projection");
                    };
                    let Some(field) = static_key(&p.key).filter(|_| !p.computed) else {
                        return self.unknown_op(m, span, "computed key in a select projection");
                    };
                    let item = match p.value.get_inner_expression() {
                        Expression::Identifier(id) => self
                            .table(id.name.as_str())
                            .filter(|t| in_query(&t.table))
                            .map(|t| ProjItem::Table(t.table.clone())),
                        Expression::StaticMemberExpression(member) => {
                            match member.object.get_inner_expression() {
                                Expression::Identifier(id) => self
                                    .table(id.name.as_str())
                                    .filter(|t| {
                                        in_query(&t.table)
                                            && t.columns
                                                .iter()
                                                .any(|c| c == member.property.name.as_str())
                                    })
                                    .map(|t| {
                                        ProjItem::Column(if joined {
                                            format!("{}.{}", t.table, member.property.name)
                                        } else {
                                            member.property.name.as_str().to_owned()
                                        })
                                    }),
                                _ => None,
                            }
                        }
                        _ => None,
                    };
                    let Some(item) = item else {
                        return self.unknown_op(
                            m,
                            span,
                            format!("projected field `{field}` is not a table or a column of a table in the query"),
                        );
                    };
                    items.push((field, item));
                }
                projection = Some(items);
            }
        }
        if !many && want == Want::Scalar && aggregate.is_none() && limit != Some(1) {
            return self.unknown_op(
                m,
                span,
                "a select result bound to a variable is only supported with `.limit(1)` (its `length` and `[0]` are modeled)",
            );
        }
        // Join conditions and the predicate use qualified columns.
        self.qualify = joined;
        let mut ir_joins = Vec::new();
        for (kind, t, step) in &joins {
            let condition = step.args.get(1).and_then(Argument::as_expression);
            let pair = condition.and_then(|c| match self.pred(m, c) {
                Expr::Bin(BinOp::Eq, a, b) => match (*a, *b) {
                    (Expr::Col(a), Expr::Col(b)) => Some((a, b)),
                    _ => None,
                },
                _ => None,
            });
            let Some((left, right)) = pair.filter(|_| step.args.len() == 2) else {
                self.qualify = false;
                return self.unknown_op(
                    m,
                    span,
                    "a join condition other than `eq(column, column)` is not supported",
                );
            };
            ir_joins.push(Join {
                kind: *kind,
                table: t.table.clone(),
                left,
                right,
            });
        }
        let pred = self.where_clause(m, where_step);
        self.qualify = false;
        if let Some(field) = aggregate {
            let local = self.tmp("count");
            let kind = StmtKind::SelectCount {
                table: table.table.clone(),
                pred: pred.clone(),
                into: local.clone(),
            };
            return match self.emit_db(m, span, conn, kind, &[&pred]) {
                Some(()) => Res::Aggregate { field, local },
                None => Res::Value(Expr::Unknown("count predicate".to_owned())),
            };
        }
        if many && order_by.is_some() {
            self.assume(
                "a multi-row select ordered by the primary key returns its rows, and locks them one row at a time, in key order (PostgreSQL locks above the sort)",
            );
        } else if many {
            self.assume(
                "a multi-row select without orderBy is modeled in primary-key order (rows returned and locked one row at a time in that order); PostgreSQL uses its plan's order: key order for an index scan, physical order for a bitmap or sequential scan (docs/phase16/MULTI-ROW-LOCKS.md), so lock-order outcomes (waits, deadlocks) of such a statement are approximate",
            );
        } else {
            self.assume(if ordered {
                "orderBy is not modeled: rows are taken in primary-key order"
            } else {
                "select without orderBy: the first row in primary-key order is used"
            });
        }
        if joined {
            self.assume("a join without orderBy: rows are taken in primary-key order of the first table, then of each joined table");
        }
        let into = self.tmp(if many { "rows" } else { "row" });
        let kind = StmtKind::Select {
            table: table.table.clone(),
            pred: pred.clone(),
            lock,
            into: into.clone(),
            joins: ir_joins,
            projection,
            many: many.then_some(midwrite_model::ir::ManyRows {
                limit: many_limit,
                order_by,
            }),
        };
        match self.emit_db(m, span, conn, kind, &[&pred]) {
            // Phase 16: the array of rows; `const [row] = ...` reads its
            // first element (`undefined` when it is empty).
            Some(()) if many => match want {
                Want::Discard => Res::Unit,
                Want::Single => Res::Value(Expr::Call {
                    func: midwrite_model::value::Builtin::At,
                    args: vec![Expr::Local(into), Expr::lit(0)],
                }),
                Want::Scalar => Res::Value(Expr::Local(into)),
            },
            Some(()) if want == Want::Scalar => {
                let length = self.length_of(m, span, &into);
                Res::Rows { row: into, length }
            }
            Some(()) => Res::Row(into),
            None => Res::Value(Expr::Unknown("select predicate".to_owned())),
        }
    }

    /// `length = row is null ? 0 : 1` for an array of at most one row.
    fn length_of(&mut self, m: &Module<'_>, span: Span, row: &str) -> String {
        let length = self.tmp("length");
        self.emit(
            m,
            span,
            Op::Let {
                local: length.clone(),
                expr: Expr::lit(1),
            },
        );
        let jump = self.emit(
            m,
            span,
            Op::JumpIf {
                cond: Expr::not(Expr::IsNull(Box::new(Expr::Local(row.to_owned())))),
                target: usize::MAX,
            },
        );
        self.emit(
            m,
            span,
            Op::Let {
                local: length.clone(),
                expr: Expr::lit(0),
            },
        );
        let end = self.here();
        self.patch(jump, end);
        length
    }

    fn mutation(
        &mut self,
        m: &Module<'_>,
        span: Span,
        conn: Conn,
        steps: &[ChainStep<'_, '_>],
        want: Want,
        kind: Mutation,
    ) -> Res {
        let table = match self.table_arg(m, steps[0].args.first()) {
            Ok(t) => t,
            Err(why) => return self.unknown_op(m, span, why),
        };
        let mut set: Option<&ChainStep<'_, '_>> = None;
        let mut values: Option<&ChainStep<'_, '_>> = None;
        let mut where_step = None;
        let mut returning = false;
        let mut returning_step: Option<&ChainStep<'_, '_>> = None;
        let mut on_conflict = OnConflict::Error;
        let mut upsert_step: Option<&ChainStep<'_, '_>> = None;
        for step in &steps[1..] {
            if !step.path.is_empty() {
                return self.unknown_op(m, span, "unsupported mutation chain");
            }
            match (kind, step.method.as_str()) {
                (Mutation::Update, "set") if set.is_none() => set = Some(step),
                (Mutation::Insert, "values") if values.is_none() => values = Some(step),
                (Mutation::Update | Mutation::Delete, "where") if where_step.is_none() => {
                    where_step = Some(step)
                }
                (_, "returning") => {
                    returning = true;
                    returning_step = (!step.args.is_empty()).then_some(step);
                }
                (Mutation::Insert, "onConflictDoNothing") => on_conflict = OnConflict::DoNothing,
                // Phase 13.
                (Mutation::Insert, "onConflictDoUpdate") if upsert_step.is_none() => {
                    upsert_step = Some(step)
                }
                (_, other) => {
                    return self.unknown_op(
                        m,
                        span,
                        format!("mutation method `{other}` is not supported"),
                    );
                }
            }
        }
        let affected = (want != Want::Discard).then(|| self.tmp("affected"));
        if want != Want::Discard && !returning {
            return self.unknown_op(
                m,
                span,
                "the result of a mutation without `returning()` is not modeled",
            );
        }
        // A single-row `insert ... returning()` returns the inserted row
        // (with generated and default values); other mutations only their
        // row count.
        let returned_row =
            (kind == Mutation::Insert && want != Want::Discard).then(|| self.tmp("inserted"));
        // Phase 12: `returning({ alias: table.column, ... })` of a
        // single-row insert: the statement returns the inserted row
        // (generated values included) and a local projection builds the
        // aliased row, so a generated key keeps its identity.
        let projection: Option<Vec<(String, String)>> = match (returning_step, &returned_row) {
            (Some(step), Some(_)) => match self.returning_projection(table, step) {
                Ok(p) => Some(p),
                Err(why) => return self.unknown_op(m, span, why),
            },
            _ => None,
        };
        let projected_row = projection.as_ref().map(|_| self.tmp("returned"));
        let affected_name = affected.clone();
        let res = match (&affected, &returned_row) {
            (Some(length), Some(row)) => Res::Rows {
                row: projected_row.clone().unwrap_or_else(|| row.clone()),
                length: length.clone(),
            },
            (Some(local), None) => Res::Affected(local.clone()),
            _ => Res::Unit,
        };
        match kind {
            Mutation::Update => {
                let Some(set) = set else {
                    return self.unknown_op(m, span, "update without `set`");
                };
                let Some(Expression::ObjectExpression(object)) = set
                    .args
                    .first()
                    .and_then(Argument::as_expression)
                    .map(Expression::get_inner_expression)
                else {
                    return self.unknown_op(m, span, "`set` argument is not an object literal");
                };
                let mut assignments = Vec::new();
                for property in &object.properties {
                    let ObjectPropertyKind::ObjectProperty(p) = property else {
                        return self.unknown_op(m, span, "spread in `set`");
                    };
                    let Some(column) = static_key(&p.key).filter(|_| !p.computed) else {
                        return self.unknown_op(m, span, "computed key in `set`");
                    };
                    if !table.columns.contains(&column) {
                        return self.unknown_op(
                            m,
                            span,
                            format!(
                                "`{column}` is not a column of table `{}` (property names must equal column names)",
                                table.table
                            ),
                        );
                    }
                    let value = self.set_value(m, &p.value);
                    assignments.push((column, value));
                }
                let pred = self.where_clause(m, where_step);
                let stmt = StmtKind::Update {
                    table: table.table.clone(),
                    pred: pred.clone(),
                    set: assignments.clone(),
                    affected: affected_name,
                };
                let mut refs: Vec<Expr> = assignments.iter().map(|(_, e)| e.clone()).collect();
                refs.push(pred);
                let refs: Vec<&Expr> = refs.iter().collect();
                match self.emit_db(m, span, conn, stmt, &refs) {
                    Some(()) => res,
                    None => Res::Value(Expr::Unknown("update".to_owned())),
                }
            }
            Mutation::Delete => {
                let pred = self.where_clause(m, where_step);
                let stmt = StmtKind::Delete {
                    table: table.table.clone(),
                    pred: pred.clone(),
                    affected: affected_name,
                };
                match self.emit_db(m, span, conn, stmt, &[&pred]) {
                    Some(()) => res,
                    None => Res::Value(Expr::Unknown("delete".to_owned())),
                }
            }
            Mutation::Insert => {
                let Some(values) = values else {
                    return self.unknown_op(m, span, "insert without `values`");
                };
                let argument = values
                    .args
                    .first()
                    .and_then(Argument::as_expression)
                    .map(Expression::get_inner_expression);
                // Phase 14: `values([{ ... }, { ... }])`, an array literal of
                // object literals (or `const` objects): one statement.
                let row_args: Vec<&Expression<'_>> = match argument {
                    Some(Expression::ArrayExpression(a)) => {
                        let mut out = Vec::new();
                        for el in &a.elements {
                            match el.as_expression().map(Expression::get_inner_expression) {
                                Some(x) => out.push(x),
                                None => {
                                    return self.unknown_op(
                                        m,
                                        span,
                                        "`values([...])` with a spread or a hole",
                                    );
                                }
                            }
                        }
                        if out.is_empty() {
                            return self.unknown_op(
                                m,
                                span,
                                "`values([])` (Drizzle throws for an empty list)",
                            );
                        }
                        if out.len() > midwrite_model::ir::MAX_INSERT_ROWS {
                            return self.unknown_op(
                                m,
                                span,
                                format!(
                                    "`values([...])` with {} rows exceeds the bound of {}",
                                    out.len(),
                                    midwrite_model::ir::MAX_INSERT_ROWS
                                ),
                            );
                        }
                        out
                    }
                    Some(x) => vec![x],
                    None => {
                        return self.unknown_op(m, span, "`values()` without an argument");
                    }
                };
                let multi = matches!(argument, Some(Expression::ArrayExpression(_)));
                let mut rows: Vec<Vec<(String, Expr)>> = Vec::new();
                for row_arg in row_args {
                    match self.insert_row(m, table, row_arg) {
                        Ok(row) => rows.push(row),
                        Err(why) => return self.unknown_op(m, span, why),
                    }
                }
                let assignments = rows.remove(0);
                let more_rows = rows;
                if multi
                    && projection
                        .as_ref()
                        .is_some_and(|p| p.iter().any(|(a, c)| a != c))
                {
                    return self.unknown_op(
                        m,
                        span,
                        "`returning({ alias: column })` with a different alias on a multi-row insert is not modeled",
                    );
                }
                // Phase 13: `onConflictDoUpdate({ target, set, setWhere })`.
                let upsert = match upsert_step {
                    None => None,
                    Some(step) => match self.upsert_config(m, table, step) {
                        Ok(u) => Some(u),
                        Err(why) => return self.unknown_op(m, span, why),
                    },
                };
                let mut refs: Vec<&Expr> = assignments
                    .iter()
                    .chain(more_rows.iter().flatten())
                    .map(|(_, e)| e)
                    .collect();
                if let Some(u) = &upsert {
                    refs.extend(u.set.iter().map(|(_, e)| e));
                    refs.extend(u.filter.iter());
                }
                let stmt = StmtKind::Insert {
                    table: table.table.clone(),
                    values: assignments.clone(),
                    on_conflict,
                    affected: affected_name,
                    returning: returned_row.clone(),
                    // Drizzle sends `RETURNING` with the projected columns.
                    returning_columns: projection.as_ref().map(|fields| {
                        let mut cols: Vec<String> = Vec::new();
                        for (_, c) in fields {
                            if !cols.contains(c) {
                                cols.push(c.clone());
                            }
                        }
                        cols
                    }),
                    upsert: upsert.clone(),
                    more_rows: more_rows.clone(),
                };
                if self.emit_db(m, span, conn, stmt, &refs).is_none() {
                    return Res::Value(Expr::Unknown("insert".to_owned()));
                }
                // Phase 14: a multi-row `returning()` is the array of rows
                // (`StmtResult::Rows`, the projected columns when the aliases
                // are the column names); `const [first] = ...` reads its
                // first element.
                if !more_rows.is_empty() {
                    return match (&returned_row, want) {
                        (Some(list), Want::Single) => Res::Value(Expr::Call {
                            func: midwrite_model::value::Builtin::At,
                            args: vec![Expr::Local(list.clone()), Expr::lit(0)],
                        }),
                        (Some(list), _) => Res::Value(Expr::Local(list.clone())),
                        (None, _) => res,
                    };
                }
                if let (Some(fields), Some(row), Some(out)) =
                    (&projection, &returned_row, &projected_row)
                {
                    // out = null; if (row != null) out = { alias: row.column }
                    self.emit(
                        m,
                        span,
                        Op::Let {
                            local: out.clone(),
                            expr: Expr::Lit(Value::Null),
                        },
                    );
                    let skip = self.emit(
                        m,
                        span,
                        Op::JumpIf {
                            cond: Expr::IsNull(Box::new(Expr::Local(row.clone()))),
                            target: usize::MAX,
                        },
                    );
                    self.emit(
                        m,
                        span,
                        Op::Let {
                            local: out.clone(),
                            expr: Expr::Object(
                                fields
                                    .iter()
                                    .map(|(alias, column)| {
                                        (
                                            alias.clone(),
                                            Expr::field(Expr::Local(row.clone()), column),
                                        )
                                    })
                                    .collect(),
                            ),
                        },
                    );
                    let end = self.here();
                    self.patch(skip, end);
                }
                res
            }
        }
    }

    /// One row of `values(...)`: an object literal, or a `const` object
    /// declared before (Phase 13); Phase 14: a clock read in a value is a
    /// statement placeholder.
    fn insert_row(
        &mut self,
        m: &Module<'_>,
        table: &TableBinding,
        argument: &Expression<'_>,
    ) -> Result<Vec<(String, Expr)>, String> {
        let not_a_column = |column: &str| {
            format!(
                "`{column}` is not a column of table `{}` (property names must equal column names)",
                table.table
            )
        };
        if let Expression::Identifier(id) = argument
            && let Some(Binding::Object(fields)) = self.lookup(id.name.as_str())
        {
            let mut out = Vec::new();
            for (column, value) in fields {
                if !table.columns.contains(&column) {
                    return Err(not_a_column(&column));
                }
                out.push((column, value));
            }
            return Ok(out);
        }
        let Expression::ObjectExpression(object) = argument else {
            return Err(
                "`values` argument is not an object literal, a `const` object or an array of them"
                    .to_owned(),
            );
        };
        let mut out = Vec::new();
        for property in &object.properties {
            let ObjectPropertyKind::ObjectProperty(p) = property else {
                return Err("spread in `values`".to_owned());
            };
            let Some(column) = static_key(&p.key).filter(|_| !p.computed) else {
                return Err("computed key in `values`".to_owned());
            };
            if !table.columns.contains(&column) {
                return Err(not_a_column(&column));
            }
            let value = self.statement_value(m, &p.value, TOP);
            out.push((column, value));
        }
        Ok(out)
    }

    /// Phase 12: the fields of `returning({ alias: table.column, ... })`:
    /// (alias, column). Only plain column references of the inserted table.
    fn returning_projection(
        &self,
        table: &TableBinding,
        step: &ChainStep<'_, '_>,
    ) -> Result<Vec<(String, String)>, String> {
        let Some(Expression::ObjectExpression(object)) = step
            .args
            .first()
            .and_then(Argument::as_expression)
            .map(Expression::get_inner_expression)
        else {
            return Err("`returning(...)` argument is not an object literal".to_owned());
        };
        let mut out = Vec::new();
        for property in &object.properties {
            let ObjectPropertyKind::ObjectProperty(p) = property else {
                return Err("spread in `returning({...})`".to_owned());
            };
            let Some(alias) = static_key(&p.key).filter(|_| !p.computed) else {
                return Err("computed key in `returning({...})`".to_owned());
            };
            let column = crate::semantic::member_path(&p.value)
                .and_then(|path| path.split_once('.').map(|(_, c)| c.to_owned()))
                .filter(|c| !c.contains('.') && table.columns.contains(c))
                .ok_or_else(|| {
                    format!(
                        "`returning({{ {alias}: ... }})` is not a column of table `{}`",
                        table.table
                    )
                })?;
            out.push((alias, column));
        }
        Ok(out)
    }

    /// Phase 13: the config of `onConflictDoUpdate({ target, set, setWhere
    /// })` (`where` is Drizzle's deprecated name of `setWhere`). A partial
    /// index arbiter (`targetWhere`) is not modeled.
    fn upsert_config(
        &mut self,
        m: &Module<'_>,
        table: &TableBinding,
        step: &ChainStep<'_, '_>,
    ) -> Result<midwrite_model::ir::Upsert, String> {
        let Some(Expression::ObjectExpression(config)) = step
            .args
            .first()
            .and_then(Argument::as_expression)
            .map(Expression::get_inner_expression)
        else {
            return Err("`onConflictDoUpdate` argument is not an object literal".to_owned());
        };
        let column = |e: &Expression<'_>| {
            crate::semantic::member_path(e)
                .and_then(|path| path.split_once('.').map(|(_, c)| c.to_owned()))
                .filter(|c| !c.contains('.') && table.columns.contains(c))
        };
        let mut target = Vec::new();
        let mut set = Vec::new();
        let mut filter = None;
        for property in &config.properties {
            let ObjectPropertyKind::ObjectProperty(p) = property else {
                return Err("spread in `onConflictDoUpdate({...})`".to_owned());
            };
            let Some(key) = static_key(&p.key).filter(|_| !p.computed) else {
                return Err("computed key in `onConflictDoUpdate({...})`".to_owned());
            };
            let value = p.value.get_inner_expression();
            match key.as_str() {
                "target" => match value {
                    Expression::ArrayExpression(a) => {
                        for el in &a.elements {
                            let c = el
                                .as_expression()
                                .and_then(column)
                                .ok_or("an upsert target is not a column of the table")?;
                            target.push(c);
                        }
                    }
                    other => target.push(
                        column(other).ok_or("the upsert target is not a column of the table")?,
                    ),
                },
                "set" => {
                    let Expression::ObjectExpression(object) = value else {
                        return Err("the upsert `set` is not an object literal".to_owned());
                    };
                    for property in &object.properties {
                        let ObjectPropertyKind::ObjectProperty(q) = property else {
                            return Err("spread in the upsert `set`".to_owned());
                        };
                        let Some(c) = static_key(&q.key).filter(|_| !q.computed) else {
                            return Err("computed key in the upsert `set`".to_owned());
                        };
                        if !table.columns.contains(&c) {
                            return Err(format!(
                                "`{c}` is not a column of table `{}`",
                                table.table
                            ));
                        }
                        let v = self.set_value(m, &q.value);
                        set.push((c, v));
                    }
                }
                "setWhere" | "where" => filter = Some(self.pred(m, value)),
                other => return Err(format!("upsert option `{other}` is not modeled")),
            }
        }
        if target.is_empty() || set.is_empty() {
            return Err("an upsert needs `target` and `set`".to_owned());
        }
        Ok(midwrite_model::ir::Upsert {
            target,
            set,
            filter,
        })
    }

    /// A value in `set({ col: <value> })`: a plain expression or a `sql`
    /// template with column references and `+ - *`.
    fn set_value(&mut self, m: &Module<'_>, e: &Expression<'_>) -> Expr {
        let cx = Cx {
            col_ok: true,
            ..TOP
        };
        // Phase 14: `new Date()`, ``sql`now()` ``.
        if let Some(clock) = self.clock_value(e) {
            return clock;
        }
        match e.get_inner_expression() {
            Expression::TaggedTemplateExpression(tagged) => {
                let is_sql = matches!(tagged.tag.get_inner_expression(),
                    Expression::Identifier(id) if id.name.as_str() == "sql");
                if !is_sql {
                    return Expr::Unknown("tagged template other than `sql`".to_owned());
                }
                let mut tokens: Vec<SqlTok> = Vec::new();
                for (index, quasi) in tagged.quasi.quasis.iter().enumerate() {
                    match sql_tokens(quasi.value.raw.as_str()) {
                        Ok(mut t) => tokens.append(&mut t),
                        Err(fragment) => {
                            return Expr::Unknown(format!(
                                "SQL fragment `{fragment}` is not supported (only + - * and integers)"
                            ));
                        }
                    }
                    if let Some(expression) = tagged.quasi.expressions.get(index) {
                        let operand = self.expr(m, expression, cx);
                        tokens.push(SqlTok::Operand(operand));
                    }
                }
                let mut parser = SqlParser { tokens, pos: 0 };
                match parser.expression() {
                    Some(expr) if parser.pos == parser.tokens.len() => expr,
                    _ => Expr::Unknown("unsupported SQL arithmetic expression".to_owned()),
                }
            }
            _ => self.expr(m, e, cx),
        }
    }
}

/// Phase 13: where an operation through a replicated database runs.
enum Route {
    /// The primary, after skipping this many leading steps (`usePrimaryOnly()`)
    /// and, if set, the first path segment (`$primary`).
    Primary(usize, bool),
    Replica,
    Unknown(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mutation {
    Update,
    Insert,
    Delete,
}

enum Callee<'a> {
    /// Phase 12: a method of a repository object or class instance.
    Method {
        abs: PathBuf,
        rel: String,
        method: String,
        this: ThisBinding,
        label: String,
    },
    Local(FnInfo<'a>),
    /// Phase 13: a function wrapped by React `cache` in the caller's module.
    Cached(FnInfo<'a>),
    Imported {
        abs: PathBuf,
        rel: String,
        export: String,
        label: String,
    },
    /// Phase 14: a closure bound to a local name.
    Closure(Box<Closure>),
    Unresolvable(String),
}

// ---------------------------------------------------------------------------
// Syntax helpers
// ---------------------------------------------------------------------------

/// Whether a statement can jump out of its block (`return`, `break`,
/// `continue`; a `throw` is caught by the enclosing handler).
fn has_jump(statement: &Statement<'_>) -> bool {
    match statement {
        Statement::ReturnStatement(_)
        | Statement::BreakStatement(_)
        | Statement::ContinueStatement(_) => true,
        Statement::BlockStatement(b) => b.body.iter().any(has_jump),
        Statement::IfStatement(i) => {
            has_jump(&i.consequent) || i.alternate.as_ref().is_some_and(has_jump)
        }
        Statement::TryStatement(t) => {
            t.block.body.iter().any(has_jump)
                || t.handler
                    .as_ref()
                    .is_some_and(|h| h.body.body.iter().any(has_jump))
                || t.finalizer
                    .as_ref()
                    .is_some_and(|f| f.body.iter().any(has_jump))
        }
        _ => false,
    }
}

/// Phase 14: whether a member chain contains an optional link (`a?.b`).
fn has_optional_link(e: &Expression<'_>) -> bool {
    match e.get_inner_expression() {
        Expression::StaticMemberExpression(m) => m.optional || has_optional_link(&m.object),
        Expression::ComputedMemberExpression(m) => m.optional || has_optional_link(&m.object),
        Expression::ChainExpression(_) => true,
        _ => false,
    }
}

fn is_nullish(e: &Expression<'_>) -> bool {
    match e.get_inner_expression() {
        Expression::NullLiteral(_) => true,
        Expression::Identifier(id) => id.name.as_str() == "undefined",
        _ => false,
    }
}

fn throw_message(argument: &Expression<'_>) -> String {
    match argument.get_inner_expression() {
        Expression::NewExpression(new) => {
            let ctor = callee_text(&new.callee).unwrap_or_else(|| "Error".to_owned());
            match new.arguments.first().and_then(Argument::as_expression) {
                Some(first) => match literal(first) {
                    Some(Expr::Lit(Value::Str(s))) => s,
                    _ => ctor,
                },
                None => ctor,
            }
        }
        _ => "thrown value".to_owned(),
    }
}

pub(crate) fn expression_kind(e: &Expression<'_>) -> &'static str {
    match e {
        Expression::TemplateLiteral(_) => "template literal",
        Expression::ObjectExpression(_) => "object literal",
        Expression::ArrayExpression(_) => "array literal",
        Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_) => "function",
        Expression::NewExpression(_) => "`new`",
        Expression::ThisExpression(_) => "`this`",
        Expression::ComputedMemberExpression(_) => "computed member access",
        Expression::TaggedTemplateExpression(_) => "tagged template",
        Expression::AssignmentExpression(_) => "assignment",
        Expression::UpdateExpression(_) => "update (++/--)",
        Expression::SequenceExpression(_) => "sequence",
        Expression::RegExpLiteral(_) => "regular expression",
        Expression::BigIntLiteral(_) => "bigint",
        _ => "other",
    }
}

/// Flatten `root.a.b(...).c(...)` into the root identifier and the method
/// calls, each with the member path before it. Built on the shared chain
/// facts ([`crate::semantic`]); this extractor does not model optional
/// chaining, `this` receivers or a directly called root, and refuses them
/// (as before Phase 8).
fn flatten<'a, 'b>(e: &'b Expression<'a>) -> Option<(String, Vec<ChainStep<'a, 'b>>)> {
    let chain = unwind(e);
    if chain.optional {
        return None;
    }
    // Phase 12: `this.field.method(...)` inside an inlined method; the root
    // is written `this` and the field is the first path segment.
    let root = match chain.root {
        Some(Root::Identifier(root)) => root,
        Some(Root::This(_)) if chain.root_call().is_none() => {
            let steps: Vec<ChainStep<'a, 'b>> = chain
                .method_calls()?
                .into_iter()
                .map(|call| ChainStep {
                    path: call.path.iter().map(|p| (*p).to_owned()).collect(),
                    method: call.method.to_owned(),
                    args: call.arguments,
                })
                .collect();
            return (!steps.is_empty()).then(|| ("this".to_owned(), steps));
        }
        _ => return None,
    };
    // Phase 12: a zero-argument call of a plain function (`getDb()`) may be
    // a database factory; the root is then written `getDb()`.
    let root_name = match chain.root_call() {
        None => root.name.as_str().to_owned(),
        Some(call) if call.arguments.is_empty() && !call.optional => {
            format!("{}()", root.name.as_str())
        }
        Some(_) => return None,
    };
    let steps: Vec<ChainStep<'a, 'b>> = chain
        .method_calls()?
        .into_iter()
        .map(|call| ChainStep {
            path: call.path.iter().map(|p| (*p).to_owned()).collect(),
            method: call.method.to_owned(),
            args: call.arguments,
        })
        .collect();
    if steps.is_empty() {
        return None;
    }
    Some((root_name, steps))
}

// ---------------------------------------------------------------------------
// `sql` template arithmetic
// ---------------------------------------------------------------------------

enum SqlTok {
    Num(i64),
    Op(char),
    Operand(Expr),
}

fn sql_tokens(text: &str) -> Result<Vec<SqlTok>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let digits: String = chars[start..i].iter().collect();
            match digits.parse() {
                Ok(n) => out.push(SqlTok::Num(n)),
                Err(_) => return Err(digits),
            }
        } else if matches!(c, '+' | '-' | '*' | '(' | ')') {
            out.push(SqlTok::Op(c));
            i += 1;
        } else if chars[i..].starts_with(&['e', 'x', 'c', 'l', 'u', 'd', 'e', 'd', '.']) {
            // Phase 13: `excluded.column` in an upsert's `set` (the row
            // proposed for insertion).
            let start = i + 9;
            let mut j = start;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            if j == start {
                return Err(chars[i..].iter().collect::<String>().trim().to_owned());
            }
            let name: String = chars[start..j].iter().collect();
            out.push(SqlTok::Operand(Expr::Col(format!("excluded.{name}"))));
            i = j;
        } else {
            return Err(chars[i..].iter().collect::<String>().trim().to_owned());
        }
    }
    Ok(out)
}

struct SqlParser {
    tokens: Vec<SqlTok>,
    pos: usize,
}

impl SqlParser {
    fn peek_op(&self) -> Option<char> {
        match self.tokens.get(self.pos) {
            Some(SqlTok::Op(c)) => Some(*c),
            _ => None,
        }
    }

    fn expression(&mut self) -> Option<Expr> {
        let mut left = self.term()?;
        while let Some(op @ ('+' | '-')) = self.peek_op() {
            self.pos += 1;
            let right = self.term()?;
            left = Expr::bin(if op == '+' { BinOp::Add } else { BinOp::Sub }, left, right);
        }
        Some(left)
    }

    fn term(&mut self) -> Option<Expr> {
        let mut left = self.factor()?;
        while self.peek_op() == Some('*') {
            self.pos += 1;
            let right = self.factor()?;
            left = Expr::bin(BinOp::Mul, left, right);
        }
        Some(left)
    }

    fn factor(&mut self) -> Option<Expr> {
        match self.tokens.get(self.pos)? {
            SqlTok::Num(n) => {
                let n = *n;
                self.pos += 1;
                Some(Expr::lit(n))
            }
            SqlTok::Operand(e) => {
                let e = e.clone();
                self.pos += 1;
                Some(e)
            }
            SqlTok::Op('(') => {
                self.pos += 1;
                let e = self.expression()?;
                (self.peek_op() == Some(')')).then(|| {
                    self.pos += 1;
                    e
                })
            }
            SqlTok::Op('-') => {
                self.pos += 1;
                let e = self.factor()?;
                Some(Expr::bin(BinOp::Sub, Expr::lit(0), e))
            }
            SqlTok::Op(_) => None,
        }
    }
}

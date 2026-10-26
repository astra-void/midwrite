//! Lowering of TypeScript functions to the concurrency IR of
//! `commitrift-model` (Phase 6, stage G).
//!
//! This is a *separate, opt-in* frontend: `commitrift check` never calls it.
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

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, ArrowFunctionBody, BindingPattern, ChainElement, Declaration,
    ExportDefaultDeclarationKind, Expression, FormalParameters, Function, FunctionBody,
    ImportDeclarationSpecifier, ModuleExportName, ObjectPropertyKind, Program, Statement,
    VariableDeclarator,
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};

use crate::semantic::{Root, member_path as callee_text, property_key as static_key, unwind};
use oxc_syntax::operator::{AssignmentOperator, BinaryOperator, LogicalOperator, UnaryOperator};

use commitrift_core::config::Config;
use commitrift_core::location::LineIndex;
use commitrift_model::ir::{
    Conn, Isolation, Join, JoinKind, LockMode, OnConflict, Op, Origin, ProjItem, SourceRef, Step,
    Stmt, StmtKind,
};
use commitrift_model::provider::{
    ActorProvider, EntryInfo, ExternalBinding, ExtractRequest, Extracted, InvocationContract,
    TableBinding, TransactionRetryBinding, WrapperFacts,
};
use commitrift_model::value::{BinOp, Expr, Value};

use crate::identity;
use crate::resolve::{Resolution, Resolver, absolute};
use crate::wrapper;

/// Lowers functions of a TypeScript project to the concurrency IR.
#[derive(Clone, Debug)]
pub struct TsActorProvider {
    root: PathBuf,
    receivers: Vec<String>,
}

impl TsActorProvider {
    /// A provider for the project at `root`. Database receiver names are
    /// read from `root/commitrift.json` when it exists (plain identifiers
    /// only); the default is `db`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let mut receivers = vec!["db".to_owned()];
        let config = root.join("commitrift.json");
        if config.is_file()
            && let Ok(config) = Config::load(&config)
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
        Self { root, receivers }
    }

    /// Override the identifiers treated as the root database handle.
    pub fn with_receivers(mut self, receivers: Vec<String>) -> Self {
        self.receivers = receivers;
        self
    }
}

impl ActorProvider for TsActorProvider {
    fn extract_schema(
        &self,
        root: &Path,
        file: &str,
    ) -> Result<commitrift_model::schema_facts::ExtractedSchema, String> {
        let root = if root.as_os_str().is_empty() {
            self.root.as_path()
        } else {
            root
        };
        crate::schema::extract_schema(root, file)
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
        let source =
            std::fs::read_to_string(&main_abs).map_err(|e| format!("cannot read `{file}`: {e}"))?;
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

        let root_abs = absolute(&root);
        let mut files: Vec<PathBuf> = crate::discover(std::slice::from_ref(&root_abs))
            .unwrap_or_default()
            .into_iter()
            .map(|p| absolute(&p))
            .collect();
        if !files.contains(&main_abs) {
            files.push(main_abs.clone());
        }
        let names: Vec<String> = files.iter().map(|p| relative(&root_abs, p)).collect();
        let resolver = Resolver::new(&files, names);

        let mut lowerer = Lowerer {
            request,
            receivers: &self.receivers,
            root: root_abs,
            files,
            resolver,
            codes: vec![Vec::new()],
            cur: 0,
            frames: Vec::new(),
            counter: 0,
            tx_open: 0,
            assumptions: Vec::new(),
            qualify: false,
        };
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

/// The name `commitrift check` gives a selected function: its binding for a
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
        Expression::ArrowFunctionExpression(arrow) => Some(FnInfo {
            name: name.to_owned(),
            is_async: arrow.r#async,
            params: &arrow.params,
            body: match &arrow.body {
                ArrowFunctionBody::FunctionBody(body) => FnBody::Block(body),
                other => FnBody::Expr(other.as_expression()?),
            },
            span: arrow.span,
        }),
        Expression::FunctionExpression(function) => info_of_function(name, function),
        _ => None,
    }
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
}

enum ArgVal {
    Handle(Binding),
    Expr(Expr),
}

struct Lowerer<'r> {
    request: &'r ExtractRequest,
    receivers: &'r [String],
    root: PathBuf,
    files: Vec<PathBuf>,
    resolver: Resolver,
    codes: Vec<Vec<Step>>,
    cur: usize,
    frames: Vec<Frame>,
    counter: u32,
    tx_open: u32,
    assumptions: Vec<String>,
    /// Column references are qualified (`table.column`): set while lowering
    /// a joined query.
    qualify: bool,
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
        let end = (span.end as usize).min(module.source.len());
        let start = (span.start as usize).min(end);
        let mut text: String = module.source[start..end]
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

    fn handle_of(&self, name: &str) -> Option<Binding> {
        match self.lookup(name) {
            Some(Binding::Tx) => Some(Binding::Tx),
            Some(Binding::Db) => Some(Binding::Db),
            Some(_) => None,
            None => self
                .receivers
                .iter()
                .any(|r| r == name)
                .then_some(Binding::Db),
        }
    }

    fn table(&self, identifier: &str) -> Option<&'r TableBinding> {
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
            Callee::Local(info) => analyze(&info, m),
            Callee::Imported {
                abs, rel, export, ..
            } => {
                let Ok(source) = std::fs::read_to_string(&abs) else {
                    return wrapper::unknown(&label, format!("cannot read `{rel}`"));
                };
                let allocator = Allocator::default();
                let program = match load(&allocator, &abs, &source) {
                    Ok(p) => p,
                    Err(e) => return wrapper::unknown(&label, format!("`{rel}`: {e}")),
                };
                let module = Module::new(&abs, rel.clone(), &source, &program);
                match find_export(&module, &export) {
                    Ok(info) => analyze(&info, &module),
                    Err(why) => wrapper::unknown(&label, why),
                }
            }
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
        });
        let mut params = Vec::new();
        for (index, item) in info.params.items.iter().enumerate() {
            if let BindingPattern::BindingIdentifier(id) = &item.pattern {
                let name = id.name.as_str().to_owned();
                self.frame_mut().used.insert(name.clone());
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
            self.emit(
                m,
                property.span,
                Op::Let {
                    local: local.clone(),
                    expr: Expr::Field(Box::new(base.clone()), key),
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
                    self.declarator(m, declarator);
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
                let message = throw_message(&s.argument);
                self.emit(m, s.span, Op::Throw { message });
            }
            Statement::TryStatement(t) => self.try_statement(m, t),
            Statement::SwitchStatement(sw) => self.switch_statement(m, sw),
            Statement::ForOfStatement(f) => self.for_of(m, f),
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
        let enter = self.emit(
            m,
            t.block.span,
            Op::Try {
                handler: usize::MAX,
                error: rethrow.clone(),
            },
        );
        self.frame_mut().tries += 1;
        self.frame_mut().scopes.push(HashMap::new());
        for statement in &t.block.body {
            self.statement(m, statement);
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
                let caught = t.finalizer.is_some().then(|| self.tmp("err"));
                let inner = caught.as_ref().map(|err| {
                    self.emit(
                        m,
                        catch.span,
                        Op::Try {
                            handler: usize::MAX,
                            error: Some(err.clone()),
                        },
                    )
                });
                if inner.is_some() {
                    self.frame_mut().tries += 1;
                }
                self.frame_mut().scopes.push(HashMap::new());
                if let Some(param) = &catch.param {
                    match &param.pattern {
                        BindingPattern::BindingIdentifier(id) => {
                            self.declare(id.name.as_str(), Binding::Opaque);
                        }
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
                if let (Some(inner), Some(err)) = (inner, caught) {
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
        let Expression::ArrayExpression(array) = f.right.get_inner_expression() else {
            self.unknown_op(
                m,
                f.span,
                "a loop over a collection of unknown length is not modeled (only `for (const x of [literal, ...])`)",
            );
            return;
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
            None => Expr::Lit(Value::Null),
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
        }
    }

    fn expression_statement(&mut self, m: &Module<'_>, expression: &Expression<'_>, span: Span) {
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
                self.emit(
                    m,
                    d.span,
                    Op::Let {
                        local,
                        expr: Expr::Lit(Value::Null),
                    },
                );
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
                if let Some(call) = call {
                    let res = self.call_value(m, call, awaited, Want::Scalar, init.span());
                    self.bind_result(m, &d.id, res, d.span, false);
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
                }
            }
            BindingPattern::ArrayPattern(array) => {
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
                    self.emit(
                        m,
                        property.span,
                        Op::Let {
                            local: local.clone(),
                            expr: Expr::Field(Box::new(base.clone()), key),
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
            return Expr::Lit(Value::Null);
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
            Some(Binding::Tx | Binding::Db) => {
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
            Some(Binding::Opaque) if cx.truthy => Expr::lit(true),
            Some(Binding::Opaque) => Expr::Unknown(format!(
                "the caught error `{name}` is not modeled (only that it was thrown)"
            )),
            Some(Binding::Object(_)) if cx.truthy => Expr::lit(true),
            Some(Binding::Object(_)) => Expr::Unknown(format!(
                "the object `{name}` is only modeled through its properties"
            )),
            None => {
                if let Some(constant) = module_constant(m, name) {
                    return constant;
                }
                let _ = cx;
                if self.table(name).is_some() {
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
                        .map_or(Expr::Lit(Value::Null), |(_, e)| e.clone());
                }
                None => {
                    if let Some(table) = self.table(name) {
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
                _ => return Expr::Unknown("`?.` on a non-identifier base".to_owned()),
            };
            let tmp = self.tmp("opt");
            let span = object.span();
            self.emit(
                m,
                span,
                Op::Let {
                    local: tmp.clone(),
                    expr: Expr::Lit(Value::Null),
                },
            );
            let jump = self.emit(
                m,
                span,
                Op::JumpIf {
                    cond: Expr::IsNull(Box::new(base.clone())),
                    target: usize::MAX,
                },
            );
            self.emit(
                m,
                span,
                Op::Let {
                    local: tmp.clone(),
                    expr: Expr::Field(Box::new(base), property.to_owned()),
                },
            );
            let end = self.here();
            self.patch(jump, end);
            return Expr::Local(tmp);
        }
        let base = self.expr(
            m,
            object,
            Cx {
                truthy: false,
                ..cx
            },
        );
        Expr::Field(Box::new(base), property.to_owned())
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
            LogicalOperator::Coalesce => {
                Expr::not(Expr::IsNull(Box::new(Expr::Local(tmp.clone()))))
            }
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
        let Resolution::File(index) = self.resolver.resolve(&m.abs, &source) else {
            return None;
        };
        let abs = self.files[index].clone();
        let text = std::fs::read_to_string(&abs).ok()?;
        let allocator = Allocator::default();
        let program = load(&allocator, &abs, &text).ok()?;
        let module = Module::new(&abs, relative(&self.root, &abs), &text, &program);
        enum_member_in(&module, &export, member)
    }

    fn binary(&mut self, m: &Module<'_>, b: &oxc_ast::ast::BinaryExpression<'_>, cx: Cx) -> Expr {
        use BinaryOperator as B;
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
        let text = callee_text(&call.callee);
        if text.as_deref() == Some("Promise.all") {
            return self.promise_all(m, call, awaited, want);
        }
        if let Some((root, steps)) = flatten(e)
            && let Some(handle) = self.handle_of(&root)
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
        match self.resolve_callee(m, &call.callee) {
            Callee::Local(info) => self.inline(m, &info, m, call, awaited, want),
            Callee::Imported {
                abs,
                rel,
                export,
                label,
            } => self.inline_imported(m, &abs, &rel, &export, &label, call, awaited, want),
            Callee::Unresolvable(reason) => self.unknown_op(
                m,
                call.span,
                format!(
                    "call to `{}`: {reason}",
                    text.as_deref().unwrap_or("<expression>")
                ),
            ),
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
                if let Some(info) = find_function(m, name) {
                    return Callee::Local(info);
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
                Callee::Unresolvable("method calls are not resolved".to_owned())
            }
            _ => Callee::Unresolvable("the callee is not a plain function reference".to_owned()),
        }
    }

    fn callee_import<'a>(
        &mut self,
        m: &Module<'_>,
        source: &str,
        export: String,
        label: &str,
    ) -> Callee<'a> {
        match self.resolver.resolve(&m.abs, source) {
            Resolution::File(index) => {
                let abs = self.files[index].clone();
                let rel = relative(&self.root, &abs);
                Callee::Imported {
                    abs,
                    rel,
                    export,
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
        let source = match std::fs::read_to_string(abs) {
            Ok(s) => s,
            Err(e) => {
                return self.unknown_op(
                    caller,
                    call.span,
                    format!("call to `{label}`: cannot read `{rel}`: {e}"),
                );
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
        let info = match find_export(&module, export) {
            Ok(info) => info,
            Err(why) => {
                return self.unknown_op(caller, call.span, format!("call to `{label}`: {why}"));
            }
        };
        self.inline(caller, &info, &module, call, awaited, want)
    }

    /// Inline `info` (declared in `callee_module`) at `call` (written in
    /// `caller`).
    fn inline(
        &mut self,
        caller: &Module<'_>,
        info: &FnInfo<'_>,
        callee_module: &Module<'_>,
        call: &oxc_ast::ast::CallExpression<'_>,
        awaited: bool,
        want: Want,
    ) -> Res {
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
            let (child, ()) = self.with_child(caller, call.span, |this| {
                this.inline_here(caller, info, callee_module, call, Want::Discard);
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
        self.inline_here(caller, info, callee_module, call, want)
    }

    fn inline_here(
        &mut self,
        caller: &Module<'_>,
        info: &FnInfo<'_>,
        callee_module: &Module<'_>,
        call: &oxc_ast::ast::CallExpression<'_>,
        want: Want,
    ) -> Res {
        // Evaluate the arguments in the caller's frame.
        let mut args: Vec<ArgVal> = Vec::new();
        for argument in &call.arguments {
            match argument.as_expression() {
                Some(expression) => {
                    let inner = expression.get_inner_expression();
                    if let Expression::Identifier(id) = inner
                        && let Some(handle) = self.handle_of(id.name.as_str())
                    {
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
        self.frames.push(Frame {
            function: info.name.clone(),
            file: callee_module.rel.clone(),
            prefix,
            scopes: vec![HashMap::new()],
            rets: vec![Ret {
                result: result.clone(),
                patches: Vec::new(),
                main: false,
            }],
            used,
            key: (callee_module.abs.clone(), info.name.clone()),
            tries: 0,
            breaks: Vec::new(),
        });
        self.emit(
            callee_module,
            call.span,
            Op::Let {
                local: result.clone(),
                expr: Expr::Lit(Value::Null),
            },
        );
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
                            None => Expr::Lit(Value::Null),
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

    fn table_arg(&mut self, arg: Option<&Argument<'_>>) -> Result<&'r TableBinding, String> {
        let Some(expression) = arg.and_then(Argument::as_expression) else {
            return Err("missing table argument".to_owned());
        };
        let Expression::Identifier(id) = expression.get_inner_expression() else {
            return Err("the table is not a plain identifier".to_owned());
        };
        self.table(id.name.as_str()).ok_or_else(|| {
            format!(
                "table identifier `{}` is not bound to a table of the scenario",
                id.name
            )
        })
    }

    fn pred(&mut self, m: &Module<'_>, e: &Expression<'_>) -> Expr {
        let cx = Cx {
            col_ok: true,
            ..TOP
        };
        let e = e.get_inner_expression();
        let Expression::CallExpression(call) = e else {
            return Expr::Unknown("predicate is not a Drizzle operator call".to_owned());
        };
        let Expression::Identifier(op) = call.callee.get_inner_expression() else {
            return Expr::Unknown("predicate operator is not a plain identifier".to_owned());
        };
        let args: Vec<&Expression<'_>> = call
            .arguments
            .iter()
            .filter_map(Argument::as_expression)
            .collect();
        if args.len() != call.arguments.len() {
            return Expr::Unknown("spread argument in a predicate".to_owned());
        }
        let name = op.name.as_str();
        let binary = |this: &mut Self, op: BinOp| -> Expr {
            if args.len() != 2 {
                return Expr::Unknown(format!("`{name}` expects two arguments"));
            }
            let a = this.expr(m, args[0], cx);
            let b = this.expr(m, args[1], cx);
            Expr::bin(op, a, b)
        };
        match name {
            "eq" => binary(self, BinOp::Eq),
            "ne" => binary(self, BinOp::Ne),
            "gt" => binary(self, BinOp::Gt),
            "gte" => binary(self, BinOp::Ge),
            "lt" => binary(self, BinOp::Lt),
            "lte" => binary(self, BinOp::Le),
            "and" | "or" => {
                if args.is_empty() {
                    return Expr::Unknown(format!("`{name}()` without arguments"));
                }
                let op = if name == "and" { BinOp::And } else { BinOp::Or };
                let mut it = args.into_iter();
                let mut acc = self.pred(m, it.next().expect("non-empty"));
                for next in it {
                    let right = self.pred(m, next);
                    acc = Expr::bin(op, acc, right);
                }
                acc
            }
            "not" if args.len() == 1 => Expr::not(self.pred(m, args[0])),
            "isNull" | "isNotNull" if args.len() == 1 => {
                let inner = self.expr(m, args[0], cx);
                let is_null = Expr::IsNull(Box::new(inner));
                if name == "isNull" {
                    is_null
                } else {
                    Expr::not(is_null)
                }
            }
            other => Expr::Unknown(format!(
                "predicate operator `{other}` is not supported (supported: eq ne gt gte lt lte and or not isNull isNotNull)"
            )),
        }
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
            self.emit(m, span, Op::Unknown { detail });
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
        let Some(table) = self.table(&step.path[1]) else {
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
        if want == Want::Discard {
            return self.unknown_op(
                m,
                span,
                "a select result is only supported as `const [row] = await db.select()...`, `const rows = await db.select()...limit(1)` (or a count)",
            );
        }
        let mut table: Option<&TableBinding> = None;
        let mut joins: Vec<(JoinKind, &TableBinding, &ChainStep<'_, '_>)> = Vec::new();
        let mut where_step = None;
        let mut limit: Option<i64> = None;
        let mut lock = LockMode::None;
        let mut ordered = false;
        for step in &steps[1..] {
            if !step.path.is_empty() {
                return self.unknown_op(m, span, "unsupported select chain");
            }
            match step.method.as_str() {
                "from" => match self.table_arg(step.args.first()) {
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
                    match self.table_arg(step.args.first()) {
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
                "orderBy" => ordered = true,
                "for" => {
                    let mode = step
                        .args
                        .first()
                        .and_then(Argument::as_expression)
                        .and_then(literal);
                    if !matches!(&mode, Some(Expr::Lit(Value::Str(s))) if s == "update") {
                        return self.unknown_op(
                            m,
                            span,
                            "only `.for(\"update\")` row locking is supported",
                        );
                    }
                    lock = LockMode::ForUpdate;
                    if let Some(options) = step.args.get(1).and_then(Argument::as_expression) {
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
        if lock != LockMode::None && limit != Some(1) {
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
        if want == Want::Scalar && limit != Some(1) && steps[0].args.is_empty() {
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
        if want == Want::Scalar && aggregate.is_none() && limit != Some(1) {
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
        self.assume(if ordered {
            "orderBy is not modeled: rows are taken in primary-key order"
        } else {
            "select without orderBy: the first row in primary-key order is used"
        });
        if joined {
            self.assume("a join without orderBy: rows are taken in primary-key order of the first table, then of each joined table");
        }
        let into = self.tmp("row");
        let kind = StmtKind::Select {
            table: table.table.clone(),
            pred: pred.clone(),
            lock,
            into: into.clone(),
            joins: ir_joins,
            projection,
        };
        match self.emit_db(m, span, conn, kind, &[&pred]) {
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
        let table = match self.table_arg(steps[0].args.first()) {
            Ok(t) => t,
            Err(why) => return self.unknown_op(m, span, why),
        };
        let mut set: Option<&ChainStep<'_, '_>> = None;
        let mut values: Option<&ChainStep<'_, '_>> = None;
        let mut where_step = None;
        let mut returning = false;
        let mut returning_projected = false;
        let mut on_conflict = OnConflict::Error;
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
                    returning_projected = !step.args.is_empty();
                }
                (Mutation::Insert, "onConflictDoNothing") => on_conflict = OnConflict::DoNothing,
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
        if returned_row.is_some() && returning_projected {
            return self.unknown_op(
                m,
                span,
                "`returning({...})` with a projection is not modeled (use `returning()`)",
            );
        }
        let affected_name = affected.clone();
        let res = match (&affected, &returned_row) {
            (Some(length), Some(row)) => Res::Rows {
                row: row.clone(),
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
                // `values(obj)` with `const obj = { ... }` declared before.
                let declared = match argument {
                    Some(Expression::Identifier(id)) => match self.lookup(id.name.as_str()) {
                        Some(Binding::Object(fields)) => Some(fields),
                        _ => None,
                    },
                    _ => None,
                };
                let mut assignments = Vec::new();
                let from_declared = declared.is_some();
                if let Some(fields) = declared {
                    for (column, value) in fields {
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
                        assignments.push((column, value));
                    }
                }
                let object = match argument {
                    Some(Expression::ObjectExpression(object)) => Some(object),
                    _ if from_declared => None,
                    _ => {
                        return self.unknown_op(
                            m,
                            span,
                            "`values` argument is not a single object literal or a `const` object (multi-row inserts are not supported)",
                        );
                    }
                };
                for property in object.map(|o| o.properties.as_slice()).unwrap_or_default() {
                    let ObjectPropertyKind::ObjectProperty(p) = property else {
                        return self.unknown_op(m, span, "spread in `values`");
                    };
                    let Some(column) = static_key(&p.key).filter(|_| !p.computed) else {
                        return self.unknown_op(m, span, "computed key in `values`");
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
                    let value = self.expr(m, &p.value, TOP);
                    assignments.push((column, value));
                }
                let refs: Vec<&Expr> = assignments.iter().map(|(_, e)| e).collect();
                let stmt = StmtKind::Insert {
                    table: table.table.clone(),
                    values: assignments.clone(),
                    on_conflict,
                    affected: affected_name,
                    returning: returned_row,
                };
                match self.emit_db(m, span, conn, stmt, &refs) {
                    Some(()) => res,
                    None => Res::Value(Expr::Unknown("insert".to_owned())),
                }
            }
        }
    }

    /// A value in `set({ col: <value> })`: a plain expression or a `sql`
    /// template with column references and `+ - *`.
    fn set_value(&mut self, m: &Module<'_>, e: &Expression<'_>) -> Expr {
        let cx = Cx {
            col_ok: true,
            ..TOP
        };
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mutation {
    Update,
    Insert,
    Delete,
}

enum Callee<'a> {
    Local(FnInfo<'a>),
    Imported {
        abs: PathBuf,
        rel: String,
        export: String,
        label: String,
    },
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

fn expression_kind(e: &Expression<'_>) -> &'static str {
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
    if chain.optional || chain.root_call().is_some() {
        return None;
    }
    let Some(Root::Identifier(root)) = chain.root else {
        return None;
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
    Some((root.name.as_str().to_owned(), steps))
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

//! Project-wide index of candidate entry functions.
//!
//! For every module of a project this lists, without executing anything:
//!
//! * the functions a caller outside the module can reach, each as a source
//!   selector (`identity.rs` syntax) with the syntactic reason it is an
//!   entry candidate: an exported function, a callback passed to a call that
//!   initializes an exported binding (`withMember(schema, async (...) =>
//!   ...)`), a method of an exported class, a function-valued property of an
//!   exported object;
//! * whether the module is a Server Action module (`"use server"`) and
//!   whether it is a Next.js route module (`route.ts` exporting `GET`,
//!   `POST`, ...);
//! * the project files it imports (resolved like the analyzer resolves
//!   imports), which is what an extraction of one of its functions can read;
//! * transaction helpers: functions that pass one of their parameters as the
//!   callback of `<receiver>.transaction(...)`.
//!
//! An entry *candidate* is a syntactic fact, not a claim that something calls
//! it concurrently: the discovery engine reports how each entry may be
//! invoked separately (`midwrite-discover`).

use std::path::{Path, PathBuf};

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrowFunctionBody, BindingPattern, ClassElement, Declaration, Expression,
    FormalParameters, FunctionBody, MethodDefinitionKind, ObjectPropertyKind, Program, PropertyKey,
    Statement,
};
use oxc_ast_visit::Visit;
use oxc_parser::Parser;
use oxc_span::{SourceType, Span};
use serde::Serialize;

use midwrite_core::location::LineIndex;

use crate::resolve::{Resolution, Resolver, absolute};

/// Why a function is listed as an entry candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntryKind {
    /// An exported function of a `"use server"` module (or a callback
    /// passed to the call that initializes such an export).
    ServerAction,
    /// `GET`/`POST`/`PUT`/`PATCH`/`DELETE` exported by a `route.ts` module.
    RouteHandler,
    /// Any other exported function.
    ExportedFunction,
    /// A callback passed to the call that initializes an exported binding
    /// (outside a `"use server"` module).
    WrappedCallback,
    /// A method of an exported class.
    ClassMethod,
    /// A function-valued property of an exported object literal.
    ObjectProperty,
    /// The resolver of a tRPC procedure in a router object
    /// (`router({ name: procedure.input(s).mutation(async ({ ctx, input }) => …) })`).
    TrpcProcedure,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ServerAction => "server-action",
            Self::RouteHandler => "route-handler",
            Self::ExportedFunction => "exported-function",
            Self::WrappedCallback => "wrapped-callback",
            Self::ClassMethod => "class-method",
            Self::ObjectProperty => "object-property",
            Self::TrpcProcedure => "trpc-procedure",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryFact {
    /// A source selector (`src/app/x.ts#name`, `#name/arg[1]`, ...).
    pub selector: String,
    pub kind: EntryKind,
    /// Display name: the binding, `Class.method`, `obj.prop` or
    /// `binding(callback)`.
    pub name: String,
    /// Start of the function node (its identity anchor).
    pub line: u32,
    pub column: u32,
    /// The callee a wrapped callback is passed to, as written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrapper: Option<String>,
    /// Whether the function is declared `async`.
    pub is_async: bool,
}

/// A function that runs one of its parameters as a transaction callback.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionHelper {
    /// The function's name as callers write it.
    pub name: String,
    pub file: String,
    pub line: u32,
    /// Index of the parameter passed as the transaction callback.
    pub callback_argument: usize,
    /// Index of a parameter whose properties feed the transaction options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options_argument: Option<usize>,
    /// The body contains a loop: the helper may run the callback more than
    /// once (its loop is not analyzed).
    pub has_loop: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleFacts {
    /// Path relative to the project root, `/`-separated.
    pub file: String,
    pub use_server: bool,
    pub route_module: bool,
    /// Project files this module imports (relative paths, sorted).
    pub imports: Vec<String>,
    /// Import specifiers that are packages (not project files).
    pub packages: Vec<String>,
    /// Import specifiers that lead nowhere or are ambiguous.
    pub unresolved: Vec<String>,
    pub entries: Vec<EntryFact>,
    /// The module calls `pgTable(` (a Drizzle schema module).
    pub declares_tables: bool,
    pub transaction_helpers: Vec<TransactionHelper>,
    /// Exports the index does not list as entries, with the reason
    /// (`export default`, re-exports, destructured bindings).
    pub skipped: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_error: Option<String>,
}

const ROUTE_METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// Index every TypeScript module under `root` (the analyzer's file
/// discovery, the analyzer's import resolution). `receivers` are the
/// identifiers treated as the root database handle (transaction helpers are
/// recognized on them).
pub fn index_project(root: &Path, receivers: &[String]) -> Result<Vec<ModuleFacts>, String> {
    let root_abs = absolute(root);
    let mut files: Vec<PathBuf> = crate::discover(std::slice::from_ref(&root_abs))
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|p| absolute(&p))
        .collect();
    files.sort();
    let names: Vec<String> = files.iter().map(|p| relative(&root_abs, p)).collect();
    let mut resolver = Resolver::new(&files, names.clone());
    let mut out = Vec::with_capacity(files.len());
    for (abs, rel) in files.iter().zip(&names) {
        out.push(index_module(abs, rel, &names, &mut resolver, receivers));
    }
    Ok(out)
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn index_module(
    abs: &Path,
    rel: &str,
    names: &[String],
    resolver: &mut Resolver,
    receivers: &[String],
) -> ModuleFacts {
    let mut facts = ModuleFacts {
        file: rel.to_owned(),
        ..ModuleFacts::default()
    };
    let source = match std::fs::read_to_string(abs) {
        Ok(s) => s,
        Err(e) => {
            facts.parse_error = Some(format!("cannot read: {e}"));
            return facts;
        }
    };
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(abs).unwrap_or_else(|_| SourceType::ts());
    let parsed = Parser::new(&allocator, &source, source_type).parse();
    if parsed.fatal_error || parsed.diagnostics.errors().next().is_some() {
        let message = parsed
            .diagnostics
            .errors()
            .next()
            .map_or_else(|| "fatal".to_owned(), |e| e.message.to_string());
        facts.parse_error = Some(format!("parse error: {message}"));
        return facts;
    }
    let program = parsed.program;
    let lines = LineIndex::new(&source);
    facts.use_server = program
        .directives
        .iter()
        .any(|d| d.directive.as_str() == "use server");
    let stem = Path::new(rel)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    facts.route_module = stem == "route";
    facts.declares_tables = source.contains("pgTable(");

    let mut imports = std::collections::BTreeSet::new();
    let mut packages = std::collections::BTreeSet::new();
    let mut unresolved = std::collections::BTreeSet::new();
    for specifier in module_specifiers(&program) {
        match resolver.resolve(abs, &specifier) {
            Resolution::File(i) => {
                imports.insert(names[i].clone());
            }
            Resolution::Package => {
                packages.insert(specifier);
            }
            Resolution::NotFound(_) | Resolution::Ambiguous(_) => {
                unresolved.insert(specifier);
            }
        }
    }
    facts.imports = imports.into_iter().collect();
    facts.packages = packages.into_iter().collect();
    facts.unresolved = unresolved.into_iter().collect();

    let mut lister = Lister {
        rel,
        lines: &lines,
        use_server: facts.use_server,
        route_module: facts.route_module,
        entries: Vec::new(),
        skipped: Vec::new(),
    };
    for statement in &program.body {
        lister.statement(statement);
    }
    facts.entries = lister.entries;
    facts.skipped = lister.skipped;
    facts.transaction_helpers = transaction_helpers(&program, rel, &lines, receivers);
    facts
}

fn module_specifiers(program: &Program<'_>) -> Vec<String> {
    let mut out = Vec::new();
    for statement in &program.body {
        let source = match statement {
            Statement::ImportDeclaration(i) => &i.source,
            Statement::ExportFromDeclaration(e) => &e.source,
            Statement::ExportAllDeclaration(e) => &e.source,
            _ => continue,
        };
        out.push(source.value.as_str().unwrap_or_default().to_owned());
    }
    out
}

struct Lister<'a> {
    rel: &'a str,
    lines: &'a LineIndex<'a>,
    use_server: bool,
    route_module: bool,
    entries: Vec<EntryFact>,
    skipped: Vec<String>,
}

fn callee_text(e: &Expression<'_>) -> Option<String> {
    match e.get_inner_expression() {
        Expression::Identifier(id) => Some(id.name.as_str().to_owned()),
        Expression::StaticMemberExpression(m) => {
            Some(format!("{}.{}", callee_text(&m.object)?, m.property.name))
        }
        _ => None,
    }
}

fn key_name(key: &PropertyKey<'_>) -> Option<String> {
    match key {
        PropertyKey::StaticIdentifier(id) => Some(id.name.as_str().to_owned()),
        PropertyKey::StringLiteral(s) => s.value.as_str().map(str::to_owned),
        _ => None,
    }
}

impl Lister<'_> {
    fn position(&self, span: Span) -> (u32, u32) {
        self.lines.position(span.start)
    }

    fn push(&mut self, path: String, kind: EntryKind, name: String, span: Span, is_async: bool) {
        let (line, column) = self.position(span);
        self.entries.push(EntryFact {
            selector: format!("{}#{path}", self.rel),
            kind,
            name,
            line,
            column,
            wrapper: None,
            is_async,
        });
    }

    fn function_kind(&self, name: &str) -> EntryKind {
        if self.route_module && ROUTE_METHODS.contains(&name) {
            EntryKind::RouteHandler
        } else if self.use_server {
            EntryKind::ServerAction
        } else {
            EntryKind::ExportedFunction
        }
    }

    fn statement(&mut self, statement: &Statement<'_>) {
        match statement {
            Statement::ExportDeclaration(e) => self.declaration(&e.declaration),
            Statement::ExportNamedDeclaration(e) => {
                for specifier in &e.specifiers {
                    self.skipped.push(format!(
                        "`export {{ {} }}` at line {} (export list; select the declaration by name)",
                        specifier.exported.name(),
                        self.position(e.span).0
                    ));
                }
            }
            Statement::ExportFromDeclaration(e) => {
                self.skipped
                    .push(format!("re-export at line {}", self.position(e.span).0));
            }
            Statement::ExportDefaultDeclaration(e) => {
                self.skipped.push(format!(
                    "`export default` at line {} (a selector needs a binding name)",
                    self.position(e.span).0
                ));
            }
            _ => {}
        }
    }

    fn declaration(&mut self, d: &Declaration<'_>) {
        match d {
            Declaration::FunctionDeclaration(f) => {
                if let Some(id) = &f.id
                    && f.body.is_some()
                {
                    let name = id.name.as_str().to_owned();
                    let kind = self.function_kind(&name);
                    self.push(name.clone(), kind, name, f.span, f.r#async);
                }
            }
            Declaration::ClassDeclaration(c) => {
                let Some(id) = &c.id else { return };
                let class = id.name.as_str().to_owned();
                for element in &c.body.body {
                    if let ClassElement::MethodDefinition(def) = element
                        && def.kind == MethodDefinitionKind::Method
                        && def.value.body.is_some()
                        && let Some(method) = key_name(&def.key)
                    {
                        let step = if def.r#static {
                            format!("static[{method}]")
                        } else {
                            format!("method[{method}]")
                        };
                        self.push(
                            format!("{class}/{step}"),
                            EntryKind::ClassMethod,
                            format!("{class}.{method}"),
                            def.value.span,
                            def.value.r#async,
                        );
                    }
                }
            }
            Declaration::VariableDeclaration(v) => {
                for declarator in &v.declarations {
                    let (BindingPattern::BindingIdentifier(id), Some(init)) =
                        (&declarator.id, &declarator.init)
                    else {
                        self.skipped.push(format!(
                            "destructured export at line {}",
                            self.position(declarator.span).0
                        ));
                        continue;
                    };
                    self.binding(id.name.as_str(), init);
                }
            }
            _ => {}
        }
    }

    /// The tRPC procedures of a router object passed as argument `index` of
    /// the call initializing `name`: properties whose value is a builder
    /// chain ending in `.mutation(fn)`, `.query(fn)` or `.subscription(fn)`
    /// with an inline resolver.
    fn procedures(
        &mut self,
        name: &str,
        index: usize,
        object: &oxc_ast::ast::ObjectExpression<'_>,
    ) {
        for property in &object.properties {
            let ObjectPropertyKind::ObjectProperty(p) = property else {
                continue;
            };
            if p.computed {
                continue;
            }
            let Some(key) = key_name(&p.key) else {
                continue;
            };
            let Expression::CallExpression(call) = p.value.get_inner_expression() else {
                continue;
            };
            let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression()
            else {
                continue;
            };
            let kind = member.property.name.as_str();
            if !is_trpc_resolver_callee(&call.callee) {
                continue;
            }
            let [argument] = call.arguments.as_slice() else {
                continue;
            };
            let (span, is_async) = match argument.as_expression().map(|e| e.get_inner_expression())
            {
                Some(Expression::ArrowFunctionExpression(a)) => (a.span, a.r#async),
                Some(Expression::FunctionExpression(f)) => (f.span, f.r#async),
                _ => continue,
            };
            self.push(
                format!("{name}/arg[{index}]/property[{key}]/arg[0]"),
                EntryKind::TrpcProcedure,
                format!("{name}.{key} ({kind})"),
                span,
                is_async,
            );
            if let Some(last) = self.entries.last_mut() {
                last.wrapper = callee_text(&call.callee);
            }
        }
    }

    fn binding(&mut self, name: &str, init: &Expression<'_>) {
        match init.get_inner_expression() {
            Expression::ArrowFunctionExpression(a) => {
                let kind = self.function_kind(name);
                self.push(name.to_owned(), kind, name.to_owned(), a.span, a.r#async);
            }
            Expression::FunctionExpression(f) => {
                let kind = self.function_kind(name);
                self.push(name.to_owned(), kind, name.to_owned(), f.span, f.r#async);
            }
            Expression::CallExpression(call) => {
                let wrapper = callee_text(&call.callee);
                for (index, argument) in call.arguments.iter().enumerate() {
                    let Some(e) = argument.as_expression() else {
                        continue;
                    };
                    if let Expression::ObjectExpression(object) = e.get_inner_expression() {
                        self.procedures(name, index, object);
                        continue;
                    }
                    let (span, is_async) = match e.get_inner_expression() {
                        Expression::ArrowFunctionExpression(a) => (a.span, a.r#async),
                        Expression::FunctionExpression(f) => (f.span, f.r#async),
                        _ => continue,
                    };
                    let kind = if self.use_server {
                        EntryKind::ServerAction
                    } else if is_trpc_resolver_callee(&call.callee) {
                        // `export const p = procedure.input(s).mutation(fn)`.
                        EntryKind::TrpcProcedure
                    } else {
                        EntryKind::WrappedCallback
                    };
                    let display = format!("{name}({})", wrapper.as_deref().unwrap_or("<call>"));
                    self.push(
                        format!("{name}/arg[{index}]"),
                        kind,
                        display,
                        span,
                        is_async,
                    );
                    if let Some(last) = self.entries.last_mut() {
                        last.wrapper.clone_from(&wrapper);
                    }
                }
            }
            Expression::ObjectExpression(object) => {
                for property in &object.properties {
                    let ObjectPropertyKind::ObjectProperty(p) = property else {
                        continue;
                    };
                    if p.computed {
                        continue;
                    }
                    let Some(key) = key_name(&p.key) else {
                        continue;
                    };
                    let (span, is_async) = match p.value.get_inner_expression() {
                        Expression::ArrowFunctionExpression(a) => (a.span, a.r#async),
                        Expression::FunctionExpression(f) => (f.span, f.r#async),
                        _ => continue,
                    };
                    self.push(
                        format!("{name}/property[{key}]"),
                        EntryKind::ObjectProperty,
                        format!("{name}.{key}"),
                        span,
                        is_async,
                    );
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Transaction helpers
// ---------------------------------------------------------------------------

fn param_names(params: &FormalParameters<'_>) -> Vec<Option<String>> {
    params
        .items
        .iter()
        .map(|item| match &item.pattern {
            BindingPattern::BindingIdentifier(id) => Some(id.name.as_str().to_owned()),
            _ => None,
        })
        .collect()
}

struct TxFinder<'r> {
    receivers: &'r [String],
    params: Vec<Option<String>>,
    callback: Option<usize>,
    options: Option<usize>,
    has_loop: bool,
    depth: u32,
}

impl TxFinder<'_> {
    fn param_index(&self, e: &Expression<'_>) -> Option<usize> {
        match e.get_inner_expression() {
            Expression::Identifier(id) => self
                .params
                .iter()
                .position(|p| p.as_deref() == Some(id.name.as_str())),
            _ => None,
        }
    }

    /// The parameter an options object is built from (`{ isolationLevel:
    /// options.isolationLevel }`) or the parameter itself.
    fn options_param(&self, e: &Expression<'_>) -> Option<usize> {
        if let Some(i) = self.param_index(e) {
            return Some(i);
        }
        let Expression::ObjectExpression(object) = e.get_inner_expression() else {
            return None;
        };
        object.properties.iter().find_map(|p| match p {
            ObjectPropertyKind::ObjectProperty(p) => match p.value.get_inner_expression() {
                Expression::StaticMemberExpression(m) => self.param_index(&m.object),
                _ => None,
            },
            ObjectPropertyKind::SpreadProperty(_) => None,
        })
    }
}

impl<'a> Visit<'a> for TxFinder<'_> {
    fn visit_call_expression(&mut self, it: &oxc_ast::ast::CallExpression<'a>) {
        if self.depth == 0
            && let Expression::StaticMemberExpression(m) = it.callee.get_inner_expression()
            && m.property.name.as_str() == "transaction"
            && let Expression::Identifier(receiver) = m.object.get_inner_expression()
            && self.receivers.iter().any(|r| r == receiver.name.as_str())
            && let Some(first) = it.arguments.first().and_then(Argument::as_expression)
            && let Some(index) = self.param_index(first)
        {
            self.callback = Some(index);
            self.options = it
                .arguments
                .get(1)
                .and_then(Argument::as_expression)
                .and_then(|e| self.options_param(e));
        }
        oxc_ast_visit::walk::walk_call_expression(self, it);
    }
    fn visit_for_statement(&mut self, it: &oxc_ast::ast::ForStatement<'a>) {
        self.has_loop = true;
        oxc_ast_visit::walk::walk_for_statement(self, it);
    }
    fn visit_while_statement(&mut self, it: &oxc_ast::ast::WhileStatement<'a>) {
        self.has_loop = true;
        oxc_ast_visit::walk::walk_while_statement(self, it);
    }
    fn visit_do_while_statement(&mut self, it: &oxc_ast::ast::DoWhileStatement<'a>) {
        self.has_loop = true;
        oxc_ast_visit::walk::walk_do_while_statement(self, it);
    }
    fn visit_for_of_statement(&mut self, it: &oxc_ast::ast::ForOfStatement<'a>) {
        self.has_loop = true;
        oxc_ast_visit::walk::walk_for_of_statement(self, it);
    }
    fn visit_function(
        &mut self,
        it: &oxc_ast::ast::Function<'a>,
        flags: oxc_syntax::scope::ScopeFlags,
    ) {
        self.depth += 1;
        oxc_ast_visit::walk::walk_function(self, it, flags);
        self.depth -= 1;
    }
    fn visit_arrow_function_expression(&mut self, it: &oxc_ast::ast::ArrowFunctionExpression<'a>) {
        self.depth += 1;
        oxc_ast_visit::walk::walk_arrow_function_expression(self, it);
        self.depth -= 1;
    }
}

fn transaction_helpers(
    program: &Program<'_>,
    rel: &str,
    lines: &LineIndex<'_>,
    receivers: &[String],
) -> Vec<TransactionHelper> {
    let mut out = Vec::new();
    let mut consider =
        |name: &str, params: &FormalParameters<'_>, body: &FunctionBody<'_>, span: Span| {
            let mut finder = TxFinder {
                receivers,
                params: param_names(params),
                callback: None,
                options: None,
                has_loop: false,
                depth: 0,
            };
            for statement in &body.statements {
                finder.visit_statement(statement);
            }
            if let Some(callback_argument) = finder.callback {
                out.push(TransactionHelper {
                    name: name.to_owned(),
                    file: rel.to_owned(),
                    line: lines.position(span.start).0,
                    callback_argument,
                    options_argument: finder.options,
                    has_loop: finder.has_loop,
                });
            }
        };
    for statement in &program.body {
        let declaration = match statement {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            other => other.as_declaration(),
        };
        match declaration {
            Some(Declaration::FunctionDeclaration(f)) => {
                if let (Some(id), Some(body)) = (&f.id, &f.body) {
                    consider(id.name.as_str(), &f.params, body, f.span);
                }
            }
            Some(Declaration::VariableDeclaration(v)) => {
                for d in &v.declarations {
                    let (BindingPattern::BindingIdentifier(id), Some(init)) = (&d.id, &d.init)
                    else {
                        continue;
                    };
                    match init.get_inner_expression() {
                        Expression::ArrowFunctionExpression(a) => {
                            if let ArrowFunctionBody::FunctionBody(body) = &a.body {
                                consider(id.name.as_str(), &a.params, body, a.span);
                            }
                        }
                        Expression::FunctionExpression(f) => {
                            if let Some(body) = &f.body {
                                consider(id.name.as_str(), &f.params, body, f.span);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// `<builder>.mutation` / `.query` / `.subscription` where the builder is a
/// tRPC procedure chain (`procedure.input(s)`, `t.procedure.use(mw)`, or a
/// binding named like `protectedProcedure`).
pub(crate) fn is_trpc_resolver_callee(callee: &Expression<'_>) -> bool {
    let Expression::StaticMemberExpression(member) = callee.get_inner_expression() else {
        return false;
    };
    if !matches!(
        member.property.name.as_str(),
        "mutation" | "query" | "subscription"
    ) {
        return false;
    }
    let mut cur = member.object.get_inner_expression();
    loop {
        match cur {
            Expression::CallExpression(call) => match call.callee.get_inner_expression() {
                Expression::StaticMemberExpression(m) => {
                    if matches!(
                        m.property.name.as_str(),
                        "input" | "use" | "output" | "meta"
                    ) {
                        return true;
                    }
                    cur = m.object.get_inner_expression();
                }
                _ => return false,
            },
            Expression::StaticMemberExpression(m) => {
                return m.property.name == "procedure";
            }
            Expression::Identifier(id) => {
                return id.name.as_str().to_ascii_lowercase().ends_with("procedure");
            }
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(files: &[(&str, &str)]) -> Vec<ModuleFacts> {
        // One directory per call: tests run in parallel.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "midwrite-entries-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for (name, text) in files {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let out = index_project(&dir, &["db".to_owned()]).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    #[test]
    fn entries_are_listed_with_selectors_and_kinds() {
        let modules = index(&[
            (
                "src/app/actions.ts",
                "\"use server\";\nimport { wrap } from \"../lib/wrap\";\nexport const invite = wrap(schema, async (data, user) => {});\nexport async function remove(id: number) {}\nfunction helper() {}\n",
            ),
            (
                "src/lib/wrap.ts",
                "export function wrap(s: unknown, f: unknown) { return f; }\n",
            ),
            (
                "src/app/api/orders/route.ts",
                "export async function POST(request: Request) {}\nexport const handlers = { create: async () => {}, name: 1 };\nexport class Service { async place() {} static make() {} }\nexport default function page() {}\n",
            ),
        ]);
        let actions = modules
            .iter()
            .find(|m| m.file == "src/app/actions.ts")
            .unwrap();
        assert!(actions.use_server);
        assert_eq!(actions.imports, vec!["src/lib/wrap.ts".to_owned()]);
        let selectors: Vec<_> = actions
            .entries
            .iter()
            .map(|e| (e.selector.as_str(), e.kind))
            .collect();
        assert_eq!(
            selectors,
            vec![
                ("src/app/actions.ts#invite/arg[1]", EntryKind::ServerAction),
                ("src/app/actions.ts#remove", EntryKind::ServerAction),
            ]
        );
        assert_eq!(actions.entries[0].wrapper.as_deref(), Some("wrap"));
        let route = modules
            .iter()
            .find(|m| m.file == "src/app/api/orders/route.ts")
            .unwrap();
        let kinds: Vec<_> = route
            .entries
            .iter()
            .map(|e| (e.selector.rsplit('#').next().unwrap().to_owned(), e.kind))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("POST".to_owned(), EntryKind::RouteHandler),
                (
                    "handlers/property[create]".to_owned(),
                    EntryKind::ObjectProperty
                ),
                ("Service/method[place]".to_owned(), EntryKind::ClassMethod),
                ("Service/static[make]".to_owned(), EntryKind::ClassMethod),
            ]
        );
        assert_eq!(route.skipped.len(), 1, "{:?}", route.skipped);
    }

    #[test]
    fn trpc_procedures_are_listed_with_their_resolvers() {
        let modules = index(&[(
            "src/routers/orders.ts",
            "import { createTRPCRouter, protectedProcedure, publicProcedure, t } from \"../trpc\";\n\
             import { z } from \"zod\";\n\
             import { other } from \"./other\";\n\
             const named = async () => {};\n\
             export const ordersRouter = createTRPCRouter({\n\
               create: protectedProcedure.input(z.object({ id: z.string() })).mutation(async ({ ctx, input }) => {}),\n\
               list: publicProcedure.query(async ({ ctx }) => []),\n\
               raw: t.procedure.query(() => 1),\n\
               named: protectedProcedure.mutation(named),\n\
               nested: other,\n\
               notTrpc: cache.query(async () => {}),\n\
             });\n\
             export const single = protectedProcedure.input(z.string()).mutation(async ({ input }) => {});\n",
        )]);
        let entries: Vec<_> = modules[0]
            .entries
            .iter()
            .map(|e| (e.selector.rsplit('#').next().unwrap().to_owned(), e.kind))
            .collect();
        assert_eq!(
            entries,
            vec![
                (
                    "ordersRouter/arg[0]/property[create]/arg[0]".to_owned(),
                    EntryKind::TrpcProcedure
                ),
                (
                    "ordersRouter/arg[0]/property[list]/arg[0]".to_owned(),
                    EntryKind::TrpcProcedure
                ),
                (
                    "ordersRouter/arg[0]/property[raw]/arg[0]".to_owned(),
                    EntryKind::TrpcProcedure
                ),
                ("single/arg[0]".to_owned(), EntryKind::TrpcProcedure),
            ]
        );
        assert_eq!(modules[0].entries[0].name, "ordersRouter.create (mutation)");
    }

    #[test]
    fn transaction_helpers_are_recognized_with_their_loops() {
        let modules = index(&[(
            "src/tx.ts",
            "export async function withTx(work, options) { return db.transaction(work, { isolationLevel: options.level }); }\nexport async function withRetry(work) { for (;;) { try { return await db.transaction(work); } catch {} } }\nexport async function other(work) { return pool.transaction(work); }\n",
        )]);
        let helpers = &modules[0].transaction_helpers;
        assert_eq!(helpers.len(), 2);
        assert_eq!(helpers[0].name, "withTx");
        assert_eq!(helpers[0].options_argument, Some(1));
        assert!(!helpers[0].has_loop);
        assert_eq!(helpers[1].name, "withRetry");
        assert!(helpers[1].has_loop);
    }
}

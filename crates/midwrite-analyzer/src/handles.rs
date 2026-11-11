//! Database handle provenance.
//!
//! Which database a handle denotes decides where its statements run. A
//! drizzle-orm `withReplicas(primary, [replica, ...])` database sends reads
//! (`select`, `selectDistinct`, `selectDistinctOn`, `$count`, `with`,
//! `$with`, the `query` getter) to a replica and everything else that it
//! defines (`insert`, `update`, `delete`, `execute`, `transaction`,
//! `refreshMaterializedView`) to the primary (drizzle-orm 0.45.4,
//! `pg-core/db.js`; `docs/architecture/source-extraction.md`). Binding such a
//! handle as the primary would invent read-your-writes behavior.
//!
//! This module answers, without executing anything and without type
//! checking, what a *value* (a module-level `const`, a call of a project
//! function, `await`), a *type* (`Awaited<ReturnType<typeof connectDb>>`, an
//! imported alias, `typeof db`, `PgWithReplicas<...>`) or a *function result*
//! is:
//!
//! * [`HandleFact::Primary`]: a root drizzle database (`drizzle(...)` of a
//!   `drizzle-orm/<driver>` module, a declared `...Database` type of
//!   drizzle-orm, `$primary` of a replicated database);
//! * [`HandleFact::Replicated`]: the result of `withReplicas` imported from
//!   `drizzle-orm/pg-core` (built-in, verified routing) or of a project
//!   function DECLARED as a replica factory
//!   ([`ReplicaFactoryBinding`]; its body is not interpreted), whose primary
//!   argument is itself a recognized primary;
//! * [`HandleFact::Unknown`]: anything else, with the reason. An
//!   unrecognized wrapper is never treated as replicated or as primary.
//!
//! Function bodies are read only to find a single returned expression
//! (`connectDb = async () => { return db; }`); parameters are unknown (no
//! substitution), so a wrapper that returns its argument stays unknown.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ArrowFunctionBody, BindingPattern, Declaration, ExportDefaultDeclarationKind, Expression,
    FormalParameters, FunctionBody, ImportDeclarationSpecifier, Program, Statement, TSType,
    TSTypeName, TSTypeQueryExprName,
};
use oxc_ast_visit::Visit;
use oxc_parser::Parser;
use oxc_span::SourceType;
use oxc_syntax::scope::ScopeFlags;

use midwrite_core::location::LineIndex;
use midwrite_model::provider::ReplicaFactoryBinding;

use crate::context::{ProjectContext, Resolved};

/// The pinned drizzle-orm version whose `withReplicas` routing was read.
pub const DRIZZLE_ROUTING_VERSION: &str = "0.45.4";

/// Methods a drizzle-orm `withReplicas` database sends to a replica.
pub const REPLICA_METHODS: [&str; 6] = [
    "select",
    "selectDistinct",
    "selectDistinctOn",
    "$count",
    "with",
    "$with",
];

/// Methods a drizzle-orm `withReplicas` database sends to the primary.
pub const PRIMARY_METHODS: [&str; 6] = [
    "insert",
    "update",
    "delete",
    "execute",
    "transaction",
    "refreshMaterializedView",
];

/// How a replicated database routes, and how that is known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Routing {
    /// Where the replicated database comes from (`withReplicas` of
    /// drizzle-orm/pg-core at `file:line`, a declared factory, a type).
    pub origin: String,
    /// The declared factory (`module#export`), or `None` for drizzle-orm's
    /// own `withReplicas`.
    pub declared: Option<String>,
    /// Methods returning a database that sends everything to the primary
    /// (declared: `usePrimaryOnly`).
    pub primary_only_methods: Vec<String>,
    /// Properties holding the primary (`$primary`).
    pub primary_properties: Vec<String>,
    /// Reads may go to the primary or to a replica: a parameter typed as a
    /// plain drizzle database in a project that builds a replicated one (a
    /// `withReplicas` database is assignable to its primary's type).
    pub ambiguous: bool,
}

impl Routing {
    fn builtin(origin: String) -> Self {
        Self {
            origin,
            declared: None,
            primary_only_methods: Vec::new(),
            primary_properties: vec!["$primary".to_owned()],
            ambiguous: false,
        }
    }

    /// A handle that may be the primary or a replicated database.
    pub fn ambiguous(origin: String) -> Self {
        Self {
            origin,
            declared: None,
            primary_only_methods: Vec::new(),
            primary_properties: Vec::new(),
            ambiguous: true,
        }
    }

    /// The routing contract, as printed with results.
    pub fn contract(&self) -> String {
        match &self.declared {
            None => format!(
                "drizzle-orm `withReplicas` routing (verified in drizzle-orm {DRIZZLE_ROUTING_VERSION} pg-core/db.js): {} and the relational `query` read from a replica; {} use the primary; `$primary` is the primary",
                REPLICA_METHODS.join("/"),
                PRIMARY_METHODS.join("/")
            ),
            Some(f) => format!(
                "declared replica factory `{f}` (an assumption; its body is not interpreted): drizzle-orm `withReplicas` routing — {} and the relational `query` read from a replica; {} use the primary{}{}",
                REPLICA_METHODS.join("/"),
                PRIMARY_METHODS.join("/"),
                if self.primary_only_methods.is_empty() {
                    String::new()
                } else {
                    format!(
                        "; `{}()` returns a database using only the primary",
                        self.primary_only_methods.join("()`/`")
                    )
                },
                if self.primary_properties.is_empty() {
                    String::new()
                } else {
                    format!("; `{}` is the primary", self.primary_properties.join("`/`"))
                }
            ),
        }
    }
}

/// What a value, type or function result is, as a database handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandleFact {
    /// A root drizzle database (statements run on the primary).
    Primary {
        origin: String,
        /// Known only from a declared type (a `withReplicas` database is
        /// assignable to it).
        by_type: bool,
    },
    Replicated(Routing),
    Unknown(String),
}

impl HandleFact {
    fn unknown(why: impl Into<String>) -> Self {
        Self::Unknown(why.into())
    }
}

/// A declared replica factory, resolved to its declaration.
#[derive(Clone, Debug)]
struct Factory {
    file: usize,
    local: String,
    binding: ReplicaFactoryBinding,
}

const MAX_DEPTH: u8 = 48;

/// Provenance queries for one extraction (the project context and the
/// replica factories its request declares).
pub struct Provenance {
    ctx: Arc<ProjectContext>,
    factories: Vec<Factory>,
    key: String,
    /// Declared factories that could not be resolved.
    pub errors: Vec<String>,
}

fn parse<'a>(allocator: &'a Allocator, abs: &Path, text: &'a str) -> Option<Program<'a>> {
    let source_type = SourceType::from_path(abs).unwrap_or_else(|_| SourceType::ts());
    let parsed = Parser::new(allocator, text, source_type).parse();
    (!parsed.fatal_error).then_some(parsed.program)
}

/// Every import of a module including `import type`: local → (specifier,
/// imported; `default`, `*`).
fn all_imports(program: &Program<'_>) -> HashMap<String, (String, String)> {
    let mut out = HashMap::new();
    for statement in &program.body {
        let Statement::ImportDeclaration(import) = statement else {
            continue;
        };
        let from = import.source.value.as_str().unwrap_or_default().to_owned();
        for specifier in import.specifiers.iter().flatten() {
            let (local, imported) = match specifier {
                ImportDeclarationSpecifier::ImportSpecifier(x) => (
                    x.local.name.as_str().to_owned(),
                    x.imported.name().as_str().to_owned(),
                ),
                ImportDeclarationSpecifier::ImportDefaultSpecifier(x) => {
                    (x.local.name.as_str().to_owned(), "default".to_owned())
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(x) => {
                    (x.local.name.as_str().to_owned(), "*".to_owned())
                }
            };
            out.insert(local, (from.clone(), imported));
        }
    }
    out
}

fn is_drizzle(specifier: &str) -> bool {
    specifier == "drizzle-orm" || specifier.starts_with("drizzle-orm/")
}

/// A top-level declaration of a module, by name.
enum TopLevel<'p, 'a> {
    Const(&'p Expression<'a>),
    Mutable,
    Function(FnRef<'p, 'a>),
    TypeAlias(&'p oxc_ast::ast::TSTypeAliasDeclaration<'a>),
    Other(&'static str),
}

#[derive(Clone, Copy)]
struct FnRef<'p, 'a> {
    params: &'p FormalParameters<'a>,
    body: FnBodyRef<'p, 'a>,
    return_type: Option<&'p TSType<'a>>,
}

#[derive(Clone, Copy)]
enum FnBodyRef<'p, 'a> {
    Block(&'p FunctionBody<'a>),
    Expr(&'p Expression<'a>),
}

fn fn_of_expression<'p, 'a>(e: &'p Expression<'a>) -> Option<FnRef<'p, 'a>> {
    match e.get_inner_expression() {
        Expression::ArrowFunctionExpression(a) => Some(FnRef {
            params: &a.params,
            body: match &a.body {
                ArrowFunctionBody::FunctionBody(b) => FnBodyRef::Block(b),
                other => FnBodyRef::Expr(other.as_expression()?),
            },
            return_type: a.return_type.as_ref().map(|t| &t.type_annotation),
        }),
        Expression::FunctionExpression(f) => Some(FnRef {
            params: &f.params,
            body: FnBodyRef::Block(f.body.as_deref()?),
            return_type: f.return_type.as_ref().map(|t| &t.type_annotation),
        }),
        _ => None,
    }
}

fn top_level<'p, 'a>(program: &'p Program<'a>, name: &str) -> Option<TopLevel<'p, 'a>> {
    for statement in &program.body {
        let declaration = match statement {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            Statement::ExportDefaultDeclaration(e) if name == "default" => {
                return match &e.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                        Some(TopLevel::Function(FnRef {
                            params: &f.params,
                            body: FnBodyRef::Block(f.body.as_deref()?),
                            return_type: f.return_type.as_ref().map(|t| &t.type_annotation),
                        }))
                    }
                    other => other.as_expression().map(TopLevel::Const),
                };
            }
            other => other.as_declaration(),
        };
        match declaration {
            Some(Declaration::VariableDeclaration(v)) => {
                for d in &v.declarations {
                    let BindingPattern::BindingIdentifier(id) = &d.id else {
                        continue;
                    };
                    if id.name.as_str() != name {
                        continue;
                    }
                    if !v.kind.is_const() {
                        return Some(TopLevel::Mutable);
                    }
                    let init = d.init.as_ref()?;
                    return Some(match fn_of_expression(init) {
                        Some(f) => TopLevel::Function(f),
                        None => TopLevel::Const(init),
                    });
                }
            }
            Some(Declaration::FunctionDeclaration(f))
                if f.id.as_ref().is_some_and(|id| id.name.as_str() == name) =>
            {
                return Some(TopLevel::Function(FnRef {
                    params: &f.params,
                    body: FnBodyRef::Block(f.body.as_deref()?),
                    return_type: f.return_type.as_ref().map(|t| &t.type_annotation),
                }));
            }
            Some(Declaration::TSTypeAliasDeclaration(t)) if t.id.name.as_str() == name => {
                return Some(TopLevel::TypeAlias(t));
            }
            Some(Declaration::ClassDeclaration(c))
                if c.id.as_ref().is_some_and(|id| id.name.as_str() == name) =>
            {
                return Some(TopLevel::Other("a class"));
            }
            Some(Declaration::TSInterfaceDeclaration(i)) if i.id.name.as_str() == name => {
                return Some(TopLevel::Other("an interface"));
            }
            _ => {}
        }
    }
    None
}

/// The expression a function body returns, when the body is exactly one
/// `return <expr>` statement.
fn single_return<'p, 'a>(body: &'p FunctionBody<'a>) -> Result<&'p Expression<'a>, String> {
    struct Returns(usize);
    impl<'a> Visit<'a> for Returns {
        fn visit_return_statement(&mut self, _: &oxc_ast::ast::ReturnStatement<'a>) {
            self.0 += 1;
        }
        fn visit_function(&mut self, _: &oxc_ast::ast::Function<'a>, _: ScopeFlags) {}
        fn visit_arrow_function_expression(
            &mut self,
            _: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
        }
    }
    let mut counter = Returns(0);
    counter.visit_function_body(body);
    let Some(Statement::ReturnStatement(last)) = body.statements.last() else {
        return Err("the function does not end with a `return`".to_owned());
    };
    if counter.0 != 1 {
        return Err("the function has more than one `return`".to_owned());
    }
    // Anything before the `return` could have effects the caller would skip
    // if the call were replaced by its result.
    if body.statements.len() != 1 {
        return Err("the function does more than return a value".to_owned());
    }
    last.argument
        .as_ref()
        .ok_or_else(|| "the function returns nothing".to_owned())
}

/// Names a function body binds: its parameters (unknown values) and its
/// top-level `const` declarations.
struct Scope<'p, 'a> {
    locals: HashMap<String, Option<&'p Expression<'a>>>,
}

impl<'p, 'a> Scope<'p, 'a> {
    fn empty() -> Self {
        Self {
            locals: HashMap::new(),
        }
    }

    fn of(f: &FnRef<'p, 'a>) -> Self {
        let mut locals = HashMap::new();
        for item in &f.params.items {
            let mut names = Vec::new();
            binding_names(&item.pattern, &mut names);
            for n in names {
                locals.insert(n, None);
            }
        }
        if let Some(rest) = &f.params.rest {
            let mut names = Vec::new();
            binding_names(&rest.rest.argument, &mut names);
            for n in names {
                locals.insert(n, None);
            }
        }
        if let FnBodyRef::Block(body) = f.body {
            for statement in &body.statements {
                if let Statement::VariableDeclaration(v) = statement {
                    for d in &v.declarations {
                        let mut names = Vec::new();
                        binding_names(&d.id, &mut names);
                        let simple = matches!(&d.id, BindingPattern::BindingIdentifier(_));
                        for n in names {
                            let init = if v.kind.is_const() && simple {
                                d.init.as_ref()
                            } else {
                                None
                            };
                            locals.insert(n, init);
                        }
                    }
                }
            }
        }
        Self { locals }
    }
}

fn binding_names(pattern: &BindingPattern<'_>, out: &mut Vec<String>) {
    match pattern {
        BindingPattern::BindingIdentifier(id) => out.push(id.name.as_str().to_owned()),
        BindingPattern::ObjectPattern(o) => {
            for p in &o.properties {
                binding_names(&p.value, out);
            }
            if let Some(rest) = &o.rest {
                binding_names(&rest.argument, out);
            }
        }
        BindingPattern::ArrayPattern(a) => {
            for e in a.elements.iter().flatten() {
                binding_names(e, out);
            }
            if let Some(rest) = &a.rest {
                binding_names(&rest.argument, out);
            }
        }
        BindingPattern::AssignmentPattern(a) => binding_names(&a.left, out),
    }
}

impl Provenance {
    pub fn new(ctx: Arc<ProjectContext>, factories: &[ReplicaFactoryBinding]) -> Self {
        let mut resolved = Vec::new();
        let mut errors = Vec::new();
        for f in factories {
            let Some((module, export)) = f.factory.rsplit_once('#') else {
                errors.push(format!(
                    "replica factory `{}`: expected `module#export`",
                    f.factory
                ));
                continue;
            };
            let abs = crate::resolve::absolute(&ctx.root.join(module));
            let Some(file) = ctx.index_of(&abs) else {
                errors.push(format!(
                    "replica factory `{}`: `{module}` is not an analyzed module",
                    f.factory
                ));
                continue;
            };
            match ctx.resolve_export(file, export) {
                Ok(Resolved::Binding { file, local }) => resolved.push(Factory {
                    file,
                    local,
                    binding: f.clone(),
                }),
                Ok(Resolved::Namespace { .. }) => errors.push(format!(
                    "replica factory `{}`: `{export}` is a namespace",
                    f.factory
                )),
                Err(why) => errors.push(format!("replica factory `{}`: {why}", f.factory)),
            }
        }
        let key = format!(
            "{:016x}",
            midwrite_core::diagnostic::stable_hash(&[&format!("{factories:?}")])
        );
        Self {
            ctx,
            factories: resolved,
            key,
            errors,
        }
    }

    fn memo(&self, key: String, compute: impl FnOnce() -> HandleFact) -> HandleFact {
        let key = format!("{}:{key}", self.key);
        if let Some(v) = self.ctx.handle_memo(&key) {
            return v;
        }
        let v = compute();
        self.ctx.set_handle_memo(key, v.clone());
        v
    }

    fn rel(&self, file: usize) -> String {
        self.ctx.rel(&self.ctx.files[file])
    }

    /// What the module-level name `local` of `file` (declared there or
    /// imported) denotes.
    pub fn value(&self, file: usize, local: &str) -> HandleFact {
        self.value_at(file, local, 0)
    }

    fn value_at(&self, file: usize, local: &str, depth: u8) -> HandleFact {
        if depth > MAX_DEPTH {
            return HandleFact::unknown("handle provenance is nested too deeply");
        }
        self.memo(format!("value:{file}:{local}"), || {
            let (file, local) = match self.ctx.resolve_local(file, local) {
                Ok(Resolved::Binding { file, local }) => (file, local),
                Ok(Resolved::Namespace { .. }) => {
                    return HandleFact::unknown(format!("`{local}` is a namespace"));
                }
                Err(why) => return HandleFact::Unknown(why),
            };
            let abs = self.ctx.files[file].clone();
            let Ok(text) = self.ctx.source(&abs) else {
                return HandleFact::unknown(format!("cannot read `{}`", self.rel(file)));
            };
            let allocator = Allocator::default();
            let Some(program) = parse(&allocator, &abs, &text) else {
                return HandleFact::unknown(format!("`{}` does not parse", self.rel(file)));
            };
            let lines = LineIndex::new(&text);
            match top_level(&program, &local) {
                Some(TopLevel::Const(init)) => {
                    self.expr(file, &program, &lines, init, &Scope::empty(), depth + 1)
                }
                Some(TopLevel::Mutable) => HandleFact::unknown(format!(
                    "`{local}` in `{}` is not a `const` (it may be reassigned)",
                    self.rel(file)
                )),
                Some(TopLevel::Function(_)) => HandleFact::unknown(format!(
                    "`{local}` in `{}` is a function, not a database",
                    self.rel(file)
                )),
                Some(TopLevel::TypeAlias(_)) | Some(TopLevel::Other(_)) | None => {
                    HandleFact::unknown(format!(
                        "`{local}` in `{}` is not a module-level `const`",
                        self.rel(file)
                    ))
                }
            }
        })
    }

    /// What calling the module-level function `local` of `file` returns
    /// (awaited): its declared return type, else its single returned
    /// expression.
    pub fn call_result(&self, file: usize, local: &str) -> HandleFact {
        self.call_result_at(file, local, 0)
    }

    fn call_result_at(&self, file: usize, local: &str, depth: u8) -> HandleFact {
        if depth > MAX_DEPTH {
            return HandleFact::unknown("handle provenance is nested too deeply");
        }
        self.memo(format!("call:{file}:{local}"), || {
            let (file, local) = match self.ctx.resolve_local(file, local) {
                Ok(Resolved::Binding { file, local }) => (file, local),
                Ok(Resolved::Namespace { .. }) => {
                    return HandleFact::unknown(format!("`{local}` is a namespace"));
                }
                Err(why) => return HandleFact::Unknown(why),
            };
            if let Some(f) = self.factory(file, &local) {
                return HandleFact::unknown(format!(
                    "`{}` is a replica factory: its result depends on its arguments",
                    f.binding.factory
                ));
            }
            let abs = self.ctx.files[file].clone();
            let Ok(text) = self.ctx.source(&abs) else {
                return HandleFact::unknown(format!("cannot read `{}`", self.rel(file)));
            };
            let allocator = Allocator::default();
            let Some(program) = parse(&allocator, &abs, &text) else {
                return HandleFact::unknown(format!("`{}` does not parse", self.rel(file)));
            };
            let lines = LineIndex::new(&text);
            let Some(TopLevel::Function(f)) = top_level(&program, &local) else {
                return HandleFact::unknown(format!(
                    "`{local}` in `{}` is not a function declared at module level",
                    self.rel(file)
                ));
            };
            self.function_result(file, &program, &lines, &f, &local, depth + 1)
        })
    }

    fn function_result(
        &self,
        file: usize,
        program: &Program<'_>,
        lines: &LineIndex<'_>,
        f: &FnRef<'_, '_>,
        name: &str,
        depth: u8,
    ) -> HandleFact {
        if let Some(t) = f.return_type {
            let declared = self.ts_type(file, program, t, depth + 1);
            if !matches!(declared, HandleFact::Unknown(_)) {
                return declared;
            }
        }
        let scope = Scope::of(f);
        let returned = match f.body {
            FnBodyRef::Expr(e) => e,
            FnBodyRef::Block(body) => match single_return(body) {
                Ok(e) => e,
                Err(why) => {
                    return HandleFact::unknown(format!("`{name}` in `{}`: {why}", self.rel(file)));
                }
            },
        };
        self.expr(file, program, lines, returned, &scope, depth + 1)
    }

    fn factory(&self, file: usize, local: &str) -> Option<&Factory> {
        self.factories
            .iter()
            .find(|f| f.file == file && f.local == local)
    }

    /// What an expression written in `file` evaluates to.
    fn expr(
        &self,
        file: usize,
        program: &Program<'_>,
        lines: &LineIndex<'_>,
        e: &Expression<'_>,
        scope: &Scope<'_, '_>,
        depth: u8,
    ) -> HandleFact {
        if depth > MAX_DEPTH {
            return HandleFact::unknown("handle provenance is nested too deeply");
        }
        match e.get_inner_expression() {
            Expression::AwaitExpression(a) => {
                self.expr(file, program, lines, &a.argument, scope, depth + 1)
            }
            Expression::Identifier(id) => {
                let name = id.name.as_str();
                match scope.locals.get(name) {
                    Some(Some(init)) => {
                        // A function-local `const`: resolved in the same
                        // scope (a self-reference is cut by the depth bound).
                        self.expr(file, program, lines, init, scope, depth + 1)
                    }
                    Some(None) => HandleFact::unknown(format!(
                        "`{name}` is a parameter or a non-`const` local (arguments are not substituted)"
                    )),
                    None => self.value_at(file, name, depth + 1),
                }
            }
            Expression::StaticMemberExpression(m) => {
                let property = m.property.name.as_str();
                match self.expr(file, program, lines, &m.object, scope, depth + 1) {
                    HandleFact::Replicated(r)
                        if r.primary_properties.iter().any(|p| p == property) =>
                    {
                        HandleFact::Primary {
                            origin: format!(
                                "`.{property}` of a replicated database ({})",
                                r.origin
                            ),
                            by_type: false,
                        }
                    }
                    HandleFact::Unknown(why) => HandleFact::Unknown(why),
                    _ => HandleFact::unknown(format!(
                        "`.{property}` of a database is not a recognized database"
                    )),
                }
            }
            Expression::ConditionalExpression(c) => {
                let a = self.expr(file, program, lines, &c.consequent, scope, depth + 1);
                let b = self.expr(file, program, lines, &c.alternate, scope, depth + 1);
                match (a, b) {
                    (
                        HandleFact::Primary { origin, by_type: x },
                        HandleFact::Primary { by_type: y, .. },
                    ) => HandleFact::Primary {
                        origin: format!("both branches of a conditional ({origin})"),
                        by_type: x || y,
                    },
                    (HandleFact::Replicated(r), HandleFact::Replicated(s)) if r == s => {
                        HandleFact::Replicated(r)
                    }
                    _ => HandleFact::unknown(
                        "the branches of a conditional are not the same kind of database",
                    ),
                }
            }
            Expression::CallExpression(call) => {
                // `x.usePrimaryOnly()` of a declared replicated database.
                if let Expression::StaticMemberExpression(m) = call.callee.get_inner_expression() {
                    let method = m.property.name.as_str();
                    return match self.expr(file, program, lines, &m.object, scope, depth + 1) {
                        HandleFact::Replicated(r)
                            if call.arguments.is_empty()
                                && r.primary_only_methods.iter().any(|p| p == method) =>
                        {
                            HandleFact::Primary {
                                origin: format!(
                                    "`.{method}()` of a replicated database ({})",
                                    r.origin
                                ),
                                by_type: false,
                            }
                        }
                        _ => HandleFact::unknown(format!(
                            "the result of the method call `.{method}(...)` is not a recognized database"
                        )),
                    };
                }
                let Expression::Identifier(callee) = call.callee.get_inner_expression() else {
                    return HandleFact::unknown("the callee is not a plain function reference");
                };
                let name = callee.name.as_str();
                if scope.locals.contains_key(name) {
                    return HandleFact::unknown(format!(
                        "`{name}` is a local of the function (not resolved)"
                    ));
                }
                let at = {
                    let (line, _) = lines.position(call.span.start);
                    format!("{}:{line}", self.rel(file))
                };
                // A package import: drizzle-orm's own constructors.
                let imports = all_imports(program);
                if let Some((specifier, imported)) = imports.get(name)
                    && is_drizzle(specifier)
                {
                    if imported == "drizzle" && specifier.starts_with("drizzle-orm/") {
                        return HandleFact::Primary {
                            origin: format!("`drizzle(...)` of `{specifier}` at {at}"),
                            by_type: false,
                        };
                    }
                    if imported == "withReplicas" && specifier == "drizzle-orm/pg-core" {
                        let primary = call
                            .arguments
                            .first()
                            .and_then(|a| a.as_expression())
                            .map(|a| self.expr(file, program, lines, a, scope, depth + 1));
                        return match primary {
                            Some(HandleFact::Primary { by_type: false, .. }) => {
                                HandleFact::Replicated(Routing::builtin(format!(
                                    "`withReplicas` of drizzle-orm/pg-core at {at}"
                                )))
                            }
                            Some(HandleFact::Primary { by_type: true, .. }) => {
                                HandleFact::Replicated(Routing::builtin(format!(
                                    "`withReplicas` of drizzle-orm/pg-core at {at} (primary known by its declared type)"
                                )))
                            }
                            _ => HandleFact::unknown(format!(
                                "the primary argument of `withReplicas` at {at} is not a recognized drizzle database"
                            )),
                        };
                    }
                    return HandleFact::unknown(format!(
                        "`{imported}` of `{specifier}` is not a recognized database constructor"
                    ));
                }
                // A project function: a declared replica factory, or a
                // function whose result is known.
                let resolved = match self.ctx.resolve_local(file, name) {
                    Ok(Resolved::Binding { file, local }) => (file, local),
                    Ok(Resolved::Namespace { .. }) => {
                        return HandleFact::unknown(format!("`{name}` is a namespace"));
                    }
                    Err(why) => return HandleFact::Unknown(why),
                };
                if let Some(f) = self.factory(resolved.0, &resolved.1) {
                    let index = f.binding.primary_argument;
                    let primary = call
                        .arguments
                        .get(index)
                        .and_then(|a| a.as_expression())
                        .map(|a| self.expr(file, program, lines, a, scope, depth + 1));
                    return match primary {
                        Some(HandleFact::Primary { .. }) => HandleFact::Replicated(Routing {
                            origin: format!(
                                "the declared replica factory `{}` at {at}",
                                f.binding.factory
                            ),
                            declared: Some(f.binding.factory.clone()),
                            primary_only_methods: f.binding.primary_only_methods.clone(),
                            primary_properties: f.binding.primary_properties.clone(),
                            ambiguous: false,
                        }),
                        Some(HandleFact::Unknown(why)) => HandleFact::unknown(format!(
                            "argument {index} of the declared replica factory `{}` at {at} is not a recognized drizzle database: {why}",
                            f.binding.factory
                        )),
                        _ => HandleFact::unknown(format!(
                            "argument {index} of the declared replica factory `{}` at {at} is not a recognized drizzle database",
                            f.binding.factory
                        )),
                    };
                }
                self.call_result_at(resolved.0, &resolved.1, depth + 1)
            }
            other => HandleFact::unknown(format!(
                "an expression of kind `{}` is not a recognized database",
                crate::concurrency_kind(other)
            )),
        }
    }

    /// What a type annotation written in `file` denotes.
    fn ts_type(&self, file: usize, program: &Program<'_>, t: &TSType<'_>, depth: u8) -> HandleFact {
        if depth > MAX_DEPTH {
            return HandleFact::unknown("handle provenance is nested too deeply");
        }
        match t {
            TSType::TSParenthesizedType(p) => {
                self.ts_type(file, program, &p.type_annotation, depth + 1)
            }
            TSType::TSTypeQuery(q) => match &q.expr_name {
                TSTypeQueryExprName::IdentifierReference(id) => {
                    self.value_at(file, id.name.as_str(), depth + 1)
                }
                _ => HandleFact::unknown("`typeof` of a qualified name or import type"),
            },
            TSType::TSUnionType(_) => {
                HandleFact::unknown("a union type (the connection kind depends on the caller)")
            }
            TSType::TSIntersectionType(i) => {
                // `Database & { extra?: ... }`: the one member that is not a
                // type literal decides.
                let mut found = None;
                for member in &i.types {
                    if matches!(member, TSType::TSTypeLiteral(_)) {
                        continue;
                    }
                    if found.is_some() {
                        return HandleFact::unknown("an intersection of more than one named type");
                    }
                    found = Some(member);
                }
                match found {
                    Some(member) => self.ts_type(file, program, member, depth + 1),
                    None => HandleFact::unknown("an intersection of type literals"),
                }
            }
            TSType::TSTypeReference(r) => {
                let TSTypeName::IdentifierReference(id) = &r.type_name else {
                    return HandleFact::unknown("a qualified type name");
                };
                let name = id.name.as_str();
                let first_arg = r.type_arguments.as_ref().and_then(|a| a.params.first());
                match name {
                    "Awaited" | "Promise" | "NonNullable" | "Readonly" => {
                        return match first_arg {
                            Some(inner) => self.ts_type(file, program, inner, depth + 1),
                            None => {
                                HandleFact::unknown(format!("`{name}` without a type argument"))
                            }
                        };
                    }
                    "ReturnType" => {
                        return match first_arg {
                            Some(TSType::TSTypeQuery(q)) => match &q.expr_name {
                                TSTypeQueryExprName::IdentifierReference(f) => {
                                    self.call_result_at(file, f.name.as_str(), depth + 1)
                                }
                                _ => HandleFact::unknown("`ReturnType` of a qualified name"),
                            },
                            _ => HandleFact::unknown(
                                "`ReturnType` of something other than `typeof f`",
                            ),
                        };
                    }
                    _ => {}
                }
                self.named_type(file, program, name, depth + 1)
            }
            _ => HandleFact::unknown("the type is not a reference to a database type"),
        }
    }

    /// A type named `name` in `file`: a drizzle-orm type, or a type alias of
    /// this module or of the project module it is imported from.
    fn named_type(&self, file: usize, program: &Program<'_>, name: &str, depth: u8) -> HandleFact {
        if depth > MAX_DEPTH {
            return HandleFact::unknown("handle provenance is nested too deeply");
        }
        let imports = all_imports(program);
        if let Some((specifier, imported)) = imports.get(name) {
            if is_drizzle(specifier) {
                if imported == "PgWithReplicas" && specifier == "drizzle-orm/pg-core" {
                    return HandleFact::Replicated(Routing::builtin(
                        "the type `PgWithReplicas` of drizzle-orm/pg-core".to_owned(),
                    ));
                }
                if imported.ends_with("Database") && imported != "Database" {
                    return HandleFact::Primary {
                        origin: format!("the type `{imported}` of `{specifier}`"),
                        by_type: true,
                    };
                }
                return HandleFact::unknown(format!(
                    "the type `{imported}` of `{specifier}` is not a database type"
                ));
            }
            let here = self.ctx.files[file].clone();
            let target = match self.ctx.follow(&here, specifier) {
                Ok(t) => t,
                Err(why) => return HandleFact::Unknown(why),
            };
            return self.exported_type(target, imported, depth + 1, &mut BTreeSet::new());
        }
        match top_level(program, name) {
            Some(TopLevel::TypeAlias(alias)) => {
                if alias.type_parameters.is_some() {
                    return HandleFact::unknown(format!(
                        "`{name}` is a generic type alias (type arguments are not substituted)"
                    ));
                }
                self.ts_type(file, program, &alias.type_annotation, depth + 1)
            }
            Some(TopLevel::Other(what)) => {
                HandleFact::unknown(format!("`{name}` is {what}, not a database type"))
            }
            _ => HandleFact::unknown(format!(
                "`{name}` is not a type alias of `{}`",
                self.rel(file)
            )),
        }
    }

    /// The type `name` exported by `file` (following `export { } from` and
    /// `export *`).
    fn exported_type(
        &self,
        file: usize,
        name: &str,
        depth: u8,
        visiting: &mut BTreeSet<(usize, String)>,
    ) -> HandleFact {
        if depth > MAX_DEPTH || !visiting.insert((file, name.to_owned())) {
            return HandleFact::unknown("type re-exports are nested too deeply or cyclic");
        }
        self.memo(format!("type:{file}:{name}"), || {
            let abs = self.ctx.files[file].clone();
            let Ok(text) = self.ctx.source(&abs) else {
                return HandleFact::unknown(format!("cannot read `{}`", self.rel(file)));
            };
            let allocator = Allocator::default();
            let Some(program) = parse(&allocator, &abs, &text) else {
                return HandleFact::unknown(format!("`{}` does not parse", self.rel(file)));
            };
            if let Some(TopLevel::TypeAlias(_)) = top_level(&program, name) {
                return self.named_type(file, &program, name, depth + 1);
            }
            // `export { X } from "./m"`, `export type { X } from`, and
            // `import { X }; export { X }`.
            let mut stars = Vec::new();
            for statement in &program.body {
                match statement {
                    Statement::ExportNamedDeclaration(e) => {
                        for s in &e.specifiers {
                            if s.exported.name().as_str() == name {
                                let local = s.local.name().as_str().to_owned();
                                return self.named_type(file, &program, &local, depth + 1);
                            }
                        }
                    }
                    Statement::ExportFromDeclaration(e) => {
                        for s in &e.specifiers {
                            if s.exported.name().as_str() == name {
                                let specifier = e.source.value.as_str().unwrap_or_default();
                                let local = s.local.name().as_str().to_owned();
                                return match self.ctx.follow(&abs, specifier) {
                                    Ok(t) => self.exported_type(
                                        t,
                                        &local,
                                        depth + 1,
                                        &mut BTreeSet::new(),
                                    ),
                                    Err(why) => HandleFact::Unknown(why),
                                };
                            }
                        }
                    }
                    Statement::ExportAllDeclaration(e) if e.exported.is_none() => {
                        stars.push(e.source.value.as_str().unwrap_or_default().to_owned());
                    }
                    _ => {}
                }
            }
            let mut found = Vec::new();
            for specifier in stars {
                if let Ok(t) = self.ctx.follow(&abs, &specifier) {
                    let fact = self.exported_type(t, name, depth + 1, &mut BTreeSet::new());
                    if !matches!(&fact, HandleFact::Unknown(w) if w.starts_with("not exported")) {
                        found.push(fact);
                    }
                }
            }
            match found.len() {
                1 => found.remove(0),
                0 => HandleFact::unknown(format!(
                    "not exported: no type `{name}` in `{}`",
                    self.rel(file)
                )),
                _ => HandleFact::unknown(format!(
                    "the type `{name}` is exported by more than one `export *` module of `{}`",
                    self.rel(file)
                )),
            }
        })
    }

    /// What a type annotation written in `file` (given as source text
    /// positions of the parsed program) denotes: entry parameters.
    pub fn annotation(&self, file: usize, program: &Program<'_>, t: &TSType<'_>) -> HandleFact {
        self.ts_type(file, program, t, 0)
    }

    /// Whether the project builds a replicated database: a declared replica
    /// factory, or a module importing `withReplicas` from
    /// `drizzle-orm/pg-core`. Then a handle known only by a plain drizzle
    /// database type may be a replicated one.
    pub fn replicating(&self) -> Option<String> {
        if let Some(f) = self.factories.first() {
            return Some(format!(
                "the declared replica factory `{}`",
                f.binding.factory
            ));
        }
        self.ctx.builtin_replicas()
    }
}

impl ProjectContext {
    /// The first module (if any) that imports a binding named
    /// `withReplicas` (from drizzle-orm, from another package or from a
    /// project module). Such a project is taken to build a replicated
    /// database: a handle known only by its name or by a plain drizzle type
    /// may be one. Files are scanned once (text first; only a module whose
    /// text mentions the name is summarized).
    pub fn builtin_replicas(&self) -> Option<String> {
        if let Some(v) = self.replicas_scan() {
            return v;
        }
        let mut found = None;
        for (index, abs) in self.files.iter().enumerate() {
            let Ok(text) = std::fs::read_to_string(abs) else {
                continue;
            };
            if !text.contains("withReplicas") {
                continue;
            }
            let summary = self.summary(index);
            if let Some((specifier, _)) = summary
                .imports
                .values()
                .find(|(_, imported)| imported == "withReplicas")
            {
                found = Some(format!(
                    "`withReplicas` of `{specifier}` imported by `{}`",
                    self.rel(abs)
                ));
                break;
            }
        }
        self.set_replicas_scan(found.clone());
        found
    }
}

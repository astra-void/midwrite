//! The project analysis context shared by every extraction (Phase 12,
//! Milestone 6).
//!
//! Before Phase 12 each extraction discovered the project's files, built a
//! module resolver and read every module it touched again. A
//! [`ProjectContext`] does the project-level work once and keeps it for all
//! entries of a run:
//!
//! * the module list (one discovery pass) and the [`Resolver`] with its
//!   `tsconfig`/workspace caches;
//! * module sources, read once;
//! * per-module [`ModuleSummary`]s: imports, exports, `export *` lists and
//!   top-level declarations — compact facts, not ASTs (Oxc ASTs live in a
//!   per-use arena and are dropped after use);
//! * export resolution through re-exports (`export { x } from`, `export *
//!   from`, `export * as ns from`, `import { x }; export { x }`), shared by
//!   callee and table resolution.
//!
//! Every fact is derived from file contents only; a context is never reused
//! across runs whose files may differ (the discovery cache keys extractions
//! by the digests of the files they read).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    BindingPattern, Declaration, ExportDefaultDeclarationKind, ImportDeclarationSpecifier,
    ModuleExportName, Program, Statement,
};
use oxc_parser::Parser;
use oxc_span::SourceType;

use crate::resolve::{Resolution, Resolver, absolute};

/// What an exported name of a module is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportTarget {
    /// A top-level binding of the module (`export const x`, `export { y as
    /// x }` of a local `y`). An imported binding re-exported this way is
    /// [`ExportTarget::From`].
    Local(String),
    /// `export { a as x } from "spec"`, or `import { a } from "spec";
    /// export { a as x }`.
    From { specifier: String, imported: String },
    /// `export * as x from "spec"`, or a re-exported namespace import.
    Namespace(String),
}

/// Compact facts of one module.
#[derive(Clone, Debug, Default)]
pub struct ModuleSummary {
    /// Local name → (specifier, imported name; `default`, `*`).
    pub imports: BTreeMap<String, (String, String)>,
    pub exports: BTreeMap<String, ExportTarget>,
    /// `export * from "spec"`, in order.
    pub stars: Vec<String>,
    /// Top-level declared names (functions, classes, variables, enums).
    pub declared: BTreeSet<String>,
    pub parse_error: Option<String>,
}

/// Where an exported name is declared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// The declaration `local` of the module at `file` (index into
    /// [`ProjectContext::files`]); `local` is `default` for a default export.
    Binding { file: usize, local: String },
    /// A namespace object of the module at `file`.
    Namespace { file: usize },
}

#[derive(Default, Debug)]
pub struct ContextStats {
    pub contexts: AtomicU64,
    pub source_reads: AtomicU64,
    pub source_hits: AtomicU64,
    pub summaries: AtomicU64,
    pub resolutions: AtomicU64,
}

pub struct ProjectContext {
    /// Absolute project root.
    pub root: PathBuf,
    /// Absolute, normalized paths of the analyzed files.
    pub files: Vec<PathBuf>,
    by_path: HashMap<PathBuf, usize>,
    resolver: Mutex<Resolver>,
    sources: Mutex<HashMap<PathBuf, Arc<str>>>,
    summaries: Mutex<HashMap<usize, Arc<ModuleSummary>>>,
    /// Memoized derived facts (`db-factory:<file>:<name>` → bool).
    facts: Mutex<HashMap<String, bool>>,
    /// Memoized import closures (files a module reaches through imports and
    /// re-exports, itself included).
    closures: Mutex<HashMap<usize, Arc<BTreeSet<usize>>>>,
    /// Memoized handle provenance (`crate::handles`), keyed by the
    /// declared replica factories and the query.
    handles: Mutex<HashMap<String, crate::handles::HandleFact>>,
    /// The scan for drizzle-orm `withReplicas` imports (once).
    replicas: Mutex<Option<Option<String>>>,
    pub stats: ContextStats,
}

impl std::fmt::Debug for ProjectContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectContext")
            .field("root", &self.root)
            .field("files", &self.files.len())
            .finish_non_exhaustive()
    }
}

pub(crate) fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

impl ProjectContext {
    /// Discover the files of the project at `root` (one pass) and build the
    /// resolver.
    pub fn new(root: &Path) -> Self {
        let root = absolute(root);
        let files: Vec<PathBuf> = crate::discover(std::slice::from_ref(&root))
            .unwrap_or_default()
            .into_iter()
            .map(|p| absolute(&p))
            .collect();
        Self::with_files(root, files)
    }

    pub fn with_files(root: PathBuf, files: Vec<PathBuf>) -> Self {
        let names: Vec<String> = files.iter().map(|p| relative(&root, p)).collect();
        let resolver = Resolver::new(&files, names);
        let by_path = files
            .iter()
            .enumerate()
            .map(|(i, p)| (p.clone(), i))
            .collect();
        let context = Self {
            root,
            files,
            by_path,
            resolver: Mutex::new(resolver),
            sources: Mutex::new(HashMap::new()),
            summaries: Mutex::new(HashMap::new()),
            facts: Mutex::new(HashMap::new()),
            closures: Mutex::new(HashMap::new()),
            handles: Mutex::new(HashMap::new()),
            replicas: Mutex::new(None),
            stats: ContextStats::default(),
        };
        context.stats.contexts.fetch_add(1, Ordering::Relaxed);
        context
    }

    pub fn index_of(&self, abs: &Path) -> Option<usize> {
        self.by_path.get(abs).copied()
    }

    pub fn rel(&self, abs: &Path) -> String {
        relative(&self.root, abs)
    }

    pub(crate) fn resolve(&self, importer: &Path, specifier: &str) -> Resolution {
        self.stats.resolutions.fetch_add(1, Ordering::Relaxed);
        self.resolver
            .lock()
            .expect("resolver lock")
            .resolve(importer, specifier)
    }

    /// The text of a file, read once.
    pub fn source(&self, abs: &Path) -> Result<Arc<str>, String> {
        if let Some(text) = self.sources.lock().expect("sources lock").get(abs) {
            self.stats.source_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(text.clone());
        }
        self.stats.source_reads.fetch_add(1, Ordering::Relaxed);
        let text: Arc<str> = std::fs::read_to_string(abs)
            .map_err(|e| format!("cannot read `{}`: {e}", self.rel(abs)))?
            .into();
        self.sources
            .lock()
            .expect("sources lock")
            .insert(abs.to_path_buf(), text.clone());
        Ok(text)
    }

    /// The summary of file `index`, built once.
    pub fn summary(&self, index: usize) -> Arc<ModuleSummary> {
        if let Some(s) = self.summaries.lock().expect("summaries lock").get(&index) {
            return s.clone();
        }
        let abs = self.files[index].clone();
        let summary = match self.source(&abs) {
            Ok(text) => {
                let allocator = Allocator::default();
                let source_type = SourceType::from_path(&abs).unwrap_or_else(|_| SourceType::ts());
                let parsed = Parser::new(&allocator, &text, source_type).parse();
                if parsed.fatal_error || parsed.diagnostics.errors().next().is_some() {
                    ModuleSummary {
                        parse_error: Some("parse error".to_owned()),
                        ..ModuleSummary::default()
                    }
                } else {
                    summarize(&parsed.program)
                }
            }
            Err(e) => ModuleSummary {
                parse_error: Some(e),
                ..ModuleSummary::default()
            },
        };
        self.stats.summaries.fetch_add(1, Ordering::Relaxed);
        let summary = Arc::new(summary);
        self.summaries
            .lock()
            .expect("summaries lock")
            .insert(index, summary.clone());
        summary
    }

    /// Where `name`, exported by file `file`, is declared, following
    /// re-exports. A name provided by more than one `export *` is ambiguous
    /// (TypeScript does not export it either); a cycle or a package is
    /// reported.
    pub fn resolve_export(&self, file: usize, name: &str) -> Result<Resolved, String> {
        let mut visiting = BTreeSet::new();
        self.export_in(file, name, &mut visiting)
    }

    fn export_in(
        &self,
        file: usize,
        name: &str,
        visiting: &mut BTreeSet<(usize, String)>,
    ) -> Result<Resolved, String> {
        if !visiting.insert((file, name.to_owned())) {
            return Err(format!(
                "`{name}` of `{}` is part of a re-export cycle",
                self.rel(&self.files[file])
            ));
        }
        let summary = self.summary(file);
        let here = self.files[file].clone();
        match summary.exports.get(name) {
            Some(ExportTarget::Local(local)) => {
                return Ok(Resolved::Binding {
                    file,
                    local: local.clone(),
                });
            }
            Some(ExportTarget::From {
                specifier,
                imported,
            }) => {
                let target = self.follow(&here, specifier)?;
                if imported == "*" {
                    return Ok(Resolved::Namespace { file: target });
                }
                return self.export_in(target, imported, visiting);
            }
            Some(ExportTarget::Namespace(specifier)) => {
                return Ok(Resolved::Namespace {
                    file: self.follow(&here, specifier)?,
                });
            }
            None => {}
        }
        if name == "default" {
            return Err(format!(
                "`{}` has no default export",
                self.rel(&self.files[file])
            ));
        }
        let mut found: Vec<Resolved> = Vec::new();
        let mut reasons = Vec::new();
        for specifier in &summary.stars {
            let target = match self.follow(&here, specifier) {
                Ok(t) => t,
                Err(e) => {
                    reasons.push(e);
                    continue;
                }
            };
            match self.export_in(target, name, visiting) {
                Ok(r) => {
                    if !found.contains(&r) {
                        found.push(r);
                    }
                }
                Err(e) => reasons.push(e),
            }
        }
        match found.len() {
            1 => Ok(found.remove(0)),
            0 => Err(format!(
                "no export `{name}` in `{}`{}",
                self.rel(&self.files[file]),
                if summary.stars.is_empty() {
                    String::new()
                } else {
                    format!(
                        " or its `export *` modules{}",
                        reasons
                            .iter()
                            .find(|r| r.contains("package") || r.contains("cycle"))
                            .map(|r| format!(" ({r})"))
                            .unwrap_or_default()
                    )
                }
            )),
            _ => Err(format!(
                "`{name}` is exported by more than one `export *` module of `{}` (ambiguous)",
                self.rel(&self.files[file])
            )),
        }
    }

    /// The file a specifier imported by `importer` leads to.
    pub fn follow(&self, importer: &Path, specifier: &str) -> Result<usize, String> {
        match self.resolve(importer, specifier) {
            Resolution::File(i) => Ok(i),
            Resolution::Package => {
                Err(format!("`{specifier}` is a package, which is not analyzed"))
            }
            Resolution::NotFound(why) => Err(why),
            Resolution::Ambiguous(c) => Err(format!(
                "the import `{specifier}` is ambiguous: {}",
                c.join(", ")
            )),
        }
    }

    /// The files `file` reaches through value imports and re-exports
    /// (itself included).
    pub fn closure(&self, file: usize) -> Arc<BTreeSet<usize>> {
        if let Some(c) = self.closures.lock().expect("closures lock").get(&file) {
            return c.clone();
        }
        let mut seen = BTreeSet::new();
        let mut todo = vec![file];
        while let Some(f) = todo.pop() {
            if !seen.insert(f) {
                continue;
            }
            let summary = self.summary(f);
            let here = self.files[f].clone();
            let specifiers = summary
                .imports
                .values()
                .map(|(s, _)| s)
                .chain(summary.stars.iter())
                .chain(summary.exports.values().filter_map(|e| match e {
                    ExportTarget::From { specifier, .. } | ExportTarget::Namespace(specifier) => {
                        Some(specifier)
                    }
                    ExportTarget::Local(_) => None,
                }));
            for specifier in specifiers {
                if let Ok(next) = self.follow(&here, specifier) {
                    todo.push(next);
                }
            }
        }
        let closure = Arc::new(seen);
        self.closures
            .lock()
            .expect("closures lock")
            .insert(file, closure.clone());
        closure
    }

    /// Where a local name of file `file` is declared: itself, or the
    /// declaration its import leads to.
    pub fn resolve_local(&self, file: usize, local: &str) -> Result<Resolved, String> {
        let summary = self.summary(file);
        if let Some((specifier, imported)) = summary.imports.get(local) {
            let target = self.follow(&self.files[file].clone(), specifier)?;
            if imported == "*" {
                return Ok(Resolved::Namespace { file: target });
            }
            return self.resolve_export(target, imported);
        }
        if summary.declared.contains(local) {
            return Ok(Resolved::Binding {
                file,
                local: local.to_owned(),
            });
        }
        Err(format!("`{local}` is not declared or imported"))
    }
}

impl ProjectContext {
    pub(crate) fn handle_memo(&self, key: &str) -> Option<crate::handles::HandleFact> {
        self.handles.lock().expect("handles lock").get(key).cloned()
    }

    pub(crate) fn set_handle_memo(&self, key: String, fact: crate::handles::HandleFact) {
        self.handles.lock().expect("handles lock").insert(key, fact);
    }

    pub(crate) fn replicas_scan(&self) -> Option<Option<String>> {
        self.replicas.lock().expect("replicas lock").clone()
    }

    pub(crate) fn set_replicas_scan(&self, found: Option<String>) {
        *self.replicas.lock().expect("replicas lock") = Some(found);
    }

    /// Whether the function `name` declared in file `file` returns
    /// a Drizzle database (its declared return type, or `Promise` of it, is
    /// a `drizzle-orm` `...Database` type, directly or through a type alias
    /// of this module or of a module it imports). Such a zero-argument call
    /// (`getDb()`) is a root database handle. Declared types only: the body
    /// is not interpreted.
    pub fn returns_drizzle_database(&self, file: usize, name: &str) -> bool {
        let key = format!("db-factory:{file}:{name}");
        if let Some(v) = self.facts.lock().expect("facts lock").get(&key) {
            return *v;
        }
        let v = self.db_factory(file, name);
        self.facts.lock().expect("facts lock").insert(key, v);
        v
    }

    /// Whether the type named `name` in file `file` is a Drizzle
    /// database type (see [`Self::returns_drizzle_database`]).
    pub fn is_database_type(&self, file: usize, name: &str) -> bool {
        let key = format!("db-type:{file}:{name}");
        if let Some(v) = self.facts.lock().expect("facts lock").get(&key) {
            return *v;
        }
        let abs = self.files[file].clone();
        let v = self.source(&abs).is_ok_and(|text| {
            let allocator = Allocator::default();
            let source_type = SourceType::from_path(&abs).unwrap_or_else(|_| SourceType::ts());
            let parsed = Parser::new(&allocator, &text, source_type).parse();
            !parsed.fatal_error && self.is_drizzle_database(file, &parsed.program, name, 0)
        });
        self.facts.lock().expect("facts lock").insert(key, v);
        v
    }

    fn db_factory(&self, file: usize, name: &str) -> bool {
        let abs = self.files[file].clone();
        let Ok(text) = self.source(&abs) else {
            return false;
        };
        let allocator = Allocator::default();
        let source_type = SourceType::from_path(&abs).unwrap_or_else(|_| SourceType::ts());
        let parsed = Parser::new(&allocator, &text, source_type).parse();
        if parsed.fatal_error {
            return false;
        }
        let Some(type_name) = returned_type(&parsed.program, name) else {
            return false;
        };
        self.is_drizzle_database(file, &parsed.program, &type_name, 0)
    }

    fn is_drizzle_database(
        &self,
        file: usize,
        program: &Program<'_>,
        name: &str,
        depth: u8,
    ) -> bool {
        if depth > 3 {
            return false;
        }
        let imports = type_imports(program);
        if let Some((specifier, imported)) = imports.get(name) {
            if specifier == "drizzle-orm" || specifier.starts_with("drizzle-orm/") {
                return imported.ends_with("Database") && imported != "Database";
            }
            if !crate::generators::is_project_specifier(specifier) {
                return false;
            }
            let Ok(target) = self.follow(&self.files[file].clone(), specifier) else {
                return false;
            };
            let abs = self.files[target].clone();
            let Ok(text) = self.source(&abs) else {
                return false;
            };
            let allocator = Allocator::default();
            let source_type = SourceType::from_path(&abs).unwrap_or_else(|_| SourceType::ts());
            let parsed = Parser::new(&allocator, &text, source_type).parse();
            return self.is_drizzle_database(target, &parsed.program, imported, depth + 1);
        }
        match alias_target(program, name) {
            Some(target) => self.is_drizzle_database(file, program, &target, depth + 1),
            None => false,
        }
    }
}

/// Every import of a module including `import type`, local → (specifier,
/// imported).
fn type_imports(program: &Program<'_>) -> BTreeMap<String, (String, String)> {
    let mut out = BTreeMap::new();
    for statement in &program.body {
        let Statement::ImportDeclaration(import) = statement else {
            continue;
        };
        let from = import.source.value.as_str().unwrap_or_default().to_owned();
        for specifier in import.specifiers.iter().flatten() {
            if let ImportDeclarationSpecifier::ImportSpecifier(x) = specifier {
                out.insert(
                    x.local.name.as_str().to_owned(),
                    (from.clone(), export_name(&x.imported)),
                );
            }
        }
    }
    out
}

/// The name of a type reference, unwrapping `Promise<T>`.
pub(crate) fn reference_name(t: &oxc_ast::ast::TSType<'_>) -> Option<String> {
    let oxc_ast::ast::TSType::TSTypeReference(r) = t else {
        return None;
    };
    let oxc_ast::ast::TSTypeName::IdentifierReference(id) = &r.type_name else {
        return None;
    };
    if id.name.as_str() == "Promise" {
        let inner = r.type_arguments.as_ref()?.params.first()?;
        return reference_name(inner);
    }
    Some(id.name.as_str().to_owned())
}

/// The declared return type of the top-level function `name`.
fn returned_type(program: &Program<'_>, name: &str) -> Option<String> {
    for statement in &program.body {
        let declaration = match statement {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            other => other.as_declaration(),
        };
        match declaration {
            Some(Declaration::FunctionDeclaration(f))
                if f.id.as_ref().is_some_and(|id| id.name.as_str() == name) =>
            {
                return reference_name(&f.return_type.as_ref()?.type_annotation);
            }
            Some(Declaration::VariableDeclaration(v)) => {
                for d in &v.declarations {
                    let BindingPattern::BindingIdentifier(id) = &d.id else {
                        continue;
                    };
                    if id.name.as_str() != name {
                        continue;
                    }
                    return match d.init.as_ref()?.get_inner_expression() {
                        oxc_ast::ast::Expression::ArrowFunctionExpression(a) => {
                            reference_name(&a.return_type.as_ref()?.type_annotation)
                        }
                        oxc_ast::ast::Expression::FunctionExpression(f) => {
                            reference_name(&f.return_type.as_ref()?.type_annotation)
                        }
                        _ => None,
                    };
                }
            }
            _ => {}
        }
    }
    None
}

/// `type name = Target<...>` in the module: `Target`.
fn alias_target(program: &Program<'_>, name: &str) -> Option<String> {
    for statement in &program.body {
        let declaration = match statement {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            other => other.as_declaration(),
        };
        if let Some(Declaration::TSTypeAliasDeclaration(t)) = declaration
            && t.id.name.as_str() == name
        {
            return reference_name(&t.type_annotation);
        }
    }
    None
}

fn export_name(name: &ModuleExportName<'_>) -> String {
    name.name().as_str().to_owned()
}

fn summarize(program: &Program<'_>) -> ModuleSummary {
    let mut s = ModuleSummary::default();
    let mut local_exports: Vec<(String, String)> = Vec::new();
    for statement in &program.body {
        match statement {
            Statement::ImportDeclaration(import) => {
                if import.import_kind.is_type() {
                    continue;
                }
                let from = import.source.value.as_str().unwrap_or_default().to_owned();
                for specifier in import.specifiers.iter().flatten() {
                    let (local, imported) = match specifier {
                        ImportDeclarationSpecifier::ImportSpecifier(x) => {
                            if x.import_kind.is_type() {
                                continue;
                            }
                            (x.local.name.as_str(), export_name(&x.imported))
                        }
                        ImportDeclarationSpecifier::ImportDefaultSpecifier(x) => {
                            (x.local.name.as_str(), "default".to_owned())
                        }
                        ImportDeclarationSpecifier::ImportNamespaceSpecifier(x) => {
                            (x.local.name.as_str(), "*".to_owned())
                        }
                    };
                    s.imports.insert(local.to_owned(), (from.clone(), imported));
                }
            }
            Statement::ExportDeclaration(e) => {
                for name in declared_names(&e.declaration) {
                    s.declared.insert(name.clone());
                    s.exports.insert(name.clone(), ExportTarget::Local(name));
                }
            }
            Statement::ExportNamedDeclaration(e) => {
                if e.export_kind.is_type() {
                    continue;
                }
                for specifier in &e.specifiers {
                    if specifier.export_kind.is_type() {
                        continue;
                    }
                    local_exports.push((
                        export_name(&specifier.exported),
                        export_name(&specifier.local),
                    ));
                }
            }
            Statement::ExportFromDeclaration(e) => {
                if e.export_kind.is_type() {
                    continue;
                }
                let from = e.source.value.as_str().unwrap_or_default().to_owned();
                for specifier in &e.specifiers {
                    if specifier.export_kind.is_type() {
                        continue;
                    }
                    s.exports.insert(
                        export_name(&specifier.exported),
                        ExportTarget::From {
                            specifier: from.clone(),
                            imported: export_name(&specifier.local),
                        },
                    );
                }
            }
            Statement::ExportAllDeclaration(e) => {
                if e.export_kind.is_type() {
                    continue;
                }
                let from = e.source.value.as_str().unwrap_or_default().to_owned();
                match &e.exported {
                    Some(name) => {
                        s.exports
                            .insert(export_name(name), ExportTarget::Namespace(from));
                    }
                    None => s.stars.push(from),
                }
            }
            Statement::ExportDefaultDeclaration(e) => {
                let local = match &e.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                        f.id.as_ref().map(|id| id.name.as_str().to_owned())
                    }
                    ExportDefaultDeclarationKind::ClassDeclaration(c) => {
                        c.id.as_ref().map(|id| id.name.as_str().to_owned())
                    }
                    other => other
                        .as_expression()
                        .and_then(|x| match x.get_inner_expression() {
                            oxc_ast::ast::Expression::Identifier(id) => {
                                Some(id.name.as_str().to_owned())
                            }
                            _ => None,
                        }),
                };
                if let Some(local) = &local {
                    s.declared.insert(local.clone());
                }
                s.exports.insert(
                    "default".to_owned(),
                    ExportTarget::Local(local.unwrap_or_else(|| "default".to_owned())),
                );
            }
            other => {
                if let Some(d) = other.as_declaration() {
                    for name in declared_names(d) {
                        s.declared.insert(name);
                    }
                }
            }
        }
    }
    // `export { a as b }`: of a local declaration, or of an import.
    for (exported, local) in local_exports {
        let target = match s.imports.get(&local) {
            Some((specifier, imported)) if imported == "*" => {
                ExportTarget::Namespace(specifier.clone())
            }
            Some((specifier, imported)) => ExportTarget::From {
                specifier: specifier.clone(),
                imported: imported.clone(),
            },
            None => ExportTarget::Local(local),
        };
        s.exports.insert(exported, target);
    }
    s
}

fn declared_names(d: &Declaration<'_>) -> Vec<String> {
    match d {
        Declaration::VariableDeclaration(v) => v
            .declarations
            .iter()
            .filter_map(|x| match &x.id {
                BindingPattern::BindingIdentifier(id) => Some(id.name.as_str().to_owned()),
                _ => None,
            })
            .collect(),
        Declaration::FunctionDeclaration(f) => {
            f.id.iter().map(|id| id.name.as_str().to_owned()).collect()
        }
        Declaration::ClassDeclaration(c) => {
            c.id.iter().map(|id| id.name.as_str().to_owned()).collect()
        }
        Declaration::TSEnumDeclaration(e) => vec![e.id.name.as_str().to_owned()],
        _ => Vec::new(),
    }
}

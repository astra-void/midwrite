//! Midwrite TypeScript analyzer.
//!
//! Parses TypeScript/TSX with [Oxc](https://oxc.rs), resolves identifiers with
//! Oxc's semantic analysis, recognizes Drizzle ORM operations and configured
//! external effects, lowers each function to the Effect IR of
//! [`midwrite_core`], and runs the core failure-path analysis.
//!
//! Two entry points:
//!
//! * [`Analyzer::analyze_source`] analyzes one file on its own: calls of
//!   functions declared in the same file are summarized, imported functions
//!   are unknown.
//! * [`Analyzer::analyze_project`] (used by [`Analyzer::analyze_paths`] and
//!   the CLI) analyzes a set of files together: calls of functions imported
//!   from other analyzed files are summarized as well (see
//!   `docs/ARCHITECTURE.md`).
//!
//! ```
//! use midwrite_analyzer::Analyzer;
//! use midwrite_core::config::Config;
//!
//! let analyzer = Analyzer::new(Config::default());
//! let analysis = analyzer
//!     .analyze_source(
//!         "orders.ts",
//!         r#"
//!         export async function completeOrder(id: string) {
//!           await db.update(orders).set({ status: "paid" });
//!           await queue.publish("order.completed", { id });
//!         }
//!         "#,
//!     )
//!     .unwrap();
//! assert_eq!(analysis.diagnostics[0].code.as_str(), "MW1001");
//! ```

mod bindings;
mod builtins;
mod classes;
mod concurrency;
pub mod context;
mod discover;
pub mod entries;
mod error;
mod exports;
mod functions;
pub mod generators;
pub mod handles;
pub mod identity;
mod link;
mod lower;
pub mod packages;
mod project;
mod recognize;
mod resolve;
mod row_flow;
pub mod schema;
mod sdk;
mod semantic;
mod suppression;

pub(crate) use concurrency::expression_kind as concurrency_kind;
mod values;
mod wrapper;
mod zod;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use oxc_allocator::Allocator;
use oxc_ast_visit::Visit;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;

use midwrite_core::absint::summary::{FunctionFacts, solve};
use midwrite_core::absint::{AbsValue, Atom};
use midwrite_core::analysis::{
    AnalysisOptions, LocalCall, TraceOutcome, analyze_function_in, inline_calls_with,
};
use midwrite_core::config::Config;
use midwrite_core::diagnostic::Diagnostic;
use midwrite_core::ir::{CallResolution, EffectId, FileIr, FunctionIr};
use midwrite_core::location::{LineIndex, Sources};
use midwrite_core::report::AnalysisCounts;
use midwrite_core::report::{FailureKind, FileFailure, Report};

pub use concurrency::TsActorProvider;
pub use discover::{
    DEFAULT_EXCLUDED_DIRS, discover, discover_excluding, display_path, is_source_file,
};
pub use error::{DiscoveryError, ParseFailure};
pub use project::{MEMORY_PROBE, PhaseHeap, PhaseTimings, ProjectAnalysis};
pub use values::ValueStats;

use classes::ClassInfo;
use exports::ExportTarget;
use link::{ClassTable, ImportResolver, Linked, ModuleView, NoImports, link_facts, link_target};
use lower::{CallTarget, CompactFacts, FactTable, LocalCallSite};

/// The lowered IR and the diagnostics of one file.
#[derive(Clone, Debug)]
pub struct FileAnalysis {
    pub ir: FileIr,
    pub diagnostics: Vec<Diagnostic>,
    /// With traces enabled: one outcome per diagnostic, in the same order.
    /// Empty otherwise.
    pub traces: Vec<TraceOutcome>,
}

/// Runs the frontend and the analysis with one configuration.
#[derive(Clone, Debug, Default)]
pub struct Analyzer {
    config: Config,
    options: AnalysisOptions,
}

/// One file lowered on its own, before any call is summarized.
#[derive(Debug)]
pub(crate) struct LoweredModule {
    pub suppressions: suppression::Suppressions,
    pub functions: Vec<FunctionIr>,
    /// The calls each function makes of functions the frontend could name.
    pub calls: Vec<Vec<LocalCallSite>>,
    /// Summary facts of each function, with their target/label table.
    pub facts: Vec<CompactFacts>,
    pub fact_table: FactTable,
    /// Function start offset → index in `functions`.
    pub index: HashMap<u32, usize>,
    pub classes: HashMap<u32, ClassInfo>,
    pub exports: ModuleExports,
    pub stats: ValueStats,
}

impl LoweredModule {
    pub(crate) fn view(&self, offset: usize) -> ModuleView<'_> {
        ModuleView {
            classes: &self.classes,
            index: &self.index,
            offset,
        }
    }
}

/// Counters of Phase 5 call resolution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResolutionCounts {
    /// Method calls resolved by dispatch on a known receiver.
    pub methods_resolved: usize,
    /// Method calls whose dispatch could not be resolved.
    pub methods_unresolved: usize,
    /// Method resolutions that rely on a recorded assumption.
    pub methods_assumed: usize,
    /// Immediately invoked function expressions resolved.
    pub immediate_calls: usize,
    /// Calls of a parameter (callbacks), linked per instantiation.
    pub callback_sites: usize,
    /// Classes collected over all modules.
    pub classes: usize,
}

/// The variables branch arms of `nodes` state facts about.
fn tested_variables(nodes: &[midwrite_core::ir::Node]) -> std::collections::BTreeSet<String> {
    use midwrite_core::ir::Node;
    fn walk(nodes: &[Node], out: &mut std::collections::BTreeSet<String>) {
        for node in nodes {
            match node {
                Node::Branch(branch) => {
                    for arm in &branch.arms {
                        out.extend(
                            arm.facts
                                .iter()
                                .filter_map(|f| f.variable().map(str::to_owned)),
                        );
                        walk(&arm.body, out);
                    }
                }
                Node::Switch(switch) => {
                    for case in &switch.cases {
                        out.extend(
                            case.facts
                                .iter()
                                .filter_map(|f| f.variable().map(str::to_owned)),
                        );
                        walk(&case.body, out);
                    }
                }
                Node::Transaction(scope) => walk(&scope.body, out),
                Node::Loop(lp) => walk(&lp.body, out),
                Node::Call(call) => walk(&call.body, out),
                Node::Try(t) => {
                    walk(&t.block, out);
                    if let Some(h) = &t.handler {
                        walk(&h.body, out);
                    }
                    if let Some(f) = &t.finalizer {
                        walk(f, out);
                    }
                }
                Node::Effect(_) | Node::Exit(_) => {}
            }
        }
    }
    let mut out = std::collections::BTreeSet::new();
    walk(nodes, &mut out);
    out
}

/// Renumber the function atoms of `value` (`f` maps an id to its new id).
pub(crate) fn renumber(value: &AbsValue, f: &dyn Fn(u32) -> Option<u32>) -> AbsValue {
    if !value
        .atoms()
        .iter()
        .any(|atom| matches!(atom, Atom::Function { .. } | Atom::Record { .. }))
    {
        return value.clone();
    }
    value.substitute(&|atom| match atom {
        Atom::Function { id } => Some(match f(*id) {
            Some(id) => AbsValue::atom(Atom::Function { id }),
            None => AbsValue::top(),
        }),
        _ => None,
    })
}

/// Link the call sites of one function of module `module` to the table.
#[allow(clippy::too_many_arguments)]
pub(crate) fn link_sites(
    module: usize,
    sites: &[LocalCallSite],
    views: &[ModuleView<'_>],
    classes: &ClassTable,
    imports: &mut dyn ImportResolver,
    follow_imports: bool,
    counts: &mut AnalysisCounts,
    resolution: &mut ResolutionCounts,
    unlinked: &mut Vec<(EffectId, String)>,
) -> Vec<LocalCall> {
    let offset = views[module].offset;
    let globalize = |id: u32| Some(id + u32::try_from(offset).unwrap_or(0));
    let mut out = Vec::with_capacity(sites.len());
    for site in sites {
        let mut call = LocalCall::new(site.effect, None, site.placeholder);
        call.arguments = site.arguments.clone();
        call.values = site
            .values
            .iter()
            .map(|v| renumber(v, &globalize))
            .collect();
        call.receiver = site.receiver.as_ref().map(|v| renumber(v, &globalize));
        // Function-free by construction (`entry_shape`): nothing to renumber.
        call.entry_values = site.entry_values.clone();
        call.result = site.result;
        if let Some((binding, members)) = &site.binding {
            call.binding = Some(binding.clone());
            call.binding_members = *members;
        }
        call.argument_effects = site
            .argument_effects
            .iter()
            .map(|(parameter, effect)| (*parameter, *effect))
            .collect();
        let imported = matches!(site.target, CallTarget::Imported { .. });
        if imported && !follow_imports {
            out.push(call);
            continue;
        }
        let linked = link_target(module, &site.target, views, classes, imports);
        match (&site.target, &linked) {
            (CallTarget::Local(_), _) => counts.same_file_calls += 1,
            (CallTarget::Imported { member: None, .. }, Linked::Function(..)) => {
                counts.cross_file_calls_resolved += 1;
            }
            (
                CallTarget::Imported {
                    member: Some(_), ..
                },
                Linked::Function(_, None),
            ) => {
                counts.cross_file_calls_resolved += 1;
            }
            (CallTarget::Imported { .. }, Linked::Unlinked { package: true, .. }) => {
                counts.package_calls += 1;
            }
            (CallTarget::Imported { .. }, Linked::Unlinked { .. }) => {
                counts.cross_file_calls_unresolved += 1;
            }
            _ => {}
        }
        match linked {
            Linked::Function(callee, how) => {
                if let Some(CallResolution::Method { assumption, .. }) = &how {
                    resolution.methods_resolved += 1;
                    resolution.methods_assumed += usize::from(assumption.is_some());
                }
                call.callee = Some(callee);
                call.resolution =
                    how.or_else(|| site.immediate.then_some(CallResolution::Immediate));
                if site.immediate {
                    resolution.immediate_calls += 1;
                }
            }
            Linked::Callback(parameter) => {
                resolution.callback_sites += 1;
                call.parameter = Some(parameter);
                call.resolution = Some(CallResolution::Callback { parameter });
            }
            Linked::Unlinked { reason, .. } => {
                if matches!(
                    site.target,
                    CallTarget::Method { .. } | CallTarget::Super { .. }
                ) {
                    resolution.methods_unresolved += 1;
                }
                unlinked.push((site.effect, reason));
            }
            Linked::Nothing | Linked::NotAFunction => {}
        }
        out.push(call);
    }
    out
}
/// What a module exports, by exported name (`default` for the default export).
#[derive(Debug, Default)]
pub(crate) struct ModuleExports {
    /// Usually one entry per name; several mean a duplicate export.
    pub named: BTreeMap<String, Vec<ExportEntry>>,
    /// `export * from` specifiers.
    pub stars: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExportEntry {
    /// A function of this module, by index.
    Function(usize),
    /// An object service: function properties by name, as function indices.
    Object(BTreeMap<String, usize>),
    /// A class of this module, by start offset.
    Class(u32),
    ReExport {
        module: String,
        name: String,
    },
    NotAFunction,
}

impl Analyzer {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            options: AnalysisOptions::default(),
        }
    }

    #[must_use]
    pub fn with_options(mut self, options: AnalysisOptions) -> Self {
        self.options = options;
        self
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn options(&self) -> &AnalysisOptions {
        &self.options
    }

    /// Parse and lower `source` without summarizing any call.
    pub(crate) fn lower_module(
        &self,
        path: &str,
        source: &str,
        sdk: Option<&sdk::ImportedClients>,
    ) -> Result<LoweredModule, ParseFailure> {
        let allocator = Allocator::default();
        let source_type =
            SourceType::from_path(Path::new(path)).unwrap_or_else(|_| SourceType::ts());
        let lines = LineIndex::new(source);
        let parsed = Parser::new(&allocator, source, source_type).parse();
        if let Some(error) = parsed.diagnostics.errors().next() {
            let position = error
                .labels
                .first()
                .map(|label| lines.position(label.offset()));
            return Err(ParseFailure {
                message: error.message.to_string(),
                line: position.map(|(line, _)| line),
                column: position.map(|(_, column)| column),
            });
        }
        if parsed.fatal_error {
            return Err(ParseFailure {
                message: "the parser could not recover from a syntax error".to_owned(),
                line: None,
                column: None,
            });
        }

        let suppressions =
            suppression::Suppressions::parse(source, &parsed.program.comments, &lines)?;

        let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
        let table = bindings::SymbolTable::collect(&parsed.program, semantic.scoping(), sdk);
        let recognizer = recognize::Recognizer {
            config: &self.config,
            scoping: semantic.scoping(),
            table: &table,
            source,
            lookalikes: std::cell::OnceCell::new(),
        };
        let mut finder = functions::FunctionFinder::new(&recognizer, &lines);
        finder.visit_program(&parsed.program);
        let stats = finder.stats;
        let (mut functions, mut calls, facts, fact_table, index) = finder.finish();
        // A callee's return facts are renamed to the caller's binding of the
        // result only when some branch of the caller tests that binding:
        // otherwise they only keep paths apart.
        for (function, sites) in functions.iter().zip(calls.iter_mut()) {
            if sites.iter().all(|site| site.binding.is_none()) {
                continue;
            }
            let tested = tested_variables(&function.body);
            for site in sites.iter_mut() {
                if let Some((binding, _)) = &site.binding
                    && !tested.iter().any(|variable| {
                        variable == binding
                            || variable
                                .strip_prefix(binding.as_str())
                                .is_some_and(|rest| rest.starts_with('.'))
                    })
                {
                    site.binding = None;
                }
            }
        }
        // Function values are numbered by start offset while lowering; from
        // here on, by index in the module.
        let local = |start: u32| index.get(&start).and_then(|i| u32::try_from(*i).ok());
        for function in &mut functions {
            if let Some(returns) = &mut function.returns {
                **returns = renumber(returns, &local);
            }
        }
        for sites in &mut calls {
            for site in sites {
                for value in &mut site.values {
                    *value = renumber(value, &local);
                }
                if let Some(receiver) = &mut site.receiver {
                    *receiver = renumber(receiver, &local);
                }
            }
        }
        let raw = exports::collect(&parsed.program, semantic.scoping());
        let mut exports = ModuleExports {
            named: BTreeMap::new(),
            stars: raw.stars,
        };
        for (name, target) in raw.named {
            let entry = match target {
                ExportTarget::Function(start) => index
                    .get(&start)
                    .map_or(ExportEntry::NotAFunction, |&i| ExportEntry::Function(i)),
                ExportTarget::Class(start) => ExportEntry::Class(start),
                ExportTarget::Local(symbol) => {
                    if let Some(start) = table.local_function(symbol) {
                        index
                            .get(&start)
                            .map_or(ExportEntry::NotAFunction, |&i| ExportEntry::Function(i))
                    } else if let Some(start) = table.class_of(symbol) {
                        ExportEntry::Class(start)
                    } else if let Some(methods) = table.object_methods(symbol) {
                        ExportEntry::Object(
                            methods
                                .iter()
                                .filter_map(|(name, start)| {
                                    index.get(start).map(|&i| (name.clone(), i))
                                })
                                .collect(),
                        )
                    } else if let Some(import) = table.import(symbol).filter(|i| !i.type_only) {
                        match &import.kind {
                            bindings::ImportKind::Named(imported) => ExportEntry::ReExport {
                                module: import.module.clone(),
                                name: imported.clone(),
                            },
                            bindings::ImportKind::Default => ExportEntry::ReExport {
                                module: import.module.clone(),
                                name: "default".to_owned(),
                            },
                            bindings::ImportKind::Namespace => ExportEntry::NotAFunction,
                        }
                    } else {
                        ExportEntry::NotAFunction
                    }
                }
                ExportTarget::ReExport { module, name } => ExportEntry::ReExport { module, name },
                ExportTarget::Other => ExportEntry::NotAFunction,
            };
            exports.named.entry(name).or_default().push(entry);
        }
        Ok(LoweredModule {
            suppressions,
            functions,
            calls,
            facts,
            fact_table,
            index,
            classes: table.classes().clone(),
            exports,
            stats,
        })
    }

    /// Parse `source` and lower every function in it. `path` selects the
    /// dialect (`.tsx` enables JSX) and is recorded in the IR. Awaited calls
    /// of functions declared in the same file are replaced by the callees'
    /// summaries; calls of imported functions stay unknown effects.
    pub fn lower_source(&self, path: &str, source: &str) -> Result<FileIr, ParseFailure> {
        self.lower_source_with_suppressions(path, source)
            .map(|(ir, _)| ir)
    }

    fn lower_source_with_suppressions(
        &self,
        path: &str,
        source: &str,
    ) -> Result<(FileIr, suppression::Suppressions), ParseFailure> {
        let module = self.lower_module(path, source, None)?;
        let views = [module.view(0)];
        let mut imports = NoImports;
        let classes = ClassTable::build(&views, &mut imports);
        let mut counts = AnalysisCounts::default();
        let mut resolution = ResolutionCounts::default();
        let mut unlinked: Vec<(usize, EffectId, String)> = Vec::new();
        let calls: Vec<Vec<LocalCall>> = module
            .calls
            .iter()
            .enumerate()
            .map(|(index, sites)| {
                let mut details = Vec::new();
                let linked = link_sites(
                    0,
                    sites,
                    &views,
                    &classes,
                    &mut imports,
                    false,
                    &mut counts,
                    &mut resolution,
                    &mut details,
                );
                unlinked.extend(details.into_iter().map(|(e, d)| (index, e, d)));
                linked
            })
            .collect();
        let facts: Vec<FunctionFacts> = module
            .facts
            .iter()
            .map(|facts| link_facts(0, facts, &module.fact_table, &views, &classes, &mut imports))
            .collect();
        let names: Vec<String> = module.functions.iter().map(|f| f.name.clone()).collect();
        let (solved, _) = solve(&facts, &names);
        let mut functions = module.functions;
        for (index, effect, detail) in unlinked {
            if let Some(effect) = functions[index].effect_mut(effect)
                && let midwrite_core::ir::EffectKind::Unknown { detail: text, .. } =
                    &mut effect.kind
            {
                *text = detail;
            }
        }
        inline_calls_with(&mut functions, &calls, None, Some(&solved), &self.options);
        Ok((
            FileIr {
                path: path.to_owned(),
                functions,
                sources: Vec::new(),
            },
            module.suppressions,
        ))
    }

    /// Lower and analyze `source` on its own.
    pub fn analyze_source(&self, path: &str, source: &str) -> Result<FileAnalysis, ParseFailure> {
        let (ir, suppressions) = self.lower_source_with_suppressions(path, source)?;
        let mut diagnostics: Vec<Diagnostic> = ir
            .functions
            .iter()
            .flat_map(|function| {
                analyze_function_in(function, Sources::single(path), &self.options).diagnostics
            })
            .collect();
        midwrite_core::recovery::apply_contracts(
            &mut diagnostics,
            &self.config.recovery_contracts,
            self.options.policy,
        );
        diagnostics.retain(|diagnostic| !suppressions.suppresses(diagnostic));
        diagnostics.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        Ok(FileAnalysis {
            ir,
            diagnostics,
            traces: Vec::new(),
        })
    }

    /// Read and analyze one file on its own.
    pub fn analyze_file(&self, path: &Path) -> Result<FileAnalysis, FileFailure> {
        let display = display_path(path);
        let source = read(path)?;
        self.analyze_source(&display, &source)
            .map_err(|failure| parse_failure(display, failure))
    }

    /// Discover source files under `inputs` and analyze them together, in a
    /// deterministic order. `config_label` is recorded in the report.
    pub fn analyze_paths(
        &self,
        inputs: &[PathBuf],
        config_label: Option<String>,
    ) -> Result<Report, DiscoveryError> {
        let project = self.analyze_project(inputs)?;
        Ok(project.into_report(config_label))
    }

    /// Discover source files under `inputs` and analyze them as one project:
    /// calls of functions imported from other analyzed files are summarized.
    /// Files matching the configuration's `exclude` patterns are skipped.
    pub fn analyze_project(&self, inputs: &[PathBuf]) -> Result<ProjectAnalysis, DiscoveryError> {
        let files = discover_excluding(inputs, &|relative| self.config.is_excluded(relative))?;
        Ok(project::analyze(self, &files, false, None))
    }

    /// Like [`Self::analyze_project`], and also build a failure trace for
    /// every finding with a failure boundary (see
    /// [`ProjectAnalysis::into_trace_report`]). The diagnostics are the same
    /// as without traces. Nothing is executed.
    pub fn trace_project(&self, inputs: &[PathBuf]) -> Result<ProjectAnalysis, DiscoveryError> {
        let files = discover_excluding(inputs, &|relative| self.config.is_excluded(relative))?;
        Ok(project::analyze(self, &files, true, None))
    }

    /// Like [`Self::trace_project`] for `inputs` relative to `root`, with
    /// file paths in diagnostics and traces relative to `root` regardless of
    /// the working directory (used to link replay harness cases to findings
    /// by stable fingerprints).
    pub fn trace_project_at(
        &self,
        root: &Path,
        inputs: &[PathBuf],
    ) -> Result<ProjectAnalysis, DiscoveryError> {
        let inputs: Vec<PathBuf> = inputs.iter().map(|input| root.join(input)).collect();
        let files = discover_excluding(&inputs, &|relative| self.config.is_excluded(relative))?;
        Ok(project::analyze(self, &files, true, Some(root)))
    }
}

fn read(path: &Path) -> Result<String, FileFailure> {
    std::fs::read_to_string(path).map_err(|error| FileFailure {
        file: display_path(path),
        kind: FailureKind::Read,
        message: error.to_string(),
        line: None,
        column: None,
    })
}

fn parse_failure(file: String, failure: ParseFailure) -> FileFailure {
    FileFailure {
        file,
        kind: FailureKind::Parse,
        message: failure.message,
        line: failure.line,
        column: failure.column,
    }
}

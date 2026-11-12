//! Project analysis: every discovered file is lowered, imports of functions
//! from other analyzed files are resolved ([`crate::resolve`]), all functions
//! are placed in one function table so that the core can summarize calls
//! across files, and each function is then analyzed.
//!
//! Function identity is the position in the project table (file order, then
//! source order within the file), never an AST symbol id, which is only
//! unique within one file. A call is linked to a function only through an
//! import that resolves to a module file and an export that resolves to a
//! function declaration; names alone never link two functions.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use midwrite_core::absint::summary::{FunctionFacts, SolveStats, solve};
use midwrite_core::analysis::{
    InlineStats, LocalCall, ProjectFiles, TraceOutcome, analyze_function_in, inline_calls_with,
    resolve_parameters, trace_function_in,
};
use midwrite_core::ir::{DbHandle, EffectKind, FileIr, FunctionIr};
use midwrite_core::location::{FileId, Sources};
use midwrite_core::report::{AnalysisCounts, FileFailure, Report, order_findings};
use midwrite_core::trace::{FailureTrace, SkippedFinding, TraceReport};

use crate::link::{ClassTable, Export, ImportResolver, ModuleView, link_facts};
use crate::resolve::{Resolution, Resolver, absolute};
use crate::values::ValueStats;
use crate::{
    Analyzer, ExportEntry, FileAnalysis, LoweredModule, ResolutionCounts, display_path, link_sites,
    parse_failure, read,
};

/// The result of analyzing a set of files together.
#[derive(Clone, Debug, Default)]
pub struct ProjectAnalysis {
    /// Every file that was analyzed, in discovery order.
    pub files: Vec<FileAnalysis>,
    /// Files that could not be read or parsed.
    pub failures: Vec<FileFailure>,
    pub files_discovered: usize,
    pub counts: AnalysisCounts,
    pub timings: PhaseTimings,
}

/// A function returning the number of live heap bytes, installed by a
/// profiling tool (the corpus runner's `memory` command) to measure memory
/// at phase boundaries. Unset in normal use: nothing is measured.
pub static MEMORY_PROBE: std::sync::OnceLock<fn() -> usize> = std::sync::OnceLock::new();

fn probe() -> Option<usize> {
    MEMORY_PROBE.get().map(|probe| probe())
}

/// Live heap bytes at the end of each phase, when [`MEMORY_PROBE`] is set.
#[derive(Clone, Copy, Debug, Default)]
pub struct PhaseHeap {
    pub start: usize,
    pub lowered: usize,
    pub resolved: usize,
    pub summarized: usize,
    pub analyzed: usize,
}

/// Wall-clock time spent in each phase (not deterministic; not part of the
/// JSON report).
#[derive(Clone, Copy, Debug, Default)]
pub struct PhaseTimings {
    /// Reading, parsing, semantic analysis and lowering.
    pub lower: Duration,
    /// Module and export resolution (the project call graph).
    pub resolve: Duration,
    /// Summary expansion and inlining.
    pub summaries: Duration,
    /// Path enumeration, hazard detection and rendering.
    pub analysis: Duration,
    pub inline: InlineStats,
    /// Of `resolve`: the effect-summary fixpoint (inertness, parameter use).
    pub solve: Duration,
    pub solve_stats: SolveStats,
    /// The value analysis of every function body (during `lower`).
    pub values: ValueStats,
    pub resolution: ResolutionCounts,
    /// Set only when a memory probe is installed.
    pub heap: Option<PhaseHeap>,
}

impl ProjectAnalysis {
    pub fn functions(&self) -> usize {
        self.files.iter().map(|file| file.ir.functions.len()).sum()
    }

    pub fn into_report(self, config: Option<String>) -> Report {
        let functions = self.functions();
        let diagnostics = self
            .files
            .into_iter()
            .flat_map(|file| file.diagnostics)
            .collect();
        Report::new(
            config,
            self.files_discovered,
            functions,
            diagnostics,
            self.failures,
        )
        .with_analysis(self.counts)
    }

    /// The failure traces of an analysis run with traces enabled
    /// ([`crate::Analyzer::trace_project`]). Finding ids are the
    /// fingerprints the JSON report of the same run uses.
    pub fn into_trace_report(self, config: Option<String>) -> TraceReport {
        let mut items: Vec<(midwrite_core::diagnostic::Diagnostic, Option<TraceOutcome>)> =
            Vec::new();
        for file in self.files {
            let mut outcomes = file.traces.into_iter();
            for diagnostic in file.diagnostics {
                items.push((diagnostic, outcomes.next()));
            }
        }
        order_findings(&mut items);
        let mut traces: Vec<FailureTrace> = Vec::new();
        let mut skipped = Vec::new();
        for (diagnostic, outcome) in items {
            match outcome {
                Some(TraceOutcome::Trace(mut trace)) => {
                    trace.assign_finding(&diagnostic.fingerprint);
                    traces.push(*trace);
                }
                Some(TraceOutcome::Skipped(reason)) => skipped.push(SkippedFinding {
                    finding_id: diagnostic.fingerprint,
                    code: diagnostic.code.as_str().to_owned(),
                    function: diagnostic.function,
                    location: diagnostic.location,
                    reason,
                }),
                None => skipped.push(SkippedFinding {
                    finding_id: diagnostic.fingerprint,
                    code: diagnostic.code.as_str().to_owned(),
                    function: diagnostic.function,
                    location: diagnostic.location,
                    reason: "traces were not requested for this analysis".to_owned(),
                }),
            }
        }
        TraceReport::new(config, traces, skipped)
    }
}

struct Linker<'a> {
    modules: &'a [LoweredModule],
    /// Absolute path of each module's file.
    paths: &'a [PathBuf],
    names: &'a [String],
    offsets: &'a [usize],
    resolver: Resolver,
    exports: HashMap<(usize, String), Export>,
    /// Module resolutions by (importing module, specifier).
    resolutions: HashMap<(usize, String), Resolution>,
}

impl Linker<'_> {
    /// The function `name` exported by module `module` denotes.
    fn export(&mut self, module: usize, name: &str, depth: usize) -> Export {
        let key = (module, name.to_owned());
        if let Some(found) = self.exports.get(&key) {
            return found.clone();
        }
        let found = self.compute_export(module, name, depth);
        self.exports.insert(key, found.clone());
        found
    }

    fn compute_export(&mut self, module: usize, name: &str, depth: usize) -> Export {
        let file = &self.names[module];
        if depth > 16 {
            return Export::Unlinked(format!(
                "re-exports of `{name}` from {file} are nested too deeply or circular"
            ));
        }
        let entries = self.modules[module].exports.named.get(name).cloned();
        match entries.as_deref() {
            Some([ExportEntry::Function(index)]) => Export::Function(self.offsets[module] + index),
            Some([ExportEntry::Object(methods)]) => Export::Object(
                methods
                    .iter()
                    .map(|(name, index)| (name.clone(), self.offsets[module] + index))
                    .collect(),
            ),
            Some([ExportEntry::Class(start)]) => Export::Class(module, *start),
            Some([ExportEntry::NotAFunction]) => Export::Unlinked(format!(
                "`{name}` exported by {file} is not a function declaration Midwrite can summarize"
            )),
            Some(
                [
                    ExportEntry::ReExport {
                        module: specifier,
                        name: inner,
                    },
                ],
            ) => match self.module(module, specifier) {
                Ok(target) => self.export(target, inner, depth + 1),
                Err(reason) => Export::Unlinked(format!(
                    "{file} re-exports `{name}` from \"{specifier}\": {reason}"
                )),
            },
            Some(_) => Export::Unlinked(format!(
                "{file} exports `{name}` more than once; Midwrite does not choose one"
            )),
            None if name == "default" => Export::Unlinked(format!("{file} has no default export")),
            None => {
                // `export * from` does not re-export `default`.
                let stars = self.modules[module].exports.stars.clone();
                let mut found: Vec<Export> = Vec::new();
                let mut providers = Vec::new();
                for specifier in &stars {
                    let Ok(target) = self.module(module, specifier) else {
                        continue;
                    };
                    if !self.exports_name(target, name, depth + 1) {
                        continue;
                    }
                    providers.push(self.names[target].clone());
                    let export = self.export(target, name, depth + 1);
                    if !found.contains(&export) {
                        found.push(export);
                    }
                }
                match found.len() {
                    0 => Export::Unlinked(format!("{file} has no export named `{name}`")),
                    1 => found.pop().expect("one provider"),
                    _ => Export::Unlinked(format!(
                        "`{name}` is provided by several `export *` modules of {file} ({}); Midwrite does not choose one",
                        providers.join(", ")
                    )),
                }
            }
        }
    }

    /// Whether module `module` exports `name`, directly or through `export *`.
    fn exports_name(&mut self, module: usize, name: &str, depth: usize) -> bool {
        if depth > 16 {
            return false;
        }
        if self.modules[module].exports.named.contains_key(name) {
            return true;
        }
        let stars = self.modules[module].exports.stars.clone();
        stars
            .iter()
            .any(|specifier| match self.module(module, specifier) {
                Ok(target) => self.exports_name(target, name, depth + 1),
                Err(_) => false,
            })
    }

    /// The module a specifier imported by module `from` leads to.
    fn module(&mut self, from: usize, specifier: &str) -> Result<usize, String> {
        match self.resolver.resolve(&self.paths[from], specifier) {
            Resolution::File(file) => Ok(file),
            Resolution::Package => Err("it is a package import, which is not followed".to_owned()),
            Resolution::NotFound(reason) => Err(reason),
            Resolution::Ambiguous(files) => Err(format!(
                "several files match ({}); Midwrite does not choose one",
                files.join(", ")
            )),
        }
    }
}

impl ImportResolver for Linker<'_> {
    fn module(&mut self, from: usize, specifier: &str) -> Resolution {
        let key = (from, specifier.to_owned());
        if let Some(found) = self.resolutions.get(&key) {
            return found.clone();
        }
        let found = self.resolver.resolve(&self.paths[from], specifier);
        self.resolutions.insert(key, found.clone());
        found
    }

    fn export(&mut self, module: usize, name: &str) -> Export {
        Linker::export(self, module, name, 0)
    }

    fn module_name(&self, module: usize) -> &str {
        &self.names[module]
    }
}

pub(crate) fn analyze(
    analyzer: &Analyzer,
    files: &[PathBuf],
    with_traces: bool,
    root: Option<&std::path::Path>,
) -> ProjectAnalysis {
    let options = analyzer.options();
    let heap_start = probe();
    // Paths in diagnostics: as given, or relative to `root`.
    let name_of = |file: &PathBuf| match root.and_then(|root| file.strip_prefix(root).ok()) {
        Some(relative) => display_path(relative),
        None => display_path(file),
    };
    let start = Instant::now();

    // 1. Lower every file on its own (in parallel; results keep file order).
    let mut modules: Vec<LoweredModule> = Vec::new();
    let mut source_snapshots: Vec<std::sync::Arc<str>> = Vec::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut failures = Vec::new();
    // DEV-148: SDK clients each file imports from project modules.
    let sdk_imports = if analyzer.config().builtin_effects {
        imported_sdk_clients(files, &name_of)
    } else {
        vec![crate::sdk::ImportedClients::new(); files.len()]
    };
    let lowered_files = parallel_map(
        files.iter().zip(&sdk_imports).collect(),
        |(file, sdk): (&PathBuf, &crate::sdk::ImportedClients)| {
            let display = name_of(file);
            read(file).and_then(|source| {
                let source: std::sync::Arc<str> = source.into();
                let module = analyzer
                    .lower_module(&display, &source, Some(sdk))
                    .map_err(|failure| parse_failure(display.clone(), failure))?;
                Ok((module, source))
            })
        },
    );
    for (file, lowered) in files.iter().zip(lowered_files) {
        let display = name_of(file);
        match lowered {
            Ok((module, source)) => {
                modules.push(module);
                source_snapshots.push(source);
                paths.push(absolute(file));
                names.push(display);
            }
            Err(failure) => failures.push(failure),
        }
    }
    crate::row_flow::refine(analyzer.config(), &paths, &source_snapshots, &mut modules);
    drop(source_snapshots);
    let lowered_at = Instant::now();
    let heap_lowered = probe();

    // 2. One function table; link calls.
    let mut offsets = Vec::with_capacity(modules.len() + 1);
    let mut total = 0;
    for module in &modules {
        offsets.push(total);
        total += module.functions.len();
    }
    offsets.push(total);
    let mut counts = AnalysisCounts::default();
    let mut functions: Vec<FunctionIr> = Vec::with_capacity(total);
    let mut file_ids: Vec<FileId> = Vec::with_capacity(total);
    let mut calls: Vec<Vec<LocalCall>> = Vec::with_capacity(total);
    let mut linker = Linker {
        modules: &modules,
        paths: &paths,
        names: &names,
        offsets: &offsets,
        resolver: Resolver::new(&paths, names.clone()),
        exports: HashMap::new(),
        resolutions: HashMap::new(),
    };
    let views: Vec<ModuleView<'_>> = modules
        .iter()
        .enumerate()
        .map(|(index, module)| module.view(offsets[index]))
        .collect();
    let classes = ClassTable::build(&views, &mut linker);
    let mut resolution = ResolutionCounts {
        classes: classes.len(),
        ..ResolutionCounts::default()
    };
    let mut facts: Vec<FunctionFacts> = Vec::with_capacity(total);
    let mut names_of: Vec<String> = Vec::with_capacity(total);
    let mut values = ValueStats::default();
    // Unknown-effect details for calls that could not be linked.
    let mut unlinked: Vec<(usize, midwrite_core::ir::EffectId, String)> = Vec::new();
    for (module_index, module) in modules.iter().enumerate() {
        values.add(&module.stats);
        for (function_index, sites) in module.calls.iter().enumerate() {
            let global = offsets[module_index] + function_index;
            let mut details = Vec::new();
            calls.push(link_sites(
                module_index,
                sites,
                &views,
                &classes,
                &mut linker,
                true,
                &mut counts,
                &mut resolution,
                &mut details,
            ));
            unlinked.extend(
                details
                    .into_iter()
                    .map(|(effect, detail)| (global, effect, detail)),
            );
            facts.push(link_facts(
                module_index,
                &module.facts[function_index],
                &module.fact_table,
                &views,
                &classes,
                &mut linker,
            ));
            names_of.push(module.functions[function_index].name.clone());
        }
        for _ in &module.functions {
            file_ids.push(FileId(u32::try_from(module_index).unwrap_or(u32::MAX)));
        }
    }
    drop(views);
    drop(classes);
    let solve_start = Instant::now();
    let (solved, solve_stats) = solve(&facts, &names_of);
    let solve_time = solve_start.elapsed();
    let mut atomic_helpers =
        transaction_only_helpers(&modules, &calls, &facts, analyzer.config().closed_world);
    drop(facts);
    drop(linker);
    // Lowering-only data is not needed past linking.
    for module in &mut modules {
        module.calls = Vec::new();
        module.facts = Vec::new();
        module.fact_table = crate::lower::FactTable::default();
        module.classes = HashMap::new();
        module.index = HashMap::new();
    }
    let lengths: Vec<usize> = modules.iter().map(|m| m.functions.len()).collect();
    for module in &mut modules {
        functions.append(&mut module.functions);
    }
    for (function, effect, detail) in unlinked {
        if let Some(effect) = functions[function].effect_mut(effect)
            && let EffectKind::Unknown { detail: text, .. } = &mut effect.kind
        {
            *text = detail;
        }
    }
    let resolved_at = Instant::now();
    let heap_resolved = probe();

    // 3. Summaries across the whole table.
    let inline = inline_calls_with(
        &mut functions,
        &calls,
        Some(ProjectFiles {
            ids: &file_ids,
            names: &names,
        }),
        Some(&solved),
        options,
    );
    // Recheck expanded bodies: a nested helper may write on a different
    // handle. Only a single transaction parameter may own every write.
    atomic_helpers.retain(|index| one_parameter_writes(&functions[*index]).is_some());
    drop(solved);
    counts.calls_summarized = inline.summarized;
    let inlined_at = Instant::now();
    let heap_summarized = probe();

    // 4. Analyze each function in its file (files in parallel).
    let mut per_file: Vec<(usize, Vec<FunctionIr>)> = Vec::with_capacity(modules.len());
    let mut functions = functions.into_iter();
    for (module_index, length) in lengths.iter().enumerate() {
        per_file.push((module_index, functions.by_ref().take(*length).collect()));
    }
    let suppressions: HashMap<_, _> = names
        .iter()
        .zip(&modules)
        .map(|(name, module)| (name.as_str(), &module.suppressions))
        .collect();
    let results = parallel_map(per_file, |(module_index, mut file_functions)| {
        let own = FileId(u32::try_from(module_index).unwrap_or(u32::MAX));
        let sources = compact_sources(&mut file_functions, own, &names);
        let src = Sources {
            file: &names[module_index],
            others: &sources,
        };
        let mut counts = AnalysisCounts::default();
        let mut pairs = Vec::new();
        for (local, function) in file_functions.iter().enumerate() {
            if resolve_parameters(function).has_recognized_effects() {
                counts.functions_with_effects += 1;
            }
            let (result, outcomes) = if with_traces {
                trace_function_in(function, src, options)
            } else {
                (analyze_function_in(function, src, options), Vec::new())
            };
            counts.truncated_functions += usize::from(result.truncated);
            counts.pruned_paths += result.pruned;
            counts.merged_paths += result.merged;
            let mut outcomes = outcomes.into_iter();
            pairs.extend(result.diagnostics.into_iter().filter_map(|diagnostic| {
                let outcome = outcomes.next();
                if atomic_helpers.contains(&(offsets[module_index] + local))
                    && diagnostic.code
                        == midwrite_core::diagnostic::DiagnosticCode::IndependentDbCommits
                {
                    None
                } else {
                    Some((diagnostic, outcome))
                }
            }));
        }
        let contracts = &analyzer.config().recovery_contracts;
        if !contracts.is_empty() {
            for (diagnostic, _) in &mut pairs {
                midwrite_core::recovery::apply_contracts(
                    std::slice::from_mut(diagnostic),
                    contracts,
                    options.policy,
                );
            }
        }
        // Match the primary reported file and line, keeping each trace
        // paired with its diagnostic throughout filtering and sorting.
        pairs.retain(|(diagnostic, _)| {
            !suppressions
                .get(diagnostic.location.file.as_str())
                .is_some_and(|directives| directives.suppresses(diagnostic))
        });
        pairs.sort_by(|a, b| a.0.sort_key().cmp(&b.0.sort_key()));
        let (diagnostics, traces): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
        let analysis = FileAnalysis {
            ir: FileIr {
                path: names[module_index].clone(),
                functions: file_functions,
                sources,
            },
            diagnostics,
            traces: traces.into_iter().flatten().collect(),
        };
        (analysis, counts)
    });
    let mut analyzed = Vec::with_capacity(results.len());
    for (analysis, file_counts) in results {
        counts.functions_with_effects += file_counts.functions_with_effects;
        counts.truncated_functions += file_counts.truncated_functions;
        counts.pruned_paths += file_counts.pruned_paths;
        counts.merged_paths += file_counts.merged_paths;
        analyzed.push(analysis);
    }
    let analyzed_at = Instant::now();
    let heap_analyzed = probe();

    ProjectAnalysis {
        files: analyzed,
        failures,
        files_discovered: files.len(),
        counts,
        timings: PhaseTimings {
            lower: lowered_at - start,
            resolve: resolved_at - lowered_at,
            summaries: inlined_at - resolved_at,
            analysis: analyzed_at - inlined_at,
            inline,
            solve: solve_time,
            solve_stats,
            values,
            resolution,
            heap: heap_start.map(|start| PhaseHeap {
                start,
                lowered: heap_lowered.unwrap_or(0),
                resolved: heap_resolved.unwrap_or(0),
                summarized: heap_summarized.unwrap_or(0),
                analyzed: heap_analyzed.unwrap_or(0),
            }),
        },
    }
}

/// DEV-148: for every file, the SDK clients it imports from project modules
/// (`import { resend } from "@/services/resend"`, where that module does
/// `export const resend = new Resend(key)`), following re-exports.
fn imported_sdk_clients(
    files: &[PathBuf],
    name_of: &(dyn Fn(&PathBuf) -> String + Sync),
) -> Vec<crate::sdk::ImportedClients> {
    use oxc_allocator::Allocator;
    use oxc_parser::Parser;
    use oxc_span::SourceType;
    let facts: Vec<crate::sdk::FileFacts> =
        parallel_map(files.iter().collect(), |file: &PathBuf| {
            let Ok(source) = std::fs::read_to_string(file) else {
                return crate::sdk::FileFacts::default();
            };
            let allocator = Allocator::default();
            let source_type = SourceType::from_path(file).unwrap_or_else(|_| SourceType::ts());
            let parsed = Parser::new(&allocator, &source, source_type).parse();
            if parsed.fatal_error {
                return crate::sdk::FileFacts::default();
            }
            crate::sdk::file_facts(&parsed.program)
        });
    if facts.iter().all(|f| f.exports.is_empty()) {
        return vec![crate::sdk::ImportedClients::new(); files.len()];
    }
    let paths: Vec<PathBuf> = files.iter().map(|f| absolute(f)).collect();
    let mut resolver = Resolver::new(&paths, files.iter().map(name_of).collect());
    fn lookup(
        file: usize,
        name: &str,
        depth: usize,
        facts: &[crate::sdk::FileFacts],
        paths: &[PathBuf],
        resolver: &mut Resolver,
    ) -> Option<crate::sdk::Export> {
        if depth > 8 {
            return None;
        }
        let here = &facts[file];
        if let Some(export) = here.exports.get(name) {
            return Some(*export);
        }
        for (exported, specifier, imported) in &here.reexports {
            if exported == name
                && let Resolution::File(target) = resolver.resolve(&paths[file], specifier)
            {
                return lookup(target, imported, depth + 1, facts, paths, resolver);
            }
        }
        if name == "default" {
            return None;
        }
        for specifier in &here.stars {
            if let Resolution::File(target) = resolver.resolve(&paths[file], specifier)
                && let Some(export) = lookup(target, name, depth + 1, facts, paths, resolver)
            {
                return Some(export);
            }
        }
        None
    }
    facts
        .iter()
        .enumerate()
        .map(|(file, here)| {
            let mut out = crate::sdk::ImportedClients::new();
            for (specifier, name) in &here.imports {
                if let Resolution::File(target) = resolver.resolve(&paths[file], specifier)
                    && let Some(export) = lookup(target, name, 0, &facts, &paths, &mut resolver)
                {
                    out.insert((specifier.clone(), name.clone()), export);
                }
            }
            out
        })
        .collect()
}

/// `items.map(f)` on a few threads, in input order. The output does not
/// depend on the number of threads or on scheduling. At most 8 threads, or
/// the number in the `MIDWRITE_THREADS` environment variable.
fn parallel_map<T: Send, R: Send>(items: Vec<T>, f: impl Fn(T) -> R + Sync) -> Vec<R> {
    let available = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let threads = std::env::var("MIDWRITE_THREADS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| available.min(8))
        .min(items.len().max(1));
    if threads <= 1 {
        return items.into_iter().map(f).collect();
    }
    let queue = std::sync::Mutex::new(items.into_iter().enumerate());
    let mut results: Vec<(usize, R)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let next = queue.lock().expect("queue lock").next();
                        let Some((index, item)) = next else { break };
                        out.push((index, f(item)));
                    }
                    out
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("worker thread"))
            .collect()
    });
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
}

/// A narrow closed-world proof: every recognized write of the helper uses
/// one parameter, and every observed call passes a transaction for it.
/// Calls that were not lowered (e.g. unawaited calls), forwarded function
/// values and unobserved entry points prevent refinement. Expanded nested
/// calls are rechecked before applying the refinement.
/// Helpers whose every call passes a transaction for the one parameter all
/// their writes use: their independent-commit findings are dropped. With
/// `closedWorld`, any such function; without it, only module-private
/// helpers (not exported, only ever called directly), whose callers are
/// all in the analyzed project by construction.
fn transaction_only_helpers(
    modules: &[LoweredModule],
    calls: &[Vec<LocalCall>],
    facts: &[FunctionFacts],
    closed_world: bool,
) -> BTreeSet<usize> {
    let mut atomic = BTreeSet::new();
    let mut index = 0;
    for module in modules {
        for (local, function) in module.functions.iter().enumerate() {
            let current = index;
            index += 1;
            let private_calls = module.private_helpers.get(&local).copied();
            if !closed_world && private_calls.is_none() {
                continue;
            }
            let Some(parameter) = one_parameter_writes(function) else {
                continue;
            };
            let incoming: Vec<_> = calls
                .iter()
                .flatten()
                .filter(|c| c.callee == Some(current))
                .collect();
            let fact_count = facts
                .iter()
                .flat_map(|f| &f.calls)
                .filter(|c| c.callee == current)
                .count();
            let escapes = facts
                .iter()
                .flat_map(|f| &f.calls)
                .flat_map(|c| &c.args)
                .any(|a| *a == midwrite_core::absint::summary::ArgFact::Function(current));
            // Every invocation is observed: with `closedWorld`, as call facts
            // (which are kept for every body then); for a private helper, as
            // one linked call site per direct-call reference to its binding.
            let complete = if closed_world {
                fact_count == incoming.len()
            } else {
                private_calls == Some(incoming.len())
            };
            if !escapes
                && !incoming.is_empty()
                && complete
                && incoming.iter().all(|call| {
                    matches!(
                        call.arguments
                            .get(parameter as usize)
                            .and_then(Option::as_ref)
                            .map(|a| &a.handle),
                        Some(DbHandle::Transaction { .. })
                    )
                })
            {
                atomic.insert(current);
            }
        }
    }
    atomic
}

fn one_parameter_writes(function: &FunctionIr) -> Option<u32> {
    if !function.transactions.is_empty() {
        return None;
    }
    let mut parameter = None;
    for effect in function.effects() {
        match &effect.kind {
            EffectKind::External { .. } => return None,
            EffectKind::DatabaseWrite { handle, .. } => match handle {
                DbHandle::Parameter { index, .. } if parameter.is_none_or(|p| p == *index) => {
                    parameter = Some(*index)
                }
                _ => return None,
            },
            _ => {}
        }
    }
    parameter
}

/// Rewrite project-wide file ids to the per-file source table.
fn compact_sources(functions: &mut [FunctionIr], own: FileId, names: &[String]) -> Vec<String> {
    let mut referenced: BTreeSet<u32> = BTreeSet::new();
    for function in functions.iter_mut() {
        function.map_locations(&mut |location| {
            if let Some(id) = location.file
                && id != own
            {
                referenced.insert(id.0);
            }
        });
    }
    let table: HashMap<u32, u32> = referenced
        .iter()
        .enumerate()
        .map(|(local, &global)| (global, u32::try_from(local).unwrap_or(u32::MAX)))
        .collect();
    for function in functions.iter_mut() {
        function.map_locations(&mut |location| {
            location.file = match location.file {
                Some(id) if id == own => None,
                Some(id) => table.get(&id.0).map(|&local| FileId(local)),
                None => None,
            };
        });
    }
    referenced
        .iter()
        .map(|&global| names[global as usize].clone())
        .collect()
}

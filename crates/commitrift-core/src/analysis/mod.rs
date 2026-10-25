//! Failure-path analysis over the Effect IR.
//!
//! The analysis asks one question per function: *if the process stopped
//! between two operations that each become durable or visible on their own,
//! what state would be left behind?*
//!
//! 1. [`paths`] enumerates the paths through the function (bounded), and
//!    discards paths whose branch decisions contradict each other.
//! 2. [`detect`] replays each path with a transaction model to find the
//!    points where state becomes durable (commits) or visible (external
//!    effects), and pairs adjacent points into candidate hazards.
//! 3. [`render`] turns the best candidate for each distinct hazard into a
//!    diagnostic with a failure sequence, evidence and limitations.
//!
//! Before that, [`inline_calls`] replaces awaited calls of functions the
//! frontend resolved (same file, or imported in project analysis) with the
//! callee's summarized body (see [`summary`]).

mod detect;
mod paths;
pub use paths::{CompactStats, PathMode, path_mode, path_mode_from_env, set_path_mode};
mod render;
mod summary;
mod trace;

pub use summary::{
    CallArgument, InlineStats, LocalCall, MAX_SPECIALIZATIONS, ProjectFiles, RETURN_VARIABLE,
    inline_calls, inline_calls_with, inline_local_calls, resolve_parameters,
};

use crate::diagnostic::{Diagnostic, Policy};
use crate::ir::FunctionIr;
use crate::location::Sources;
use crate::trace::FailureTrace;

/// Tuning knobs for the analysis.
#[derive(Clone, Debug)]
pub struct AnalysisOptions {
    /// Maximum number of paths explored per function. Functions with more
    /// paths get a CP9001 diagnostic and are analyzed partially.
    pub max_paths: usize,
    /// How many levels of calls are inlined from summaries. `0` disables
    /// summaries: every call stays an unknown effect.
    pub max_call_depth: usize,
    /// Which diagnostics are reported and at which severity.
    pub policy: Policy,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        Self {
            max_paths: 512,
            max_call_depth: 3,
            policy: Policy::default(),
        }
    }
}

/// The result of analyzing one function.
#[derive(Clone, Debug, Default)]
pub struct FunctionAnalysis {
    pub diagnostics: Vec<Diagnostic>,
    /// Number of paths analyzed.
    pub paths: usize,
    /// The path budget was exhausted: only a prefix of the paths was analyzed.
    pub truncated: bool,
    /// Paths discarded because their branch decisions contradict each other.
    pub pruned: usize,
    /// Paths merged into an equivalent path when leaving an inlined call.
    pub merged: usize,
}

/// Analyze one function. `file` is the path reported in diagnostics.
pub fn analyze_function(
    function: &FunctionIr,
    file: &str,
    options: &AnalysisOptions,
) -> Vec<Diagnostic> {
    analyze_function_in(function, Sources::single(file), options).diagnostics
}

/// Analyze one function whose locations may refer to other files
/// (operations inlined from other modules).
pub fn analyze_function_in(
    function: &FunctionIr,
    src: Sources<'_>,
    options: &AnalysisOptions,
) -> FunctionAnalysis {
    run(function, src, options, false).0
}

/// What [`trace_function_in`] produced for one diagnostic.
#[derive(Clone, Debug)]
pub enum TraceOutcome {
    Trace(Box<FailureTrace>),
    /// No trace: the reason.
    Skipped(String),
}

/// Analyze one function like [`analyze_function_in`] and also build a
/// [`FailureTrace`] for every diagnostic that has a failure boundary.
/// `outcomes[i]` belongs to `analysis.diagnostics[i]`. The diagnostics are
/// identical to those of [`analyze_function_in`].
pub fn trace_function_in(
    function: &FunctionIr,
    src: Sources<'_>,
    options: &AnalysisOptions,
) -> (FunctionAnalysis, Vec<TraceOutcome>) {
    run(function, src, options, true)
}

static DIFFERENTIAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static DIFFERENTIAL_LOG: std::sync::Mutex<(usize, CompactStats, Vec<String>)> =
    std::sync::Mutex::new((
        0,
        CompactStats {
            built: 0,
            reused: 0,
            shared_facts: 0,
        },
        Vec::new(),
    ));

/// Compare both path modes on every function analyzed from now on (Phase 7
/// differential testing; `COMMITRIFT_PATHS_DIFFERENTIAL=1` in the CLI).
pub fn enable_path_differential(on: bool) {
    DIFFERENTIAL.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Functions compared, summed compact counters and differences so far
/// (and reset).
pub fn take_path_differential() -> (usize, CompactStats, Vec<String>) {
    let mut log = DIFFERENTIAL_LOG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::mem::take(&mut *log)
}

/// Phase 7 differential check: enumerate `function`'s paths in both
/// [`PathMode`]s and compare them event by event (by IR identity), plus the
/// truncation flag and the pruned/merged counters. Returns the compact
/// summary counters, or the first difference.
pub fn compare_path_modes(
    function: &FunctionIr,
    options: &AnalysisOptions,
    include_reads: bool,
) -> Result<CompactStats, String> {
    let function = resolve_parameters(function);
    let function = function.as_ref();
    let (legacy, _) =
        paths::enumerate_in(function, options.max_paths, include_reads, PathMode::Legacy);
    let (compact, stats) = paths::enumerate_in(
        function,
        options.max_paths,
        include_reads,
        PathMode::Compact,
    );
    if legacy.truncated != compact.truncated
        || legacy.pruned != compact.pruned
        || legacy.merged != compact.merged
    {
        return Err(format!(
            "`{}`: truncated/pruned/merged legacy {}/{}/{} compact {}/{}/{}",
            function.name,
            legacy.truncated,
            legacy.pruned,
            legacy.merged,
            compact.truncated,
            compact.pruned,
            compact.merged
        ));
    }
    if legacy.traces.len() != compact.traces.len() {
        return Err(format!(
            "`{}`: {} legacy traces, {} compact",
            function.name,
            legacy.traces.len(),
            compact.traces.len()
        ));
    }
    for (i, (a, b)) in legacy.traces.iter().zip(&compact.traces).enumerate() {
        if !paths::same_trace(a, b) {
            return Err(format!("`{}`: trace {i} differs", function.name));
        }
    }
    Ok(stats)
}

fn run(
    function: &FunctionIr,
    src: Sources<'_>,
    options: &AnalysisOptions,
    with_traces: bool,
) -> (FunctionAnalysis, Vec<TraceOutcome>) {
    if DIFFERENTIAL.load(std::sync::atomic::Ordering::Relaxed) {
        let result = compare_path_modes(function, options, with_traces);
        let mut log = DIFFERENTIAL_LOG
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        log.0 += 1;
        match result {
            Ok(stats) => {
                log.1.built += stats.built;
                log.1.reused += stats.reused;
                log.1.shared_facts += stats.shared_facts;
            }
            Err(difference) => log.2.push(difference),
        }
    }
    let function = resolve_parameters(function);
    let function = function.as_ref();
    let paths = paths::enumerate_with(function, options.max_paths, with_traces);
    let findings = detect::detect(function, &paths.traces, options, &src);
    let renderer = render::Renderer {
        function,
        src,
        policy: options.policy,
    };
    let truncated = paths.truncated && function.has_recognized_effects();
    let mut pairs: Vec<(Diagnostic, Option<TraceOutcome>)> = findings
        .into_iter()
        .map(|finding| {
            let draft = with_traces
                .then(|| renderer.draft_trace(&paths.traces[finding.trace], &finding, truncated));
            let diagnostic = renderer.render(finding);
            let outcome = draft.map(|draft| match draft {
                Ok(draft) => TraceOutcome::Trace(Box::new(draft.finish(&diagnostic))),
                Err(reason) => TraceOutcome::Skipped(reason),
            });
            (diagnostic, outcome)
        })
        .collect();
    if truncated {
        let diagnostic = renderer.truncated(options.max_paths);
        let outcome =
            with_traces.then(|| TraceOutcome::Skipped(trace::skip_reason(diagnostic.code)));
        pairs.push((diagnostic, outcome));
    }
    pairs.sort_by(|a, b| a.0.sort_key().cmp(&b.0.sort_key()));
    let (diagnostics, outcomes): (Vec<Diagnostic>, Vec<Option<TraceOutcome>>) =
        pairs.into_iter().unzip();
    (
        FunctionAnalysis {
            diagnostics,
            paths: paths.traces.len(),
            truncated,
            pruned: paths.pruned,
            merged: paths.merged,
        },
        outcomes.into_iter().flatten().collect(),
    )
}

#[cfg(test)]
mod tests;

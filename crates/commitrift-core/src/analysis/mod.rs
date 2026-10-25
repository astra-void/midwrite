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

fn run(
    function: &FunctionIr,
    src: Sources<'_>,
    options: &AnalysisOptions,
    with_traces: bool,
) -> (FunctionAnalysis, Vec<TraceOutcome>) {
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

//! The result of a scan. Serialized as the JSON output format
//! (see `docs/DIAGNOSTICS.md` for the schema).

use serde::Serialize;

use crate::diagnostic::{BaselineState, Diagnostic, Severity};

/// Bumped on any incompatible change to the JSON output.
pub const REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub schema_version: u32,
    pub tool: ToolInfo,
    /// Path of the configuration file used, or `None` for built-in defaults.
    pub config: Option<String>,
    pub summary: Summary,
    pub diagnostics: Vec<Diagnostic>,
    /// Files that could not be read or parsed. They were not analyzed.
    pub failures: Vec<FileFailure>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub name: &'static str,
    pub version: &'static str,
}

impl Default for ToolInfo {
    fn default() -> Self {
        Self {
            name: "midwrite",
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub files_discovered: usize,
    pub files_analyzed: usize,
    pub files_failed: usize,
    pub functions_analyzed: usize,
    pub diagnostics: SeverityCounts,
    /// Reported places where the analysis is known to be incomplete (the
    /// `MW9001` diagnostics of this report). More than zero: the absence of
    /// a finding there is not a clean result. Zero does not mean that the
    /// analysis is complete, only that no gap was recognized.
    pub known_gaps: usize,
    /// Measured facts about the analysis itself (not about correctness).
    pub analysis: AnalysisCounts,
    /// Set when the run compares against a baseline file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<BaselineCounts>,
}

/// Counters describing how much of the code the analysis covered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisCounts {
    /// Functions with at least one recognized operation (after summaries).
    pub functions_with_effects: usize,
    /// Functions whose paths exceeded `--max-paths` (results are partial).
    pub truncated_functions: usize,
    /// Paths discarded because their branch conditions contradict each other.
    pub pruned_paths: usize,
    /// Paths merged into an equivalent path when leaving an inlined call.
    pub merged_paths: usize,
    /// Awaited calls of functions declared in the same file.
    pub same_file_calls: usize,
    /// Awaited calls of imported functions resolved to a function of
    /// another analyzed file.
    pub cross_file_calls_resolved: usize,
    /// Awaited calls of imported functions from relative or aliased modules
    /// that could not be resolved (missing module, missing or ambiguous
    /// export, not a function).
    pub cross_file_calls_unresolved: usize,
    /// Awaited calls of functions imported from packages (not followed).
    pub package_calls: usize,
    /// Calls replaced by the callee's summary.
    pub calls_summarized: usize,
    /// Promise continuations (`.then/.catch/.finally`) whose handler
    /// contains a recognized operation but is not analyzed in sequence
    /// (each is part of an `MW9001` diagnostic). Handlers without a
    /// recognized operation, and other unmodeled asynchronous code, are
    /// not counted: zero is not a statement about recall.
    pub unmodeled_continuations: usize,
}

/// Comparison with a baseline file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaselineCounts {
    /// Diagnostics not in the baseline.
    pub new: usize,
    /// Diagnostics found in the baseline.
    pub existing: usize,
    /// Baseline entries no longer reported.
    pub resolved: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeverityCounts {
    pub error: usize,
    pub warning: usize,
    pub info: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileFailure {
    pub file: String,
    pub kind: FailureKind,
    pub message: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    Read,
    Parse,
}

/// Sort findings deterministically (by [`Diagnostic::sort_key`], stable)
/// and make identical fingerprints unique: findings with the same code,
/// file, function and labels are told apart by their order of appearance
/// (`<fingerprint>-2`, `-3`, ...). Every output that names findings (the
/// report, baselines, failure traces) goes through this function, so their
/// finding ids agree.
pub fn order_findings<T>(items: &mut [(Diagnostic, T)]) {
    items.sort_by(|a, b| a.0.sort_key().cmp(&b.0.sort_key()));
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (diagnostic, _) in items.iter_mut() {
        let count = seen.entry(diagnostic.fingerprint.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            diagnostic.fingerprint = format!("{}-{}", diagnostic.fingerprint, count);
        }
    }
}

impl Report {
    /// Build a report, sorting diagnostics and failures deterministically and
    /// computing the summary counts.
    pub fn new(
        config: Option<String>,
        files_discovered: usize,
        functions_analyzed: usize,
        diagnostics: Vec<Diagnostic>,
        mut failures: Vec<FileFailure>,
    ) -> Self {
        failures.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
        let mut items: Vec<(Diagnostic, ())> = diagnostics.into_iter().map(|d| (d, ())).collect();
        order_findings(&mut items);
        let diagnostics: Vec<Diagnostic> = items.into_iter().map(|(d, ())| d).collect();
        let mut counts = SeverityCounts::default();
        let known_gaps = diagnostics
            .iter()
            .filter(|d| d.kind == crate::diagnostic::DiagnosticKind::AnalysisLimitation)
            .count();
        for diagnostic in &diagnostics {
            match diagnostic.severity {
                Severity::Error => counts.error += 1,
                Severity::Warning => counts.warning += 1,
                Severity::Info => counts.info += 1,
            }
        }
        Self {
            schema_version: REPORT_SCHEMA_VERSION,
            tool: ToolInfo::default(),
            config,
            summary: Summary {
                files_discovered,
                files_analyzed: files_discovered.saturating_sub(failures.len()),
                files_failed: failures.len(),
                functions_analyzed,
                diagnostics: counts,
                known_gaps,
                analysis: AnalysisCounts::default(),
                baseline: None,
            },
            diagnostics,
            failures,
        }
    }

    #[must_use]
    pub fn with_analysis(mut self, analysis: AnalysisCounts) -> Self {
        self.summary.analysis = analysis;
        self
    }

    /// Number of diagnostics at or above `threshold`.
    pub fn count_at_or_above(&self, threshold: Severity) -> usize {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity >= threshold)
            .count()
    }

    /// Number of diagnostics at or above `threshold` that are not in the
    /// baseline (all of them when no baseline is used).
    pub fn count_new_at_or_above(&self, threshold: Severity) -> usize {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity >= threshold)
            .filter(|diagnostic| diagnostic.baseline != Some(BaselineState::Existing))
            .count()
    }
}

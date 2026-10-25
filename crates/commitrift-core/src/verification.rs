//! Verification levels: how strongly a failure path is supported.
//!
//! The levels are deliberately separate from [`crate::diagnostic::Confidence`]
//! (how certain the *static analysis* is about a path) and are never derived
//! from it. A finding starts as [`VerificationLevel::StaticCandidate`]; only
//! runtime replay of an explicitly authorized harness can produce the
//! `RUNTIME_*` levels. See `docs/phase3/ARCHITECTURE.md`.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VerificationLevel {
    /// A possible failure path derived from source analysis. Nothing was
    /// executed.
    StaticCandidate,
    /// The abstract transaction model was run over the static trace. It
    /// shows what the analysis *predicts* survives; no application code or
    /// real effect was executed, so it is no stronger than the trace itself.
    Simulated,
    /// An authorized, instrumented harness was executed, the failure was
    /// injected at the selected checkpoint, and a declared invariant was
    /// observed to fail.
    RuntimeReproduced,
    /// The controlled run completed and every declared invariant held under
    /// the recorded conditions. This does not prove the application safe.
    RuntimeNotReproduced,
    /// The run did not establish an outcome (unsupported semantics, setup
    /// failure, timeout, checkpoint never reached, failing control run, ...).
    Inconclusive,
}

impl VerificationLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StaticCandidate => "STATIC_CANDIDATE",
            Self::Simulated => "SIMULATED",
            Self::RuntimeReproduced => "RUNTIME_REPRODUCED",
            Self::RuntimeNotReproduced => "RUNTIME_NOT_REPRODUCED",
            Self::Inconclusive => "INCONCLUSIVE",
        }
    }

    /// Whether the level rests on executing the application.
    pub fn is_runtime(self) -> bool {
        matches!(
            self,
            Self::RuntimeReproduced | Self::RuntimeNotReproduced | Self::Inconclusive
        )
    }
}

impl fmt::Display for VerificationLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a failure is injected at a checkpoint. The modes are not
/// interchangeable: an exception runs `catch`/`finally` blocks and
/// transaction rollbacks in the application, process termination runs
/// nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureMode {
    /// No injection: the control run.
    None,
    /// The checkpoint throws (in an async function: the promise rejects).
    Exception,
    /// The supervisor kills the process (SIGKILL) while it waits at the
    /// checkpoint. No application code runs afterwards.
    ProcessKill,
}

impl FailureMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Exception => "exception",
            Self::ProcessKill => "processKill",
        }
    }
}

impl fmt::Display for FailureMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Bumped on any change to the verification report JSON. Version 2 (Phase
/// 4) only adds optional fields (`environment.isolation`, `cases[].key`,
/// `cases[].recoveryObservation`, `runs[].injectedFaultSurfaced`); version 1
/// reports still load.
pub const VERIFICATION_SCHEMA_VERSION: u32 = 2;
pub const VERIFICATION_REPORT_KIND: &str = "commitrift.verification";

/// The result of `commitrift replay`: what was executed, under which
/// conditions, and what was observed. Durations are the only values that
/// change between identical runs; they never enter an identifier.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationReport {
    pub schema_version: u32,
    /// Always `commitrift.verification`.
    pub kind: String,
    pub tool: crate::trace::ToolStamp,
    pub harness: HarnessInfo,
    pub source: SourceInfo,
    pub environment: EnvironmentInfo,
    /// The run without injection that establishes that the declared
    /// invariants hold when nothing fails. A failing control run makes every
    /// case inconclusive.
    pub control: Option<RunRecord>,
    pub cases: Vec<CaseResult>,
    pub summary: VerificationSummary,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessInfo {
    pub id: String,
    pub description: String,
    /// Path of the manifest as given on the command line.
    pub manifest: String,
    /// Hash of the manifest bytes (FNV-1a 64), to tie the report to the
    /// exact declarations that were executed.
    pub manifest_hash: String,
    pub command: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    /// Git revision of the harness directory, when it is in a repository
    /// with at least one commit.
    pub revision: Option<String>,
    /// Whether the harness directory has uncommitted changes.
    pub dirty: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentInfo {
    pub os: String,
    pub arch: String,
    pub postgres: Option<String>,
    pub runtime: Option<String>,
    /// How external network access of the harness was restricted.
    pub network_isolation: String,
    /// The isolation backend: `sandbox-exec`, `bubblewrap` or `none`
    /// (schema version 2; absent in version 1 reports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<String>,
    /// What database the harness was given.
    pub database: String,
    /// Names (never values) of the environment variables passed to the
    /// harness.
    pub environment_variables: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaseResult {
    pub id: String,
    /// Stable case identity across runs and revisions: `<harness>/<case>`
    /// (schema version 2; absent in version 1 reports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub description: String,
    pub static_link: StaticLink,
    /// The finding the case targets (diagnostic fingerprint).
    pub finding_id: Option<String>,
    pub trace_id: Option<String>,
    pub code: Option<String>,
    /// The static trace of the finding, embedded for the viewer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<crate::trace::FailureTrace>,
    pub failure_mode: FailureMode,
    pub checkpoint: Option<CheckpointSelector>,
    /// Where the selected checkpoint lies relative to the trace's boundary.
    pub boundary: BoundaryRelation,
    /// How every run's initial state was prepared.
    pub initial_state: String,
    pub runs: Vec<RunRecord>,
    pub verification: VerificationLevel,
    /// Every run gave the same verification level.
    pub consistent: bool,
    /// The level the harness author declared as expected, if any.
    pub expected: Option<VerificationLevel>,
    pub matches_expectation: Option<bool>,
    /// What the case's recovery scenarios were observed to do (schema
    /// version 2; absent without recovery scenarios and in version 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_observation: Option<RecoveryObservation>,
    pub limitations: Vec<String>,
}

/// The observed effect of a case's recovery scenarios (a relay, a retry).
/// Provenance `runtimeObservation`: established by executing this harness,
/// for this harness and environment only. It never changes the case's
/// verification level: a window that was reproduced stays reproduced even
/// when the recovery repaired the state afterwards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryObservation {
    /// Always `runtimeObservation`.
    pub source: String,
    pub scenarios: Vec<String>,
    /// Runs that executed the recovery scenarios.
    pub runs: usize,
    /// Runs in which a declared invariant was violated after the failure
    /// and every invariant held after recovery.
    pub restored_runs: usize,
    /// Runs in which every invariant held after recovery.
    pub held_after_recovery_runs: usize,
    pub summary: String,
}

impl RecoveryObservation {
    /// Derive the observation from the runs of a case with recovery
    /// scenarios.
    pub fn of(scenarios: &[String], runs: &[RunRecord]) -> Option<Self> {
        if scenarios.is_empty() || runs.is_empty() {
            return None;
        }
        let holds = |run: &RunRecord, stage: &str| {
            let checks: Vec<&InvariantResult> =
                run.invariants.iter().filter(|i| i.stage == stage).collect();
            !checks.is_empty() && checks.iter().all(|i| i.holds)
        };
        let violated = |run: &RunRecord, stage: &str| {
            run.invariants.iter().any(|i| i.stage == stage && !i.holds)
        };
        let executed: Vec<&RunRecord> = runs
            .iter()
            .filter(|r| r.observations.contains_key("afterRecovery"))
            .collect();
        let held = executed
            .iter()
            .filter(|r| holds(r, "afterRecovery"))
            .count();
        let restored = executed
            .iter()
            .filter(|r| violated(r, "after") && holds(r, "afterRecovery"))
            .count();
        let summary = if executed.is_empty() {
            "The recovery scenarios did not run (the case ended before them).".to_owned()
        } else {
            format!(
                "Observed at runtime: after {} every declared invariant held in {held} of {} run(s); in {restored} of them an invariant had been violated before recovery. This harness and environment only.",
                scenarios.join(", "),
                executed.len()
            )
        };
        Some(Self {
            source: "runtimeObservation".to_owned(),
            scenarios: scenarios.to_vec(),
            runs: executed.len(),
            restored_runs: restored,
            held_after_recovery_runs: held,
            summary,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticLink {
    pub status: StaticLinkStatus,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StaticLinkStatus {
    /// The case's finding selector matched exactly one finding with a trace.
    Matched,
    /// No finding matches: the static analysis does not report the window
    /// this case exercises.
    NotFound,
    /// Several findings match the selector.
    Ambiguous,
    /// The case names no finding (a negative or exploratory case).
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BoundaryRelation {
    /// The checkpoint is one of the trace's boundary checkpoints.
    OnTraceBoundary,
    /// The checkpoint is not adjacent to the trace's boundary (for example
    /// a negative case before the first commit).
    OutsideTraceBoundary,
    /// There is no trace or no injection.
    NotApplicable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointSelector {
    pub name: String,
    /// Which occurrence of the checkpoint (1 = the first).
    pub occurrence: u32,
    /// When set, only checkpoints with this label count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// One execution of the harness from a fresh initial state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub run: u32,
    pub scenario: String,
    pub failure_mode: FailureMode,
    /// The failure was actually injected at the selected checkpoint.
    pub injected: bool,
    /// Every checkpoint the harness reported, in order, with the
    /// supervisor's answer.
    pub checkpoints: Vec<CheckpointEvent>,
    /// Checks run against the database while the harness was paused at the
    /// selected checkpoint, before injecting.
    pub pre_injection_checks: Vec<ConditionResult>,
    pub exit: ExitInfo,
    /// `returned`, `threw`, `killed`, `timedOut`, `notStarted`, `crashed`.
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Exception mode: whether the scenario ended with the injected fault
    /// (possibly wrapped by a library), as the harness reported it. `false`
    /// with outcome `threw` means the application turned it into another
    /// error; outcome `returned` means it handled the fault. Absent for other
    /// modes and in schema version 1 reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub injected_fault_surfaced: Option<bool>,
    /// Observations by stage (`before`, `afterFailure`, `afterRecovery`).
    pub observations:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, serde_json::Value>>,
    pub invariants: Vec<InvariantResult>,
    pub prediction: Vec<PredictionResult>,
    pub recovery: Vec<RecoveryRecord>,
    /// Wall-clock duration (volatile; not part of any identity).
    pub duration_ms: u64,
    pub verification: VerificationLevel,
    pub reason: String,
    pub logs: Logs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointEvent {
    pub seq: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// `continue`, `throw` or `kill`.
    pub action: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExitInfo {
    /// `exited`, `signaled`, `timedOut` or `notStarted`.
    pub kind: String,
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConditionResult {
    pub description: String,
    pub holds: bool,
    pub observed: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvariantResult {
    pub id: String,
    pub stage: String,
    pub holds: bool,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PredictionResult {
    pub observation: String,
    pub expected: serde_json::Value,
    pub observed: serde_json::Value,
    pub matches: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryRecord {
    pub scenario: String,
    pub outcome: String,
    pub exit: ExitInfo,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Logs {
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationSummary {
    pub cases: usize,
    pub reproduced: usize,
    pub not_reproduced: usize,
    pub inconclusive: usize,
    /// Cases whose level matches the level the harness declares as expected.
    pub matched_expectations: usize,
    pub mismatched_expectations: usize,
}

impl VerificationSummary {
    pub fn of(cases: &[CaseResult]) -> Self {
        let count = |level| cases.iter().filter(|c| c.verification == level).count();
        Self {
            cases: cases.len(),
            reproduced: count(VerificationLevel::RuntimeReproduced),
            not_reproduced: count(VerificationLevel::RuntimeNotReproduced),
            inconclusive: count(VerificationLevel::Inconclusive),
            matched_expectations: cases
                .iter()
                .filter(|c| c.matches_expectation == Some(true))
                .count(),
            mismatched_expectations: cases
                .iter()
                .filter(|c| c.matches_expectation == Some(false))
                .count(),
        }
    }
}

impl CaseResult {
    /// The stable identity of the case: `key`, or `<harness>/<id>` for
    /// version 1 reports.
    pub fn stable_key(&self, harness: &str) -> String {
        self.key
            .clone()
            .unwrap_or_else(|| format!("{harness}/{}", self.id))
    }
}

pub const REGRESSION_REPORT_KIND: &str = "commitrift.regression";
pub const REGRESSION_SCHEMA_VERSION: u32 = 1;

/// How a case's runtime result compares with a baseline verification report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RegressionStatus {
    /// Reproduced before and now.
    StillReproduced,
    /// Reproduced before, not now (a fix, or a harness change): review and
    /// update the baseline.
    NoLongerReproduced,
    /// Not reproduced before, reproduced now: a regression.
    NewlyReproduced,
    /// Not reproduced before and now.
    StillNotReproduced,
    /// The current run is inconclusive. Never a pass.
    Inconclusive,
    /// In the baseline, but not executed now. Never a pass.
    NotExecuted,
    /// Executed now, not in the baseline (or inconclusive there).
    NotInBaseline,
}

impl RegressionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StillReproduced => "stillReproduced",
            Self::NoLongerReproduced => "noLongerReproduced",
            Self::NewlyReproduced => "newlyReproduced",
            Self::StillNotReproduced => "stillNotReproduced",
            Self::Inconclusive => "inconclusive",
            Self::NotExecuted => "notExecuted",
            Self::NotInBaseline => "notInBaseline",
        }
    }

    /// A change of runtime outcome that needs review.
    pub fn is_change(self) -> bool {
        matches!(self, Self::NoLongerReproduced | Self::NewlyReproduced)
    }

    /// The comparison could not be made.
    pub fn is_unverified(self) -> bool {
        matches!(self, Self::Inconclusive | Self::NotExecuted)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegressionEntry {
    /// `<harness>/<case>`.
    pub key: String,
    pub finding_id: Option<String>,
    pub trace_id: Option<String>,
    pub code: Option<String>,
    pub failure_mode: FailureMode,
    pub checkpoint: Option<CheckpointSelector>,
    pub baseline: Option<VerificationLevel>,
    pub current: Option<VerificationLevel>,
    pub status: RegressionStatus,
    /// Runs of the current case that reproduced, out of `runs`.
    pub reproduced_runs: usize,
    pub runs: usize,
    /// Invariants violated in the current runs (ids, deduplicated).
    pub violated_invariants: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegressionSummary {
    pub cases: usize,
    pub still_reproduced: usize,
    pub no_longer_reproduced: usize,
    pub newly_reproduced: usize,
    pub still_not_reproduced: usize,
    pub inconclusive: usize,
    pub not_executed: usize,
    pub not_in_baseline: usize,
}

/// The comparison of a verification report with a baseline report
/// (`commitrift replay --baseline`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegressionReport {
    pub schema_version: u32,
    /// Always `commitrift.regression`.
    pub kind: String,
    pub tool: crate::trace::ToolStamp,
    pub harness: String,
    pub baseline_manifest_hash: String,
    pub current_manifest_hash: String,
    pub source: SourceInfo,
    pub environment: EnvironmentInfo,
    pub entries: Vec<RegressionEntry>,
    pub summary: RegressionSummary,
}

/// Compare `current` with `baseline` case by case (by stable key).
pub fn compare_with_baseline(
    baseline: &VerificationReport,
    current: &VerificationReport,
) -> RegressionReport {
    use std::collections::BTreeMap;
    let previous: BTreeMap<String, &CaseResult> = baseline
        .cases
        .iter()
        .map(|c| (c.stable_key(&baseline.harness.id), c))
        .collect();
    let mut entries: Vec<RegressionEntry> = Vec::new();
    for case in &current.cases {
        let key = case.stable_key(&current.harness.id);
        let before = previous
            .get(&key)
            .map(|c| c.verification)
            .filter(|level| level.is_runtime());
        let now = case.verification;
        let status = match (before, now) {
            (_, VerificationLevel::Inconclusive) => RegressionStatus::Inconclusive,
            (Some(VerificationLevel::RuntimeReproduced), VerificationLevel::RuntimeReproduced) => {
                RegressionStatus::StillReproduced
            }
            (
                Some(VerificationLevel::RuntimeReproduced),
                VerificationLevel::RuntimeNotReproduced,
            ) => RegressionStatus::NoLongerReproduced,
            (
                Some(VerificationLevel::RuntimeNotReproduced),
                VerificationLevel::RuntimeReproduced,
            ) => RegressionStatus::NewlyReproduced,
            (
                Some(VerificationLevel::RuntimeNotReproduced),
                VerificationLevel::RuntimeNotReproduced,
            ) => RegressionStatus::StillNotReproduced,
            _ => RegressionStatus::NotInBaseline,
        };
        let mut violated: Vec<String> = case
            .runs
            .iter()
            .flat_map(|r| {
                r.invariants
                    .iter()
                    .filter(|i| !i.holds)
                    .map(|i| i.id.clone())
            })
            .collect();
        violated.sort();
        violated.dedup();
        entries.push(RegressionEntry {
            key,
            finding_id: case.finding_id.clone(),
            trace_id: case.trace_id.clone(),
            code: case.code.clone(),
            failure_mode: case.failure_mode,
            checkpoint: case.checkpoint.clone(),
            baseline: previous
                .get(&case.stable_key(&current.harness.id))
                .map(|c| c.verification),
            current: Some(now),
            status,
            reproduced_runs: case
                .runs
                .iter()
                .filter(|r| r.verification == VerificationLevel::RuntimeReproduced)
                .count(),
            runs: case.runs.len(),
            violated_invariants: violated,
        });
    }
    let executed: std::collections::BTreeSet<String> =
        entries.iter().map(|e| e.key.clone()).collect();
    for (key, case) in &previous {
        if !executed.contains(key) {
            entries.push(RegressionEntry {
                key: key.clone(),
                finding_id: case.finding_id.clone(),
                trace_id: case.trace_id.clone(),
                code: case.code.clone(),
                failure_mode: case.failure_mode,
                checkpoint: case.checkpoint.clone(),
                baseline: Some(case.verification),
                current: None,
                status: RegressionStatus::NotExecuted,
                reproduced_runs: 0,
                runs: 0,
                violated_invariants: Vec::new(),
            });
        }
    }
    let count = |status| entries.iter().filter(|e| e.status == status).count();
    let summary = RegressionSummary {
        cases: entries.len(),
        still_reproduced: count(RegressionStatus::StillReproduced),
        no_longer_reproduced: count(RegressionStatus::NoLongerReproduced),
        newly_reproduced: count(RegressionStatus::NewlyReproduced),
        still_not_reproduced: count(RegressionStatus::StillNotReproduced),
        inconclusive: count(RegressionStatus::Inconclusive),
        not_executed: count(RegressionStatus::NotExecuted),
        not_in_baseline: count(RegressionStatus::NotInBaseline),
    };
    RegressionReport {
        schema_version: REGRESSION_SCHEMA_VERSION,
        kind: REGRESSION_REPORT_KIND.to_owned(),
        tool: crate::trace::ToolStamp::default(),
        harness: current.harness.id.clone(),
        baseline_manifest_hash: baseline.harness.manifest_hash.clone(),
        current_manifest_hash: current.harness.manifest_hash.clone(),
        source: current.source.clone(),
        environment: current.environment.clone(),
        entries,
        summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(cases: &[(&str, VerificationLevel)]) -> VerificationReport {
        VerificationReport {
            schema_version: VERIFICATION_SCHEMA_VERSION,
            kind: VERIFICATION_REPORT_KIND.to_owned(),
            tool: crate::trace::ToolStamp::default(),
            harness: HarnessInfo {
                id: "h".to_owned(),
                description: String::new(),
                manifest: String::new(),
                manifest_hash: "0".to_owned(),
                command: Vec::new(),
            },
            source: SourceInfo::default(),
            environment: EnvironmentInfo::default(),
            control: None,
            cases: cases
                .iter()
                .map(|(id, level)| CaseResult {
                    id: (*id).to_owned(),
                    key: None,
                    description: String::new(),
                    static_link: StaticLink {
                        status: StaticLinkStatus::None,
                        detail: String::new(),
                    },
                    finding_id: None,
                    trace_id: None,
                    code: None,
                    trace: None,
                    failure_mode: FailureMode::ProcessKill,
                    checkpoint: None,
                    boundary: BoundaryRelation::NotApplicable,
                    initial_state: String::new(),
                    runs: Vec::new(),
                    verification: *level,
                    consistent: true,
                    expected: None,
                    matches_expectation: None,
                    recovery_observation: None,
                    limitations: Vec::new(),
                })
                .collect(),
            summary: VerificationSummary::default(),
            limitations: Vec::new(),
        }
    }

    #[test]
    fn regressions_distinguish_every_transition_and_never_pass_inconclusive_results() {
        use VerificationLevel::*;
        let baseline = report(&[
            ("a", RuntimeReproduced),
            ("b", RuntimeReproduced),
            ("c", RuntimeNotReproduced),
            ("d", RuntimeNotReproduced),
            ("e", RuntimeReproduced),
            ("f", RuntimeReproduced),
            ("g", Inconclusive),
        ]);
        let current = report(&[
            ("a", RuntimeReproduced),
            ("b", RuntimeNotReproduced),
            ("c", RuntimeReproduced),
            ("d", RuntimeNotReproduced),
            ("e", Inconclusive),
            ("g", RuntimeReproduced),
            ("new", RuntimeNotReproduced),
        ]);
        let result = compare_with_baseline(&baseline, &current);
        let status = |key: &str| {
            result
                .entries
                .iter()
                .find(|e| e.key == format!("h/{key}"))
                .unwrap()
                .status
        };
        assert_eq!(status("a"), RegressionStatus::StillReproduced);
        assert_eq!(status("b"), RegressionStatus::NoLongerReproduced);
        assert_eq!(status("c"), RegressionStatus::NewlyReproduced);
        assert_eq!(status("d"), RegressionStatus::StillNotReproduced);
        assert_eq!(status("e"), RegressionStatus::Inconclusive);
        assert_eq!(status("f"), RegressionStatus::NotExecuted);
        assert_eq!(status("g"), RegressionStatus::NotInBaseline);
        assert_eq!(status("new"), RegressionStatus::NotInBaseline);
        assert!(status("e").is_unverified() && status("f").is_unverified());
        assert!(status("b").is_change() && status("c").is_change());
        assert_eq!(result.summary.cases, 8);
        assert_eq!(result.kind, REGRESSION_REPORT_KIND);
    }

    #[test]
    fn levels_serialize_as_documented() {
        for (level, text) in [
            (VerificationLevel::StaticCandidate, "\"STATIC_CANDIDATE\""),
            (VerificationLevel::Simulated, "\"SIMULATED\""),
            (
                VerificationLevel::RuntimeReproduced,
                "\"RUNTIME_REPRODUCED\"",
            ),
            (
                VerificationLevel::RuntimeNotReproduced,
                "\"RUNTIME_NOT_REPRODUCED\"",
            ),
            (VerificationLevel::Inconclusive, "\"INCONCLUSIVE\""),
        ] {
            assert_eq!(serde_json::to_string(&level).unwrap(), text);
            assert_eq!(format!("\"{level}\""), text);
            assert_eq!(
                serde_json::from_str::<VerificationLevel>(text).unwrap(),
                level
            );
        }
        assert!(!VerificationLevel::Simulated.is_runtime());
        assert!(!VerificationLevel::StaticCandidate.is_runtime());
        assert!(VerificationLevel::RuntimeReproduced.is_runtime());
    }
}

//! The Failure Trace IR: a verification-oriented, serializable description
//! of one candidate execution path behind a finding.
//!
//! The Effect IR ([`crate::ir`]) describes what a function *may* do. A
//! [`FailureTrace`] fixes one path through it (the one the diagnostic
//! reports), lists every step on that path in order, and marks the
//! **injection boundary**: the point after which a failure leaves the state
//! the diagnostic describes. It is the input of runtime replay
//! (`midwrite replay`) and of the HTML viewer.
//!
//! A trace is a *static candidate* ([`VerificationLevel::StaticCandidate`]).
//! Its [`Simulation`] runs the analysis' own transaction model over the
//! steps (what survives a process crash at the boundary); that is labeled
//! [`VerificationLevel::Simulated`] and is not evidence that the
//! application behaves that way. See `docs/reference/trace-format.md`.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::diagnostic::stable_hash;
use crate::location::SourceLocation;
use crate::verification::{FailureMode, VerificationLevel};

/// Bumped on any incompatible change to the trace JSON.
pub const TRACE_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureTrace {
    /// Stable identifier: a hash of the finding id, the boundary and the
    /// kinds and labels of the steps (no line numbers).
    pub id: String,
    /// The fingerprint of the diagnostic the trace explains.
    pub finding_id: String,
    pub code: String,
    pub function: String,
    /// Where the diagnostic is reported.
    pub location: SourceLocation,
    /// Always [`VerificationLevel::StaticCandidate`] for a trace produced by
    /// the analysis.
    pub verification: VerificationLevel,
    pub completeness: Completeness,
    pub steps: Vec<TraceStep>,
    pub injection: InjectionPoint,
    pub simulation: Simulation,
    /// What must hold for the trace to describe a real execution.
    pub assumptions: Vec<String>,
    pub limitations: Vec<String>,
    /// How the operations before and after the boundary are related by
    /// value provenance (schema v2; absent in v1 reports). Descriptive only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_relation: Option<crate::absint::ValueRelation>,
}

/// Whether every operation the trace relies on was understood.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Completeness {
    /// Every step in the failure window is a recognized operation or a
    /// branch decision, and the operations are ordered.
    Complete,
    /// The order of operations or the failure point is approximated
    /// (concurrent or unawaited operations, an approximated catch entry).
    Approximated,
    /// The window contains operations with unknown semantics, or the path
    /// reaches unsupported control flow. Not a basis for a reproduction plan.
    Incomplete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepKind {
    DbRead,
    DbWrite,
    TxBegin,
    TxCommit,
    TxRollback,
    External,
    Unknown,
    Branch,
    /// A transaction callback returns a value the analysis correlates with
    /// later branches.
    Return,
    /// Control flow the analysis does not model; the path stops here.
    Unsupported,
}

impl StepKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DbRead => "dbRead",
            Self::DbWrite => "dbWrite",
            Self::TxBegin => "txBegin",
            Self::TxCommit => "txCommit",
            Self::TxRollback => "txRollback",
            Self::External => "external",
            Self::Unknown => "unknown",
            Self::Branch => "branch",
            Self::Return => "return",
            Self::Unsupported => "unsupported",
        }
    }

    /// Whether the step itself changes state outside the process.
    pub fn is_effect(self) -> bool {
        matches!(self, Self::DbWrite | Self::External | Self::Unknown)
    }
}

/// The part a step plays in the finding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepRole {
    /// Becomes durable or visible before the boundary.
    CompletedFirst,
    /// Has not happened at the boundary.
    NotYetPerformed,
    /// A write staged in a transaction that is open at the boundary.
    StagedWrite,
    /// The transaction whose commit or rollback matters.
    Transaction,
    /// Lies strictly inside the failure window.
    InWindow,
}

/// The state of a step when the failure is injected at the boundary,
/// according to the abstract model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepState {
    /// A branch decision that was taken.
    Taken,
    /// Executed; no durable effect of its own (read, begin, return, or an
    /// unknown operation whose effect is unknown).
    Completed,
    /// A database write (or a transaction) that has committed.
    Committed,
    /// A database write staged in a transaction still open at the boundary.
    Staged,
    /// A write discarded by a rollback before the boundary.
    RolledBack,
    /// An external effect that was performed.
    Performed,
    /// After the boundary: not executed.
    NotReached,
}

/// One call through which an inlined operation is reached.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallFrame {
    pub callee: String,
    /// The call as written, such as `notify(id)`.
    pub label: String,
    pub location: SourceLocation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceStep {
    /// Position on the path, from 0.
    pub index: usize,
    pub kind: StepKind,
    /// Source-derived label (`db.update(orders)`, `tx#0`, a condition).
    pub label: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<SourceLocation>,
    /// Calls through which an inlined operation is reached, outermost first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub call_chain: Vec<CallFrame>,
    /// For transaction steps: the transaction. For a write: the transaction
    /// whose handle it is issued on (it is staged only while that
    /// transaction is open).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction: Option<String>,
    /// For a nested transaction (savepoint): its parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_transaction: Option<String>,
    /// The database handle an operation is issued on, as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    /// External effect category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub best_effort: bool,
    /// `awaited`, `returned`, `fireAndForget` or `concurrent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<StepRole>,
    pub at_failure: StepState,
}

/// The kind of failure window, by diagnostic code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BoundaryKind {
    /// MW1001: after a commit, before an external effect.
    AfterCommitBeforeExternal,
    /// MW1002: after an external effect, before a commit.
    AfterExternalBeforeCommit,
    /// MW1003: after an external effect, while the transaction with staged
    /// writes is still open.
    AfterExternalInsideTransaction,
    /// MW1004: between two independent commits.
    BetweenCommits,
    /// MW1005: between two independent external effects.
    BetweenExternalEffects,
}

/// A named runtime checkpoint at which a harness can inject the failure.
/// Names follow the replay protocol (`docs/architecture/replay-protocol.md`);
/// the harness, not the analysis, decides where its checkpoints are.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointHint {
    /// `after-db-commit`, `before-event-publish`, ...
    pub name: String,
    /// The step the checkpoint is adjacent to.
    pub step: usize,
    /// A resource name that identifies the operation (a table, a label).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InjectionPoint {
    pub boundary: BoundaryKind,
    /// The last step completed when the failure happens.
    pub after_step: usize,
    /// The operation that has not happened yet (`None` when the path ends
    /// before it, which only happens for MW1003 on a rollback path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_step: Option<usize>,
    /// Steps strictly between the two: a failure anywhere among them leaves
    /// the same partial state unless one of them is itself an effect.
    pub window: Vec<usize>,
    pub description: String,
    /// The checkpoint right after `after_step` and the one right before
    /// `before_step`; injecting at either of them targets this boundary.
    pub checkpoints: Vec<CheckpointHint>,
    /// Transactions open at the boundary.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_transactions: Vec<String>,
}

/// The abstract model's prediction of what survives a process crash at the
/// boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Simulation {
    /// Always [`VerificationLevel::Simulated`].
    pub verification: VerificationLevel,
    pub failure_mode: FailureMode,
    /// Steps whose effects are durable or visible (committed writes,
    /// performed external effects).
    pub durable: Vec<usize>,
    /// Writes staged in a transaction open at the boundary: rolled back when
    /// the process dies.
    pub lost: Vec<usize>,
    /// Effects after the boundary that are not performed.
    pub not_performed: Vec<usize>,
    /// The predicted surviving state in one sentence.
    pub claim: String,
    /// What the model does not capture.
    pub not_modeled: Vec<String>,
}

fn root_of<'a>(parents: &BTreeMap<&'a str, Option<&'a str>>, mut tx: &'a str) -> &'a str {
    for _ in 0..64 {
        match parents.get(tx) {
            Some(Some(parent)) => tx = parent,
            _ => break,
        }
    }
    tx
}

/// Whether `tx` is `ancestor` or nested (transitively) inside it.
fn within<'a>(
    parents: &BTreeMap<&'a str, Option<&'a str>>,
    mut tx: &'a str,
    ancestor: &str,
) -> bool {
    for _ in 0..64 {
        if tx == ancestor {
            return true;
        }
        match parents.get(tx) {
            Some(Some(parent)) => tx = parent,
            _ => return false,
        }
    }
    false
}

/// Run the abstract transaction model over `steps` with a process crash
/// right after `after_step`. Returns the state of each step and the
/// prediction.
///
/// Rules: a write commits immediately unless it is issued on the handle of
/// a transaction open at that point, in which case it is staged in that
/// transaction's outermost (root) transaction. A root transaction's commit
/// makes its staged writes durable; a rollback discards the writes of the
/// transaction and the transactions nested in it. Everything still staged
/// at the crash is lost. External effects are performed when their step
/// completes.
pub fn simulate_crash(steps: &[TraceStep], after_step: usize) -> (Vec<StepState>, Simulation) {
    let mut states = vec![StepState::NotReached; steps.len()];
    let mut parents: BTreeMap<&str, Option<&str>> = BTreeMap::new();
    let mut open: Vec<&str> = Vec::new();
    // Staged writes by root transaction: (issuing transaction, step).
    let mut staged: BTreeMap<&str, Vec<(&str, usize)>> = BTreeMap::new();
    for (index, step) in steps.iter().enumerate().take(after_step.saturating_add(1)) {
        states[index] = match step.kind {
            StepKind::Branch => StepState::Taken,
            StepKind::DbRead | StepKind::Return | StepKind::Unknown | StepKind::Unsupported => {
                StepState::Completed
            }
            StepKind::TxBegin => {
                if let Some(tx) = step.transaction.as_deref() {
                    parents.insert(tx, step.parent_transaction.as_deref());
                    open.push(tx);
                }
                StepState::Completed
            }
            StepKind::DbWrite => match step.transaction.as_deref() {
                Some(tx) if open.contains(&tx) => {
                    staged
                        .entry(root_of(&parents, tx))
                        .or_default()
                        .push((tx, index));
                    StepState::Staged
                }
                _ => StepState::Committed,
            },
            StepKind::TxCommit | StepKind::TxRollback => {
                let committed = step.kind == StepKind::TxCommit;
                let Some(tx) = step.transaction.as_deref() else {
                    continue;
                };
                if let Some(position) = open.iter().rposition(|open| *open == tx) {
                    open.truncate(position);
                }
                let is_root = !matches!(parents.get(tx), Some(Some(_)));
                if is_root {
                    for (_, write) in staged.remove(tx).unwrap_or_default() {
                        states[write] = if committed {
                            StepState::Committed
                        } else {
                            StepState::RolledBack
                        };
                    }
                } else if !committed {
                    let root = root_of(&parents, tx);
                    if let Some(writes) = staged.get_mut(root) {
                        writes.retain(|(issuer, write)| {
                            let discard = within(&parents, issuer, tx);
                            if discard {
                                states[*write] = StepState::RolledBack;
                            }
                            !discard
                        });
                    }
                }
                if committed {
                    StepState::Committed
                } else {
                    StepState::RolledBack
                }
            }
            StepKind::External => StepState::Performed,
        };
    }

    let durable: Vec<usize> = (0..steps.len())
        .filter(|&i| {
            matches!(
                (steps[i].kind, states[i]),
                (StepKind::DbWrite, StepState::Committed)
                    | (StepKind::External, StepState::Performed)
            )
        })
        .collect();
    let lost: Vec<usize> = (0..steps.len())
        .filter(|&i| states[i] == StepState::Staged)
        .collect();
    let not_performed: Vec<usize> = (0..steps.len())
        .filter(|&i| {
            states[i] == StepState::NotReached
                && matches!(
                    steps[i].kind,
                    StepKind::DbWrite | StepKind::External | StepKind::TxCommit
                )
        })
        .collect();
    let names = |indices: &[usize]| {
        if indices.is_empty() {
            "nothing".to_owned()
        } else {
            indices
                .iter()
                .map(|&i| format!("`{}` (step {i})", steps[i].label))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    let mut claim = format!(
        "If the process dies right after step {after_step}: durable or visible: {}; lost with the open transaction: {}; never performed: {}.",
        names(&durable),
        names(&lost),
        names(&not_performed)
    );
    if lost.is_empty() {
        claim = format!(
            "If the process dies right after step {after_step}: durable or visible: {}; never performed: {}.",
            names(&durable),
            names(&not_performed)
        );
    }
    let simulation = Simulation {
        verification: VerificationLevel::Simulated,
        failure_mode: FailureMode::ProcessKill,
        durable,
        lost,
        not_performed,
        claim,
        not_modeled: vec![
            "Exceptions: an injected exception runs catch/finally handlers and transaction rollbacks in the application, which the model does not execute; only a process crash is simulated.".to_owned(),
            "Effects of unknown operations, and the database's own failure modes (failed commits, isolation anomalies).".to_owned(),
            "Anything outside the analyzed function: retries, reconciliation jobs, relays, consumers.".to_owned(),
        ],
    };
    (states, simulation)
}

/// The output of `midwrite trace`: one trace per finding with a failure
/// boundary, and the findings that have none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceReport {
    pub schema_version: u32,
    /// Always `midwrite.traces`.
    pub kind: String,
    pub tool: ToolStamp,
    pub config: Option<String>,
    pub summary: TraceSummary,
    pub traces: Vec<FailureTrace>,
    pub skipped: Vec<SkippedFinding>,
}

pub const TRACE_REPORT_KIND: &str = "midwrite.traces";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStamp {
    pub name: String,
    pub version: String,
}

impl Default for ToolStamp {
    fn default() -> Self {
        Self {
            name: "midwrite".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceSummary {
    /// Diagnostics considered.
    pub findings: usize,
    pub traces: usize,
    pub complete: usize,
    pub approximated: usize,
    pub incomplete: usize,
    pub skipped: usize,
}

/// A diagnostic without a trace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedFinding {
    pub finding_id: String,
    pub code: String,
    pub function: String,
    pub location: SourceLocation,
    pub reason: String,
}

impl TraceReport {
    pub fn new(
        config: Option<String>,
        traces: Vec<FailureTrace>,
        skipped: Vec<SkippedFinding>,
    ) -> Self {
        let count = |c: Completeness| traces.iter().filter(|t| t.completeness == c).count();
        let summary = TraceSummary {
            findings: traces.len() + skipped.len(),
            traces: traces.len(),
            complete: count(Completeness::Complete),
            approximated: count(Completeness::Approximated),
            incomplete: count(Completeness::Incomplete),
            skipped: skipped.len(),
        };
        Self {
            schema_version: TRACE_SCHEMA_VERSION,
            kind: TRACE_REPORT_KIND.to_owned(),
            tool: ToolStamp::default(),
            config,
            summary,
            traces,
            skipped,
        }
    }
}

impl FailureTrace {
    /// Recompute [`Self::id`] from the finding id, the boundary and the
    /// steps (kinds and labels only, so that moving code keeps the id).
    pub fn compute_id(&self) -> String {
        let mut parts: Vec<String> = vec![
            "trace/v1".to_owned(),
            self.finding_id.clone(),
            format!("{:?}", self.injection.boundary),
            self.injection.after_step.to_string(),
            self.injection
                .before_step
                .map_or_else(String::new, |s| s.to_string()),
        ];
        for step in &self.steps {
            parts.push(format!("{}={}", step.kind.as_str(), step.label));
        }
        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        format!("tr-{:016x}", stable_hash(&parts))
    }

    /// Attach the trace to a finding id and recompute its own id.
    pub fn assign_finding(&mut self, finding_id: &str) {
        self.finding_id = finding_id.to_owned();
        self.id = self.compute_id();
    }

    /// Structural checks against the analysis model. An empty result means
    /// the trace is internally consistent: indices are in range and ordered,
    /// the simulation agrees with the step states, and the operation the
    /// finding says completes first is durable while the one it says has
    /// not happened is not performed.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for (position, step) in self.steps.iter().enumerate() {
            if step.index != position {
                problems.push(format!("step {position} has index {}", step.index));
            }
        }
        let len = self.steps.len();
        let after = self.injection.after_step;
        if after >= len {
            problems.push(format!("afterStep {after} is out of range ({len} steps)"));
            return problems;
        }
        if let Some(before) = self.injection.before_step {
            if before >= len {
                problems.push(format!("beforeStep {before} is out of range ({len} steps)"));
                return problems;
            }
            if before <= after {
                problems.push(format!(
                    "beforeStep {before} does not follow afterStep {after}"
                ));
            }
        }
        if self
            .injection
            .window
            .iter()
            .any(|&w| w <= after || self.injection.before_step.is_some_and(|b| w >= b))
        {
            problems.push("window steps lie outside the boundary".to_owned());
        }
        let (states, simulation) = simulate_crash(&self.steps, after);
        for (step, state) in self.steps.iter().zip(&states) {
            if step.at_failure != *state {
                problems.push(format!(
                    "step {} is {:?} but the model gives {:?}",
                    step.index, step.at_failure, state
                ));
            }
        }
        if simulation.durable != self.simulation.durable
            || simulation.lost != self.simulation.lost
            || simulation.not_performed != self.simulation.not_performed
        {
            problems.push("the simulation does not match the steps".to_owned());
        }
        let first = &self.steps[after];
        let first_ok = match self.injection.boundary {
            BoundaryKind::AfterCommitBeforeExternal | BoundaryKind::BetweenCommits => {
                states[after] == StepState::Committed
            }
            BoundaryKind::AfterExternalBeforeCommit
            | BoundaryKind::AfterExternalInsideTransaction
            | BoundaryKind::BetweenExternalEffects => {
                first.kind == StepKind::External && states[after] == StepState::Performed
            }
        };
        if !first_ok {
            problems.push(format!(
                "step {after} (`{}`) is not durable at the boundary",
                first.label
            ));
        }
        match (self.injection.boundary, self.injection.before_step) {
            (BoundaryKind::AfterExternalInsideTransaction, _) => {
                if simulation.lost.is_empty() {
                    problems.push("no staged write is lost at the boundary".to_owned());
                }
            }
            (_, Some(before)) => {
                if !simulation.not_performed.contains(&before) {
                    problems.push(format!(
                        "step {before} (`{}`) is not after the boundary",
                        self.steps[before].label
                    ));
                }
            }
            (_, None) => problems.push("no operation follows the boundary".to_owned()),
        }
        problems
    }

    /// Human-readable rendering for `midwrite trace`.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{} {} {} in `{}` ({}) [{}; {:?}]",
            self.id,
            self.code,
            self.finding_id,
            self.function,
            self.location,
            self.verification,
            self.completeness
        );
        for step in &self.steps {
            let at = step
                .location
                .as_ref()
                .map(|l| {
                    if l.file == self.location.file {
                        format!(" @ {}:{}", l.line, l.column)
                    } else {
                        format!(" @ {}:{}:{}", l.file, l.line, l.column)
                    }
                })
                .unwrap_or_default();
            let state = format!("{:?}", step.at_failure);
            let marker = match step.role {
                Some(StepRole::CompletedFirst) => " <- completes first",
                Some(StepRole::NotYetPerformed) => " <- not yet performed",
                Some(StepRole::StagedWrite) => " <- staged",
                Some(StepRole::Transaction) => " <- transaction",
                Some(StepRole::InWindow) => " <- in failure window",
                None => "",
            };
            let _ = writeln!(
                out,
                "  {:>3}. {:<11} {:<11} {}{at}{marker}",
                step.index,
                step.kind.as_str(),
                state,
                step.label
            );
            if step.index == self.injection.after_step {
                let checkpoints = self
                    .injection
                    .checkpoints
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ");
                let _ = writeln!(
                    out,
                    "       ---- failure injection boundary ({checkpoints}) ----"
                );
            }
        }
        let _ = writeln!(out, "  simulated: {}", self.simulation.claim);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(index: usize, kind: StepKind, label: &str, tx: Option<&str>) -> TraceStep {
        TraceStep {
            index,
            kind,
            label: label.to_owned(),
            description: String::new(),
            location: None,
            call_chain: Vec::new(),
            transaction: tx.map(str::to_owned),
            parent_transaction: None,
            handle: None,
            table: None,
            operation: None,
            category: None,
            best_effort: false,
            execution: None,
            role: None,
            at_failure: StepState::NotReached,
        }
    }

    #[test]
    fn crash_after_an_autocommit_write_keeps_it_and_skips_the_publish() {
        let steps = vec![
            step(0, StepKind::DbWrite, "db.update(orders)", None),
            step(1, StepKind::External, "queue.publish(...)", None),
        ];
        let (states, sim) = simulate_crash(&steps, 0);
        assert_eq!(states, [StepState::Committed, StepState::NotReached]);
        assert_eq!(sim.durable, [0]);
        assert_eq!(sim.not_performed, [1]);
        assert!(sim.lost.is_empty());
        assert_eq!(sim.verification, VerificationLevel::Simulated);
        assert_eq!(sim.failure_mode, FailureMode::ProcessKill);
    }

    #[test]
    fn crash_inside_an_open_transaction_loses_its_staged_writes() {
        let steps = vec![
            step(0, StepKind::TxBegin, "db.transaction", Some("tx#0")),
            step(1, StepKind::DbWrite, "tx.update(orders)", Some("tx#0")),
            step(2, StepKind::External, "queue.publish(...)", None),
            step(3, StepKind::TxCommit, "db.transaction", Some("tx#0")),
        ];
        let (states, sim) = simulate_crash(&steps, 2);
        assert_eq!(states[1], StepState::Staged);
        assert_eq!(states[2], StepState::Performed);
        assert_eq!(sim.lost, [1]);
        assert_eq!(sim.durable, [2]);
        assert_eq!(sim.not_performed, [3]);
        // After the commit, the write is durable.
        let (states, sim) = simulate_crash(&steps, 3);
        assert_eq!(states[1], StepState::Committed);
        assert_eq!(sim.durable, [1, 2]);
    }

    #[test]
    fn nested_rollback_discards_only_the_savepoint_writes() {
        let mut nested_begin = step(2, StepKind::TxBegin, "tx.transaction", Some("tx#1"));
        nested_begin.parent_transaction = Some("tx#0".to_owned());
        let steps = vec![
            step(0, StepKind::TxBegin, "db.transaction", Some("tx#0")),
            step(1, StepKind::DbWrite, "tx.insert(a)", Some("tx#0")),
            nested_begin,
            step(3, StepKind::DbWrite, "sp.insert(b)", Some("tx#1")),
            step(4, StepKind::TxRollback, "tx.transaction", Some("tx#1")),
            step(5, StepKind::TxCommit, "db.transaction", Some("tx#0")),
        ];
        let (states, sim) = simulate_crash(&steps, 5);
        assert_eq!(states[1], StepState::Committed);
        assert_eq!(states[3], StepState::RolledBack);
        assert_eq!(sim.durable, [1]);
    }

    #[test]
    fn a_write_on_an_ended_transaction_handle_commits_on_its_own() {
        let steps = vec![
            step(0, StepKind::TxBegin, "db.transaction", Some("tx#0")),
            step(1, StepKind::TxCommit, "db.transaction", Some("tx#0")),
            step(2, StepKind::DbWrite, "tx.insert(a)", Some("tx#0")),
        ];
        let (states, _) = simulate_crash(&steps, 2);
        assert_eq!(states[2], StepState::Committed);
    }
}

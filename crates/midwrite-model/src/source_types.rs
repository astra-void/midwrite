//! Serializable types of Phase 7 source-level verification and extraction
//! reports (always compiled; the verification itself needs the `postgres`
//! feature, see `source_verify`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::provider::EntryInfo;
use crate::value::Row;

/// How one actor's original code is invoked (from the scenario).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceActor {
    /// The harness entry (`entries` in the harness manifest).
    pub entry: String,
    /// The input the harness passes to the original function.
    pub input: Json,
}

/// The scenario's `sourceVerification` section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSpec {
    /// Path of the harness manifest, relative to the scenario file.
    #[serde(default)]
    pub harness: Option<String>,
    pub actors: BTreeMap<String, SourceActor>,
    /// When the model finds NO counterexample (a safe or
    /// idempotent alternative), run this bounded recovery schedule against
    /// the original code instead and check that it keeps the invariant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_probe: Option<RecoveryProbe>,
}

/// Which recovery schedule to run when there is no counterexample.
/// The model checker picks the SHORTEST complete schedule within the
/// declared bounds that contains the required events in order; the
/// schedule is the model's, never written by hand.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryProbe {
    /// Events in order: `external:<channel>`, `crash:<actor>`,
    /// `retry:<actor>`, `commit:<actor>`, `error:<SQLSTATE>`,
    /// `statement:<actor>` (any statement of the actor), `blocked:<actor>`
    /// (a statement of the actor starts waiting for a lock).
    pub require: Vec<String>,
    /// The invariant that must hold (default: every final invariant).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invariant: Option<String>,
}

/// One model statement and what the original code did for it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementComparison {
    pub transition: usize,
    pub actor: String,
    /// The model operation (description and source).
    pub model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model_source: String,
    pub predicted: String,
    /// The SQL Drizzle actually sent (empty if none was matched).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sql: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub runtime_source: String,
    pub observed: String,
    /// `agree`, `differ`, `not-observed`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub differences: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub run: u32,
    /// `followed`, `diverged`, `inconclusive`, `blocked` (from the
    /// coordinator).
    pub coordination: String,
    pub statements_agree: bool,
    pub tables_agree: bool,
    pub outcomes_agree: bool,
    /// The invariant evaluated on the observed final rows: `violated`,
    /// `holds`, or `not-evaluated`.
    pub invariant: String,
    pub reproduced: bool,
    pub elapsed_ms: u64,
    /// Crash checkpoints, restarts, effects and attempts agree
    /// with the model (absent when the schedule has none of them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_agrees: Option<bool>,
    /// Phase 8, recovery probes only: the run followed the model's
    /// recovery schedule, everything agreed, and the invariant HOLDS on the
    /// observed state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_reproduced: Option<bool>,
}

/// The operation that was in flight at a crash, as the model
/// describes it and as the wire and PostgreSQL showed it. Each fact says
/// whether it was observed (by the proxy or by PostgreSQL) — none is
/// inferred from timing.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InFlightCheck {
    pub operation: String,
    /// The fate the model chose: `not-reached`, `answered-and-withheld`,
    /// `held-for-later-delivery`.
    pub model_fate: String,
    /// The fate the run established: the same names.
    pub observed_fate: String,
    pub mechanism: String,
    pub forwarded_to_server: bool,
    pub withheld_from_server: bool,
    /// PostgreSQL's own answer, as the proxy observed it before withholding
    /// it from the client.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub server_answer: String,
    /// The application received an answer (never, in a verified run).
    pub client_acknowledged: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub backend_state_before_kill: String,
    pub session_open_after_kill: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wire_events: Vec<String>,
    /// What the application could conclude (always ambiguous after a lost
    /// acknowledgment) and what PostgreSQL did: kept apart.
    pub application_knowledge: String,
    pub physical_outcome: String,
}

/// A stuck state observed on the original code (every remaining
/// task waits for a pool connection, or for a lock the application itself
/// holds), next to what the model predicted.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StuckCheck {
    /// What the model says it is.
    pub model_kind: String,
    pub observed: crate::source_types::StuckObservation,
    /// `agree` or `differ`.
    pub status: String,
}

/// What the run observed at the stuck state (see `midwrite_replay`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StuckObservation {
    pub kind: String,
    pub quiet_ms: u64,
    pub unfinished: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<(i64, i64, i64)>,
    #[serde(default)]
    pub lock_waits: usize,
    pub confirmed: bool,
    pub detail: String,
}

/// The server completing a held operation.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionCheck {
    pub transition: usize,
    pub actor: String,
    pub attempt: u32,
    pub server_answer: String,
    pub session_ended: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_tables: BTreeMap<String, Vec<Row>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub runtime_tables: BTreeMap<String, Vec<Row>>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub differences: Vec<String>,
}

/// One connection that broke under a live worker, as the model
/// predicts it and as it was observed.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LossCheck {
    pub transition: usize,
    pub actor: String,
    pub attempt: u32,
    pub operation: String,
    pub model_fate: String,
    pub observed_fate: String,
    pub mechanism: String,
    pub forwarded_to_server: bool,
    pub withheld_from_server: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub server_answer: String,
    pub client_severed: bool,
    /// The worker's operating-system process id, and the operating system's
    /// answer that it was alive after the loss.
    pub worker_pid: u32,
    pub worker_alive_after: bool,
    pub worker_connected_after: bool,
    /// The driver's error for the operation, quoted as reported.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub driver_error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver_sqlstate: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub backend_state_before: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_pid: Option<i64>,
    pub session_ended: bool,
    pub session_open: bool,
    /// Statements issued on the broken client afterwards (rejected by the
    /// driver without reaching the wire), and the backends of the connections
    /// the application used instead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dead_connection_statements: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub later_backend_pids: Vec<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wire_events: Vec<String>,
    /// The application's knowledge and the physical outcome, kept apart.
    pub application_knowledge: String,
    pub physical_outcome: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_tables: BTreeMap<String, Vec<Row>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub runtime_tables: BTreeMap<String, Vec<Row>>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub differences: Vec<String>,
}

/// One planned crash, as the model describes it and as it
/// happened.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashCheck {
    /// Set when an operation was in flight at the crash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_flight: Option<InFlightCheck>,
    pub transition: usize,
    pub actor: String,
    pub attempt: u32,
    /// The model's crash boundary.
    pub boundary: String,
    /// What the worker was doing when it was killed (observed).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub checkpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub never_sent: Option<String>,
    #[serde(default)]
    pub pid: u32,
    pub killed_by_signal: bool,
    /// PostgreSQL ended the worker's sessions (rolled back what they had
    /// not committed).
    pub sessions_ended: bool,
    #[serde(default)]
    pub sessions_ended_ms: u64,
    /// Committed rows right after the crash: the model's and PostgreSQL's.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_tables: BTreeMap<String, Vec<Row>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub runtime_tables: BTreeMap<String, Vec<Row>>,
    /// `agree`, `differ`, `not-observed`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub differences: Vec<String>,
}

/// One new execution of an actor.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestartCheck {
    pub transition: usize,
    pub actor: String,
    pub attempt: u32,
    /// The declared mechanism that starts it.
    pub mechanism: String,
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub previous_pid: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub previous_outcome: String,
    /// `agree` (a new process started at the planned point), `not-observed`.
    pub status: String,
}

/// The external effects the model predicts and the channel test
/// double received (`channel key=… actor#attempt [deduplicated]`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectComparison {
    pub model: Vec<String>,
    pub runtime: Vec<String>,
    pub agree: bool,
}

/// Crash and recovery evidence of the first run.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryVerification {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crashes: Vec<CrashCheck>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restarts: Vec<RestartCheck>,
    /// Held operations the server completed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completions: Vec<CompletionCheck>,
    /// Connections that broke under a live worker.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub losses: Vec<LossCheck>,
    pub effects: EffectComparison,
    /// (actor, executions in the model, executions observed).
    pub attempts: Vec<(String, u32, u32)>,
    /// Declared contracts the run relies on (channel test doubles, retry
    /// mechanisms): assumptions, not proofs about real providers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contracts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub server_settings: Vec<String>,
    /// What the final state means: which retries remain possible.
    pub quiescence: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceVerification {
    pub performed: bool,
    /// `reproduced`, `not-reproduced`, `diverged`, `inconclusive`,
    /// `unsupported`, `blocked`.
    pub status: String,
    pub harness: String,
    pub runs: Vec<RunSummary>,
    /// Details of the first run (statements, final rows, actors).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub statements: Vec<StatementComparison>,
    /// Statements the application sent outside the modeled functions
    /// (wrapper preconditions), with their source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outside_model: Vec<String>,
    /// Statements nobody expected (after the plan or instead of a step).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unexpected: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_tables: BTreeMap<String, Vec<Row>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub runtime_tables: BTreeMap<String, Vec<Row>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actors: Vec<(String, String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assumptions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postgres_version: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub isolation: String,
    pub note: String,
    /// Phase 8 evidence classification (absent for Phase 7 runs, whose
    /// `status` keeps its meaning): `ORIGINAL_CODE_CRASH_REPRODUCED`,
    /// `ORIGINAL_CODE_RECOVERY_REPRODUCED` (a probe with a crash or retry),
    /// `ORIGINAL_CODE_INVARIANT_HELD` (a probe without one), `INCONCLUSIVE`,
    /// `UNSUPPORTED`, `DIVERGED`, `NOT_REPRODUCED`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evidence: String,
    /// The schedule that was run, when it is a recovery probe
    /// rather than a counterexample (`kind actor[/thread]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probe_schedule: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<RecoveryVerification>,
    /// The extracted schema facts compared with the catalog of the
    /// database the original code ran on (after its migrations).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_fidelity: Option<crate::schema_fidelity::SchemaFidelity>,
    /// Statements the database driver issued for itself (catalog
    /// reads after connecting): not part of the application, not held.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub driver_internal: Vec<String>,
    /// Transaction configuration a driver sent in a statement of
    /// its own right after a planned BEGIN (postgres-js: `set transaction
    /// isolation level ...`), released as part of that BEGIN.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transaction_config: Vec<String>,
    /// The proxy's redacted wire log of the first run (only with
    /// in-flight steps).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wire_log: Vec<String>,
    /// The stuck state the run observed (termination
    /// counterexamples).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stuck: Option<StuckCheck>,
    /// Processes that ran several tasks (name → tasks) and the
    /// pool capacity the run verified.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_processes: Vec<String>,
}

/// Semantic operation coverage of one extracted actor. Counts
/// operations of the concurrency IR, not AST nodes.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    /// Database statements, transaction control and external effects
    /// represented exactly within the modeled subset.
    pub represented_operations: usize,
    /// Operations whose behavior rests on a declared assumption (wrapper
    /// invocation, declared preconditions).
    pub assumed_operations: usize,
    /// Operations lowered to `Unknown` (reaching one makes the result
    /// `UNSUPPORTED`).
    pub unsupported_operations: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsupported: Vec<String>,
}

/// The machine-readable extraction report of one actor.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractionReport {
    pub actor: String,
    pub entry: EntryInfo,
    pub source_coverage: Coverage,
    /// Facts the extractor derived from source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_facts: Vec<String>,
    /// Facts the scenario supplied (not proven from source).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scenario_assumptions: Vec<String>,
}

impl SourceVerification {
    /// Whether any Phase 9 field is present (the report is then schema 4).
    pub fn has_phase9_fields(&self) -> bool {
        !self.wire_log.is_empty()
            || self.stuck.is_some()
            || !self.shared_processes.is_empty()
            || self.recovery.as_ref().is_some_and(|r| {
                !r.completions.is_empty() || r.crashes.iter().any(|c| c.in_flight.is_some())
            })
    }

    /// Whether any Phase 10 field is present (the report is then schema 5).
    pub fn has_phase10_fields(&self) -> bool {
        self.recovery.as_ref().is_some_and(|r| !r.losses.is_empty())
    }

    /// Whether any Phase 8 field is present (the report is then schema 3).
    pub fn has_phase8_fields(&self) -> bool {
        !self.evidence.is_empty()
            || !self.probe_schedule.is_empty()
            || self.recovery.is_some()
            || self.schema_fidelity.is_some()
            || !self.driver_internal.is_empty()
            || !self.transaction_config.is_empty()
            || self
                .runs
                .iter()
                .any(|r| r.recovery_agrees.is_some() || r.recovery_reproduced.is_some())
    }
}

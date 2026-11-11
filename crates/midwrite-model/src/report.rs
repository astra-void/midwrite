//! The versioned model-checking report and the driver that produces it.
//!
//! The report keeps four things apart that are easy to blur: what the model
//! *found* (a counterexample within bounds), what PostgreSQL *confirmed*
//! (the same schedule replayed on a real server), what remains *unverified*
//! (everything else), and the *assumptions* under which each statement
//! holds. Serialization is deterministic: no timestamps, ordered maps, and
//! timing in a separate optional object.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::explore::{ExploreOptions, ExploreResult, Truncation, Verdict, explore_with};
use crate::invariant::When;
use crate::ir::{Op, Origin, SourceRef};
use crate::model::{Bounds, Model, Strategy};
use crate::value::{Row, Value};
use crate::witness::{Witness, build_witness};

pub const REPORT_KIND: &str = "midwrite.model-check";
pub const REPORT_SCHEMA_VERSION: u32 = 1;
/// Version of the model semantics (docs/concepts/model-semantics.md). Bumped when a
/// change can alter a verdict.
pub const MODEL_VERSION: &str = "1.0.0";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioInfo {
    pub name: String,
    pub description: String,
    pub scenario_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Revision of the analyzed project, if known (never inferred).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_revision: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub primary_key: Vec<String>,
    pub unique: Vec<Vec<String>>,
    pub checks: Vec<String>,
    pub initial_rows: Vec<Row>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseInfo {
    pub isolation: String,
    pub tables: Vec<TableInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorInfo {
    pub name: String,
    pub function: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    pub arguments: Vec<(String, Value)>,
    pub process: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<String>,
    /// `extracted` (derived from TypeScript), `declared` (a fixture in the
    /// scenario) or `mixed`.
    pub origin: String,
    pub tasks: usize,
    pub operations: usize,
    /// Source locations of the database and external operations, in order.
    pub source_refs: Vec<SourceRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundsInfo {
    pub declared: Bounds,
    pub partial_order_reduction: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_order_reduction_disabled_because: Option<String>,
    pub strategy: String,
    /// `dpor` when dynamic partial-order reduction ran (the sleep
    /// sets did not); absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reduction: Option<String>,
    /// The mechanisms of `source-dpor` (`stateful-sleep`,
    /// `stateful`, `stateless-sleep`); absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_dpor_mode: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsInfo {
    pub states: u64,
    pub transitions: u64,
    pub terminal_states: u64,
    pub duplicate_state_hits: u64,
    pub sleep_pruned_transitions: u64,
    pub context_pruned_transitions: u64,
    pub max_depth: u32,
    pub schedules_in_explored_graph: u64,
    pub visited_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsupportedInfo {
    pub actor: String,
    pub detail: String,
    pub location: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplorationInfo {
    pub verdict: String,
    /// Every behavior within the declared bounds was explored.
    pub complete: bool,
    pub truncation: Truncation,
    pub metrics: MetricsInfo,
    pub unsupported: Vec<UnsupportedInfo>,
    pub stuck_states: usize,
    /// What the stuck states are (absent when there are none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stuck: Vec<String>,
    /// Phase 9 (`--profile-storage`): where the memory of the visited-state
    /// structures goes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<crate::explore::StorageProfile>,
    /// Phase 10 (`source-dpor` only): what the reduction did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reduction_metrics: Option<ReductionMetrics>,
}

/// Counters of the source-set DPOR search.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReductionMetrics {
    /// Reversible races found, and backtrack points added for them.
    pub races: u64,
    pub backtrack_points: u64,
    /// Event summaries of cached states checked against the current trace.
    pub summary_checks: u64,
    /// Distinct summaries and event descriptors held, and an estimate of
    /// their memory (the process's peak RSS is the measured figure).
    pub summaries: u64,
    pub descriptors: u64,
    pub summary_bytes: u64,
    /// Transitions not taken because they sleep.
    pub sleep_pruned_transitions: u64,
    pub max_frames: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvariantInfo {
    pub name: String,
    pub when: When,
    pub formula: String,
    #[serde(default)]
    pub description: String,
    /// `violated`, `holds-within-bounds` or `not-established` (the search
    /// ended before it could be decided).
    pub status: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceLevel {
    /// No counterexample, nothing to confirm.
    None,
    /// The model found a schedule; nothing else was run.
    ModelCounterexample,
    /// The same schedule was replayed on PostgreSQL and every prediction
    /// matched, including the final committed rows.
    PostgresConfirmedSchedule,
    /// PostgreSQL behaved differently from the model on this schedule.
    PostgresDidNotReproduce,
    /// Runtime validation was requested but could not be carried out.
    RuntimeInconclusive,
    /// The actors' ORIGINAL application code ran under the
    /// counterexample's schedule against PostgreSQL, every statement and
    /// outcome matched the model, and the invariant is violated on the rows
    /// the run left (`sourceVerification`).
    OriginalCodeReproduced,
    /// As `OriginalCodeReproduced`, for a counterexample that
    /// crashes a process and/or retries an operation: the worker was really
    /// killed (SIGKILL) at the planned checkpoint, PostgreSQL ended its
    /// sessions, and the retry ran as a new process.
    OriginalCodeCrashReproduced,
    /// There is NO counterexample within the bounds; the model's
    /// recovery schedule (`sourceVerification.recoveryProbe`) ran against the
    /// original code with a real crash and retry, matched the model, and
    /// the invariant HOLDS on the observed state. Bounded evidence about
    /// one schedule, not a proof.
    OriginalCodeRecoveryReproduced,
    /// No counterexample within the bounds; a model schedule
    /// WITHOUT crash or retry (`recoveryProbe`, for example two concurrent
    /// consumers) ran against the original code, matched the model, and the
    /// invariant holds on the observed state. Bounded evidence, not a proof.
    OriginalCodeInvariantHeld,
    /// As `OriginalCodeCrashReproduced`, for a counterexample in
    /// which an operation (a COMMIT or an autocommit write) was IN FLIGHT
    /// when the worker was killed: the proxy established on the wire what the
    /// server received, PostgreSQL's own state confirmed the outcome, and the
    /// application never received an answer.
    OriginalCodeInFlightFailureReproduced,
    /// A connection broke under a live worker (not a kill).
    OriginalCodeConnectionLossReproduced,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub level: EvidenceLevel,
    pub summary: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeVerification {
    pub performed: bool,
    /// `confirmed`, `disagreement`, `blocked` or `not-requested`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postgres_version: Option<String>,
    #[serde(default)]
    pub observations: Vec<crate::oracle_types::Observation>,
    #[serde(default)]
    pub sqlstates: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tables_agree: Option<bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_tables: BTreeMap<String, Vec<Row>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub postgres_tables: BTreeMap<String, Vec<Row>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sql_origin: BTreeMap<String, String>,
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDiagnostic {
    pub code: String,
    pub severity: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timing {
    pub wall_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_violation_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCheckReport {
    pub kind: String,
    pub schema_version: u32,
    pub model_version: String,
    pub tool: ToolInfo,
    pub scenario: ScenarioInfo,
    pub database: DatabaseInfo,
    pub actors: Vec<ActorInfo>,
    pub bounds: BoundsInfo,
    pub exploration: ExplorationInfo,
    pub invariants: Vec<InvariantInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counterexample: Option<Witness>,
    pub evidence: Evidence,
    pub runtime: RuntimeVerification,
    pub assumptions: Vec<String>,
    pub diagnostics: Vec<ModelDiagnostic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<Timing>,
    /// Phase 7 (report schema 2): extraction reports of actors selected by
    /// a source selector.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extraction: Vec<crate::source_types::ExtractionReport>,
    /// Phase 7 (report schema 2): the original-code verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_verification: Option<crate::source_types::SourceVerification>,
    /// Phase 8 (report schema 3): the facts behind every table read from
    /// the application's Drizzle schema, each with its origin.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schema_facts: Vec<crate::schema_facts::TableFacts>,
    /// Phase 9 (report schema 4): what the application could and could not
    /// know at each lost acknowledgment of the counterexample, with the
    /// other physical world replayed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epistemic: Option<crate::twin::EpistemicReport>,
}

impl ModelCheckReport {
    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("report serializes");
        s.push('\n');
        s
    }

    pub fn verdict_str(&self) -> &str {
        &self.exploration.verdict
    }
}

#[derive(Clone, Debug, Default)]
pub struct CheckOptions {
    /// Replay the counterexample against a real PostgreSQL.
    pub validate_postgres: bool,
    pub include_timing: bool,
    pub scenario_file: Option<String>,
    pub source_revision: Option<String>,
    pub tool_version: String,
    /// What the scenario loader learned (extraction reports, Phase 7).
    pub load: crate::scenario::LoadReport,
    /// Run the counterexample against the original code.
    pub source: Option<SourceOptions>,
    /// Report where the memory of the visited-state structures goes.
    pub profile_storage: bool,
}

/// `--verify-source`: the harness and the explicit authorization.
#[derive(Clone, Debug, Default)]
pub struct SourceOptions {
    pub harness: std::path::PathBuf,
    pub spec: crate::source_types::SourceSpec,
    /// `--allow-execution`.
    pub allow_execution: bool,
    pub repeat: u32,
}

/// Extraction reports for actors selected by a source selector.
fn extraction_reports(
    model: &Model,
    load: &crate::scenario::LoadReport,
) -> Vec<crate::source_types::ExtractionReport> {
    use crate::ir::Op;
    let mut out = Vec::new();
    for (name, entry) in &load.entries {
        let Some(actor) = model.actors.iter().find(|a| &a.name == name) else {
            continue;
        };
        let mut coverage = crate::source_types::Coverage::default();
        for step in actor.codes.iter().flatten() {
            match &step.op {
                Op::Db { .. }
                | Op::Begin { .. }
                | Op::Commit
                | Op::Rollback
                | Op::External { .. }
                | Op::AppRead { .. }
                | Op::AppWrite { .. } => coverage.represented_operations += 1,
                Op::Unknown { detail, .. } => {
                    coverage.unsupported_operations += 1;
                    coverage
                        .unsupported
                        .push(format!("{}: {detail}", step.src.label()));
                }
                _ => {}
            }
        }
        let mut source_facts = vec![format!(
            "entry `{}` is the function at {} (`{}` in `midwrite check`)",
            entry.selector, entry.anchor, entry.static_name
        )];
        if let Some(w) = &entry.wrapper {
            source_facts.push(format!(
                "`{}`{}: {}{}",
                w.wrapper,
                w.defined_at
                    .as_deref()
                    .map(|d| format!(" ({d})"))
                    .unwrap_or_default(),
                w.invocation,
                if w.detail.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", w.detail)
                }
            ));
            for g in &w.guards {
                source_facts.push(format!("guard before the call: {g}"));
            }
            if !w.arguments.is_empty() {
                source_facts.push(format!("the wrapper passes ({})", w.arguments.join(", ")));
            }
        }
        let mut scenario_assumptions = Vec::new();
        if let Some(c) = &entry.contract {
            if let Some(i) = &c.invocation {
                scenario_assumptions.push(format!("invocation: {i}"));
            }
            scenario_assumptions
                .extend(c.preconditions.iter().map(|p| format!("precondition: {p}")));
        }
        // The wrapper's own work around the call is not part of the actor:
        // it is assumed (its preconditions), not represented.
        coverage.assumed_operations =
            usize::from(entry.wrapper.is_some()) + scenario_assumptions.len();
        out.push(crate::source_types::ExtractionReport {
            actor: name.clone(),
            entry: entry.clone(),
            source_coverage: coverage,
            source_facts,
            scenario_assumptions,
        });
    }
    out
}

#[cfg(feature = "postgres")]
fn verify_source(
    model: &Model,
    w: &Witness,
    load: &crate::scenario::LoadReport,
    so: &SourceOptions,
) -> crate::source_types::SourceVerification {
    use crate::source_types::SourceVerification;
    let refused = |status: &str, harness: &str, why: String| SourceVerification {
        performed: false,
        status: status.to_owned(),
        harness: harness.to_owned(),
        runs: Vec::new(),
        statements: Vec::new(),
        outside_model: Vec::new(),
        unexpected: Vec::new(),
        model_tables: BTreeMap::new(),
        runtime_tables: BTreeMap::new(),
        actors: Vec::new(),
        problems: vec![why],
        assumptions: Vec::new(),
        postgres_version: None,
        isolation: String::new(),
        note: String::new(),
        ..Default::default()
    };
    let harness = match midwrite_replay::source::load_manifest(&so.harness) {
        Ok(h) => h,
        Err(e) => return refused("blocked", &so.harness.display().to_string(), e),
    };
    let Some(invariant) = model.invariants.iter().find(|i| i.name == w.invariant) else {
        return refused(
            "unsupported",
            &harness.manifest.harness,
            "the violated invariant is unknown".to_owned(),
        );
    };
    let final_state = match crate::witness::replay(model, &w.schedule, false) {
        Ok(r) => r.state,
        Err(e) => return refused("unsupported", &harness.manifest.harness, e.reason),
    };
    crate::source_verify::verify(&crate::source_verify::VerifyRequest {
        model,
        schedule: &w.schedule,
        final_state: &final_state,
        invariant,
        harness: &harness,
        spec: &so.spec,
        entries: &load.entries,
        allow_execution: so.allow_execution,
        repeat: so.repeat,
        probe: false,
        extracted_schema: load.extracted_schema.as_ref(),
    })
}

/// No counterexample; run the model's recovery schedule against
/// the original code instead (`sourceVerification.recoveryProbe`).
#[cfg(feature = "postgres")]
fn verify_probe(
    model: &Model,
    load: &crate::scenario::LoadReport,
    so: &SourceOptions,
    probe: &crate::source_types::RecoveryProbe,
) -> crate::source_types::SourceVerification {
    use crate::source_types::SourceVerification;
    let refused = |status: &str, harness: &str, why: String| SourceVerification {
        performed: false,
        status: status.to_owned(),
        harness: harness.to_owned(),
        problems: vec![why],
        evidence: if status == "blocked" {
            "INCONCLUSIVE"
        } else {
            "UNSUPPORTED"
        }
        .to_owned(),
        ..Default::default()
    };
    let harness = match midwrite_replay::source::load_manifest(&so.harness) {
        Ok(h) => h,
        Err(e) => return refused("blocked", &so.harness.display().to_string(), e),
    };
    let name = &harness.manifest.harness;
    let finals: Vec<&crate::invariant::Invariant> = model
        .invariants
        .iter()
        .filter(|i| match &probe.invariant {
            Some(n) => &i.name == n,
            None => i.when == crate::invariant::When::Final,
        })
        .collect();
    let [invariant] = finals.as_slice() else {
        return refused(
            "unsupported",
            name,
            "the recovery probe must name exactly one invariant (`invariant`) when the scenario has several".to_owned(),
        );
    };
    let schedule = match crate::probe::find(model, &probe.require) {
        Ok(s) => s,
        Err(e) => return refused("unsupported", name, e),
    };
    let final_state = match crate::witness::replay(model, &schedule, false) {
        Ok(r) => r.state,
        Err(e) => return refused("unsupported", name, e.reason),
    };
    if !invariant.holds(model, &final_state) {
        return refused(
            "unsupported",
            name,
            "the probe's final state violates the invariant in the model: it is a counterexample, not a recovery".to_owned(),
        );
    }
    let mut out = crate::source_verify::verify(&crate::source_verify::VerifyRequest {
        model,
        schedule: &schedule,
        final_state: &final_state,
        invariant,
        harness: &harness,
        spec: &so.spec,
        entries: &load.entries,
        allow_execution: so.allow_execution,
        repeat: so.repeat,
        probe: true,
        extracted_schema: load.extracted_schema.as_ref(),
    });
    out.probe_schedule = crate::probe::render(model, &schedule);
    out
}

#[cfg(not(feature = "postgres"))]
fn verify_probe(
    _: &Model,
    _: &crate::scenario::LoadReport,
    so: &SourceOptions,
    _: &crate::source_types::RecoveryProbe,
) -> crate::source_types::SourceVerification {
    crate::source_types::SourceVerification {
        performed: false,
        status: "blocked".to_owned(),
        harness: so.harness.display().to_string(),
        problems: vec!["this build does not include source verification".to_owned()],
        ..Default::default()
    }
}

#[cfg(not(feature = "postgres"))]
fn verify_source(
    _: &Model,
    _: &Witness,
    _: &crate::scenario::LoadReport,
    so: &SourceOptions,
) -> crate::source_types::SourceVerification {
    crate::source_types::SourceVerification {
        performed: false,
        status: "blocked".to_owned(),
        harness: so.harness.display().to_string(),
        runs: Vec::new(),
        statements: Vec::new(),
        outside_model: Vec::new(),
        unexpected: Vec::new(),
        model_tables: BTreeMap::new(),
        runtime_tables: BTreeMap::new(),
        actors: Vec::new(),
        problems: vec!["this build has no PostgreSQL support (feature `postgres`)".to_owned()],
        assumptions: Vec::new(),
        postgres_version: None,
        isolation: String::new(),
        note: String::new(),
        ..Default::default()
    }
}

/// Diagnostic codes of the concurrency category. They are separate from the
/// MW1001–MW9001 failure-path codes and never reuse them.
pub fn explain(code: &str) -> Option<&'static str> {
    Some(match code {
        "MW7001" => {
            "A declared invariant is violated by a bounded concurrent schedule of the modeled actors. This is a MODEL counterexample: it is a possible execution under the scenario's assumptions (schema, isolation level, actors, bounds), not proof that the application is exposed. Read the witness schedule and the assumptions; replay it with --validate-postgres to see whether a real PostgreSQL permits it."
        }
        "MW7002" => {
            "The search stopped at a resource limit (states, transitions, steps, time, memory) before every behavior within the bounds was explored. No counterexample was found in the part explored; that says nothing about the rest."
        }
        "MW7003" => {
            "The scenario reaches an operation the model does not understand (raw SQL, an unsupported construct). Unknown behavior is never treated as a no-op, so exploration of those paths stops and the result cannot be a verification."
        }
        "MW7101" => {
            "PostgreSQL did not reproduce the model's counterexample: some statement behaved differently from the model's prediction. Either the model is imprecise for this construct (please report the mismatch) or PostgreSQL prevents the schedule. The model result must not be reported as a confirmed race."
        }
        "MW7103" => {
            "The original code did not reproduce the counterexample. With --verify-source --allow-execution, Midwrite ran the actors' original application code under the model's counterexample schedule against a disposable PostgreSQL database, and the run did not reproduce the violation: the application sent different statements (diverged), PostgreSQL answered differently, an expected statement never came (inconclusive), or the invariant held on the observed rows. The model counterexample keeps its own evidence level; the differences are reported statement by statement. A difference can reveal a modeling gap, an unsupported construct, or a declared assumption the run did not establish."
        }
        "MW7102" => {
            "PostgreSQL validation was requested but could not run (no PostgreSQL server binaries, or the cluster could not be driven). The counterexample remains a model counterexample."
        }
        _ => return None,
    })
}

fn describe_retry(r: &crate::ir::OperationRetry) -> String {
    format!("{:?}", r.trigger)
}

fn actor_infos(model: &Model) -> Vec<ActorInfo> {
    model
        .actors
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let mut refs = Vec::new();
            let mut extracted = 0;
            let mut declared = 0;
            let mut ops = 0;
            for step in a.codes.iter().flatten() {
                ops += 1;
                match step.src.origin {
                    Origin::Extracted => extracted += 1,
                    Origin::Declared => declared += 1,
                }
                if matches!(
                    step.op,
                    Op::Db { .. }
                        | Op::External { .. }
                        | Op::Begin { .. }
                        | Op::Commit
                        | Op::Unknown { .. }
                ) {
                    refs.push(step.src.clone());
                }
            }
            ActorInfo {
                name: a.name.clone(),
                function: a.function.name.clone(),
                file: a.function.file.clone(),
                arguments: a.arguments.clone(),
                process: model.process_names[model.process_of[i]].clone(),
                retry: a.retry.as_ref().map(describe_retry),
                origin: match (extracted > 0, declared > 0) {
                    (true, true) => "mixed",
                    (true, false) => "extracted",
                    _ => "declared",
                }
                .to_owned(),
                tasks: a.codes.len(),
                operations: ops,
                source_refs: refs,
            }
        })
        .collect()
}

fn database_info(model: &Model) -> DatabaseInfo {
    DatabaseInfo {
        isolation: model.isolation.name().to_owned(),
        tables: model
            .schema
            .tables
            .iter()
            .enumerate()
            .map(|(i, t)| TableInfo {
                name: t.name.clone(),
                columns: t.columns.iter().map(|c| c.name.clone()).collect(),
                primary_key: t.primary_key.clone(),
                unique: t.unique.clone(),
                checks: t.checks.iter().map(|c| c.render()).collect(),
                initial_rows: model.initial_rows[i].clone(),
            })
            .collect(),
    }
}

fn exploration_info(
    model: &Model,
    r: &ExploreResult,
    profile: bool,
    source_dpor: bool,
) -> ExplorationInfo {
    let unsupported = r
        .unsupported
        .iter()
        .map(|u| UnsupportedInfo {
            actor: model.actors[u.actor].name.clone(),
            detail: u.detail.clone(),
            location: u.src.label(),
        })
        .collect();
    ExplorationInfo {
        verdict: r.verdict.as_str().to_owned(),
        complete: !r.truncation.resource_limit() && r.unsupported.is_empty() && r.stuck.is_empty(),
        truncation: r.truncation.clone(),
        metrics: MetricsInfo {
            states: r.metrics.states,
            transitions: r.metrics.transitions,
            terminal_states: r.metrics.terminal_states,
            duplicate_state_hits: r.metrics.duplicate_state_hits,
            sleep_pruned_transitions: r.metrics.sleep_pruned_transitions,
            context_pruned_transitions: r.metrics.context_pruned_transitions,
            max_depth: r.metrics.max_depth,
            schedules_in_explored_graph: r.metrics.schedules_in_explored_graph,
            visited_bytes: r.metrics.visited_bytes,
        },
        unsupported,
        storage: if profile {
            r.metrics.storage.clone()
        } else {
            None
        },
        reduction_metrics: source_dpor.then_some(ReductionMetrics {
            races: r.metrics.dpor_races,
            backtrack_points: r.metrics.dpor_backtracks,
            summary_checks: r.metrics.dpor_summary_checks,
            summaries: r.metrics.dpor_summaries,
            descriptors: r.metrics.dpor_descriptors,
            summary_bytes: r.metrics.dpor_summary_bytes,
            sleep_pruned_transitions: r.metrics.sleep_pruned_transitions,
            max_frames: r.metrics.dpor_max_frames,
        }),
        stuck_states: r.stuck.len(),
        stuck: {
            let mut kinds: Vec<String> = r
                .stuck
                .iter()
                .filter(|x| x.kind != crate::explore::StuckKind::Unexplained)
                .map(|x| x.kind.describe().to_owned())
                .collect();
            kinds.sort();
            kinds.dedup();
            kinds
        },
    }
}

fn not_requested() -> RuntimeVerification {
    RuntimeVerification {
        performed: false,
        status: "not-requested".into(),
        postgres_version: None,
        observations: Vec::new(),
        sqlstates: Vec::new(),
        tables_agree: None,
        model_tables: BTreeMap::new(),
        postgres_tables: BTreeMap::new(),
        sql_origin: BTreeMap::new(),
        note: "no runtime validation was requested (--validate-postgres)".into(),
    }
}

#[cfg(feature = "postgres")]
const RUNTIME_NOTE: &str = "Each actor ran on its own PostgreSQL connection (a psql process) against a private, disposable cluster; the model's schedule decided which session sent its next statement, and a statement counts as blocked only when the server reported a lock wait. Deadlock victim choice depends on PostgreSQL's deadlock timer and is steered with delays. Not exercised: the original TypeScript/Drizzle code, application-level retries, external services.";

#[cfg(feature = "postgres")]
fn validate_on_postgres(
    model: &Model,
    w: &Witness,
) -> (RuntimeVerification, EvidenceLevel, String) {
    use crate::oracle::{PgEnv, run_schedule};
    let mut env = match PgEnv::start() {
        Ok(e) => e,
        Err(reason) => {
            return (
                RuntimeVerification {
                    performed: false,
                    status: "blocked".into(),
                    note: format!("PostgreSQL validation was requested but is blocked: {reason}"),
                    ..not_requested()
                },
                EvidenceLevel::RuntimeInconclusive,
                "PostgreSQL validation was requested but could not run; the counterexample is a model counterexample only".into(),
            );
        }
    };
    let version = env.version().to_owned();
    let result = env
        .database(model)
        .and_then(|db| run_schedule(&env, &db, model, &w.schedule));
    match result {
        Ok(run) => {
            let confirmed = run.agree;
            let n = run.observations.len();
            let rt = RuntimeVerification {
                performed: true,
                status: if confirmed { "confirmed" } else { "disagreement" }.into(),
                postgres_version: Some(version),
                observations: run.observations.clone(),
                sqlstates: run.sqlstates.clone(),
                tables_agree: Some(run.tables_agree),
                model_tables: run.model_tables.clone(),
                postgres_tables: run.postgres_tables.clone(),
                sql_origin: run.sql_origin.clone(),
                note: RUNTIME_NOTE.into(),
            };
            if confirmed {
                let db_only = model
                    .invariants
                    .iter()
                    .find(|i| i.name == w.invariant)
                    .is_some_and(|i| i.formula.reads_only_database());
                let scope = if db_only {
                    "the invariant reads only committed rows, so it is violated on the real server"
                } else {
                    "the invariant also reads actor outcomes, external effects or in-process state, which the replay derives from the model's application logic given these database results (not observed in a running application)"
                };
                (
                    rt,
                    EvidenceLevel::PostgresConfirmedSchedule,
                    format!("the schedule was replayed on PostgreSQL: all {n} database outcomes matched the model's predictions and the final committed rows are identical; {scope}"),
                )
            } else {
                (
                    rt,
                    EvidenceLevel::PostgresDidNotReproduce,
                    "PostgreSQL did not reproduce the schedule as the model predicted; this is not a confirmed race (see runtime.observations for the first disagreement)".into(),
                )
            }
        }
        Err(e) => (
            RuntimeVerification {
                performed: false,
                status: "blocked".into(),
                postgres_version: Some(version),
                note: format!("PostgreSQL validation failed to run: {e}"),
                ..not_requested()
            },
            EvidenceLevel::RuntimeInconclusive,
            "PostgreSQL validation failed to run; the counterexample is a model counterexample only".into(),
        ),
    }
}

#[cfg(not(feature = "postgres"))]
fn validate_on_postgres(_: &Model, _: &Witness) -> (RuntimeVerification, EvidenceLevel, String) {
    (
        RuntimeVerification {
            performed: false,
            status: "blocked".into(),
            note: "this build does not include PostgreSQL validation".into(),
            ..not_requested()
        },
        EvidenceLevel::RuntimeInconclusive,
        "PostgreSQL validation is not available in this build".into(),
    )
}

/// Explore the model and assemble the report.
pub fn check(model: &Model, opts: &CheckOptions) -> ModelCheckReport {
    let explore_opts = ExploreOptions::from_model(model);
    let wants_dpor = model.options.dpor || model.options.source_dpor.is_some();
    let dpor_refusal = if wants_dpor {
        crate::explore::dpor_inapplicable(model)
    } else {
        None
    };
    let source_dpor = if dpor_refusal.is_none() {
        model.options.source_dpor
    } else {
        None
    };
    let use_dpor = wants_dpor && dpor_refusal.is_none();
    let result = if let Some(mode) = source_dpor {
        use crate::model::SourceDpor as M;
        crate::explore::explore_sdpor(
            model,
            &explore_opts,
            match mode {
                M::StatefulSleep => crate::explore::SdporConfig::STATEFUL_SLEEP,
                M::Stateful => crate::explore::SdporConfig::STATEFUL,
                M::StatelessSleep => crate::explore::SdporConfig::STATELESS_SLEEP,
            },
        )
    } else if use_dpor && std::env::var_os("MIDWRITE_DPOR_PROFILE").is_some() {
        // Phase 10 profiling hook: the classic search with its side tables.
        let (r, prof) = crate::explore::explore_dpor_profiled(model, &explore_opts);
        eprintln!(
            "dpor-profile {}",
            serde_json::to_string(&prof).unwrap_or_default()
        );
        r
    } else if use_dpor {
        crate::explore::explore_dpor(model, &explore_opts)
    } else {
        explore_with(model, &explore_opts)
    };
    let witness = result
        .counterexample
        .as_ref()
        .and_then(|ce| build_witness(model, ce, model.options.minimize));

    let mut diagnostics = Vec::new();
    let violated: Option<String> = result.counterexample.as_ref().map(|c| c.invariant.clone());
    let invariants = model
        .invariants
        .iter()
        .map(|inv| InvariantInfo {
            name: inv.name.clone(),
            when: inv.when,
            formula: inv.formula.render(),
            description: inv.description.clone(),
            status: if violated.as_deref() == Some(inv.name.as_str()) {
                "violated"
            } else if result.verdict == Verdict::NoCounterexampleWithinBounds {
                "holds-within-bounds"
            } else {
                "not-established"
            }
            .to_owned(),
        })
        .collect();

    let (runtime, level, summary) = match (&witness, opts.validate_postgres) {
        (Some(w), true) => validate_on_postgres(model, w),
        (Some(_), false) => (
            not_requested(),
            EvidenceLevel::ModelCounterexample,
            "a schedule found by the model checker; it has not been run on a real database".into(),
        ),
        (None, _) => (
            not_requested(),
            EvidenceLevel::None,
            match result.verdict {
                Verdict::NoCounterexampleWithinBounds => {
                    "no invariant violation exists in any behavior within the declared bounds (this does not prove the application race-free)".into()
                }
                Verdict::Inconclusive => "the search stopped before covering the declared bounds".into(),
                Verdict::Unsupported => "the scenario reaches behavior the model does not support".into(),
                Verdict::CounterexampleFound => "a counterexample was found but could not be rebuilt as a witness".into(),
            },
        ),
    };

    // The original code under the counterexample's schedule.
    let source_verification = opts.source.as_ref().map(|so| match &witness {
        Some(w) => verify_source(model, w, &opts.load, so),
        None if result.verdict == Verdict::NoCounterexampleWithinBounds
            && so.spec.recovery_probe.is_some() =>
        {
            let probe = so.spec.recovery_probe.as_ref().expect("checked");
            verify_probe(model, &opts.load, so, probe)
        }
        None => crate::source_types::SourceVerification {
            performed: false,
            status: "not-run".to_owned(),
            harness: so.harness.display().to_string(),
            runs: Vec::new(),
            statements: Vec::new(),
            outside_model: Vec::new(),
            unexpected: Vec::new(),
            model_tables: BTreeMap::new(),
            runtime_tables: BTreeMap::new(),
            actors: Vec::new(),
            problems: vec!["there is no counterexample to run".to_owned()],
            assumptions: Vec::new(),
            postgres_version: None,
            isolation: String::new(),
            note: String::new(),
            ..Default::default()
        },
    });
    let (level, summary) = match &source_verification {
        Some(sv)
            if sv.status == "reproduced"
                && sv.evidence == "ORIGINAL_CODE_CONNECTION_LOSS_REPRODUCED" =>
        {
            (
                EvidenceLevel::OriginalCodeConnectionLossReproduced,
                format!(
                    "the actors' original application code ran under this schedule against PostgreSQL in {} of {} run(s) while a connection BROKE under a live process (the proxy closed the client's connection, the worker was not signalled and was alive afterwards): the proxy established on the wire what the server received, PostgreSQL's own state confirmed the outcome, the driver reported an error to the application, which went on; every statement, effect and outcome matched the model, and the invariant is violated on the observed state",
                    sv.runs.iter().filter(|r| r.reproduced).count(),
                    sv.runs.len()
                ),
            )
        }
        Some(sv)
            if sv.status == "reproduced"
                && sv.evidence == "ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED" =>
        {
            (
                EvidenceLevel::OriginalCodeInFlightFailureReproduced,
                format!(
                    "the actors' original application code ran under this schedule against PostgreSQL in {} of {} run(s) with an operation IN FLIGHT when the worker was killed (SIGKILL): the proxy established on the wire what the server received, PostgreSQL's own state confirmed the outcome, the application never received an answer, the retry ran as a new process, every statement, effect and outcome matched the model, and the invariant is violated on the observed state",
                    sv.runs.iter().filter(|r| r.reproduced).count(),
                    sv.runs.len()
                ),
            )
        }
        Some(sv)
            if sv.status == "reproduced" && sv.evidence == "ORIGINAL_CODE_CRASH_REPRODUCED" =>
        {
            (
                EvidenceLevel::OriginalCodeCrashReproduced,
                format!(
                    "the actors' original application code ran under this crash/retry schedule against PostgreSQL in {} of {} run(s): workers were killed with SIGKILL at the planned checkpoints, PostgreSQL ended their sessions, retries ran as new processes, every statement, effect, result and outcome matched the model, and the invariant is violated on the observed state",
                    sv.runs.iter().filter(|r| r.reproduced).count(),
                    sv.runs.len()
                ),
            )
        }
        Some(sv) if sv.status == "recovery-reproduced" => (
            EvidenceLevel::OriginalCodeRecoveryReproduced,
            format!(
                "no counterexample within the bounds; the model's recovery schedule ran against the original code in {} of {} run(s) (real SIGKILL, new-process retry), matched the model, and the invariant holds on the observed state — bounded evidence about this schedule, not a proof",
                sv.runs
                    .iter()
                    .filter(|r| r.recovery_reproduced == Some(true))
                    .count(),
                sv.runs.len()
            ),
        ),
        Some(sv) if sv.status == "invariant-held" => (
            EvidenceLevel::OriginalCodeInvariantHeld,
            format!(
                "no counterexample within the bounds; the model's probe schedule ran against the original code in {} of {} run(s), matched the model, and the invariant holds on the observed state — bounded evidence about this schedule, not a proof",
                sv.runs
                    .iter()
                    .filter(|r| r.recovery_reproduced == Some(true))
                    .count(),
                sv.runs.len()
            ),
        ),
        Some(sv) if sv.status == "reproduced" => (
            EvidenceLevel::OriginalCodeReproduced,
            format!(
                "the actors' original application code ran under this schedule against PostgreSQL in {} of {} run(s): every statement, result and outcome matched the model, and the invariant is violated on the rows the run left",
                sv.runs.iter().filter(|r| r.reproduced).count(),
                sv.runs.len()
            ),
        ),
        _ => (level, summary),
    };

    if let Some(w) = &witness {
        diagnostics.push(ModelDiagnostic {
            code: "MW7001".into(),
            severity: "warning".into(),
            message: format!(
                "invariant `{}` can be violated by a bounded schedule of {} transitions ({} context switches); evidence: {}",
                w.invariant,
                w.steps.len(),
                w.context_switches,
                serde_json::to_value(level).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
            ),
        });
    }
    match result.verdict {
        Verdict::Inconclusive => diagnostics.push(ModelDiagnostic {
            code: "MW7002".into(),
            severity: "info".into(),
            message: "exploration stopped at a resource limit before covering the declared bounds"
                .into(),
        }),
        Verdict::Unsupported => {
            for u in &result.unsupported {
                diagnostics.push(ModelDiagnostic {
                    code: "MW7003".into(),
                    severity: "warning".into(),
                    message: format!(
                        "actor {}: unsupported behavior at {}: {}",
                        model.actors[u.actor].name,
                        u.src.label(),
                        u.detail
                    ),
                });
            }
        }
        _ => {}
    }
    match level {
        EvidenceLevel::PostgresDidNotReproduce => diagnostics.push(ModelDiagnostic {
            code: "MW7101".into(),
            severity: "info".into(),
            message: "PostgreSQL did not reproduce the model's counterexample".into(),
        }),
        EvidenceLevel::RuntimeInconclusive => diagnostics.push(ModelDiagnostic {
            code: "MW7102".into(),
            severity: "info".into(),
            message: runtime.note.clone(),
        }),
        _ => {}
    }
    if let Some(sv) = &source_verification
        && witness.is_some()
        && sv.status != "reproduced"
    {
        diagnostics.push(ModelDiagnostic {
            code: "MW7103".into(),
            severity: "info".into(),
            message: format!(
                "the original code did not reproduce the counterexample (source verification: {}){}",
                sv.status,
                sv.problems
                    .first()
                    .map(|p| format!(": {p}"))
                    .unwrap_or_default()
            ),
        });
    }
    let extraction = extraction_reports(model, &opts.load);
    let epistemic = witness
        .as_ref()
        .map(|w| crate::twin::analyze(model, &w.schedule))
        .filter(|r| !r.twins.is_empty());

    let disabled_because = if let Some(why) = &dpor_refusal {
        Some(format!(
            "dynamic partial-order reduction was requested but is not applicable ({why}); sleep sets ran instead"
        ))
    } else if explore_opts.por && model.bounds.max_context_switches.is_some() {
        Some(
            "maxContextSwitches is set (a commuted ordering can need more context switches)"
                .to_owned(),
        )
    } else if !model.options.por {
        Some("disabled in the scenario options".to_owned())
    } else {
        None
    };
    ModelCheckReport {
        kind: REPORT_KIND.into(),
        schema_version: if source_dpor.is_some()
            || source_verification
                .as_ref()
                .is_some_and(crate::source_types::SourceVerification::has_phase10_fields)
        {
            5
        } else if epistemic.is_some()
            || model.bounds.max_in_flight > 0
            || model.bounds.crash_policy != crate::model::CrashPolicy::Any
            || model.has_pools()
            || use_dpor
            || opts.profile_storage
            || !result.stuck.is_empty()
            || source_verification
                .as_ref()
                .is_some_and(crate::source_types::SourceVerification::has_phase9_fields)
        {
            4
        } else if !opts.load.schema.is_empty()
            || source_verification
                .as_ref()
                .is_some_and(crate::source_types::SourceVerification::has_phase8_fields)
        {
            3
        } else if extraction.is_empty() && source_verification.is_none() {
            REPORT_SCHEMA_VERSION
        } else {
            2
        },
        model_version: MODEL_VERSION.into(),
        tool: ToolInfo {
            name: "midwrite".into(),
            version: opts.tool_version.clone(),
        },
        scenario: ScenarioInfo {
            name: model.name.clone(),
            description: model.description.clone(),
            scenario_version: crate::scenario::SCHEMA_VERSION,
            file: opts.scenario_file.clone(),
            source_revision: opts.source_revision.clone(),
        },
        database: database_info(model),
        actors: actor_infos(model),
        bounds: BoundsInfo {
            reduction: if source_dpor.is_some() {
                Some("source-dpor".to_owned())
            } else {
                use_dpor.then(|| "dpor".to_owned())
            },
            source_dpor_mode: source_dpor.map(|m| m.as_str().to_owned()),
            declared: model.bounds.clone(),
            partial_order_reduction: use_dpor
                || (explore_opts.por && model.bounds.max_context_switches.is_none()),
            partial_order_reduction_disabled_because: disabled_because,
            strategy: match (
                use_dpor || (explore_opts.por && model.bounds.max_context_switches.is_none()),
                explore_opts.strategy,
            ) {
                (true, _) | (_, Strategy::Dfs) => "dfs",
                _ => "bfs",
            }
            .to_owned(),
        },
        exploration: exploration_info(model, &result, opts.profile_storage, source_dpor.is_some()),
        invariants,
        counterexample: witness,
        evidence: Evidence { level, summary },
        runtime,
        assumptions: model.assumptions.clone(),
        diagnostics,
        timing: opts.include_timing.then_some(Timing {
            wall_ms: result.metrics.wall_ms,
            first_violation_ms: result.metrics.first_violation_ms,
        }),
        extraction,
        source_verification,
        schema_facts: opts.load.schema.clone(),
        epistemic,
    }
}

/// A plain-text summary for the terminal.
pub fn render_text(model: &Model, r: &ModelCheckReport, verbose: bool) -> String {
    let mut out = String::new();
    out.push_str(&format!("{}: {}\n", r.scenario.name, r.exploration.verdict));
    out.push_str(&format!(
        "  isolation {} · {} actor(s) · {} state(s), {} transition(s){}\n",
        r.database.isolation,
        r.actors.len(),
        r.exploration.metrics.states,
        r.exploration.metrics.transitions,
        if r.bounds.reduction.as_deref() == Some("dpor") {
            " (dynamic partial-order reduction on)"
        } else if r.bounds.partial_order_reduction {
            " (sleep-set reduction on)"
        } else {
            ""
        }
    ));
    if let Some(w) = &r.counterexample {
        out.push('\n');
        out.push_str(&crate::witness::render_text(model, w));
    }
    if let Some(e) = &r.epistemic {
        out.push_str("\nApplication knowledge at each lost acknowledgment:\n");
        for t in &e.twins {
            out.push_str(&format!(
                "  transition {} — {} of {}: PostgreSQL {} (reported schedule); application knowledge: {}\n",
                t.transition,
                t.operation,
                t.actor,
                t.actual.label(),
                t.knowledge
            ));
            out.push_str(&format!(
                "    other world ({}): {}{}\n",
                t.other.label(),
                t.twin,
                if t.detail.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", t.detail)
                }
            ));
            for (a, b) in t.actual_verdicts.iter().zip(&t.twin_verdicts) {
                out.push_str(&format!(
                    "    invariant `{}`: {} here, {} in the other world\n",
                    a.invariant,
                    if a.holds { "holds" } else { "VIOLATED" },
                    if b.holds { "holds" } else { "VIOLATED" }
                ));
            }
            if let Some((here, other)) = &t.first_difference {
                out.push_str(&format!(
                    "    first observation that differs: here `{here}` / other world `{other}`\n"
                ));
            }
            out.push_str(&format!("    {}\n", t.reading));
        }
    }
    out.push_str(&format!(
        "\nEvidence: {:?} — {}\n",
        r.evidence.level, r.evidence.summary
    ));
    if r.runtime.performed || r.runtime.status == "blocked" {
        out.push_str(&format!("PostgreSQL validation: {}", r.runtime.status));
        if let Some(v) = &r.runtime.postgres_version {
            out.push_str(&format!(" ({v})"));
        }
        out.push('\n');
        for o in &r.runtime.observations {
            out.push_str(&format!(
                "  {} #{:<2} {:<3} predicted: {:<28} observed: {}\n",
                if o.agree { "=" } else { "≠" },
                o.transition,
                o.actor,
                o.predicted,
                o.observed
            ));
        }
        if !r.runtime.performed {
            out.push_str(&format!("  {}\n", r.runtime.note));
        }
    }
    for x in &r.extraction {
        out.push_str(&format!(
            "\nExtraction ({}): {} — {} represented, {} assumed, {} unsupported operation(s); declared contract: {}\n",
            x.actor,
            x.entry.selector,
            x.source_coverage.represented_operations,
            x.source_coverage.assumed_operations,
            x.source_coverage.unsupported_operations,
            x.entry.contract_status
        ));
        for f in &x.source_facts {
            out.push_str(&format!("  source:   {f}\n"));
        }
        for a in &x.scenario_assumptions {
            out.push_str(&format!("  declared: {a}\n"));
        }
    }
    if let Some(sv) = &r.source_verification {
        out.push_str(&format!(
            "\nOriginal-code verification: {} (harness `{}`{})\n",
            sv.status,
            sv.harness,
            sv.postgres_version
                .as_deref()
                .map(|v| format!(", {v}"))
                .unwrap_or_default()
        ));
        for run in &sv.runs {
            out.push_str(&format!(
                "  run {}: coordination {}, statements {}, final rows {}, outcomes {}, invariant on observed rows: {}{}\n",
                run.run,
                run.coordination,
                if run.statements_agree { "agree" } else { "differ" },
                if run.tables_agree { "agree" } else { "differ" },
                if run.outcomes_agree { "agree" } else { "differ" },
                run.invariant,
                if run.reproduced { " → reproduced" } else { "" }
            ));
        }
        for c in &sv.statements {
            out.push_str(&format!(
                "  {} #{:<2} {:<8} model: {} [{}]\n           sql:   {} [{}]\n           predicted {} · observed {}\n",
                if c.status == "agree" { "=" } else { "≠" },
                c.transition,
                c.actor,
                c.model,
                c.model_source,
                if c.sql.is_empty() { "(none)" } else { &c.sql },
                c.runtime_source,
                c.predicted,
                c.observed
            ));
            for d in &c.differences {
                out.push_str(&format!("           difference: {d}\n"));
            }
        }
        for o in &sv.outside_model {
            out.push_str(&format!(
                "  outside the model (declared preconditions): {o}\n"
            ));
        }
        for u in &sv.unexpected {
            out.push_str(&format!("  unexpected: {u}\n"));
        }
        for (actor, model_outcome, runtime) in &sv.actors {
            out.push_str(&format!(
                "  actor {actor}: model {model_outcome}, original code {runtime}\n"
            ));
        }
        for p in &sv.problems {
            out.push_str(&format!("  problem: {p}\n"));
        }
        if !sv.note.is_empty() {
            out.push_str(&format!("  {}\n", sv.note));
        }
    }
    let t = &r.exploration.truncation;
    if t.resource_limit() {
        out.push_str("\nTruncated by:");
        for (on, name) in [
            (t.max_states, "maxStates"),
            (t.max_transitions, "maxTransitions"),
            (t.max_wall_time, "maxWallMs"),
            (t.max_memory, "maxMemoryBytes"),
            (t.max_steps, "maxSteps"),
        ] {
            if on {
                out.push_str(&format!(" {name}"));
            }
        }
        out.push('\n');
    }
    for u in &r.exploration.unsupported {
        out.push_str(&format!(
            "\nUnsupported: actor {} at {}: {}\n",
            u.actor, u.location, u.detail
        ));
    }
    out.push_str("\nWhat this does and does not say: ");
    out.push_str(match r.exploration.verdict.as_str() {
        "NO_COUNTEREXAMPLE_WITHIN_BOUNDS" => "no invariant violation exists in any behavior of the modeled actors within the declared bounds and assumptions. It does not prove the application race-free.\n",
        "COUNTEREXAMPLE_FOUND" if r.evidence.level == EvidenceLevel::OriginalCodeReproduced => "the application's original code, run under this schedule with the declared inputs and preconditions on a disposable PostgreSQL, violated the invariant. Whether such concurrent requests occur in production, and whether the declared preconditions hold there, is not established.\n",
        "COUNTEREXAMPLE_FOUND" => "a bounded schedule of the modeled actors violates the invariant under the stated assumptions. It is not by itself a confirmed application bug.\n",
        "INCONCLUSIVE" => "the search ended at a resource limit; nothing is established beyond the explored part.\n",
        _ => "the scenario cannot be verified: it reaches unsupported behavior.\n",
    });
    if verbose {
        out.push_str("\nAssumptions:\n");
        for a in &r.assumptions {
            out.push_str(&format!("  - {a}\n"));
        }
    }
    for d in &r.diagnostics {
        out.push_str(&format!("{} {}: {}\n", d.severity, d.code, d.message));
    }
    out
}

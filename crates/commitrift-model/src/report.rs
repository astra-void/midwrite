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

pub const REPORT_KIND: &str = "commitrift.model-check";
pub const REPORT_SCHEMA_VERSION: u32 = 1;
/// Version of the model semantics (docs/phase6/SEMANTICS.md). Bumped when a
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
}

/// Diagnostic codes of the concurrency category. They are separate from the
/// CP1001–CP9001 failure-path codes and never reuse them.
pub fn explain(code: &str) -> Option<&'static str> {
    Some(match code {
        "CP7001" => {
            "A declared invariant is violated by a bounded concurrent schedule of the modeled actors. This is a MODEL counterexample: it is a possible execution under the scenario's assumptions (schema, isolation level, actors, bounds), not proof that the application is exposed. Read the witness schedule and the assumptions; replay it with --validate-postgres to see whether a real PostgreSQL permits it."
        }
        "CP7002" => {
            "The search stopped at a resource limit (states, transitions, steps, time, memory) before every behavior within the bounds was explored. No counterexample was found in the part explored; that says nothing about the rest."
        }
        "CP7003" => {
            "The scenario reaches an operation the model does not understand (raw SQL, an unsupported construct). Unknown behavior is never treated as a no-op, so exploration of those paths stops and the result cannot be a verification."
        }
        "CP7101" => {
            "PostgreSQL did not reproduce the model's counterexample: some statement behaved differently from the model's prediction. Either the model is imprecise for this construct (please report the mismatch) or PostgreSQL prevents the schedule. The model result must not be reported as a confirmed race."
        }
        "CP7102" => {
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

fn exploration_info(model: &Model, r: &ExploreResult) -> ExplorationInfo {
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
        stuck_states: r.stuck.len(),
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
    let result = explore_with(model, &explore_opts);
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

    if let Some(w) = &witness {
        diagnostics.push(ModelDiagnostic {
            code: "CP7001".into(),
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
            code: "CP7002".into(),
            severity: "info".into(),
            message: "exploration stopped at a resource limit before covering the declared bounds"
                .into(),
        }),
        Verdict::Unsupported => {
            for u in &result.unsupported {
                diagnostics.push(ModelDiagnostic {
                    code: "CP7003".into(),
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
            code: "CP7101".into(),
            severity: "info".into(),
            message: "PostgreSQL did not reproduce the model's counterexample".into(),
        }),
        EvidenceLevel::RuntimeInconclusive => diagnostics.push(ModelDiagnostic {
            code: "CP7102".into(),
            severity: "info".into(),
            message: runtime.note.clone(),
        }),
        _ => {}
    }

    let disabled_because = if explore_opts.por && model.bounds.max_context_switches.is_some() {
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
        schema_version: REPORT_SCHEMA_VERSION,
        model_version: MODEL_VERSION.into(),
        tool: ToolInfo {
            name: "commitrift".into(),
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
            declared: model.bounds.clone(),
            partial_order_reduction: explore_opts.por
                && model.bounds.max_context_switches.is_none(),
            partial_order_reduction_disabled_because: disabled_because,
            strategy: match (
                explore_opts.por && model.bounds.max_context_switches.is_none(),
                explore_opts.strategy,
            ) {
                (true, _) | (_, Strategy::Dfs) => "dfs",
                _ => "bfs",
            }
            .to_owned(),
        },
        exploration: exploration_info(model, &result),
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
        if r.bounds.partial_order_reduction {
            " (sleep-set reduction on)"
        } else {
            ""
        }
    ));
    if let Some(w) = &r.counterexample {
        out.push('\n');
        out.push_str(&crate::witness::render_text(model, w));
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

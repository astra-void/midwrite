//! End-to-end differential verification against the original application
//! (Phase 7, feature `postgres`).
//!
//! Given a model counterexample of a scenario whose actors were extracted
//! from TypeScript, this module
//!
//! 1. derives a *plan* from the schedule: for each transition, the SQL
//!    statement (kind, table, source region) the application should send,
//!    and whether PostgreSQL should make it wait;
//! 2. has `commitrift_replay::source` run the original code of every actor
//!    through an authorized harness, releasing statements in plan order;
//! 3. compares, statement by statement, what the model predicted with what
//!    the application sent and PostgreSQL answered (rows, affected rows,
//!    SQLSTATEs, lock waits), the final committed rows, the actors'
//!    outcomes, and evaluates the declared invariant **on the observed
//!    rows**.
//!
//! Three comparisons are kept separate (`docs/phase7/DIFFERENTIAL-VERIFICATION.md`):
//! model vs PostgreSQL replay of model SQL (`oracle`), Drizzle-generated SQL
//! vs model statements (here, per statement), and original application
//! execution vs model (here, outcomes and state). Only a run in which all
//! of them agree, nothing unexpected happened, and the invariant is violated
//! on the observed database is `original-code-reproduced`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde_json::Value as Json;

use commitrift_replay::source::{
    self, ActorLaunch, ConnectionLossStep, CrashStep, Expected, Fate, InFlightStep, LoadedSource,
    Observed, PlanStep, Region, RestartStep, RunRequest, ServerCompleteStep, SourceRun, SqlKind,
    StuckStep,
};

use crate::db::{ColumnDefault, INITIAL_TXN, RowId, TableSchema, Version};
use crate::engine::{Action, Expect, Recorder, apply, in_flight_ops, initial_state};
use crate::exec::StmtResult;
use crate::invariant::Invariant;
use crate::ir::{Conn, Op, Origin, ProjItem, SourceRef, StmtKind};
use crate::model::Model;
use crate::provider::EntryInfo;
pub use crate::source_types::{
    CompletionCheck, CrashCheck, EffectComparison, InFlightCheck, LossCheck, RecoveryVerification,
    RestartCheck, RunSummary, SourceActor, SourceSpec, SourceVerification, StatementComparison,
};
use crate::state::{ClosedStmt, ExternalEvent, State, TKind, TStatus, Transition};
use crate::value::{Mode, Row, Value};

impl SourceVerification {
    fn refused(status: &str, harness: &str, problem: String) -> Self {
        Self {
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
            problems: vec![problem],
            assumptions: Vec::new(),
            postgres_version: None,
            isolation: String::new(),
            note: String::new(),
            ..Self::default()
        }
    }
}

const NOTE: &str = "Each actor's ORIGINAL application code ran in its own worker process (an authorized harness) against a private, disposable PostgreSQL database; every SQL statement the application sent was held until the coordinator released it in the order of the model's counterexample, and PostgreSQL's answers (rows, row counts, SQLSTATEs, lock waits reported by pg_stat_activity) were recorded. The invariant was evaluated on the rows the run left in the database. This is evidence about this application, this input and this schedule on this PostgreSQL; it is not a claim about production traffic.";

// ---------------------------------------------------------------------------
// Plan
// ---------------------------------------------------------------------------

/// What the model predicts for one released statement or effect.
#[derive(Clone, Debug)]
struct Prediction {
    transition: usize,
    actor: usize,
    stmt: Option<ClosedStmt>,
    expect: Expect,
    src: Option<SourceRef>,
    label: String,
    /// Phase 8: an external effect (payload, deduplicated by the channel).
    effect: Option<(Row, bool)>,
}

/// Phase 8: what the model predicts at a planned crash.
#[derive(Clone, Debug)]
struct CrashPrediction {
    transition: usize,
    actor: usize,
    /// 1-based execution number (runtime numbering).
    attempt: u32,
    boundary: String,
    /// Committed rows right after the crash.
    tables: BTreeMap<String, Vec<Row>>,
    /// Phase 9: the fate of the operation in flight, and its description.
    in_flight: Option<(Fate, String)>,
}

/// Phase 9: what the model predicts when the server completes a held
/// operation.
#[derive(Clone, Debug)]
struct CompletionPrediction {
    transition: usize,
    actor: usize,
    attempt: u32,
    tables: BTreeMap<String, Vec<Row>>,
}

/// Phase 8: a new execution of an actor.
#[derive(Clone, Debug)]
struct RestartPrediction {
    transition: usize,
    actor: usize,
    attempt: u32,
    mechanism: String,
}

/// Phase 10: what the model predicts when a connection breaks under a live
/// worker.
#[derive(Clone, Debug)]
struct LossPrediction {
    transition: usize,
    actor: usize,
    attempt: u32,
    fate: Fate,
    label: String,
    /// Committed rows once the server side of the loss has settled: with the
    /// server's completion for an answered operation, without it otherwise.
    tables: BTreeMap<String, Vec<Row>>,
}

#[derive(Default)]
struct Plan {
    steps: Vec<PlanStep>,
    predictions: Vec<Prediction>,
    crashes: Vec<CrashPrediction>,
    restarts: Vec<RestartPrediction>,
    completions: Vec<CompletionPrediction>,
    losses: Vec<LossPrediction>,
}

fn region(src: &SourceRef) -> Option<Region> {
    (src.origin == Origin::Extracted && !src.file.is_empty()).then(|| Region {
        file: src.file.clone(),
        start: src.line,
        end: src.end_line.max(src.line),
    })
}

fn sql_table(model: &Model, t: u16) -> String {
    model.schema.table(t).name.clone()
}

fn kind_of(stmt: &ClosedStmt) -> SqlKind {
    match stmt {
        ClosedStmt::Select { .. } | ClosedStmt::Count { .. } => SqlKind::Select,
        ClosedStmt::Insert { .. } | ClosedStmt::InsertMany { .. } => SqlKind::Insert,
        ClosedStmt::Update { .. } => SqlKind::Update,
        ClosedStmt::Delete { .. } => SqlKind::Delete,
    }
}

fn table_of(stmt: &ClosedStmt) -> u16 {
    stmt.table()
}

/// A model value as the JSON the runtime reports.
fn value_json(v: &Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(n) => Json::from(*n),
        Value::Str(s) => Json::String(s.clone()),
        Value::Gen(g) => serde_json::to_value(g).unwrap_or(Json::Null),
        Value::Row(r) => Json::Object(r.iter().map(|(k, v)| (k.clone(), value_json(v))).collect()),
        Value::List(items) => Json::Array(items.iter().map(value_json).collect()),
        Value::Undefined(_) | Value::Map(_) | Value::Set(_) => {
            serde_json::to_value(v).unwrap_or(Json::Null)
        }
    }
}

fn effect_label(channel: &str, key: Option<&Value>) -> String {
    format!(
        "EXTERNAL {channel}{}",
        key.map(|k| format!(" key={}", k.render()))
            .unwrap_or_default()
    )
}

/// The operation an actor performs next, as the coordinator must observe it
/// when the model crashes the actor's process before it: announced (and
/// held), or — `waiting` — a released statement waiting for a lock.
/// The boundary text of a crash while a task waits for a pool connection.
const POOL_WAIT: &str = "while waiting for a connection of the pool";

fn checkpoint(
    model: &Model,
    s: &State,
    a: usize,
    transition: usize,
) -> Result<(Option<Expected>, bool, String), String> {
    let name = &model.actors[a].name;
    let running: Vec<_> = s.actors[a]
        .threads
        .iter()
        .filter(|t| t.status == TStatus::Running)
        .collect();
    let [thread] = running.as_slice() else {
        return Err(format!(
            "the model crashes {name} while {} of its tasks run: a crash checkpoint is only observable with one running task",
            running.len()
        ));
    };
    if thread.lease == crate::state::Lease::Waiting {
        return Ok((
            None,
            false,
            format!("{POOL_WAIT} (the task has sent nothing)"),
        ));
    }
    if let Some(p) = &thread.pending {
        let blocker_active = p.blocked_on.is_some_and(|b| s.txn_active(b));
        if blocker_active {
            return Ok((
                None,
                true,
                "while its statement waits for a lock (in flight, not executed)".to_owned(),
            ));
        }
        // The blocker has finished: PostgreSQL lets the statement proceed at
        // once, while the model resumes it only at a later transition. At
        // the crash the statement may already have executed. Inside a
        // transaction that never commits, the outcome is the same; an
        // autocommit statement may have committed by itself.
        if p.conn == Conn::Pool {
            return Err(format!(
                "the model crashes {name} after the lock its autocommit statement waited for was released: PostgreSQL may already have executed and committed the statement, which the model rolls back; the crash point is not reproducible"
            ));
        }
        return Ok((
            None,
            true,
            "after the lock its statement waited for was released (in flight: the statement may have executed in its transaction, which never commits)".to_owned(),
        ));
    }
    let step = &model.actors[a].codes[thread.code as usize][thread.pc as usize];
    let (kind, table, key, label) = match &step.op {
        Op::Db { stmt } => {
            let kind = match &stmt.kind {
                StmtKind::Select { .. } | StmtKind::SelectCount { .. } => SqlKind::Select,
                StmtKind::Update { .. } => SqlKind::Update,
                StmtKind::Insert { .. } => SqlKind::Insert,
                StmtKind::Delete { .. } => SqlKind::Delete,
            };
            let table = stmt.kind.table().to_owned();
            let label = format!("{kind:?} {table}").to_uppercase();
            (kind, Some(table), None, label)
        }
        Op::Begin { .. } => (SqlKind::Begin, None, None, "BEGIN".to_owned()),
        // Phase 13: the worker holds the response after the entry returned.
        Op::Respond => (SqlKind::Respond, None, None, "RESPONSE".to_owned()),
        Op::Commit => (SqlKind::Commit, None, None, "COMMIT".to_owned()),
        Op::Rollback => (SqlKind::Rollback, None, None, "ROLLBACK".to_owned()),
        Op::External { channel, key, .. } => {
            let key = match key {
                Some(e) => e
                    .eval(&s.actors[a].locals, None, Mode::Js)
                    .map_err(|e| format!("the idempotency key at the crash point: {e}"))?,
                None => Value::Null,
            };
            let label = effect_label(channel, (key != Value::Null).then_some(&key));
            (
                SqlKind::Effect,
                Some(channel.clone()),
                Some(value_json(&key)),
                label,
            )
        }
        other => {
            return Err(format!(
                "the model crashes {name} before an operation that is not observable at runtime ({other:?})"
            ));
        }
    };
    let boundary = format!("before {label} (announced, never sent)");
    Ok((
        Some(Expected {
            actor: a,
            kind,
            table,
            region: region(&step.src),
            expect_block: false,
            transition,
            label,
            key,
        }),
        false,
        boundary,
    ))
}

fn mechanism(model: &Model, a: usize) -> String {
    match &model.actors[a].retry {
        Some(r) => format!(
            "declared retry `{:?}`{}: a new execution of the harness entry with the original input, in a new process",
            r.trigger,
            if r.guaranteed {
                " (guaranteed)"
            } else {
                " (optional)"
            }
        ),
        None => "undeclared".to_owned(),
    }
}

/// The plan for `schedule`, and what the model predicts for each released
/// statement, crash and restart.
fn plan(model: &Model, schedule: &[Transition]) -> Result<Plan, String> {
    let mut state = initial_state(model);
    let mut out = Plan::default();
    // Completions realized together with the crash (answered at once).
    let mut skip_completion: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    for (i, t) in schedule.iter().enumerate() {
        // The visible operation this transition executes (before it runs).
        let src = state
            .actors
            .get(t.actor as usize)
            .and_then(|a| a.threads.get(t.thread as usize))
            .and_then(|th| {
                model.actors[t.actor as usize]
                    .codes
                    .get(th.code as usize)
                    .and_then(|c| c.get(th.pc as usize))
            })
            .map(|s| s.src.clone());
        match t.kind {
            TKind::Deadlock => return Err("the counterexample picks a deadlock victim: victim steering is not done against original code".to_owned()),
            TKind::Replicate => return Err("the counterexample depends on read-replica lag: replication is not steered against original code".to_owned()),
            TKind::LostCommitted | TKind::LostNotCommitted => {
                // A broken connection with the process alive (Phase 10): the
                // proxy closes the client's connection; the worker is not
                // touched and its application code goes on.
                let a = t.actor as usize;
                let reached = t.kind == TKind::LostCommitted;
                let (next, waiting, _) = checkpoint(model, &state, a, i + 1)?;
                let Some(operation) = next.filter(|_| !waiting) else {
                    return Err(format!(
                        "{} has no operation whose connection can be broken at runtime",
                        model.actors[a].name
                    ));
                };
                if !matches!(
                    operation.kind,
                    SqlKind::Commit | SqlKind::Insert | SqlKind::Update | SqlKind::Delete
                ) {
                    return Err(format!(
                        "{}: only a COMMIT or an autocommit write can have its connection broken at runtime (the model breaks {})",
                        model.actors[a].name, operation.label
                    ));
                }
                let applied = apply(model, &state, *t, &mut Recorder::off())
                    .map_err(|u| format!("the schedule reaches unsupported behavior: {}", u.detail))?;
                let mut immediate = None;
                let fate = if !reached {
                    Fate::NotReached
                } else {
                    let orphan = applied
                        .state
                        .orphans
                        .iter()
                        .find(|o| {
                            o.actor as usize == a
                                && !state.orphans.iter().any(|p| p.actor == o.actor && p.seq == o.seq)
                        })
                        .ok_or("the model holds no operation for an actor whose connection broke after the server received it")?;
                    // The server completing the operation right away is one
                    // observable fact (its answer, withheld); completing it
                    // after the application has done something else is another
                    // (the operation held until then).
                    match schedule.get(i + 1) {
                        Some(n)
                            if n.kind == TKind::ServerComplete
                                && n.actor == orphan.actor
                                && u32::from(n.thread) == orphan.seq =>
                        {
                            immediate = Some(*n);
                            Fate::AnsweredAndWithheld
                        }
                        _ => Fate::HeldForLaterDelivery,
                    }
                };
                let mut settled = applied.state.clone();
                if let Some(n) = immediate {
                    settled = apply(model, &settled, n, &mut Recorder::off())
                        .map_err(|u| format!("the schedule reaches unsupported behavior: {}", u.detail))?
                        .state;
                    skip_completion.insert(i + 1);
                }
                let label = format!(
                    "{} with its connection broken, {}",
                    operation.label,
                    match fate {
                        Fate::NotReached => "never received by the server",
                        Fate::AnsweredAndWithheld =>
                            "completed by the server, answer lost with the connection",
                        Fate::HeldForLaterDelivery =>
                            "received by the server only later (after the application has gone on)",
                    }
                );
                out.steps.push(PlanStep::ConnectionLoss(ConnectionLossStep {
                    actor: a,
                    transition: i + 1,
                    operation,
                    fate,
                    label: label.clone(),
                }));
                out.losses.push(LossPrediction {
                    transition: i + 1,
                    actor: a,
                    attempt: state.actors[a].attempt + 1,
                    fate,
                    label,
                    tables: committed(model, &settled),
                });
                state = applied.state;
                continue;
            }
            TKind::ServerComplete => {
                let orphan = state
                    .orphans
                    .iter()
                    .find(|o| o.actor == t.actor && o.seq == u32::from(t.thread))
                    .ok_or("the schedule completes an operation the server does not hold")?;
                let attempt = orphan.attempt + 1;
                let applied = apply(model, &state, *t, &mut Recorder::off())
                    .map_err(|u| format!("the schedule reaches unsupported behavior: {}", u.detail))?;
                let tables = committed(model, &applied.state);
                if skip_completion.remove(&i) {
                    // Realized by the proxy answering at once (planned with
                    // the crash): no separate delivery.
                } else {
                    out.steps.push(PlanStep::ServerComplete(ServerCompleteStep {
                        actor: t.actor as usize,
                        transition: i + 1,
                        attempt,
                    }));
                    out.completions.push(CompletionPrediction {
                        transition: i + 1,
                        actor: t.actor as usize,
                        attempt,
                        tables,
                    });
                }
                state = applied.state;
                continue;
            }
            TKind::CrashInFlight => {
                let process = t.thread as usize;
                let ops = in_flight_ops(model, &state, process);
                let applied = apply(model, &state, *t, &mut Recorder::off())
                    .map_err(|u| format!("the schedule reaches unsupported behavior: {}", u.detail))?;
                let mut fates: BTreeMap<usize, Fate> = BTreeMap::new();
                for a in 0..model.actors.len() {
                    if model.process_of[a] != process
                        || !state.actors[a]
                            .threads
                            .iter()
                            .any(|th| th.status == TStatus::Running)
                    {
                        continue;
                    }
                    let Some(bit) = ops.iter().position(|(x, _, _)| *x == a) else {
                        // Not in flight: an ordinary crash checkpoint.
                        let (next, waiting, boundary) = checkpoint(model, &state, a, i + 1)?;
                        out.steps.push(PlanStep::Crash(CrashStep {
                            actor: a,
                            transition: i + 1,
                            next,
                            waiting,
                            may_have_run: waiting && boundary.starts_with("after the lock"),
                            pool_wait: boundary.starts_with(POOL_WAIT),
                            label: boundary.clone(),
                        }));
                        out.crashes.push(CrashPrediction {
                            transition: i + 1,
                            actor: a,
                            attempt: state.actors[a].attempt + 1,
                            boundary,
                            tables: committed(model, &applied.state),
                            in_flight: None,
                        });
                        continue;
                    };
                    let reaches = (t.actor >> bit) & 1 == 1;
                    let fate = if !reaches {
                        Fate::NotReached
                    } else {
                        let orphan = applied
                            .state
                            .orphans
                            .iter()
                            .find(|o| {
                                o.actor as usize == a
                                    && !state.orphans.iter().any(|p| p.actor == o.actor && p.seq == o.seq)
                            })
                            .ok_or("the model holds no operation for an actor that reached the server")?;
                        // Completions right after the crash happen before
                        // anything else does: the server answered at once.
                        let run = schedule[i + 1..]
                            .iter()
                            .take_while(|n| n.kind == TKind::ServerComplete);
                        if run.clone().any(|n| {
                            n.actor == orphan.actor && u32::from(n.thread) == orphan.seq
                        }) {
                            Fate::AnsweredAndWithheld
                        } else {
                            Fate::HeldForLaterDelivery
                        }
                    };
                    fates.insert(a, fate);
                    let (next, waiting, _) = checkpoint(model, &state, a, i + 1)?;
                    let Some(operation) = next.filter(|_| !waiting) else {
                        return Err(format!(
                            "{} has no operation that can be put in flight at runtime",
                            model.actors[a].name
                        ));
                    };
                    let label = format!(
                        "{} in flight, {}",
                        operation.label,
                        match fate {
                            Fate::NotReached => "never received by the server",
                            Fate::AnsweredAndWithheld =>
                                "completed by the server, answer withheld from the application",
                            Fate::HeldForLaterDelivery =>
                                "received by the server only later (after the application is gone)",
                        }
                    );
                    out.steps.push(PlanStep::InFlight(InFlightStep {
                        actor: a,
                        transition: i + 1,
                        operation,
                        fate,
                        label: label.clone(),
                    }));
                    out.crashes.push(CrashPrediction {
                        transition: i + 1,
                        actor: a,
                        attempt: state.actors[a].attempt + 1,
                        boundary: label.clone(),
                        tables: BTreeMap::new(),
                        in_flight: Some((fate, label)),
                    });
                }
                // The model's durable state right after the kill: the
                // completions the server performs at once are in it.
                let mut after = applied.state.clone();
                let mut immediate = 0;
                for n in schedule[i + 1..]
                    .iter()
                    .take_while(|n| n.kind == TKind::ServerComplete)
                {
                    if let Ok(x) = apply(model, &after, *n, &mut Recorder::off()) {
                        after = x.state;
                    }
                    immediate += 1;
                }
                let tables = committed(model, &after);
                for c in out.crashes.iter_mut().filter(|c| c.transition == i + 1) {
                    c.tables.clone_from(&tables);
                }
                // The completions that were part of the same kill are not
                // separate steps; the ones that were not (held) are.
                for (j, n) in schedule[i + 1..].iter().enumerate().take(immediate) {
                    let answered = out.steps.iter().any(|st| {
                        matches!(st, PlanStep::InFlight(f)
                            if f.transition == i + 1
                                && f.fate == Fate::AnsweredAndWithheld
                                && f.actor == usize::from(n.actor))
                    });
                    if answered {
                        skip_completion.insert(i + 1 + j);
                    }
                }
                state = applied.state;
                continue;
            }
            TKind::Crash => {
                let process = t.thread as usize;
                for a in 0..model.actors.len() {
                    if model.process_of[a] != process
                        || !state.actors[a]
                            .threads
                            .iter()
                            .any(|th| th.status == TStatus::Running)
                    {
                        continue;
                    }
                    let (next, waiting, boundary) = checkpoint(model, &state, a, i + 1)?;
                    out.steps.push(PlanStep::Crash(CrashStep {
                        actor: a,
                        transition: i + 1,
                        next,
                        waiting,
                        may_have_run: waiting && boundary.starts_with("after the lock"),
                        pool_wait: boundary.starts_with(POOL_WAIT),
                        label: boundary.clone(),
                    }));
                    out.crashes.push(CrashPrediction {
                        transition: i + 1,
                        actor: a,
                        attempt: state.actors[a].attempt + 1,
                        boundary,
                        tables: BTreeMap::new(),
                        in_flight: None,
                    });
                }
                let applied = apply(model, &state, *t, &mut Recorder::off())
                    .map_err(|u| format!("the schedule reaches unsupported behavior: {}", u.detail))?;
                state = applied.state;
                let tables = committed(model, &state);
                for c in out.crashes.iter_mut().filter(|c| c.transition == i + 1) {
                    c.tables.clone_from(&tables);
                }
                continue;
            }
            TKind::Retry => {
                let a = t.actor as usize;
                if model.process_of.iter().filter(|p| **p == model.process_of[a]).count() > 1 {
                    return Err(format!(
                        "{} shares its process with other tasks: a restart starts a new process, not one task of the old one",
                        model.actors[a].name
                    ));
                }
                let attempt = state.actors[a].attempt + 2;
                out.steps.push(PlanStep::Restart(RestartStep {
                    actor: a,
                    transition: i + 1,
                    attempt,
                    mechanism: mechanism(model, a),
                }));
                out.restarts.push(RestartPrediction {
                    transition: i + 1,
                    actor: a,
                    attempt,
                    mechanism: mechanism(model, a),
                });
                // A retry's own actions (if any) are of the new execution.
            }
            TKind::Step | TKind::Resume => {}
        }
        // The source of each actor's open transaction (its BEGIN): where
        // Drizzle issues the ROLLBACK after a failure.
        let tx_src: Vec<Option<SourceRef>> = (0..model.actors.len())
            .map(|a| {
                state.actors[a].begin.as_ref().and_then(|b| {
                    model.actors[a]
                        .codes
                        .get(b.code as usize)
                        .and_then(|c| c.get(b.pc as usize))
                        .map(|s| s.src.clone())
                })
            })
            .collect();
        let mut rec = Recorder::on();
        let applied = apply(model, &state, *t, &mut rec)
            .map_err(|u| format!("the schedule reaches unsupported behavior: {}", u.detail))?;
        state = applied.state;
        for action in rec.actions {
            let mut push = |kind: SqlKind,
                            table: Option<String>,
                            actor: u16,
                            stmt: Option<ClosedStmt>,
                            expect: Expect,
                            label: String,
                            key: Option<Json>,
                            effect: Option<(Row, bool)>| {
                let a = actor as usize;
                // A rollback after a failure is issued by the transaction
                // call (Drizzle's `transaction`), not by the failing
                // statement; another actor's transition may cause it.
                let src = if kind == SqlKind::Rollback && label.contains("after a failure") {
                    tx_src[a].clone().or_else(|| src.clone())
                } else {
                    src.clone()
                };
                out.steps.push(PlanStep::Release(Expected {
                    actor: a,
                    kind,
                    table,
                    region: src.as_ref().and_then(region),
                    expect_block: matches!(expect, Expect::Blocked(_)),
                    transition: i + 1,
                    label: label.clone(),
                    key,
                }));
                out.predictions.push(Prediction {
                    transition: i + 1,
                    actor: a,
                    stmt,
                    expect,
                    src: src.clone(),
                    label,
                    effect,
                });
            };
            match action {
                Action::Begin { actor, .. } => {
                    push(
                        SqlKind::Begin,
                        None,
                        actor,
                        None,
                        Expect::Ok,
                        "BEGIN".to_owned(),
                        None,
                        None,
                    );
                }
                Action::Stmt {
                    actor,
                    stmt,
                    resumed: false,
                    expect,
                    ..
                } => {
                    let label = crate::engine::describe(model, &stmt);
                    let table = Some(sql_table(model, table_of(&stmt)));
                    push(
                        kind_of(&stmt),
                        table,
                        actor,
                        Some(stmt),
                        expect,
                        label,
                        None,
                        None,
                    );
                }
                Action::Stmt {
                    actor,
                    resumed: true,
                    ..
                } => out.steps.push(PlanStep::Complete {
                    actor: actor as usize,
                    transition: i + 1,
                }),
                Action::Commit { actor, expect, .. } => {
                    push(
                        SqlKind::Commit,
                        None,
                        actor,
                        None,
                        expect,
                        "COMMIT".to_owned(),
                        None,
                        None,
                    );
                }
                Action::Rollback { actor } => {
                    push(
                        SqlKind::Rollback,
                        None,
                        actor,
                        None,
                        Expect::Ok,
                        "ROLLBACK".to_owned(),
                        None,
                        None,
                    );
                }
                Action::Abort { actor, conn } => {
                    if conn == Conn::Tx {
                        // The driver rolls the failed transaction back.
                        push(
                            SqlKind::Rollback,
                            None,
                            actor,
                            None,
                            Expect::Ok,
                            "ROLLBACK (after a failure)".to_owned(),
                            None,
                            None,
                        );
                    }
                }
                Action::External {
                    actor,
                    channel,
                    key,
                    payload,
                    deduplicated,
                    ..
                } => {
                    let label = effect_label(&channel, key.as_ref());
                    let key = Some(key.as_ref().map_or(Json::Null, value_json));
                    push(
                        SqlKind::Effect,
                        Some(channel),
                        actor,
                        None,
                        Expect::Ok,
                        label,
                        key,
                        Some((payload, deduplicated)),
                    );
                }
                Action::Respond { actor } => {
                    // Phase 13: release the held response to the client.
                    push(
                        SqlKind::Respond,
                        None,
                        actor,
                        None,
                        Expect::Ok,
                        "RESPONSE".to_owned(),
                        None,
                        None,
                    );
                }
                Action::Victim { .. } => {
                    return Err(
                        "deadlock victims are not executed against original code".to_owned()
                    );
                }
                // Planned above, from the transition.
                Action::Crash { .. } => {}
                Action::Orphan { .. } | Action::OrphanCommit { .. } => {
                    return Err(
                        "in-flight faults are not planned against original code yet".to_owned()
                    );
                }
            }
        }
    }
    Ok(out)
}

/// The coordinator plan of `schedule` (Phase 8: with crash and restart
/// steps), or why the schedule cannot be run against original code. Nothing
/// is executed.
pub fn plan_steps(model: &Model, schedule: &[Transition]) -> Result<Vec<PlanStep>, String> {
    plan(model, schedule).map(|p| p.steps)
}

fn committed(model: &Model, s: &State) -> BTreeMap<String, Vec<Row>> {
    model
        .schema
        .tables
        .iter()
        .enumerate()
        .map(|(i, t)| (t.name.clone(), s.committed_rows(model, i as u16)))
        .collect()
}

// ---------------------------------------------------------------------------
// SQL for the application's database
// ---------------------------------------------------------------------------

fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn literal(v: &Value) -> Result<String, String> {
    Ok(match v {
        Value::Null => "NULL".to_owned(),
        Value::Bool(b) => b.to_string().to_uppercase(),
        Value::Int(n) => n.to_string(),
        Value::Str(s) => format!("'{}'", s.replace('\'', "''")),
        Value::Gen(_) => return Err("a generated value in the initial state".to_owned()),
        Value::Row(_) => return Err("a row value in the initial state".to_owned()),
        Value::List(_) | Value::Map(_) | Value::Set(_) | Value::Undefined(_) => {
            return Err(format!("the value {} in the initial state", v.render()));
        }
    })
}

fn column_sql(t: &TableSchema, column: &str) -> String {
    t.columns
        .iter()
        .find(|c| c.name == column)
        .and_then(|c| c.sql_name.clone())
        .unwrap_or_else(|| column.to_owned())
}

/// Initial rows (with the application's column names) and sequence
/// positions, so that generated values start where the model's do.
fn seed_sql(model: &Model) -> Result<String, String> {
    let mut out = String::new();
    for (ti, t) in model.schema.tables.iter().enumerate() {
        for row in &model.initial_rows[ti] {
            let cols: Vec<String> = row.keys().map(|c| ident(&column_sql(t, c))).collect();
            let vals: Result<Vec<String>, String> = row.values().map(literal).collect();
            out.push_str(&format!(
                "INSERT INTO {} ({}) VALUES ({});\n",
                ident(&t.name),
                cols.join(", "),
                vals?.join(", ")
            ));
        }
    }
    for (ti, col, start) in model.sequences() {
        let t = model.schema.table(ti);
        out.push_str(&format!(
            "SELECT setval(pg_get_serial_sequence('{}', '{}'), {start}, false);\n",
            ident(&t.name).replace('\'', "''"),
            column_sql(t, &col).replace('\'', "''")
        ));
    }
    Ok(out)
}

/// One JSON array per table with the modeled columns under their model
/// names, in primary-key order.
fn final_queries(model: &Model) -> Vec<(String, String)> {
    model
        .schema
        .tables
        .iter()
        .map(|t| {
            let fields: Vec<String> = t
                .columns
                .iter()
                .map(|c| format!("'{}', {}", c.name.replace('\'', "''"), ident(&column_sql(t, &c.name))))
                .collect();
            let order: Vec<String> = if t.primary_key.is_empty() {
                t.columns.iter().map(|c| ident(&column_sql(t, &c.name))).collect()
            } else {
                t.primary_key.iter().map(|c| ident(&column_sql(t, c))).collect()
            };
            (
                t.name.clone(),
                format!(
                    "SELECT coalesce(json_agg(json_build_object({}) ORDER BY {}), '[]'::json) FROM {}",
                    fields.join(", "),
                    order.join(", "),
                    ident(&t.name)
                ),
            )
        })
        .collect()
}

fn json_to_value(v: &Json) -> Value {
    match v {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => n
            .as_i64()
            .map_or_else(|| Value::Str(n.to_string()), Value::Int),
        Json::String(s) => Value::Str(s.clone()),
        // Phase 14: a JSON array (a `json`/`jsonb` value, a multi-row
        // result).
        Json::Array(items) => Value::List(items.iter().map(json_to_value).collect()),
        Json::Object(o) => Value::Row(
            o.iter()
                .map(|(k, v)| (k.clone(), json_to_value(v)))
                .collect(),
        ),
    }
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

/// The cells (`table.column` with application names → value) a model
/// result row asserts.
fn expected_cells(model: &Model, stmt: &ClosedStmt, row: &Row) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let table_cells = |t: u16, nested: &Value, out: &mut Vec<(String, Value)>| {
        let schema = model.schema.table(t);
        match nested {
            Value::Row(r) => {
                for c in &schema.columns {
                    out.push((
                        format!("{}.{}", schema.name, column_sql(schema, &c.name)),
                        r.get(&c.name).cloned().unwrap_or(Value::Null),
                    ));
                }
            }
            _ => {
                // An unmatched LEFT JOIN: the key column is NULL.
                for k in &schema.primary_key {
                    out.push((
                        format!("{}.{}", schema.name, column_sql(schema, k)),
                        Value::Null,
                    ));
                }
            }
        }
    };
    match stmt {
        ClosedStmt::Select {
            table,
            joins,
            projection,
            ..
        } => {
            let joined = !joins.is_empty();
            match projection {
                None if !joined => table_cells(*table, &Value::Row(row.clone()), &mut out),
                None => {
                    for t in std::iter::once(*table).chain(joins.iter().map(|j| j.table)) {
                        let name = &model.schema.table(t).name;
                        table_cells(t, row.get(name).unwrap_or(&Value::Null), &mut out);
                    }
                }
                Some(items) => {
                    for (field, item) in items {
                        let v = row.get(field).cloned().unwrap_or(Value::Null);
                        match item {
                            ProjItem::Table(name) => {
                                if let Some(t) = model.schema.index(name) {
                                    let v = if joined { v } else { Value::Row(row.clone()) };
                                    table_cells(t, &v, &mut out);
                                }
                            }
                            ProjItem::Column(q) => {
                                let (t, c) = match q.split_once('.') {
                                    Some((t, c)) => (t.to_owned(), c.to_owned()),
                                    None => (model.schema.table(*table).name.clone(), q.clone()),
                                };
                                if let Some(schema) = model.schema.by_name(&t) {
                                    out.push((format!("{t}.{}", column_sql(schema, &c)), v));
                                }
                            }
                        }
                    }
                }
            }
        }
        // Phase 12: `RETURNING col, ...` asserts the listed columns only.
        ClosedStmt::Insert {
            table,
            returning_columns: Some(cols),
            ..
        }
        | ClosedStmt::InsertMany {
            table,
            returning_columns: Some(cols),
            ..
        } => {
            let schema = model.schema.table(*table);
            for c in cols {
                out.push((
                    format!("{}.{}", schema.name, column_sql(schema, c)),
                    row.get(c).cloned().unwrap_or(Value::Null),
                ));
            }
        }
        ClosedStmt::Insert { table, .. } | ClosedStmt::InsertMany { table, .. } => {
            table_cells(*table, &Value::Row(row.clone()), &mut out)
        }
        _ => {}
    }
    out
}

fn render_rows(rows: &[Json]) -> String {
    match rows {
        [] => "no row".to_owned(),
        [one] => one.to_string(),
        more => format!("{} rows", more.len()),
    }
}

/// Phase 12: the correspondence between the model's generated values
/// ([`Value::Gen`]) and the concrete values one runtime run produced. A
/// generated value matches the first concrete text it is compared with and
/// only that text afterwards; two generated values never match one text
/// (the model assumes distinct values, and a run that contradicts that is
/// reported as a difference, not absorbed).
#[derive(Default)]
struct GenMatch {
    forward: BTreeMap<crate::value::GenId, String>,
    backward: BTreeMap<String, crate::value::GenId>,
}

impl GenMatch {
    fn matches(&mut self, expected: &Value, observed: &Json) -> bool {
        match (expected, observed) {
            (Value::Gen(g), Json::String(text)) => {
                match (self.forward.get(g), self.backward.get(text)) {
                    (Some(t), _) => t == text,
                    // Phase 14: two clock readings the model keeps apart may
                    // read the same instant at run time (millisecond
                    // resolution): a clock reading maps to one text, a text
                    // may stand for several readings.
                    (None, Some(other)) if crate::db::is_clock(g) && crate::db::is_clock(other) => {
                        self.forward.insert(*g, text.clone());
                        true
                    }
                    (None, Some(_)) => false,
                    (None, None) => {
                        self.forward.insert(*g, text.clone());
                        self.backward.insert(text.clone(), *g);
                        true
                    }
                }
            }
            (Value::Gen(_), _) => false,
            (Value::Row(row), Json::Object(o)) => {
                row.len() == o.len()
                    && row
                        .iter()
                        .all(|(k, v)| o.get(k).is_some_and(|x| self.matches(v, x)))
            }
            (other, observed) => json_to_value(observed) == *other,
        }
    }
}

/// Phase 14: a checkpoint's transition, the model's committed rows there,
/// and the rows the run observed there (if it reached it).
type Checkpoint<'a> = (
    usize,
    &'a BTreeMap<String, Vec<Row>>,
    Option<&'a BTreeMap<String, Vec<Json>>>,
);

/// Whether committed rows observed at a checkpoint differ from the
/// model's, generated values matched through `generated` (extended on
/// success only).
fn tables_differ(
    generated: &mut GenMatch,
    model: &BTreeMap<String, Vec<Row>>,
    runtime: &BTreeMap<String, Vec<Row>>,
) -> bool {
    if !model.values().flatten().any(row_has_generated) {
        return runtime != model;
    }
    let runtime: BTreeMap<String, Vec<Json>> = runtime
        .iter()
        .map(|(t, rows)| {
            (
                t.clone(),
                rows.iter()
                    .map(|r| value_json(&Value::Row(r.clone())))
                    .collect(),
            )
        })
        .collect();
    !tables_match(generated, model, &runtime)
}

fn row_has_generated(row: &Row) -> bool {
    row.values().any(|v| match v {
        Value::Gen(_) => true,
        Value::Row(r) => row_has_generated(r),
        _ => false,
    })
}

/// Phase 12: whether the runtime's final rows are the model's, generated
/// values matched through `generated` (extended on success only).
fn tables_match(
    generated: &mut GenMatch,
    model: &BTreeMap<String, Vec<Row>>,
    runtime: &BTreeMap<String, Vec<Json>>,
) -> bool {
    fn assign(
        g: &GenMatch,
        model: &[Row],
        runtime: &[&Json],
        used: &mut Vec<bool>,
        i: usize,
    ) -> Option<GenMatch> {
        let Some(row) = model.get(i) else {
            return Some(GenMatch {
                forward: g.forward.clone(),
                backward: g.backward.clone(),
            });
        };
        for (j, candidate) in runtime.iter().enumerate() {
            if used[j] {
                continue;
            }
            let mut trial = GenMatch {
                forward: g.forward.clone(),
                backward: g.backward.clone(),
            };
            if trial.matches(&Value::Row(row.clone()), candidate) {
                used[j] = true;
                if let Some(done) = assign(&trial, model, runtime, used, i + 1) {
                    return Some(done);
                }
                used[j] = false;
            }
        }
        None
    }
    let names: BTreeSet<&String> = model.keys().chain(runtime.keys()).collect();
    let mut current = GenMatch {
        forward: generated.forward.clone(),
        backward: generated.backward.clone(),
    };
    for name in names {
        let m = model.get(name).map(Vec::as_slice).unwrap_or_default();
        let r: Vec<&Json> = runtime
            .get(name)
            .map(|v| v.iter().collect())
            .unwrap_or_default();
        if m.len() != r.len() {
            return false;
        }
        let mut used = vec![false; r.len()];
        match assign(&current, m, &r, &mut used, 0) {
            Some(next) => current = next,
            None => return false,
        }
    }
    *generated = current;
    true
}

fn compare_statement(
    model: &Model,
    p: &Prediction,
    o: Option<&Observed>,
    generated: &mut GenMatch,
) -> StatementComparison {
    let predicted = match &p.expect {
        Expect::Done(StmtResult::Row(None)) => "no row".to_owned(),
        Expect::Done(StmtResult::Row(Some(r))) => Value::Row(r.clone()).render(),
        Expect::Done(StmtResult::Count(n)) => format!("count {n}"),
        Expect::Done(StmtResult::Affected(n)) => format!("{n} row(s)"),
        Expect::Done(StmtResult::Rows(rows)) => format!("{} row(s) returned", rows.len()),
        Expect::Blocked(a) => format!("waits for {}", model.actors[*a as usize].name),
        Expect::Error(code) => format!("SQLSTATE {code}"),
        Expect::Ok => "ok".to_owned(),
    };
    let mut c = StatementComparison {
        transition: p.transition,
        actor: model.actors[p.actor].name.clone(),
        model: p.label.clone(),
        model_source: p.src.as_ref().map(SourceRef::label).unwrap_or_default(),
        predicted,
        sql: String::new(),
        runtime_source: String::new(),
        observed: "not observed".to_owned(),
        status: "not-observed".to_owned(),
        differences: Vec::new(),
    };
    if let Some((_, deduplicated)) = &p.effect {
        c.predicted = if *deduplicated {
            "deduplicated by the channel (declared contract)".to_owned()
        } else {
            "performed".to_owned()
        };
    }
    let Some(o) = o else { return c };
    c.sql = o.sql.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some((payload, deduplicated)) = &p.effect {
        c.runtime_source = o
            .frames
            .iter()
            .find(|f| {
                p.src
                    .as_ref()
                    .and_then(region)
                    .is_some_and(|r| r.contains(f))
            })
            .or(o.frames.first())
            .map(|f| format!("{}:{}:{}", f.file, f.line, f.column))
            .unwrap_or_else(|| "(no application frame)".to_owned());
        let receipt = &o.params["receipt"];
        let observed_dedup = receipt["deduplicated"].as_bool();
        c.observed = match (o.completed, observed_dedup) {
            (false, _) => "not acknowledged".to_owned(),
            (true, Some(true)) => "deduplicated by the channel (declared contract)".to_owned(),
            (true, _) => "performed".to_owned(),
        };
        if !o.completed {
            c.differences
                .push("the effect was not acknowledged".to_owned());
        }
        if observed_dedup != Some(*deduplicated) {
            c.differences.push(format!(
                "deduplicated: model {deduplicated}, channel double {observed_dedup:?}"
            ));
        }
        let runtime_payload = &o.params["payload"];
        for (k, v) in payload {
            match runtime_payload.get(k) {
                Some(r) if generated.matches(v, r) => {}
                Some(r) => c
                    .differences
                    .push(format!("payload.{k}: model {}, runtime {r}", v.render())),
                None => c.differences.push(format!(
                    "payload.{k}: not in the effect the application sent"
                )),
            }
        }
        c.status = if c.differences.is_empty() {
            "agree"
        } else {
            "differ"
        }
        .to_owned();
        return c;
    }
    c.runtime_source = o
        .frames
        .first()
        .map(|f| format!("{}:{}:{}", f.file, f.line, f.column))
        .unwrap_or_else(|| "(no application frame)".to_owned());
    if o.frames.is_empty() {
        c.differences.push(
            "no application stack frame: the statement is linked by kind and table only".to_owned(),
        );
    }
    if o.reconstructed {
        c.runtime_source
            .push_str(" (call chain restored across an un-awaited return)");
    }
    c.observed = match (&o.sqlstate, o.blocked, o.completed) {
        (Some(code), _, _) => format!("SQLSTATE {code}"),
        (None, true, false) => "waiting for a lock".to_owned(),
        (None, _, false) => "did not complete".to_owned(),
        (None, _, true) => match o.kind {
            Some(SqlKind::Select) => render_rows(&o.rows),
            Some(SqlKind::Insert) if !o.rows.is_empty() => render_rows(&o.rows),
            Some(SqlKind::Begin | SqlKind::Commit | SqlKind::Rollback) => o
                .command
                .clone()
                .unwrap_or_else(|| "ok".to_owned())
                .to_lowercase(),
            _ => format!("{} row(s)", o.row_count.unwrap_or(-1)),
        },
    };
    let mut diff = |d: String| c.differences.push(d);
    match &p.expect {
        Expect::Error(code) => {
            if o.sqlstate.as_deref() != Some(code.as_str()) {
                diff(format!("expected SQLSTATE {code}"));
            }
        }
        Expect::Blocked(_) => {
            if !o.blocked {
                diff("the model predicts a lock wait; PostgreSQL reported none".to_owned());
            }
        }
        _ if o.sqlstate.is_some() => diff(format!(
            "PostgreSQL failed the statement ({})",
            o.error.as_deref().unwrap_or("")
        )),
        Expect::Ok => {
            if o.kind == Some(SqlKind::Commit)
                && o.command
                    .as_deref()
                    .is_some_and(|c| !c.eq_ignore_ascii_case("COMMIT"))
            {
                diff(format!(
                    "COMMIT answered with {}",
                    o.command.as_deref().unwrap_or("")
                ));
            }
        }
        Expect::Done(result) => {
            let stmt = p.stmt.as_ref();
            match result {
                StmtResult::Affected(n) => {
                    if o.row_count != Some(*n) {
                        diff(format!(
                            "affected rows: model {n}, PostgreSQL {:?}",
                            o.row_count
                        ));
                    }
                }
                StmtResult::Count(n) => {
                    let got = o
                        .rows
                        .first()
                        .and_then(|r| r.as_object())
                        .and_then(|r| r.values().next())
                        .and_then(Json::as_i64);
                    if got != Some(*n) {
                        diff(format!("count: model {n}, PostgreSQL {got:?}"));
                    }
                }
                // Phase 14: a multi-row `INSERT ... RETURNING`: the rows in
                // order, each with the cells Drizzle requested.
                StmtResult::Rows(rows) => {
                    if o.rows.len() != rows.len() {
                        diff(format!(
                            "rows: model {}, PostgreSQL {}",
                            rows.len(),
                            o.rows.len()
                        ));
                    }
                    if let Some(stmt) = stmt {
                        for (i, (row, observed)) in rows.iter().zip(&o.rows).enumerate() {
                            let Some(observed) = observed.as_object() else {
                                diff(format!("row {i}: not an object"));
                                continue;
                            };
                            for (key, value) in expected_cells(model, stmt, row) {
                                match observed.get(&key) {
                                    Some(v) if generated.matches(&value, v) => {}
                                    Some(v) => diff(format!(
                                        "row {i}: {key}: model {}, PostgreSQL {v}",
                                        value.render()
                                    )),
                                    None => diff(format!(
                                        "row {i}: {key}: not in the result Drizzle requested"
                                    )),
                                }
                            }
                        }
                    }
                }
                StmtResult::Row(row) => {
                    let n = usize::from(row.is_some());
                    let returned = if matches!(o.kind, Some(SqlKind::Insert)) && o.rows.is_empty() {
                        o.row_count.unwrap_or(0) as usize
                    } else {
                        o.rows.len()
                    };
                    if returned != n {
                        diff(format!("rows: model {n}, PostgreSQL {returned}"));
                    }
                    if let (Some(row), Some(stmt), Some(observed)) =
                        (row, stmt, o.rows.first().and_then(Json::as_object))
                    {
                        for (key, value) in expected_cells(model, stmt, row) {
                            match observed.get(&key) {
                                Some(v) if generated.matches(&value, v) => {}
                                Some(v) => {
                                    diff(format!("{key}: model {}, PostgreSQL {v}", value.render()))
                                }
                                None => diff(format!("{key}: not in the result Drizzle requested")),
                            }
                        }
                    }
                }
            }
        }
    }
    c.status = if c
        .differences
        .iter()
        .any(|d| !d.starts_with("no application stack frame"))
    {
        "differ"
    } else {
        "agree"
    }
    .to_owned();
    c
}

/// A state whose committed rows are `tables` and whose external effect log
/// is `externals` (for evaluating an invariant on the observed run).
fn state_with(
    model: &Model,
    tables: &BTreeMap<String, Vec<Row>>,
    externals: Vec<ExternalEvent>,
) -> State {
    let mut s = initial_state(model);
    s.store.rows.clear();
    s.store.locks.clear();
    for (ti, t) in model.schema.tables.iter().enumerate() {
        for (n, row) in tables.get(&t.name).into_iter().flatten().enumerate() {
            s.store.rows.insert(
                RowId {
                    table: ti as u16,
                    actor: RowId::INITIAL,
                    n: n as u32,
                },
                std::sync::Arc::new(vec![Version {
                    values: Some(row.clone()),
                    creator: INITIAL_TXN,
                    commit: Some(0),
                }]),
            );
        }
    }
    s.externals = externals;
    s
}

fn rows_of(tables: &BTreeMap<String, Vec<Json>>) -> BTreeMap<String, Vec<Row>> {
    tables
        .iter()
        .map(|(t, rows)| {
            (
                t.clone(),
                rows.iter()
                    .filter_map(|r| match json_to_value(r) {
                        Value::Row(r) => Some(r),
                        _ => None,
                    })
                    .collect(),
            )
        })
        .collect()
}

fn describe_effect(
    names: &[String],
    channel: &str,
    key: Option<&Value>,
    actor: usize,
    attempt: u32,
    deduplicated: bool,
) -> String {
    format!(
        "{} by {}#{attempt}{}",
        effect_label(channel, key),
        names.get(actor).map_or("?", String::as_str),
        if deduplicated {
            " (deduplicated by the channel)"
        } else {
            ""
        }
    )
}

/// Everything the caller supplies.
pub struct VerifyRequest<'a> {
    pub model: &'a Model,
    pub schedule: &'a [Transition],
    pub final_state: &'a State,
    pub invariant: &'a Invariant,
    pub harness: &'a LoadedSource,
    pub spec: &'a SourceSpec,
    pub entries: &'a [(String, EntryInfo)],
    pub allow_execution: bool,
    pub repeat: u32,
    /// Phase 8: the schedule is a recovery probe of a model WITHOUT a
    /// counterexample: the invariant is expected to hold.
    pub probe: bool,
    /// Phase 8: the schema facts extracted from source, compared with the
    /// run database's catalog.
    pub extracted_schema: Option<&'a crate::schema_facts::ExtractedSchema>,
}

/// What the model says about the final state of a schedule with retries.
fn quiescence(model: &Model, s: &State) -> String {
    let en = crate::engine::enabled(model, s);
    let optional: Vec<String> = en
        .iter()
        .filter(|t| t.kind == TKind::Retry)
        .map(|t| model.actors[t.actor as usize].name.clone())
        .collect();
    let budget = s.retries >= model.bounds.max_retries;
    match (optional.is_empty(), budget) {
        (true, _) if model.actors.iter().all(|a| a.retry.is_none()) => {
            "every execution ended; no recovery mechanism is declared".to_owned()
        }
        (true, true) => format!(
            "every execution ended; the retry budget (maxRetries {}) is exhausted: further retries are outside the bounds, not excluded",
            model.bounds.max_retries
        ),
        (true, false) => {
            "every execution ended; no declared retry is enabled in this state".to_owned()
        }
        (false, _) => format!(
            "every execution ended; a further optional retry of {} remains possible within the bounds (not guaranteed: no eventual-recovery claim is made)",
            optional.join(", ")
        ),
    }
}

fn verify_schedule(req: &VerifyRequest<'_>) -> SourceVerification {
    let model = req.model;
    let harness_name = req.harness.manifest.harness.clone();
    // Eligibility: every actor extracted from source, selected by the same
    // function the harness runs.
    let mut launches = Vec::new();
    let mut assumptions = Vec::new();
    for actor in &model.actors {
        let Some(spec) = req.spec.actors.get(&actor.name) else {
            return SourceVerification::refused(
                "unsupported",
                &harness_name,
                format!(
                    "actor `{}` has no `sourceVerification.actors` entry (how to call its original code)",
                    actor.name
                ),
            );
        };
        let extracted = actor
            .codes
            .iter()
            .flatten()
            .all(|s| s.src.origin == Origin::Extracted);
        if !extracted {
            return SourceVerification::refused(
                "unsupported",
                &harness_name,
                format!(
                    "actor `{}` is (partly) declared in the scenario, not extracted from source: there is no original code to run",
                    actor.name
                ),
            );
        }
        let Some(entry) = req.harness.manifest.entries.get(&spec.entry) else {
            return SourceVerification::refused(
                "unsupported",
                &harness_name,
                format!(
                    "the harness has no entry `{}` (actor `{}`)",
                    spec.entry, actor.name
                ),
            );
        };
        let info = req
            .entries
            .iter()
            .find(|(a, _)| *a == actor.name)
            .map(|(_, e)| e);
        let selector = info.map_or(actor.function.name.as_str(), |e| e.selector.as_str());
        if entry.selector != selector {
            return SourceVerification::refused(
                "unsupported",
                &harness_name,
                format!(
                    "actor `{}` models `{selector}`, but harness entry `{}` runs `{}`",
                    actor.name, spec.entry, entry.selector
                ),
            );
        }
        let scope = info.map(|e| {
            let start = e
                .anchor
                .rsplit_once('@')
                .and_then(|(_, lc)| lc.split_once(':'))
                .and_then(|(l, _)| l.parse().ok())
                .unwrap_or(1);
            Region {
                file: e.file.clone(),
                start,
                end: e.end_line.max(start),
            }
        });
        if let Some(e) = info
            && let Some(c) = &e.contract
        {
            for p in &c.preconditions {
                assumptions.push(format!(
                    "{}: declared precondition, established by the harness input, not proven: {p}",
                    actor.name
                ));
            }
        }
        let ai = launches.len();
        let shared = model
            .process_of
            .iter()
            .filter(|p| **p == model.process_of[ai])
            .count()
            > 1;
        launches.push(ActorLaunch {
            name: actor.name.clone(),
            entry: spec.entry.clone(),
            input: spec.input.clone(),
            scope,
            process: shared.then(|| model.process_names[model.process_of[ai]].clone()),
            pool: model.pool_of(ai),
            response: actor
                .codes
                .first()
                .is_some_and(|c| c.iter().any(|s| matches!(s.op, Op::Respond))),
        });
    }
    let mut plan = match plan(model, req.schedule) {
        Ok(p) => p,
        Err(e) => return SourceVerification::refused("unsupported", &harness_name, e),
    };
    // A termination counterexample ends in a state where nothing can proceed:
    // the run must observe it.
    let stuck_kind = if req.invariant.name == crate::model::TERMINATION {
        use crate::explore::StuckKind;
        match crate::explore::classify_stuck(model, req.final_state) {
            StuckKind::PoolStarvation => Some("pool-starvation"),
            StuckKind::ApplicationWaitCycle => Some("application-wait-cycle"),
            StuckKind::Unexplained => {
                return SourceVerification::refused(
                    "unsupported",
                    &harness_name,
                    "the model's stuck state is not one the run can observe (neither pool starvation nor an application-level wait cycle)".to_owned(),
                );
            }
        }
    } else {
        None
    };
    if let Some(kind) = stuck_kind {
        plan.steps.push(PlanStep::Stuck(StuckStep {
            transition: req.schedule.len(),
            kind: kind.to_owned(),
        }));
    }
    let seed = match seed_sql(model) {
        Ok(s) => s,
        Err(e) => return SourceVerification::refused("unsupported", &harness_name, e),
    };
    let queries = final_queries(model);
    let model_tables = committed(model, req.final_state);
    for t in &model.schema.tables {
        for c in &t.columns {
            if matches!(c.default, Some(ColumnDefault::Sequence(_))) {
                assumptions.push(format!(
                    "{}.{}: generated values are compared exactly because both the model and the disposable database start the sequence at the same value",
                    t.name, c.name
                ));
            }
        }
    }
    assumptions.push("Only the columns the scenario models are compared; other columns of the application's tables keep their database defaults.".to_owned());
    let names: Vec<String> = model.actors.iter().map(|a| a.name.clone()).collect();
    let has_recovery = !plan.crashes.is_empty()
        || !plan.losses.is_empty()
        || !plan.restarts.is_empty()
        || !plan.completions.is_empty()
        || plan.predictions.iter().any(|p| p.effect.is_some());
    let mut contracts = Vec::new();
    let mut channels = BTreeMap::new();
    for (name, spec) in &model.channels {
        let dedup = match spec.dedup {
            crate::ir::Dedup::None => "none",
            crate::ir::Dedup::ByKey => "byKey",
        };
        channels.insert(name.clone(), dedup.to_owned());
    }
    let used_channels: std::collections::BTreeSet<&str> = plan
        .steps
        .iter()
        .filter_map(|s| match s {
            PlanStep::Release(e) if e.kind == SqlKind::Effect => e.table.as_deref(),
            _ => None,
        })
        .collect();
    for c in &used_channels {
        contracts.push(format!(
            "channel `{c}`: a TEST DOUBLE hosted by the coordinator receives the effects; it applies the declared deduplication `{}` — an assumption about the real provider, not evidence about it",
            channels.get(*c).map_or("none", String::as_str)
        ));
    }
    for r in &plan.restarts {
        let line = format!("{}: {}", names[r.actor], r.mechanism);
        if !contracts.contains(&line) {
            contracts.push(line);
        }
    }

    let mut out = SourceVerification::refused("inconclusive", &harness_name, String::new());
    out.problems.clear();
    out.performed = true;
    out.note = NOTE.to_owned();
    out.assumptions = assumptions;
    out.model_tables.clone_from(&model_tables);
    let repeat = req.repeat.clamp(1, 20);
    for run_index in 1..=repeat {
        let run: SourceRun = source::run(&RunRequest {
            harness: req.harness,
            allow_execution: req.allow_execution,
            // A debugging aid: keep the private run directory (worker logs)
            // when asked; never the default.
            keep_artifacts: std::env::var_os("COMMITRIFT_KEEP_ARTIFACTS").is_some(),
            actors: &launches,
            plan: &plan.steps,
            seed_sql: &seed,
            final_queries: &queries,
            settle: Duration::from_millis(150),
            channels: &channels,
            read_catalog: run_index == 1 && req.extracted_schema.is_some(),
        });
        if run.status == "blocked" {
            let mut refused =
                SourceVerification::refused("blocked", &harness_name, run.problems.join("; "));
            refused.performed = false;
            return refused;
        }
        // Statement (and effect) comparison.
        let releases: Vec<usize> = plan
            .steps
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s, PlanStep::Release(_)))
            .map(|(i, _)| i)
            .collect();
        let mut statements = Vec::new();
        // Phase 12: generated values of this run, matched to what the
        // application observed (one bijection per run).
        let mut generated = GenMatch::default();
        for (k, p) in plan.predictions.iter().enumerate() {
            let step_index = releases.get(k).copied();
            let observed = run
                .statements
                .iter()
                .find(|o| o.plan.is_some() && o.plan == step_index);
            statements.push(compare_statement(model, p, observed, &mut generated));
        }
        let statements_agree = statements.iter().all(|s| s.status == "agree");
        let mut outside = Vec::new();
        let mut unexpected = Vec::new();
        for o in &run.statements {
            let at = o
                .frames
                .first()
                .map(|f| format!(" at {}:{}:{}", f.file, f.line, f.column))
                .unwrap_or_default();
            let flat = o.sql.split_whitespace().collect::<Vec<_>>().join(" ");
            let who = format!(
                "{}{}",
                launches[o.actor].name,
                if o.attempt > 1 {
                    format!("#{}", o.attempt)
                } else {
                    String::new()
                }
            );
            match o.role.as_str() {
                "outside-model" => outside.push(format!("{who}: {flat}{at}")),
                "beyond-plan" | "unexpected" | "after-divergence" => {
                    unexpected.push(format!("{who} ({}): {flat}{at}", o.role));
                }
                _ => {}
            }
        }
        // Final rows.
        let runtime_tables = rows_of(&run.tables);
        // Phase 13: a generated value the statements never showed (a
        // database default without RETURNING) is matched where it is first
        // observed: at a crash checkpoint, before the final rows, in which
        // rows that differ only by such a value are interchangeable.
        // Phase 14: the same at every checkpoint (crash, server completion,
        // connection loss), in transition order: rows a lost connection's
        // COMMIT left are seen there first.
        let mut checkpoints: Vec<Checkpoint<'_>> = Vec::new();
        for c in &plan.crashes {
            let observed = run
                .crashes
                .iter()
                .find(|e| e.transition == c.transition && e.actor == c.actor)
                .map(|e| &e.tables);
            checkpoints.push((c.transition, &c.tables, observed));
        }
        for c in &plan.completions {
            let observed = run
                .completions
                .iter()
                .find(|e| e.transition == c.transition && e.actor == c.actor)
                .map(|e| &e.tables);
            checkpoints.push((c.transition, &c.tables, observed));
        }
        for l in &plan.losses {
            let observed = run
                .connection_losses
                .iter()
                .find(|e| e.transition == l.transition && e.actor == l.actor)
                .map(|e| &e.tables);
            checkpoints.push((l.transition, &l.tables, observed));
        }
        checkpoints.sort_by_key(|(t, _, _)| *t);
        for (_, expected, observed) in checkpoints {
            if let Some(observed) = observed
                && expected.values().flatten().any(row_has_generated)
            {
                tables_match(&mut generated, expected, observed);
            }
        }
        let tables_agree = if model_tables.values().flatten().any(row_has_generated) {
            // Phase 12: generated values match this run's concrete values
            // consistently with the statements (one bijection per run);
            // rows are matched as multisets (their order differs).
            tables_match(&mut generated, &model_tables, &run.tables)
        } else {
            runtime_tables == model_tables
        };
        // Outcomes (of each actor's last execution).
        let final_outcomes: Vec<(String, String, String)> = model
            .actors
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let model_outcome = req.final_state.actors[i].outcome.label();
                let runtime = run
                    .actors
                    .get(i)
                    .map(|r| match &r.error {
                        Some(e) => format!("{}: {e}", r.outcome),
                        None => r.outcome.clone(),
                    })
                    .unwrap_or_default();
                (a.name.clone(), model_outcome, runtime)
            })
            .collect();
        let outcomes_agree = final_outcomes.iter().all(|(_, m, r)| {
            (m == "ok" && r == "returned")
                || (m.starts_with("failed") && r.starts_with("threw"))
                || (m == "crashed" && r == "killed")
                || (m == "running" && r == "killed" && stuck_kind.is_some())
        });
        // Phase 8: crashes, restarts, effects, attempts.
        let crash_checks: Vec<CrashCheck> = plan
            .crashes
            .iter()
            .map(|c| {
                let mut check = CrashCheck {
                    in_flight: None,
                    transition: c.transition,
                    actor: names[c.actor].clone(),
                    attempt: c.attempt,
                    boundary: c.boundary.clone(),
                    model_tables: c.tables.clone(),
                    status: "not-observed".to_owned(),
                    ..CrashCheck::default()
                };
                let Some(e) = run
                    .crashes
                    .iter()
                    .find(|e| e.transition == c.transition && e.actor == c.actor)
                else {
                    return check;
                };
                check.checkpoint.clone_from(&e.checkpoint);
                check.never_sent.clone_from(&e.never_sent);
                check.pid = e.pid;
                check.killed_by_signal = e.killed_by_signal;
                check.sessions_ended = e.sessions_ended;
                check.sessions_ended_ms = e.sessions_ended_ms;
                check.runtime_tables = rows_of(&e.tables);
                if e.attempt != c.attempt {
                    check.differences.push(format!(
                        "execution: model #{}, killed worker #{}",
                        c.attempt, e.attempt
                    ));
                }
                if !e.killed_by_signal {
                    check
                        .differences
                        .push("the worker was not terminated by SIGKILL".to_owned());
                }
                let held = matches!(c.in_flight, Some((Fate::HeldForLaterDelivery, _)));
                if !e.sessions_ended && !held {
                    check.differences.push(
                        "PostgreSQL did not end the worker's sessions: whether its transaction committed is not established".to_owned(),
                    );
                }
                if let (Some((fate, _)), Some(ev)) = (&c.in_flight, &e.in_flight) {
                    let name = |f: &Fate| match f {
                        Fate::NotReached => "not-reached",
                        Fate::AnsweredAndWithheld => "answered-and-withheld",
                        Fate::HeldForLaterDelivery => "held-for-later-delivery",
                    };
                    let physical = match fate {
                        Fate::NotReached => {
                            "not committed (the server never received the operation; PostgreSQL rolled the transaction back)"
                        }
                        Fate::AnsweredAndWithheld => {
                            "completed by the server before the application died (its answer was observed on the wire)"
                        }
                        Fate::HeldForLaterDelivery => {
                            "pending on the server: the operation reaches PostgreSQL only later"
                        }
                    };
                    let mut differences = Vec::new();
                    if ev.fate != name(fate) {
                        differences.push(format!(
                            "fate: model {}, run {}",
                            name(fate),
                            ev.fate
                        ));
                    }
                    if ev.client_acknowledged {
                        differences.push("the application received an answer".to_owned());
                    }
                    match fate {
                        Fate::AnsweredAndWithheld if ev.server_answer.is_empty() => differences
                            .push("PostgreSQL's answer to the operation was not observed".to_owned()),
                        Fate::NotReached | Fate::HeldForLaterDelivery
                            if ev.backend_state_before_kill != "idle in transaction"
                                && c_is_commit(&ev.operation) =>
                        {
                            differences.push(format!(
                                "the backend was `{}` instead of `idle in transaction` while the COMMIT was withheld",
                                ev.backend_state_before_kill
                            ));
                        }
                        _ => {}
                    }
                    if matches!(fate, Fate::HeldForLaterDelivery) && !ev.session_open_after_kill {
                        differences.push(
                            "the server session did not survive the kill although the operation was to be delivered later".to_owned(),
                        );
                    }
                    check.differences.extend(differences);
                    check.in_flight = Some(InFlightCheck {
                        operation: ev.operation.clone(),
                        model_fate: name(fate).to_owned(),
                        observed_fate: ev.fate.clone(),
                        mechanism: ev.mechanism.clone(),
                        forwarded_to_server: ev.forwarded_to_server,
                        withheld_from_server: ev.withheld_from_server,
                        server_answer: ev.server_answer.clone(),
                        client_acknowledged: ev.client_acknowledged,
                        backend_state_before_kill: ev.backend_state_before_kill.clone(),
                        session_open_after_kill: ev.session_open_after_kill,
                        wire_events: ev.proxy_events.clone(),
                        application_knowledge: "ambiguous: the application sent the operation and received no answer; it cannot tell whether PostgreSQL executed it".to_owned(),
                        physical_outcome: physical.to_owned(),
                    });
                }
                // Phase 12: generated values match this run's values.
                let differ =
                    tables_differ(&mut generated, &check.model_tables, &check.runtime_tables);
                if differ {
                    check.differences.push(
                        "the committed rows after the crash differ from the model's".to_owned(),
                    );
                }
                check.status = if check.differences.is_empty() {
                    "agree"
                } else {
                    "differ"
                }
                .to_owned();
                check
            })
            .collect();
        let restart_checks: Vec<RestartCheck> = plan
            .restarts
            .iter()
            .map(|r| {
                let mut check = RestartCheck {
                    transition: r.transition,
                    actor: names[r.actor].clone(),
                    attempt: r.attempt,
                    mechanism: r.mechanism.clone(),
                    status: "not-observed".to_owned(),
                    ..RestartCheck::default()
                };
                if let Some(e) = run
                    .restarts
                    .iter()
                    .find(|e| e.transition == r.transition && e.actor == r.actor)
                {
                    check.pid = e.pid;
                    check.previous_pid = e.previous_pid;
                    check.previous_outcome.clone_from(&e.previous_outcome);
                    check.status = if e.attempt == r.attempt && e.pid != e.previous_pid {
                        "agree"
                    } else {
                        "differ"
                    }
                    .to_owned();
                }
                check
            })
            .collect();
        let completion_checks: Vec<CompletionCheck> = plan
            .completions
            .iter()
            .map(|c| {
                let mut check = CompletionCheck {
                    transition: c.transition,
                    actor: names[c.actor].clone(),
                    attempt: c.attempt,
                    model_tables: c.tables.clone(),
                    status: "not-observed".to_owned(),
                    ..CompletionCheck::default()
                };
                if let Some(e) = run
                    .completions
                    .iter()
                    .find(|e| e.transition == c.transition && e.actor == c.actor)
                {
                    check.server_answer.clone_from(&e.server_answer);
                    check.session_ended = e.session_ended;
                    check.runtime_tables = rows_of(&e.tables);
                    // Phase 14: generated values matched as after a crash.
                    if tables_differ(&mut generated, &check.model_tables, &check.runtime_tables) {
                        check.differences.push(
                            "the committed rows after the server completed the operation differ from the model's".to_owned(),
                        );
                    }
                    if !e.session_ended {
                        check.differences.push(
                            "the backend of the delivered operation did not end".to_owned(),
                        );
                    }
                    check.status = if check.differences.is_empty() {
                        "agree"
                    } else {
                        "differ"
                    }
                    .to_owned();
                }
                check
            })
            .collect();
        let loss_checks: Vec<LossCheck> = plan
            .losses
            .iter()
            .map(|l| {
                let name = |f: &Fate| match f {
                    Fate::NotReached => "not-reached",
                    Fate::AnsweredAndWithheld => "answered-and-withheld",
                    Fate::HeldForLaterDelivery => "held-for-later-delivery",
                };
                let mut check = LossCheck {
                    transition: l.transition,
                    actor: names[l.actor].clone(),
                    attempt: l.attempt,
                    operation: l.label.clone(),
                    model_fate: name(&l.fate).to_owned(),
                    model_tables: l.tables.clone(),
                    status: "not-observed".to_owned(),
                    application_knowledge: "ambiguous: the application received a driver error for the operation; the error is the same whether or not PostgreSQL executed it".to_owned(),
                    physical_outcome: match l.fate {
                        Fate::NotReached => "not committed (the server never received the operation; PostgreSQL rolled the transaction back)",
                        Fate::AnsweredAndWithheld => "completed by the server (its answer was observed on the wire before the connection was closed)",
                        Fate::HeldForLaterDelivery => "pending on the server: the operation reaches PostgreSQL only later",
                    }
                    .to_owned(),
                    ..LossCheck::default()
                };
                let Some(e) = run
                    .connection_losses
                    .iter()
                    .find(|e| e.transition == l.transition && e.actor == l.actor)
                else {
                    return check;
                };
                check.observed_fate.clone_from(&e.fate);
                check.mechanism.clone_from(&e.mechanism);
                check.forwarded_to_server = e.forwarded_to_server;
                check.withheld_from_server = e.withheld_from_server;
                check.server_answer.clone_from(&e.server_answer);
                check.client_severed = e.client_severed;
                check.worker_pid = e.worker_pid;
                check.worker_alive_after = e.worker_alive_after;
                check.worker_connected_after = e.worker_connected_after;
                check.driver_error.clone_from(&e.driver_error);
                check.driver_sqlstate.clone_from(&e.driver_sqlstate);
                check.backend_state_before.clone_from(&e.backend_state_before);
                check.backend_pid = e.backend_pid;
                check.session_ended = e.session_ended;
                check.session_open = e.session_open;
                check.dead_connection_statements.clone_from(&e.dead_connection_statements);
                check.later_backend_pids.clone_from(&e.later_backend_pids);
                check.wire_events.clone_from(&e.proxy_events);
                check.runtime_tables = rows_of(&e.tables);
                if e.attempt != l.attempt {
                    check.differences.push(format!(
                        "execution: model #{}, observed #{}",
                        l.attempt, e.attempt
                    ));
                }
                if e.fate != check.model_fate {
                    check
                        .differences
                        .push(format!("fate: model {}, run {}", check.model_fate, e.fate));
                }
                if !e.client_severed {
                    check
                        .differences
                        .push("the proxy did not close the client connection".to_owned());
                }
                if !e.worker_alive_after {
                    check.differences.push(
                        "the worker process did not survive the broken connection (the fault was a kill, not a connection loss)".to_owned(),
                    );
                }
                if e.driver_error.is_empty() {
                    check.differences.push(
                        "the driver reported no error to the application".to_owned(),
                    );
                }
                match l.fate {
                    Fate::AnsweredAndWithheld if e.server_answer.is_empty() => check
                        .differences
                        .push("PostgreSQL's answer to the operation was not observed".to_owned()),
                    Fate::HeldForLaterDelivery if !e.session_open => check.differences.push(
                        "the server session did not survive although the operation was to be delivered later".to_owned(),
                    ),
                    Fate::NotReached | Fate::AnsweredAndWithheld if !e.session_ended => check
                        .differences
                        .push("the backend session did not end after the loss".to_owned()),
                    _ => {}
                }
                if c_is_commit(&check.operation)
                    && !matches!(l.fate, Fate::AnsweredAndWithheld)
                    && e.backend_state_before != "idle in transaction"
                {
                    check.differences.push(format!(
                        "the backend was `{}` instead of `idle in transaction` while the COMMIT was withheld",
                        e.backend_state_before
                    ));
                }
                if let Some(old) = e.backend_pid
                    && e.later_backend_pids.contains(&old)
                {
                    check.differences.push(
                        "the application kept using the broken connection's backend".to_owned(),
                    );
                }
                // Phase 14: generated values matched as after a crash (a
                // loss in a table with a generated key always "differed").
                if tables_differ(&mut generated, &check.model_tables, &check.runtime_tables) {
                    check.differences.push(
                        "the committed rows after the loss differ from the model's".to_owned(),
                    );
                }
                check.status = if check.differences.is_empty() {
                    "agree"
                } else {
                    "differ"
                }
                .to_owned();
                check
            })
            .collect();
        let model_effects: Vec<String> = req
            .final_state
            .externals
            .iter()
            .map(|x| {
                describe_effect(
                    &names,
                    &x.channel,
                    x.key.as_ref(),
                    x.actor as usize,
                    x.attempt + 1,
                    x.deduplicated,
                )
            })
            .collect();
        let runtime_externals: Vec<ExternalEvent> = run
            .effects
            .iter()
            .map(|e| ExternalEvent {
                channel: e.channel.clone(),
                key: match json_to_value(&e.key) {
                    Value::Null => None,
                    v => Some(v),
                },
                payload: match json_to_value(&e.payload) {
                    Value::Row(r) => r,
                    _ => Row::new(),
                },
                actor: e.actor as u16,
                attempt: e.attempt.saturating_sub(1),
                // Not observable at runtime; no invariant reads it.
                in_txn: false,
                deduplicated: e.deduplicated,
            })
            .collect();
        let runtime_effects: Vec<String> = runtime_externals
            .iter()
            .map(|x| {
                describe_effect(
                    &names,
                    &x.channel,
                    x.key.as_ref(),
                    x.actor as usize,
                    x.attempt + 1,
                    x.deduplicated,
                )
            })
            .collect();
        let effects_agree = model_effects == runtime_effects;
        let attempts: Vec<(String, u32, u32)> = model
            .actors
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let runtime = if run.attempts.is_empty() {
                    u32::from(run.actors.get(i).is_some())
                } else {
                    run.attempts.iter().filter(|r| r.name == a.name).count() as u32
                };
                (
                    a.name.clone(),
                    req.final_state.actors[i].attempt + 1,
                    runtime,
                )
            })
            .collect();
        let attempts_agree = attempts.iter().all(|(_, m, r)| m == r);
        let recovery_agrees = has_recovery.then(|| {
            crash_checks.iter().all(|c| c.status == "agree")
                && loss_checks.iter().all(|c| c.status == "agree")
                && restart_checks.iter().all(|r| r.status == "agree")
                && completion_checks.iter().all(|c| c.status == "agree")
                && effects_agree
                && attempts_agree
        });
        // The invariant on the observed rows (and effect log).
        let formula = &req.invariant.formula;
        let invariant = if stuck_kind.is_some() {
            // The invariant is about the run never finishing: it is violated
            // when the stuck state was observed.
            if run.stuck.as_ref().is_some_and(|s| s.confirmed) {
                "violated".to_owned()
            } else {
                "holds".to_owned()
            }
        } else if formula.reads_only_database_and_externals() {
            if req.invariant.holds(
                model,
                &state_with(model, &runtime_tables, runtime_externals),
            ) {
                "holds".to_owned()
            } else {
                "violated".to_owned()
            }
        } else if !has_recovery
            && outcomes_agree
            && formula.reads_only_database_externals_and_outcomes()
        {
            // Phase 11: without crashes or retries every actor ran once and
            // its observed outcome equals the model's (checked above), so
            // the outcome terms read the observed outcomes.
            let mut observed = state_with(model, &runtime_tables, runtime_externals);
            for (i, a) in observed.actors.iter_mut().enumerate() {
                a.outcome = req.final_state.actors[i].outcome.clone();
                a.completions = req.final_state.actors[i].completions;
                // Phase 13: what the original code returned.
                if model.tracks_results() {
                    a.returned = run
                        .actors
                        .get(i)
                        .filter(|r| r.outcome == "returned")
                        .map(|r| {
                            serde_json::from_value::<Value>(r.value.clone())
                                .map_err(|e| format!("returned value: {e}"))
                        })
                        .into_iter()
                        .collect();
                }
            }
            if req.invariant.holds(model, &observed) {
                "holds".to_owned()
            } else {
                "violated".to_owned()
            }
        } else {
            "not-evaluated".to_owned()
        };
        let agreed = run.status == "followed"
            && statements_agree
            && tables_agree
            && outcomes_agree
            && unexpected.is_empty()
            && recovery_agrees != Some(false);
        let reproduced = !req.probe && agreed && invariant == "violated";
        out.runs.push(RunSummary {
            run: run_index,
            coordination: run.status.clone(),
            statements_agree,
            tables_agree,
            outcomes_agree,
            invariant: invariant.clone(),
            reproduced,
            elapsed_ms: run.elapsed_ms,
            recovery_agrees,
            recovery_reproduced: req.probe.then_some(agreed && invariant == "holds"),
        });
        if run_index == 1 {
            out.statements = statements;
            out.outside_model = outside;
            out.unexpected = unexpected;
            out.runtime_tables = runtime_tables;
            out.actors = final_outcomes;
            out.problems.clone_from(&run.problems);
            out.postgres_version = Some(run.postgres_version.clone());
            out.isolation.clone_from(&run.isolation);
            out.driver_internal.clone_from(&run.driver_internal);
            out.transaction_config = run
                .statements
                .iter()
                .filter(|o| o.role == "begin-config")
                .map(|o| {
                    format!(
                        "{}: {}",
                        launches[o.actor].name,
                        o.sql.split_whitespace().collect::<Vec<_>>().join(" ")
                    )
                })
                .collect();
            if let (Some(schema), Some(catalog)) = (req.extracted_schema, &run.catalog) {
                let tables: Vec<String> =
                    model.schema.tables.iter().map(|t| t.name.clone()).collect();
                let mut fidelity = crate::schema_fidelity::compare(schema, catalog, &tables);
                fidelity.postgres_version.clone_from(&run.postgres_version);
                out.schema_fidelity = Some(fidelity);
            }
            out.wire_log = run
                .proxy_log
                .iter()
                .filter(|e| !matches!(e.kind, commitrift_replay::proxy::EventKind::Message))
                .map(|e| {
                    format!(
                        "[{} ms] conn {} {:?}: {}",
                        e.at_ms, e.conn, e.kind, e.detail
                    )
                })
                .collect();
            if let (Some(kind), Some(st)) = (stuck_kind, &run.stuck) {
                out.stuck = Some(crate::source_types::StuckCheck {
                    model_kind: kind.to_owned(),
                    observed: crate::source_types::StuckObservation {
                        kind: st.kind.clone(),
                        quiet_ms: st.quiet_ms,
                        unfinished: st.unfinished.clone(),
                        pool: st.pool,
                        lock_waits: st.lock_waits,
                        confirmed: st.confirmed,
                        detail: st.detail.clone(),
                    },
                    status: if st.confirmed && st.kind == kind {
                        "agree"
                    } else {
                        "differ"
                    }
                    .to_owned(),
                });
            }
            out.shared_processes =
                model
                    .process_names
                    .iter()
                    .enumerate()
                    .filter_map(|(p, name)| {
                        let tasks: Vec<&str> = model
                            .actors
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| model.process_of[*i] == p)
                            .map(|(_, a)| a.name.as_str())
                            .collect();
                        (tasks.len() > 1).then(|| {
                            format!(
                            "{name}: {} concurrent tasks in one process{}",
                            tasks.join(", "),
                            model.pools.get(p).copied().flatten().map_or(String::new(), |c| format!(
                                "; connection pool of {c} (the harness pool's max was checked)"
                            ))
                        )
                        })
                    })
                    .collect();
            if has_recovery {
                out.recovery = Some(RecoveryVerification {
                    crashes: crash_checks,
                    restarts: restart_checks,
                    completions: completion_checks,
                    losses: loss_checks,
                    effects: EffectComparison {
                        model: model_effects,
                        runtime: runtime_effects,
                        agree: effects_agree,
                    },
                    attempts,
                    contracts: contracts.clone(),
                    server_settings: run.server_settings.clone(),
                    quiescence: quiescence(model, req.final_state),
                });
            }
        }
    }
    let all = |f: &dyn Fn(&RunSummary) -> bool| out.runs.iter().all(f);
    let any = |f: &dyn Fn(&RunSummary) -> bool| out.runs.iter().any(f);
    let crash_or_retry =
        !plan.crashes.is_empty() || !plan.restarts.is_empty() || !plan.losses.is_empty();
    out.status = if req.probe {
        if all(&|r| r.recovery_reproduced == Some(true)) {
            if crash_or_retry {
                "recovery-reproduced"
            } else {
                "invariant-held"
            }
        } else if any(&|r| r.coordination == "diverged") {
            "diverged"
        } else if any(&|r| r.coordination == "inconclusive") {
            "inconclusive"
        } else if any(&|r| r.recovery_reproduced == Some(true)) {
            "unstable"
        } else {
            "not-reproduced"
        }
    } else if all(&|r| r.reproduced) {
        "reproduced"
    } else if any(&|r| r.coordination == "diverged") {
        "diverged"
    } else if any(&|r| r.coordination == "inconclusive") {
        "inconclusive"
    } else if any(&|r| r.reproduced) {
        "unstable"
    } else {
        "not-reproduced"
    }
    .to_owned();
    // Phase 8 classification, only for schedules with a crash or a retry
    // (or a recovery probe): Phase 7 runs keep their meaning unchanged.
    if crash_or_retry || req.probe {
        out.evidence = evidence_label(&out.status).to_owned();
        let in_flight = plan
            .steps
            .iter()
            .any(|s| matches!(s, PlanStep::InFlight(_)));
        if in_flight && out.evidence == "ORIGINAL_CODE_CRASH_REPRODUCED" {
            out.evidence = "ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED".to_owned();
        }
        // Phase 10: a connection that broke while the worker lived is a
        // different failure from a process that died; it gets its own label.
        if !plan.losses.is_empty()
            && matches!(
                out.evidence.as_str(),
                "ORIGINAL_CODE_CRASH_REPRODUCED" | "ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED"
            )
        {
            out.evidence = "ORIGINAL_CODE_CONNECTION_LOSS_REPRODUCED".to_owned();
        }
    }
    out
}

fn c_is_commit(operation: &str) -> bool {
    operation.starts_with("COMMIT")
}

fn evidence_label(status: &str) -> &'static str {
    match status {
        "reproduced" => "ORIGINAL_CODE_CRASH_REPRODUCED",
        "recovery-reproduced" => "ORIGINAL_CODE_RECOVERY_REPRODUCED",
        "invariant-held" => "ORIGINAL_CODE_INVARIANT_HELD",
        "diverged" => "DIVERGED",
        "unsupported" => "UNSUPPORTED",
        "not-reproduced" => "NOT_REPRODUCED",
        _ => "INCONCLUSIVE",
    }
}

/// Verify a counterexample (or a recovery probe) against the original
/// code. A refusal of a schedule with a crash or a retry is classified too.
pub fn verify(req: &VerifyRequest<'_>) -> SourceVerification {
    let mut out = verify_schedule(req);
    if out.evidence.is_empty()
        && (req.probe || req.schedule.iter().any(Transition::is_crash_or_retry))
    {
        out.evidence = evidence_label(&out.status).to_owned();
    }
    out
}

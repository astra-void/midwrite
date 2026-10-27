//! The transition system: initial state, enabled transitions and their
//! effects.
//!
//! Granularity (see `docs/phase6/SEMANTICS.md`): one *visible* operation of
//! one task is one transition. Local operations (`Let`, branches, task
//! spawning, joins) are folded into the transition that precedes them, since
//! they commute with everything another task can do. A database statement is
//! atomic unless it has to wait for another transaction; waiting statements
//! are resumed by a separate `Resume` transition once the blocker finished.

use serde::{Deserialize, Serialize};

use crate::db::{DbError, RowId, TxnId};
use crate::epistemic::{Ambiguity, InFlightOp, OutcomeSet, Via, World};
use crate::exec::{StmtResult, StmtStep, run_stmt, ser_would_fail};
use crate::footprint::{Access, Footprint};
use crate::ir::{
    Conn, Dedup, Isolation, Op, OperationRetry, RetryTrigger, SourceRef, Stmt, StmtKind,
};
use crate::model::Model;
use crate::state::{
    ActorState, BeginInfo, ClosedStmt, ExternalEvent, Lease, Outcome, Pending, PoolState, State,
    TKind, TStatus, Thread, Transition, Txn,
};
use crate::value::{Expr, Locals, Mode, Row, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EventKind {
    Begin,
    Commit,
    Rollback,
    Read,
    Write,
    Local,
    External,
    App,
    Crash,
    Retry,
    Blocked,
    Resumed,
    Fail,
    Abort,
    Spawn,
    Deadlock,
    /// Phase 9: a submitted operation's acknowledgment was lost (physical
    /// outcome stated in the text; the application learns nothing).
    AckLost,
    /// Phase 9: the server completes an operation it received from a client
    /// that is gone.
    ServerCompletion,
}

/// One observable fact produced by a transition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub actor: u16,
    pub thread: u16,
    pub kind: EventKind,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sqlstate: Option<String>,
    /// For `Blocked`: the actor whose transaction is waited for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_on: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<SourceRef>,
}

/// What a statement or commit is expected to do on a real database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expect {
    Done(StmtResult),
    /// The statement waits for a lock held by the transaction of this actor.
    Blocked(u16),
    Error(String),
    Ok,
}

/// A physical action a transition performs against a database. The
/// PostgreSQL oracle replays these against a real server in lockstep.
#[derive(Clone, Debug)]
pub enum Action {
    Begin {
        actor: u16,
        iso: Isolation,
    },
    Stmt {
        actor: u16,
        thread: u16,
        conn: Conn,
        stmt: ClosedStmt,
        resumed: bool,
        expect: Expect,
    },
    Commit {
        actor: u16,
        thread: u16,
        expect: Expect,
    },
    Rollback {
        actor: u16,
    },
    /// The model aborted this actor's transaction (after an error, a
    /// failure, or a deadlock): the driver rolls it back.
    Abort {
        actor: u16,
        conn: Conn,
    },
    /// A blocked statement of this task is chosen as deadlock victim.
    Victim {
        actor: u16,
        thread: u16,
        conn: Conn,
    },
    Crash {
        actor: u16,
    },
    /// Phase 9: the actor's COMMIT reached the server, which holds it
    /// (the connection is detached from the actor; the server completes the
    /// COMMIT later, as [`Action::OrphanCommit`]).
    Orphan {
        actor: u16,
        seq: u32,
    },
    /// Phase 9: the server completes a COMMIT it received from a client that
    /// is gone.
    OrphanCommit {
        actor: u16,
        seq: u32,
        expect: Expect,
    },
    /// An external effect (Phase 8: planned against the original code's
    /// channel test double).
    External {
        actor: u16,
        thread: u16,
        channel: String,
        key: Option<Value>,
        payload: Row,
        deduplicated: bool,
    },
}

#[derive(Default)]
pub struct Recorder {
    pub on: bool,
    pub events: Vec<Event>,
    pub actions: Vec<Action>,
}

impl Recorder {
    pub fn off() -> Self {
        Self::default()
    }
    pub fn on() -> Self {
        Self {
            on: true,
            events: Vec::new(),
            actions: Vec::new(),
        }
    }
    fn act(&mut self, a: impl FnOnce() -> Action) {
        if self.on {
            self.actions.push(a());
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        actor: usize,
        thread: usize,
        kind: EventKind,
        src: Option<&SourceRef>,
        text: impl FnOnce() -> String,
        result: Option<Value>,
        sqlstate: Option<&str>,
    ) {
        if self.on {
            self.events.push(Event {
                actor: actor as u16,
                thread: thread as u16,
                kind,
                text: text(),
                result,
                sqlstate: sqlstate.map(str::to_owned),
                blocked_on: None,
                src: src.cloned(),
            });
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsupported {
    pub actor: usize,
    pub detail: String,
    pub src: SourceRef,
}

pub struct Applied {
    pub state: State,
    pub footprint: Footprint,
}

pub fn initial_state(model: &Model) -> State {
    let mut store = crate::db::Store::default();
    for (id, row) in model.initial_row_ids() {
        store.rows.insert(
            id,
            std::sync::Arc::new(vec![crate::db::Version {
                values: Some(row),
                creator: crate::db::INITIAL_TXN,
                commit: Some(0),
            }]),
        );
    }
    let actors = model
        .actors
        .iter()
        .map(|spec| ActorState {
            locals: std::sync::Arc::new(spec.arguments.iter().cloned().collect()),
            threads: vec![main_thread()],
            txn: None,
            begin: None,
            tx_attempts: 0,
            outcome: Outcome::Running,
            completions: 0,
            attempt: 0,
            txn_seq: 0,
            insert_seq: 0,
            gen_seq: 0,
            touched: false,
            ambiguous: Vec::new(),
        })
        .collect();
    let mut s = State {
        store,
        commit_seq: 0,
        txns: Default::default(),
        actors,
        apps: vec![model.app_initial.clone(); model.process_names.len()],
        externals: Vec::new(),
        crashes: 0,
        retries: 0,
        in_flight: 0,
        orphans: Vec::new(),
        pools: if model.has_pools() {
            vec![PoolState::default(); model.process_names.len()]
        } else {
            Vec::new()
        },
        deps: Default::default(),
        seqs: model.sequences().into_iter().map(|(_, _, n)| n).collect(),
        steps: 0,
        last_thread: None,
        switches: 0,
    };
    let mut rec = Recorder::off();
    let mut foot = Footprint::default();
    for a in 0..s.actors.len() {
        settle_actor(model, &mut s, a, &mut foot, &mut rec);
    }
    s
}

fn main_thread() -> Thread {
    Thread {
        lease: Lease::None,
        handlers: Vec::new(),
        code: 0,
        pc: 0,
        status: TStatus::Running,
        parent: None,
        detached: false,
        pending: None,
        failure: None,
    }
}

fn op_at<'a>(model: &'a Model, s: &State, a: usize, th: usize) -> &'a crate::ir::Step {
    let t = &s.actors[a].threads[th];
    &model.actors[a].codes[t.code as usize][t.pc as usize]
}

fn tx_busy(actor: &ActorState) -> bool {
    actor
        .threads
        .iter()
        .any(|t| t.pending.as_ref().is_some_and(|p| p.conn == Conn::Tx))
}

/// Whether the operation checks a connection out of the pool when it starts:
/// a BEGIN, or a statement outside a transaction.
fn needs_conn(op: &Op) -> bool {
    match op {
        Op::Begin { .. } => true,
        Op::Db { stmt } => stmt.conn == Conn::Pool,
        _ => false,
    }
}

/// A task reaches an operation that needs a connection of a bounded pool: it
/// takes an idle one, or queues (first come first served).
fn arrive(model: &Model, s: &mut State, a: usize, th: usize, foot: &mut Footprint) {
    let Some(cap) = model.pool_of(a) else {
        return;
    };
    if s.actors[a].threads[th].lease != Lease::None {
        return;
    }
    let p = model.process_of[a];
    foot.push(Access::Pool(p));
    if s.pools[p].leased < cap {
        s.pools[p].leased += 1;
        s.actors[a].threads[th].lease = Lease::Held;
    } else {
        s.pools[p].queue.push((a as u16, th as u16));
        s.actors[a].threads[th].lease = Lease::Waiting;
    }
}

/// A connection comes back: it goes to the first waiting request, or becomes
/// idle.
fn release_lease(model: &Model, s: &mut State, p: usize, foot: &mut Footprint) {
    if model.pools.get(p).copied().flatten().is_none() {
        return;
    }
    foot.push(Access::Pool(p));
    while !s.pools[p].queue.is_empty() {
        let (a, th) = s.pools[p].queue.remove(0);
        if let Some(t) = s.actors[a as usize].threads.get_mut(th as usize)
            && t.lease == Lease::Waiting
            && t.status == TStatus::Running
        {
            t.lease = Lease::Held;
            return;
        }
    }
    s.pools[p].leased = s.pools[p].leased.saturating_sub(1);
}

/// A task ends (or is abandoned) while holding or awaiting a connection it
/// has not used.
fn drop_thread_lease(model: &Model, s: &mut State, a: usize, th: usize, foot: &mut Footprint) {
    let Some(t) = s.actors[a].threads.get_mut(th) else {
        return;
    };
    let lease = std::mem::take(&mut t.lease);
    let p = model.process_of[a];
    match lease {
        Lease::None => {}
        Lease::Held => release_lease(model, s, p, foot),
        Lease::Waiting => {
            s.pools[p]
                .queue
                .retain(|(x, y)| (*x as usize, *y as usize) != (a, th));
        }
    }
}

/// The task's connection is used by the operation it starts now. The step
/// touches the pool: it can only run after the release or arrival that gave
/// the task its connection, and DPOR's happens-before must know that.
fn claim_lease(model: &Model, s: &mut State, a: usize, th: usize, foot: &mut Footprint) -> bool {
    if model.pool_of(a).is_none() {
        return false;
    }
    #[cfg(feature = "mutation-testing")]
    let record = !crate::footprint::mutation::claim_pool_muted();
    #[cfg(not(feature = "mutation-testing"))]
    let record = true;
    if record {
        foot.push(Access::Pool(model.process_of[a]));
    }
    let t = &mut s.actors[a].threads[th];
    let held = t.lease == Lease::Held;
    t.lease = Lease::None;
    held
}

fn uses_tx(op: &Op) -> bool {
    match op {
        Op::Db { stmt } => stmt.conn == Conn::Tx,
        Op::Commit | Op::Rollback => true,
        _ => false,
    }
}

pub fn enabled(model: &Model, s: &State) -> Vec<Transition> {
    let mut out = Vec::new();
    for (a, actor) in s.actors.iter().enumerate() {
        for (th, t) in actor.threads.iter().enumerate() {
            if t.status != TStatus::Running {
                continue;
            }
            if let Some(p) = &t.pending {
                if p.blocked_on.is_none_or(|b| !s.txn_active(b)) {
                    out.push(Transition {
                        kind: TKind::Resume,
                        actor: a as u16,
                        thread: th as u16,
                    });
                }
                continue;
            }
            let step = op_at(model, s, a, th);
            let visible = !matches!(
                step.op,
                Op::Let { .. }
                    | Op::JumpIf { .. }
                    | Op::Jump { .. }
                    | Op::Return
                    | Op::Throw { .. }
                    | Op::Try { .. }
                    | Op::EndTry
                    | Op::Rethrow { .. }
                    | Op::Join
                    | Op::Spawn { .. }
            );
            if !visible {
                continue; // blocked at a join
            }
            if uses_tx(&step.op) && tx_busy(actor) {
                continue;
            }
            if model.pool_of(a).is_some() && needs_conn(&step.op) && t.lease != Lease::Held {
                continue; // waiting for a connection of the pool
            }
            out.push(Transition {
                kind: TKind::Step,
                actor: a as u16,
                thread: th as u16,
            });
        }
    }
    let in_flight_mode = model.bounds.max_in_flight > 0 && s.in_flight < model.bounds.max_in_flight;
    if in_flight_mode {
        // Lost acknowledgments with the process alive (Phase 9).
        for (a, actor) in s.actors.iter().enumerate() {
            for th in 0..actor.threads.len() {
                if let Some(op) = candidate(model, s, a, th) {
                    out.push(Transition {
                        kind: TKind::LostNotCommitted,
                        actor: a as u16,
                        thread: th as u16,
                    });
                    let _ = op;
                    out.push(Transition {
                        kind: TKind::LostCommitted,
                        actor: a as u16,
                        thread: th as u16,
                    });
                }
            }
        }
    }
    if s.crashes < model.bounds.max_crashes {
        for (p, _) in model.process_names.iter().enumerate() {
            let alive = model.actors.iter().enumerate().any(|(a, _)| {
                model.process_of[a] == p
                    && s.actors[a]
                        .threads
                        .iter()
                        .any(|t| t.status == TStatus::Running)
                    && (s.actors[a].touched || s.actors[a].txn.is_some())
            });
            let ops = if in_flight_mode {
                in_flight_ops(model, s, p)
            } else {
                Vec::new()
            };
            if ops.is_empty() {
                if alive && model.bounds.crash_policy == crate::model::CrashPolicy::Any {
                    let first = model.process_of.iter().position(|q| *q == p).unwrap_or(0);
                    out.push(Transition {
                        kind: TKind::Crash,
                        actor: first as u16,
                        thread: p as u16,
                    });
                }
                continue;
            }
            // Operations are in flight: the crash is one transition per
            // combination of server outcomes; the plain crash (nothing was
            // submitted) is the all-"not committed" combination.
            for mask in 0u16..(1u16 << ops.len()) {
                if mask == 0 && !alive {
                    continue;
                }
                let mut trial = s.clone();
                if crash_in_flight(
                    model,
                    &mut trial,
                    p,
                    mask,
                    &mut Footprint::default(),
                    &mut Recorder::off(),
                )
                .is_ok()
                {
                    out.push(Transition {
                        kind: TKind::CrashInFlight,
                        actor: mask,
                        thread: p as u16,
                    });
                }
            }
        }
    }
    for o in &s.orphans {
        if orphan_ready(model, s, o) {
            out.push(Transition {
                kind: TKind::ServerComplete,
                actor: o.actor,
                thread: o.seq as u16,
            });
        }
    }
    if s.retries < model.bounds.max_retries {
        for (a, spec) in model.actors.iter().enumerate() {
            if retry_enabled(spec.retry.as_ref(), &s.actors[a]) {
                out.push(Transition {
                    kind: TKind::Retry,
                    actor: a as u16,
                    thread: 0,
                });
            }
        }
    }
    for (a, th) in deadlock_members(s) {
        out.push(Transition {
            kind: TKind::Deadlock,
            actor: a as u16,
            thread: th as u16,
        });
    }
    out.sort();
    out
}

/// A state is *quiescent* when no task can make progress by itself and no
/// guaranteed retry is pending: the only enabled transitions (if any) are
/// environment choices (a crash, an optional client retry or redelivery),
/// which may or may not happen. Final invariants are evaluated in quiescent
/// states.
pub fn is_quiescent(model: &Model, enabled: &[Transition]) -> bool {
    !enabled.iter().any(|t| match t.kind {
        TKind::Step | TKind::Resume | TKind::Deadlock | TKind::ServerComplete => true,
        TKind::Retry => model.actors[t.actor as usize]
            .retry
            .as_ref()
            .is_some_and(|r| r.guaranteed),
        TKind::Crash | TKind::LostCommitted | TKind::LostNotCommitted | TKind::CrashInFlight => {
            false
        }
    })
}

/// Tasks whose pending statement is part of a wait-for cycle.
pub fn deadlock_members(s: &State) -> Vec<(usize, usize)> {
    // txn → (blocked_on txn)
    let mut waits = std::collections::BTreeMap::new();
    for actor in &s.actors {
        for t in &actor.threads {
            if let Some(p) = &t.pending
                && let Some(b) = p.blocked_on
                && s.txn_active(b)
            {
                waits.insert(p.txn, b);
            }
        }
    }
    if waits.len() < 2 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (ai, actor) in s.actors.iter().enumerate() {
        for (ti, t) in actor.threads.iter().enumerate() {
            let Some(p) = &t.pending else { continue };
            let mut cur = p.txn;
            let mut hops = 0;
            while let Some(next) = waits.get(&cur) {
                hops += 1;
                if *next == p.txn {
                    out.push((ai, ti));
                    break;
                }
                if hops > waits.len() {
                    break;
                }
                cur = *next;
            }
        }
    }
    out
}

fn retry_enabled(retry: Option<&OperationRetry>, a: &ActorState) -> bool {
    let Some(r) = retry else { return false };
    if a.threads.iter().any(|t| t.status == TStatus::Running) {
        return false;
    }
    matches!(
        (&a.outcome, r.trigger),
        (
            Outcome::Failed(_),
            RetryTrigger::OnFailure | RetryTrigger::OnFailureOrCrash | RetryTrigger::AtLeastOnce,
        ) | (
            Outcome::Crashed,
            RetryTrigger::OnCrash | RetryTrigger::OnFailureOrCrash | RetryTrigger::AtLeastOnce,
        ) | (Outcome::Ok, RetryTrigger::AtLeastOnce)
    )
}

// The error path (an unsupported operation) is rare and not hot.
#[allow(clippy::result_large_err)]
pub fn apply(
    model: &Model,
    st: &State,
    t: Transition,
    rec: &mut Recorder,
) -> Result<Applied, Unsupported> {
    let mut s = st.clone();
    let mut foot = Footprint::default();
    s.steps += 1;
    match t.kind {
        TKind::Step | TKind::Resume => {
            let who = (t.actor, t.thread);
            if s.last_thread.is_some_and(|l| l != who) {
                s.switches += 1;
            }
            s.last_thread = Some(who);
            foot.push(Access::Actor(t.actor));
            if t.kind == TKind::Step {
                step_thread(
                    model,
                    &mut s,
                    t.actor as usize,
                    t.thread as usize,
                    &mut foot,
                    rec,
                )?;
            } else {
                resume_thread(
                    model,
                    &mut s,
                    t.actor as usize,
                    t.thread as usize,
                    &mut foot,
                    rec,
                );
            }
        }
        TKind::Crash => {
            foot.push(Access::Global);
            crash_process(model, &mut s, t.thread as usize, true, &mut foot, rec);
        }
        TKind::LostCommitted | TKind::LostNotCommitted => {
            foot.push(Access::Actor(t.actor));
            foot.push(Access::Budget);
            lose_acknowledgment(
                model,
                &mut s,
                t.actor as usize,
                t.thread as usize,
                t.kind == TKind::LostCommitted,
                &mut foot,
                rec,
            )?;
        }
        TKind::ServerComplete => {
            foot.push(Access::Actor(t.actor));
            server_complete(
                model,
                &mut s,
                t.actor as usize,
                t.thread as u32,
                &mut foot,
                rec,
            )
            .map_err(|detail| Unsupported {
                actor: t.actor as usize,
                detail,
                src: SourceRef::declared(0, ""),
            })?;
        }
        TKind::CrashInFlight => {
            foot.push(Access::Global);
            crash_in_flight(model, &mut s, t.thread as usize, t.actor, &mut foot, rec).map_err(
                |detail| Unsupported {
                    actor: t.actor as usize,
                    detail,
                    src: SourceRef::declared(0, ""),
                },
            )?;
        }
        TKind::Retry => {
            foot.push(Access::Global);
            retry_actor(model, &mut s, t.actor as usize, &mut foot, rec);
        }
        TKind::Deadlock => {
            foot.push(Access::Global);
            deadlock_victim(
                model,
                &mut s,
                t.actor as usize,
                t.thread as usize,
                &mut foot,
                rec,
            );
        }
    }
    Ok(Applied {
        state: s,
        footprint: foot,
    })
}

fn actor_name(model: &Model, a: usize) -> &str {
    &model.actors[a].name
}

// ---------------------------------------------------------------------
// Visible operations
// ---------------------------------------------------------------------

#[allow(clippy::result_large_err)]
fn step_thread(
    model: &Model,
    s: &mut State,
    a: usize,
    th: usize,
    foot: &mut Footprint,
    rec: &mut Recorder,
) -> Result<(), Unsupported> {
    let step = op_at(model, s, a, th);
    s.actors[a].touched = true;
    let src = Some(&step.src);
    match &step.op {
        Op::Begin { isolation, retry } => {
            if s.actors[a].txn.is_some() {
                return Err(Unsupported {
                    actor: a,
                    detail: "nested transactions and savepoints are not modeled".into(),
                    src: step.src.clone(),
                });
            }
            let iso = isolation.unwrap_or(model.isolation);
            let id = new_txn(s, a, iso, false);
            let leased = claim_lease(model, s, a, th, foot);
            if let Some(t) = s.txns.get_mut(&id) {
                t.leased = leased;
            }
            let locals = s.actors[a].locals.clone();
            let thread = &s.actors[a].threads[th];
            s.actors[a].begin = Some(BeginInfo {
                code: thread.code,
                pc: thread.pc,
                locals,
                retry: *retry,
            });
            s.actors[a].txn = Some(id);
            rec.add(
                a,
                th,
                EventKind::Begin,
                src,
                || format!("BEGIN ISOLATION LEVEL {}", iso.sql()),
                None,
                None,
            );
            rec.act(|| Action::Begin {
                actor: a as u16,
                iso,
            });
            advance(model, s, a, th, foot, rec);
        }
        Op::Commit => {
            let Some(id) = s.actors[a].txn else {
                fail_thread(
                    model,
                    s,
                    a,
                    th,
                    "COMMIT outside a transaction".into(),
                    foot,
                    rec,
                );
                return Ok(());
            };
            match commit_txn(model, s, id, foot) {
                Ok(()) => {
                    s.actors[a].txn = None;
                    rec.add(
                        a,
                        th,
                        EventKind::Commit,
                        src,
                        || "COMMIT".into(),
                        None,
                        None,
                    );
                    rec.act(|| Action::Commit {
                        actor: a as u16,
                        thread: th as u16,
                        expect: Expect::Ok,
                    });
                    advance(model, s, a, th, foot, rec);
                }
                Err(e) => {
                    rec.add(
                        a,
                        th,
                        EventKind::Commit,
                        src,
                        || format!("COMMIT → ERROR {e}"),
                        None,
                        Some(&e.sqlstate),
                    );
                    rec.act(|| Action::Commit {
                        actor: a as u16,
                        thread: th as u16,
                        expect: Expect::Error(e.sqlstate.clone()),
                    });
                    db_failure(model, s, a, th, id, Conn::Tx, e, foot, rec);
                }
            }
        }
        Op::Rollback => {
            if let Some(id) = s.actors[a].txn.take() {
                abort_txn(model, s, id, foot);
            }
            rec.add(
                a,
                th,
                EventKind::Rollback,
                src,
                || "ROLLBACK".into(),
                None,
                None,
            );
            rec.act(|| Action::Rollback { actor: a as u16 });
            advance(model, s, a, th, foot, rec);
        }
        Op::Db { stmt } => db_step(model, s, a, th, stmt, &step.src, foot, rec),
        Op::External {
            channel,
            payload,
            key,
        } => {
            let locals = s.actors[a].locals.clone();
            let evaluated: Result<(Row, Option<Value>), _> = (|| {
                let mut row = Row::new();
                for (k, e) in payload {
                    row.insert(k.clone(), e.eval(&locals, None, Mode::Js)?);
                }
                let key = match key {
                    Some(e) => match e.eval(&locals, None, Mode::Js)? {
                        Value::Null => None,
                        v => Some(v),
                    },
                    None => None,
                };
                Ok::<_, crate::value::EvalError>((row, key))
            })();
            match evaluated {
                Err(e) => fail_thread(model, s, a, th, e.to_string(), foot, rec),
                Ok((payload, key)) => {
                    let dedup = model
                        .channels
                        .get(channel)
                        .map(|c| c.dedup)
                        .unwrap_or_default();
                    let duplicate = dedup == Dedup::ByKey
                        && key.as_ref().is_some_and(|k| {
                            s.externals.iter().any(|x| {
                                &x.channel == channel
                                    && x.key.as_ref() == Some(k)
                                    && !x.deduplicated
                            })
                        });
                    let in_txn = s.actors[a].txn.is_some();
                    let attempt = s.actors[a].attempt;
                    foot.push(Access::Ext(channel.clone()));
                    rec.add(
                        a,
                        th,
                        EventKind::External,
                        src,
                        || {
                            format!(
                                "EXTERNAL {channel}{}{}{}",
                                key.as_ref()
                                    .map(|k| format!(" key={}", k.render()))
                                    .unwrap_or_default(),
                                if payload.is_empty() {
                                    String::new()
                                } else {
                                    format!(" {}", Value::Row(payload.clone()).render())
                                },
                                if duplicate {
                                    " (deduplicated by the channel)"
                                } else {
                                    ""
                                }
                            )
                        },
                        None,
                        None,
                    );
                    rec.act(|| Action::External {
                        actor: a as u16,
                        thread: th as u16,
                        channel: channel.clone(),
                        key: key.clone(),
                        payload: payload.clone(),
                        deduplicated: duplicate,
                    });
                    s.externals.push(ExternalEvent {
                        channel: channel.clone(),
                        key,
                        payload,
                        actor: a as u16,
                        attempt,
                        in_txn,
                        deduplicated: duplicate,
                    });
                    advance(model, s, a, th, foot, rec);
                }
            }
        }
        Op::AppRead { var, into } => {
            let p = model.process_of[a];
            foot.push(Access::App(p, var.clone()));
            let v = s.apps[p].get(var).cloned().unwrap_or(Value::Null);
            std::sync::Arc::make_mut(&mut s.actors[a].locals).insert(into.clone(), v.clone());
            rec.add(
                a,
                th,
                EventKind::App,
                src,
                || format!("READ app.{var} → {}", v.render()),
                Some(v.clone()),
                None,
            );
            advance(model, s, a, th, foot, rec);
        }
        Op::AppWrite { var, expr } => {
            let p = model.process_of[a];
            foot.push(Access::App(p, var.clone()));
            match expr.eval(&s.actors[a].locals, None, Mode::Js) {
                Ok(v) => {
                    rec.add(
                        a,
                        th,
                        EventKind::App,
                        src,
                        || format!("WRITE app.{var} = {}", v.render()),
                        None,
                        None,
                    );
                    s.apps[p].insert(var.clone(), v);
                    advance(model, s, a, th, foot, rec);
                }
                Err(e) => fail_thread(model, s, a, th, e.to_string(), foot, rec),
            }
        }
        Op::Unknown { detail, .. } => {
            return Err(Unsupported {
                actor: a,
                detail: detail.clone(),
                src: step.src.clone(),
            });
        }
        other => unreachable!("local operation {other:?} reached as a visible step"),
    }
    Ok(())
}

fn new_txn(s: &mut State, a: usize, iso: Isolation, implicit: bool) -> TxnId {
    let seq = s.actors[a].txn_seq;
    s.actors[a].txn_seq += 1;
    assert!(seq < 4096, "too many transactions in one actor");
    let id = TxnId(((a as u32) << 12) | seq);
    s.txns.insert(
        id,
        Txn {
            leased: false,
            actor: a as u16,
            iso,
            snapshot: None,
            implicit,
            writes: Vec::new(),
            committed: None,
            reads: Vec::new(),
        },
    );
    id
}

fn close_stmt(model: &Model, locals: &Locals, stmt: &Stmt) -> Result<ClosedStmt, String> {
    let table = |name: &str| {
        model
            .schema
            .index(name)
            .ok_or_else(|| format!("unknown table `{name}`"))
    };
    let close = |e: &Expr| e.close(locals, Mode::Sql).map_err(|e| e.to_string());
    Ok(match &stmt.kind {
        StmtKind::Select {
            table: t,
            pred,
            lock,
            joins,
            projection,
            ..
        } => ClosedStmt::Select {
            table: table(t)?,
            pred: close(pred)?,
            lock: *lock,
            joins: joins
                .iter()
                .map(|j| {
                    Ok(crate::state::ClosedJoin {
                        kind: j.kind,
                        table: table(&j.table)?,
                        left: j.left.clone(),
                        right: j.right.clone(),
                    })
                })
                .collect::<Result<_, String>>()?,
            projection: projection.clone(),
        },
        StmtKind::SelectCount { table: t, pred, .. } => ClosedStmt::Count {
            table: table(t)?,
            pred: close(pred)?,
        },
        StmtKind::Update {
            table: t,
            pred,
            set,
            ..
        } => ClosedStmt::Update {
            table: table(t)?,
            pred: close(pred)?,
            set: set
                .iter()
                .map(|(c, e)| Ok((c.clone(), close(e)?)))
                .collect::<Result<_, String>>()?,
        },
        StmtKind::Insert {
            table: t,
            values,
            on_conflict,
            returning,
            returning_columns,
            ..
        } => {
            let mut row = Row::new();
            for (c, e) in values {
                match close(e)? {
                    Expr::Lit(v) => {
                        row.insert(c.clone(), v);
                    }
                    other => {
                        return Err(format!(
                            "insert value for `{c}` is not closed: {}",
                            other.render()
                        ));
                    }
                }
            }
            ClosedStmt::Insert {
                table: table(t)?,
                values: row,
                on_conflict: *on_conflict,
                returning: returning.is_some(),
                returning_columns: returning_columns.clone(),
            }
        }
        StmtKind::Delete { table: t, pred, .. } => ClosedStmt::Delete {
            table: table(t)?,
            pred: close(pred)?,
        },
    })
}

pub fn describe(model: &Model, c: &ClosedStmt) -> String {
    let name = |t: u16| model.schema.table(t).name.clone();
    match c {
        ClosedStmt::Select {
            table,
            pred,
            lock,
            joins,
            ..
        } => format!(
            "SELECT {}{} WHERE {}{}",
            name(*table),
            joins
                .iter()
                .map(|j| format!(
                    " {} {} ON {} = {}",
                    match j.kind {
                        crate::ir::JoinKind::Inner => "JOIN",
                        crate::ir::JoinKind::Left => "LEFT JOIN",
                    },
                    name(j.table),
                    j.left,
                    j.right
                ))
                .collect::<String>(),
            pred.render(),
            match lock {
                crate::ir::LockMode::None => "",
                crate::ir::LockMode::ForUpdate => " FOR UPDATE",
                crate::ir::LockMode::SkipLocked => " FOR UPDATE SKIP LOCKED",
                crate::ir::LockMode::NoWait => " FOR UPDATE NOWAIT",
            }
        ),
        ClosedStmt::Count { table, pred } => {
            format!("COUNT {} WHERE {}", name(*table), pred.render())
        }
        ClosedStmt::Update { table, pred, set } => format!(
            "UPDATE {} SET {} WHERE {}",
            name(*table),
            set.iter()
                .map(|(c, e)| format!("{c} = {}", e.render()))
                .collect::<Vec<_>>()
                .join(", "),
            pred.render()
        ),
        ClosedStmt::Insert {
            table,
            values,
            on_conflict,
            ..
        } => format!(
            "INSERT {} {}{}",
            name(*table),
            Value::Row(values.clone()).render(),
            if *on_conflict == crate::ir::OnConflict::DoNothing {
                " ON CONFLICT DO NOTHING"
            } else {
                ""
            }
        ),
        ClosedStmt::Delete { table, pred } => {
            format!("DELETE {} WHERE {}", name(*table), pred.render())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn db_step(
    model: &Model,
    s: &mut State,
    a: usize,
    th: usize,
    stmt: &Stmt,
    src: &SourceRef,
    foot: &mut Footprint,
    rec: &mut Recorder,
) {
    let closed = match close_stmt(model, &s.actors[a].locals, stmt) {
        Ok(c) => c,
        Err(e) => {
            fail_thread(model, s, a, th, e, foot, rec);
            return;
        }
    };
    let txn = match stmt.conn {
        Conn::Tx => match s.actors[a].txn {
            Some(t) => t,
            None => {
                fail_thread(
                    model,
                    s,
                    a,
                    th,
                    "the transaction is closed".into(),
                    foot,
                    rec,
                );
                return;
            }
        },
        Conn::Pool => {
            let id = new_txn(s, a, model.isolation, true);
            let leased = claim_lease(model, s, a, th, foot);
            if let Some(t) = s.txns.get_mut(&id) {
                t.leased = leased;
            }
            id
        }
    };
    let snapshot = open_snapshot(s, txn, foot);
    let mut p = Pending {
        stmt: closed,
        conn: stmt.conn,
        txn,
        snapshot,
        todo: Vec::new(),
        affected: 0,
        found: None,
        blocked_on: None,
        started: false,
    };
    run_pending(model, s, a, th, &mut p, false, src, foot, rec);
}

/// The snapshot a statement of `txn` runs on (taken now, for the isolation
/// levels that keep one).
fn open_snapshot(s: &mut State, txn: TxnId, foot: &mut Footprint) -> u64 {
    let iso = s.txns[&txn].iso;
    if iso == Isolation::ReadCommitted {
        s.commit_seq
    } else {
        let commit_seq = s.commit_seq;
        let t = s.txns.get_mut(&txn).expect("transaction exists");
        if t.snapshot.is_none() {
            t.snapshot = Some(commit_seq);
            foot.push(Access::Snapshot);
        }
        t.snapshot.unwrap_or(commit_seq)
    }
}

fn resume_thread(
    model: &Model,
    s: &mut State,
    a: usize,
    th: usize,
    foot: &mut Footprint,
    rec: &mut Recorder,
) {
    let mut p = s.actors[a].threads[th]
        .pending
        .take()
        .expect("resume of a thread without a pending statement");
    p.blocked_on = None;
    foot.push(Access::Wait);
    let step = op_at(model, s, a, th);
    rec.add(
        a,
        th,
        EventKind::Resumed,
        Some(&step.src),
        || "(wait over, statement continues)".into(),
        None,
        None,
    );
    run_pending(model, s, a, th, &mut p, true, &step.src, foot, rec);
}

#[allow(clippy::too_many_arguments)]
fn run_pending(
    model: &Model,
    s: &mut State,
    a: usize,
    th: usize,
    p: &mut Pending,
    resumed: bool,
    src: &SourceRef,
    foot: &mut Footprint,
    rec: &mut Recorder,
) {
    let text = if rec.on {
        describe(model, &p.stmt)
    } else {
        String::new()
    };
    let step = run_stmt(model, s, p, foot);
    let stmt_action = |expect: Expect, p: &Pending| Action::Stmt {
        actor: a as u16,
        thread: th as u16,
        conn: p.conn,
        stmt: p.stmt.clone(),
        resumed,
        expect,
    };
    match step {
        StmtStep::Done(result) => {
            // An autocommit statement commits as part of the same step.
            if p.conn == Conn::Pool
                && let Err(e) = commit_txn(model, s, p.txn, foot)
            {
                rec.add(
                    a,
                    th,
                    EventKind::Write,
                    Some(src),
                    || format!("{text} → ERROR {e}"),
                    None,
                    Some(&e.sqlstate),
                );
                rec.act(|| stmt_action(Expect::Error(e.sqlstate.clone()), p));
                db_failure(model, s, a, th, p.txn, p.conn, e, foot, rec);
                return;
            }
            rec.act(|| stmt_action(Expect::Done(result.clone()), p));
            deliver(model, s, a, th, &result, src, &text, rec);
            advance(model, s, a, th, foot, rec);
        }
        StmtStep::Blocked(holder) => {
            p.blocked_on = Some(holder);
            foot.push(Access::Wait);
            // A statement that waits keeps its snapshot: where it sits in
            // the commit stream becomes observable when it resumes.
            foot.push(Access::Snapshot);
            if rec.on {
                rec.events.push(Event {
                    actor: a as u16,
                    thread: th as u16,
                    kind: EventKind::Blocked,
                    text: format!(
                        "{text} → BLOCKED waiting for {}'s transaction",
                        actor_name(model, holder.actor() as usize)
                    ),
                    result: None,
                    sqlstate: None,
                    blocked_on: Some(holder.actor()),
                    src: Some(src.clone()),
                });
            }
            rec.act(|| stmt_action(Expect::Blocked(holder.actor()), p));
            s.actors[a].threads[th].pending = Some(p.clone());
        }
        StmtStep::Failed(e) => {
            rec.add(
                a,
                th,
                EventKind::Write,
                Some(src),
                || format!("{text} → ERROR {e}"),
                None,
                Some(&e.sqlstate),
            );
            rec.act(|| stmt_action(Expect::Error(e.sqlstate.clone()), p));
            db_failure(model, s, a, th, p.txn, p.conn, e, foot, rec);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn deliver(
    model: &Model,
    s: &mut State,
    a: usize,
    th: usize,
    result: &StmtResult,
    src: &SourceRef,
    text: &str,
    rec: &mut Recorder,
) {
    let step = op_at(model, s, a, th);
    let Op::Db { stmt } = &step.op else {
        unreachable!("delivering a result to a non-statement")
    };
    let is_write = matches!(
        stmt.kind,
        StmtKind::Update { .. } | StmtKind::Insert { .. } | StmtKind::Delete { .. }
    );
    let (value, kind) = match result {
        StmtResult::Row(r) => (
            r.clone().map_or(Value::Null, Value::Row),
            if is_write {
                EventKind::Write
            } else {
                EventKind::Read
            },
        ),
        StmtResult::Count(n) => (Value::Int(*n), EventKind::Read),
        StmtResult::Affected(n) => (Value::Int(*n), EventKind::Write),
    };
    let target = match &stmt.kind {
        StmtKind::Select { into, .. } | StmtKind::SelectCount { into, .. } => Some(into.clone()),
        StmtKind::Insert {
            affected,
            returning: Some(row),
            ..
        } => {
            // `INSERT ... RETURNING *`: the row (or null) and the count.
            std::sync::Arc::make_mut(&mut s.actors[a].locals).insert(row.clone(), value.clone());
            if let Some(count) = affected {
                let n = i64::from(!value.is_null());
                std::sync::Arc::make_mut(&mut s.actors[a].locals)
                    .insert(count.clone(), Value::Int(n));
            }
            None
        }
        StmtKind::Update { affected, .. }
        | StmtKind::Insert { affected, .. }
        | StmtKind::Delete { affected, .. } => affected.clone(),
    };
    if let Some(name) = target {
        std::sync::Arc::make_mut(&mut s.actors[a].locals).insert(name, value.clone());
    }
    rec.add(
        a,
        th,
        kind,
        Some(src),
        || match result {
            StmtResult::Affected(n) => {
                format!("{text} → {n} row{}", if *n == 1 { "" } else { "s" })
            }
            _ => format!("{text} → {}", value.render()),
        },
        Some(value.clone()),
        None,
    );
}

/// A statement (or commit) failed with a database error.
#[allow(clippy::too_many_arguments)]
fn db_failure(
    model: &Model,
    s: &mut State,
    a: usize,
    th: usize,
    txn: TxnId,
    conn: Conn,
    err: DbError,
    foot: &mut Footprint,
    rec: &mut Recorder,
) {
    // The failed statement's transaction is aborted (as the driver would
    // roll it back), releasing its locks.
    let was_explicit = s.actors[a].txn == Some(txn);
    cancel_pendings(s, txn, a, th);
    abort_txn(model, s, txn, foot);
    rec.act(|| Action::Abort {
        actor: a as u16,
        conn,
    });
    if was_explicit || conn == Conn::Tx {
        s.actors[a].txn = None;
    }
    // Retry the whole transaction body on a serialization failure or
    // deadlock, if the transaction was declared retryable.
    if err.retryable()
        && th == 0
        && let Some(begin) = s.actors[a].begin.clone()
        && s.actors[a].tx_attempts < begin.retry
        && s.retries < model.bounds.max_retries
    {
        s.actors[a].tx_attempts += 1;
        s.retries += 1;
        s.actors[a].locals = begin.locals.clone();
        // Abandon children of the failed attempt.
        for i in 1..s.actors[a].threads.len() {
            let t = &mut s.actors[a].threads[i];
            t.status = TStatus::Done;
            t.pending = None;
            drop_thread_lease(model, s, a, i, foot);
        }
        let t = &mut s.actors[a].threads[0];
        t.code = begin.code;
        t.pc = begin.pc;
        t.status = TStatus::Running;
        t.pending = None;
        rec.add(
            a,
            th,
            EventKind::Retry,
            None,
            || {
                format!(
                    "retry transaction after {} (attempt {})",
                    err.sqlstate,
                    s.actors[a].tx_attempts + 1
                )
            },
            None,
            None,
        );
        settle_actor(model, s, a, foot, rec);
        return;
    }
    fail_thread(
        model,
        s,
        a,
        th,
        format!("{}: {}", err.sqlstate, err.message),
        foot,
        rec,
    );
}

/// Fail the pending statements of other threads of the same transaction.
fn cancel_pendings(s: &mut State, txn: TxnId, actor: usize, except: usize) {
    for (i, t) in s.actors[actor].threads.iter_mut().enumerate() {
        if i != except && t.pending.as_ref().is_some_and(|p| p.txn == txn) {
            t.pending = None;
            t.status = TStatus::DoneFailed;
            t.failure = Some("the transaction was aborted".into());
        }
    }
}

/// The deadlock detector aborts the blocked statement of one cycle member.
fn deadlock_victim(
    model: &Model,
    s: &mut State,
    a: usize,
    th: usize,
    foot: &mut Footprint,
    rec: &mut Recorder,
) {
    let src = op_at(model, s, a, th).src.clone();
    rec.add(
        a,
        th,
        EventKind::Deadlock,
        Some(&src),
        || "→ ERROR 40P01: deadlock detected (this transaction is the victim)".into(),
        None,
        Some("40P01"),
    );
    let p = s.actors[a].threads[th]
        .pending
        .take()
        .expect("the victim is waiting");
    rec.act(|| Action::Victim {
        actor: a as u16,
        thread: th as u16,
        conn: p.conn,
    });
    db_failure(
        model,
        s,
        a,
        th,
        p.txn,
        p.conn,
        DbError::deadlock(),
        foot,
        rec,
    );
}

// ---------------------------------------------------------------------
// Transactions
// ---------------------------------------------------------------------

pub fn commit_txn(
    model: &Model,
    s: &mut State,
    id: TxnId,
    foot: &mut Footprint,
) -> Result<(), DbError> {
    if ser_would_fail(s, id) {
        foot.push(Access::Ser);
        return Err(DbError::new(
            "40001",
            "could not serialize access due to read/write dependencies among transactions",
        ));
    }
    let txn = s
        .txns
        .get(&id)
        .expect("committing an unknown transaction")
        .clone();
    if txn.iso == Isolation::Serializable {
        foot.push(Access::Ser);
    }
    if !txn.writes.is_empty() {
        s.commit_seq += 1;
        foot.push(Access::CommitWrite);
    }
    let seq = s.commit_seq;
    for row in &txn.writes {
        let chain =
            std::sync::Arc::make_mut(s.store.rows.get_mut(row).expect("written row exists"));
        let n = chain.len();
        let prev = if n >= 2 {
            chain[n - 2].values.clone()
        } else {
            None
        };
        let v = chain.last_mut().expect("written row has a version");
        debug_assert_eq!(v.creator, id);
        v.commit = Some(seq);
        foot.push(Access::RowWrite {
            row: *row,
            old: prev,
            new: v.values.clone(),
        });
    }
    release_locks(s, id, foot);
    if s.actors.iter().any(|a| {
        a.threads
            .iter()
            .any(|t| t.pending.as_ref().is_some_and(|p| p.blocked_on == Some(id)))
    }) {
        foot.push(Access::Wait);
    }
    if model.uses_serializable() {
        s.txns.get_mut(&id).expect("exists").committed = Some(seq);
    } else {
        s.txns.remove(&id);
    }
    if txn.leased {
        if let Some(t) = s.txns.get_mut(&id) {
            t.leased = false;
        }
        release_lease(model, s, model.process_of[txn.actor as usize], foot);
    }
    Ok(())
}

fn release_locks(s: &mut State, id: TxnId, foot: &mut Footprint) {
    let held: Vec<RowId> = s
        .store
        .locks
        .iter()
        .filter(|(_, t)| **t == id)
        .map(|(r, _)| *r)
        .collect();
    for r in held {
        s.store.locks.remove(&r);
        foot.push(Access::LockChange(r));
    }
}

pub fn abort_txn(model: &Model, s: &mut State, id: TxnId, foot: &mut Footprint) {
    let Some(txn) = s.txns.remove(&id) else {
        return;
    };
    if txn.committed.is_some() {
        s.txns.insert(id, txn);
        return;
    }
    if txn.leased {
        release_lease(model, s, model.process_of[txn.actor as usize], foot);
    }
    for row in &txn.writes {
        if let Some(chain) = s.store.rows.get_mut(row) {
            let chain = std::sync::Arc::make_mut(chain);
            if let Some(removed) = chain.pop_if(|v| v.creator == id && v.commit.is_none()) {
                let previous = chain.last().and_then(|v| v.values.clone());
                foot.push(Access::RowWrite {
                    row: *row,
                    old: removed.values,
                    new: previous,
                });
            }
            if chain.is_empty() {
                s.store.rows.remove(row);
            }
        }
    }
    release_locks(s, id, foot);
    s.deps.retain(|(a, b)| *a != id && *b != id);
    if txn.iso == Isolation::Serializable {
        foot.push(Access::Ser);
    }
    if s.actors.iter().any(|a| {
        a.threads
            .iter()
            .any(|t| t.pending.as_ref().is_some_and(|p| p.blocked_on == Some(id)))
    }) {
        foot.push(Access::Wait);
    }
}

// ---------------------------------------------------------------------
// Tasks
// ---------------------------------------------------------------------

fn advance(
    model: &Model,
    s: &mut State,
    a: usize,
    th: usize,
    foot: &mut Footprint,
    rec: &mut Recorder,
) {
    s.actors[a].threads[th].pc += 1;
    settle_actor(model, s, a, foot, rec);
}

/// Run local operations of every task of the actor until each is at a
/// visible operation, waiting at a join, or finished.
fn settle_actor(model: &Model, s: &mut State, a: usize, foot: &mut Footprint, rec: &mut Recorder) {
    loop {
        let before = progress_marker(&s.actors[a]);
        let mut th = 0;
        while th < s.actors[a].threads.len() {
            settle_thread(model, s, a, th, foot, rec);
            th += 1;
        }
        if progress_marker(&s.actors[a]) == before {
            break;
        }
    }
}

fn progress_marker(a: &ActorState) -> Vec<(u32, u8, usize)> {
    a.threads
        .iter()
        .map(|t| (t.pc, t.status as u8, a.threads.len()))
        .collect()
}

fn settle_thread(
    model: &Model,
    s: &mut State,
    a: usize,
    th: usize,
    foot: &mut Footprint,
    rec: &mut Recorder,
) {
    loop {
        {
            let t = &s.actors[a].threads[th];
            if t.status != TStatus::Running || t.pending.is_some() {
                return;
            }
        }
        let step = op_at(model, s, a, th);
        let src = Some(&step.src);
        match &step.op {
            Op::Let { local, expr } => match expr.eval(&s.actors[a].locals, None, Mode::Js) {
                Ok(v) => {
                    rec.add(
                        a,
                        th,
                        EventKind::Local,
                        src,
                        || format!("{local} = {}", v.render()),
                        Some(v.clone()),
                        None,
                    );
                    std::sync::Arc::make_mut(&mut s.actors[a].locals).insert(local.clone(), v);
                    s.actors[a].threads[th].pc += 1;
                }
                Err(e) => {
                    fail_thread(model, s, a, th, e.to_string(), foot, rec);
                    return;
                }
            },
            Op::JumpIf { cond, target } => match cond.eval(&s.actors[a].locals, None, Mode::Js) {
                Ok(v) => {
                    let taken = v.truthy();
                    // Lowered `if (c) { ... }` jumps over its body when `!c`;
                    // show the source condition, not the inverted jump.
                    let (shown, value) = match cond {
                        Expr::Not(inner) => (inner.render(), !taken),
                        other => (other.render(), taken),
                    };
                    rec.add(
                        a,
                        th,
                        EventKind::Local,
                        src,
                        || format!("CHECK {shown} → {value}"),
                        Some(Value::Bool(value)),
                        None,
                    );
                    let t = &mut s.actors[a].threads[th];
                    t.pc = if taken { *target as u32 } else { t.pc + 1 };
                }
                Err(e) => {
                    fail_thread(model, s, a, th, e.to_string(), foot, rec);
                    return;
                }
            },
            Op::Jump { target } => s.actors[a].threads[th].pc = *target as u32,
            Op::Try { handler, error } => {
                let open = s.actors[a].txn.is_some();
                let t = &mut s.actors[a].threads[th];
                t.handlers.push((*handler as u32, error.clone(), open));
                t.pc += 1;
            }
            Op::EndTry => {
                let t = &mut s.actors[a].threads[th];
                t.handlers.pop();
                t.pc += 1;
            }
            Op::Rethrow { local } => {
                let reason = match s.actors[a].locals.get(local) {
                    Some(Value::Str(r)) => r.clone(),
                    Some(other) => other.render(),
                    None => "rethrown".to_owned(),
                };
                fail_thread(model, s, a, th, reason, foot, rec);
                return;
            }
            Op::Return => {
                finish_ok(model, s, a, th, foot, rec);
                return;
            }
            Op::Throw { message } => {
                rec.add(
                    a,
                    th,
                    EventKind::Fail,
                    src,
                    || format!("throw Error({message:?})"),
                    None,
                    None,
                );
                fail_thread(model, s, a, th, message.clone(), foot, rec);
                return;
            }
            Op::Join => {
                let children: Vec<usize> = s.actors[a]
                    .threads
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.parent == Some(th as u16) && !t.detached)
                    .map(|(i, _)| i)
                    .collect();
                if children
                    .iter()
                    .any(|c| s.actors[a].threads[*c].status == TStatus::Running)
                {
                    return;
                }
                let failed = children
                    .iter()
                    .find_map(|c| s.actors[a].threads[*c].failure.clone());
                if let Some(reason) = failed {
                    fail_thread(model, s, a, th, reason, foot, rec);
                    return;
                }
                s.actors[a].threads[th].pc += 1;
            }
            Op::Spawn { children, detached } => {
                rec.add(
                    a,
                    th,
                    EventKind::Spawn,
                    src,
                    || {
                        format!(
                            "spawn {} concurrent task(s){}",
                            children.len(),
                            if *detached { " (not awaited)" } else { "" }
                        )
                    },
                    None,
                    None,
                );
                for code in children {
                    s.actors[a].threads.push(Thread {
                        lease: Lease::None,
                        code: *code as u16,
                        pc: 0,
                        status: TStatus::Running,
                        parent: Some(th as u16),
                        detached: *detached,
                        pending: None,
                        failure: None,
                        handlers: Vec::new(),
                    });
                }
                s.actors[a].threads[th].pc += 1;
                // Children are settled by the actor loop.
            }
            op => {
                // A visible operation: the task reaches it.
                if needs_conn(op) {
                    arrive(model, s, a, th, foot);
                }
                return;
            }
        }
    }
}

fn finish_ok(
    model: &Model,
    s: &mut State,
    a: usize,
    th: usize,
    foot: &mut Footprint,
    rec: &mut Recorder,
) {
    s.actors[a].threads[th].status = TStatus::Done;
    drop_thread_lease(model, s, a, th, foot);
    if th == 0 {
        if let Some(id) = s.actors[a].txn.take() {
            // The task ended with its transaction still open: the
            // connection is released and the transaction rolled back.
            abort_txn(model, s, id, foot);
            rec.act(|| Action::Abort {
                actor: a as u16,
                conn: Conn::Tx,
            });
            rec.add(
                a,
                th,
                EventKind::Abort,
                None,
                || "transaction left open: rolled back".into(),
                None,
                None,
            );
        }
        s.actors[a].outcome = Outcome::Ok;
        s.actors[a].completions += 1;
        rec.add(
            a,
            th,
            EventKind::Local,
            None,
            || "return (success)".into(),
            None,
            None,
        );
    }
}

fn fail_thread(
    model: &Model,
    s: &mut State,
    a: usize,
    th: usize,
    reason: String,
    foot: &mut Footprint,
    rec: &mut Recorder,
) {
    // An active `try` handler catches the failure (Phase 7).
    if let Some((handler, error, opened_in_txn)) = s.actors[a].threads[th].handlers.pop() {
        if !opened_in_txn
            && th == 0
            && let Some(id) = s.actors[a].txn.take()
        {
            // The transaction callback re-throws: Drizzle rolls back first.
            cancel_pendings(s, id, a, 0);
            abort_txn(model, s, id, foot);
            rec.act(|| Action::Abort {
                actor: a as u16,
                conn: Conn::Tx,
            });
        }
        if let Some(local) = error {
            std::sync::Arc::make_mut(&mut s.actors[a].locals)
                .insert(local, Value::Str(reason.clone()));
        }
        let t = &mut s.actors[a].threads[th];
        t.pending = None;
        t.pc = handler;
        t.status = TStatus::Running;
        rec.add(
            a,
            th,
            EventKind::Fail,
            None,
            || format!("caught: {reason}"),
            None,
            None,
        );
        // The handler may start with local operations.
        settle_actor(model, s, a, foot, rec);
        return;
    }
    let t = &mut s.actors[a].threads[th];
    t.status = TStatus::DoneFailed;
    t.failure = Some(reason.clone());
    t.pending = None;
    drop_thread_lease(model, s, a, th, foot);
    rec.add(
        a,
        th,
        EventKind::Fail,
        None,
        || format!("failed: {reason}"),
        None,
        None,
    );
    if th == 0 {
        s.actors[a].outcome = Outcome::Failed(reason);
        if let Some(id) = s.actors[a].txn.take() {
            cancel_pendings(s, id, a, 0);
            abort_txn(model, s, id, foot);
            rec.act(|| Action::Abort {
                actor: a as u16,
                conn: Conn::Tx,
            });
            rec.add(
                a,
                th,
                EventKind::Abort,
                None,
                || "transaction rolled back".into(),
                None,
                None,
            );
        }
    } else if s.actors[a].threads[th].detached {
        // An unhandled rejection of an unawaited task terminates a Node.js
        // process.
        let p = model.process_of[a];
        rec.add(
            a,
            th,
            EventKind::Crash,
            None,
            || "unhandled rejection in an unawaited task terminates the process".into(),
            None,
            None,
        );
        crash_process(model, s, p, false, foot, rec);
    }
}

// ---------------------------------------------------------------------
// In-flight operations (Phase 9)
// ---------------------------------------------------------------------

/// The operation `th` of actor `a` could have in flight at its next step: an
/// explicit COMMIT, or a write outside a transaction.
fn candidate(model: &Model, s: &State, a: usize, th: usize) -> Option<InFlightOp> {
    let actor = &s.actors[a];
    let t = &actor.threads[th];
    if t.status != TStatus::Running || t.pending.is_some() {
        return None;
    }
    let step = op_at(model, s, a, th);
    match &step.op {
        Op::Commit => (actor.txn.is_some() && !tx_busy(actor)).then_some(InFlightOp::Commit),
        Op::Db { stmt }
            if stmt.conn == Conn::Pool
                && (model.pool_of(a).is_none() || t.lease == Lease::Held) =>
        {
            match &stmt.kind {
                StmtKind::Insert { table, .. }
                | StmtKind::Update { table, .. }
                | StmtKind::Delete { table, .. } => {
                    model.schema.index(table).map(InFlightOp::Autocommit)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The operations a process has in flight, in `(actor, task)` order, at most
/// eight (the combinations of their outcomes are enumerated).
pub fn in_flight_ops(model: &Model, s: &State, process: usize) -> Vec<(usize, usize, InFlightOp)> {
    let mut out = Vec::new();
    for a in 0..s.actors.len() {
        if model.process_of[a] != process {
            continue;
        }
        for th in 0..s.actors[a].threads.len() {
            if let Some(op) = candidate(model, s, a, th) {
                out.push((a, th, op));
            }
        }
    }
    out.truncate(8);
    out
}

/// What the server does with an in-flight operation: it never gets to it
/// (`reaches == false`), or it receives it and will complete it later (the
/// operation becomes an [`Orphan`]; `Err` when it cannot be formed).
#[allow(clippy::too_many_arguments)]
fn in_flight_effect(
    model: &Model,
    s: &mut State,
    a: usize,
    th: usize,
    op: InFlightOp,
    reaches: bool,
    foot: &mut Footprint,
    rec: &mut Recorder,
) -> Result<Option<u32>, String> {
    match op {
        InFlightOp::Commit => {
            let id = s.actors[a].txn.ok_or("COMMIT outside a transaction")?;
            s.actors[a].txn = None;
            if reaches {
                // The connection broke: the pool drops it.
                if s.txns.get(&id).is_some_and(|t| t.leased) {
                    if let Some(t) = s.txns.get_mut(&id) {
                        t.leased = false;
                    }
                    release_lease(model, s, model.process_of[a], foot);
                }
                let seq = id.0 & 0xFFF;
                let attempt = s.actors[a].attempt;
                s.orphans.push(crate::state::Orphan {
                    actor: a as u16,
                    seq,
                    op,
                    txn: Some(id),
                    stmt: None,
                    attempt,
                });
                rec.act(|| Action::Orphan {
                    actor: a as u16,
                    seq,
                });
                Ok(Some(seq))
            } else {
                cancel_pendings(s, id, a, th);
                abort_txn(model, s, id, foot);
                rec.act(|| Action::Abort {
                    actor: a as u16,
                    conn: Conn::Tx,
                });
                Ok(None)
            }
        }
        InFlightOp::Autocommit(_) => {
            if !reaches {
                // Never executed: nothing to undo (sequence values the
                // statement would have consumed are not modeled for this
                // world).
                return Ok(None);
            }
            let step = op_at(model, s, a, th);
            let Op::Db { stmt } = &step.op else {
                return Err("not a statement".into());
            };
            let closed = close_stmt(model, &s.actors[a].locals, stmt)?;
            let seq = s.actors[a].txn_seq;
            s.actors[a].txn_seq += 1;
            let attempt = s.actors[a].attempt;
            s.orphans.push(crate::state::Orphan {
                actor: a as u16,
                seq,
                op,
                txn: None,
                stmt: Some(closed),
                attempt,
            });
            rec.act(|| Action::Orphan {
                actor: a as u16,
                seq,
            });
            Ok(Some(seq))
        }
    }
}

/// Whether the server can complete the orphan now (an autocommit write that
/// would have to wait for a lock waits until its blocker finished).
fn orphan_ready(model: &Model, s: &State, o: &crate::state::Orphan) -> bool {
    let Some(stmt) = &o.stmt else {
        return true;
    };
    let mut trial = s.clone();
    let txn = new_txn(&mut trial, o.actor as usize, model.isolation, true);
    let mut foot = Footprint::default();
    let snapshot = open_snapshot(&mut trial, txn, &mut foot);
    let mut p = Pending {
        stmt: stmt.clone(),
        conn: Conn::Pool,
        txn,
        snapshot,
        todo: Vec::new(),
        affected: 0,
        found: None,
        blocked_on: None,
        started: false,
    };
    !matches!(
        run_stmt(model, &mut trial, &mut p, &mut foot),
        StmtStep::Blocked(_)
    )
}

/// The server completes a held operation: the COMMIT, or the autocommit
/// write, takes effect, or is rejected (then the physical outcome is "not
/// committed").
fn server_complete(
    model: &Model,
    s: &mut State,
    actor: usize,
    seq: u32,
    foot: &mut Footprint,
    rec: &mut Recorder,
) -> Result<(), String> {
    let at = s
        .orphans
        .iter()
        .position(|o| o.actor as usize == actor && o.seq == seq)
        .ok_or("the server holds no such operation")?;
    let o = s.orphans.remove(at);
    let mut committed = true;
    let text;
    match (&o.txn, &o.stmt) {
        (Some(id), _) => match commit_txn(model, s, *id, foot) {
            Ok(()) => {
                rec.act(|| Action::OrphanCommit {
                    actor: actor as u16,
                    seq,
                    expect: Expect::Ok,
                });
                text = "the server completes the COMMIT it received: committed".to_owned();
            }
            Err(e) => {
                abort_txn(model, s, *id, foot);
                rec.act(|| Action::OrphanCommit {
                    actor: actor as u16,
                    seq,
                    expect: Expect::Error(e.sqlstate.clone()),
                });
                committed = false;
                text = format!("the server completes the COMMIT it received: rejected ({e})");
            }
        },
        (None, Some(stmt)) => {
            let txn = new_txn(s, actor, model.isolation, true);
            let snapshot = open_snapshot(s, txn, foot);
            let mut p = Pending {
                stmt: stmt.clone(),
                conn: Conn::Pool,
                txn,
                snapshot,
                todo: Vec::new(),
                affected: 0,
                found: None,
                blocked_on: None,
                started: false,
            };
            let label = describe(model, stmt);
            let thread = 0x4000 + seq as u16;
            match run_stmt(model, s, &mut p, foot) {
                StmtStep::Done(result) => match commit_txn(model, s, txn, foot) {
                    Ok(()) => {
                        rec.act(|| Action::Stmt {
                            actor: actor as u16,
                            thread,
                            conn: Conn::Pool,
                            stmt: stmt.clone(),
                            resumed: false,
                            expect: Expect::Done(result.clone()),
                        });
                        text = format!(
                            "the server executes the statement it received: {label} (committed)"
                        );
                    }
                    Err(e) => {
                        abort_txn(model, s, txn, foot);
                        rec.act(|| Action::Stmt {
                            actor: actor as u16,
                            thread,
                            conn: Conn::Pool,
                            stmt: stmt.clone(),
                            resumed: false,
                            expect: Expect::Error(e.sqlstate.clone()),
                        });
                        committed = false;
                        text =
                            format!("the server rejects the statement it received: {label} ({e})");
                    }
                },
                StmtStep::Failed(e) => {
                    abort_txn(model, s, txn, foot);
                    rec.act(|| Action::Stmt {
                        actor: actor as u16,
                        thread,
                        conn: Conn::Pool,
                        stmt: stmt.clone(),
                        resumed: false,
                        expect: Expect::Error(e.sqlstate.clone()),
                    });
                    committed = false;
                    text = format!("the server rejects the statement it received: {label} ({e})");
                }
                StmtStep::Blocked(_) => {
                    return Err("the server cannot complete a statement that waits".into());
                }
            }
        }
        (None, None) => return Err("an orphan without an operation".into()),
    }
    if !committed && let Some(g) = s.actors[actor].ambiguous.iter_mut().find(|g| g.seq == seq) {
        g.world = World::NotCommitted;
    }
    rec.add(
        actor,
        0,
        EventKind::ServerCompletion,
        None,
        || text,
        None,
        None,
    );
    Ok(())
}

fn possible_worlds(
    _model: &Model,
    _s: &State,
    _a: usize,
    _th: usize,
    _op: InFlightOp,
) -> OutcomeSet {
    // Whether the server reached the operation is exactly what the
    // application cannot know: both are possible.
    OutcomeSet::BOTH
}

fn op_text(model: &Model, s: &State, a: usize, th: usize, op: InFlightOp) -> String {
    match op {
        InFlightOp::Commit => "COMMIT".to_owned(),
        InFlightOp::Autocommit(_) => {
            let step = op_at(model, s, a, th);
            match &step.op {
                Op::Db { stmt } => close_stmt(model, &s.actors[a].locals, stmt)
                    .map(|c| describe(model, &c))
                    .unwrap_or_else(|_| "statement".to_owned()),
                _ => "statement".to_owned(),
            }
        }
    }
}

fn lost_text(text: &str, reaches: bool, via: Via) -> String {
    let what = if reaches {
        "it reaches PostgreSQL, which will complete it"
    } else {
        "PostgreSQL never receives it (or rolls it back)"
    };
    match via {
        Via::ConnectionLost => format!(
            "{text} submitted; {what}; the acknowledgment never reaches the application (connection lost)"
        ),
        Via::ProcessCrash => {
            format!("{text} submitted; {what}; the process dies before an acknowledgment arrives")
        }
    }
}

/// The acknowledgment of the next operation of task `th` is lost while the
/// process lives: the server committed it or not, and the application gets a
/// connection error that is the same in both cases.
#[allow(clippy::result_large_err)]
fn lose_acknowledgment(
    model: &Model,
    s: &mut State,
    a: usize,
    th: usize,
    committed: bool,
    foot: &mut Footprint,
    rec: &mut Recorder,
) -> Result<(), Unsupported> {
    let step = op_at(model, s, a, th);
    let src = step.src.clone();
    let unsupported = |detail: String| Unsupported {
        actor: a,
        detail,
        src: src.clone(),
    };
    let Some(op) = candidate(model, s, a, th) else {
        return Err(unsupported(
            "an acknowledgment loss was planned where no operation is in flight".into(),
        ));
    };
    let possible = possible_worlds(model, s, a, th, op);
    let text = if rec.on {
        op_text(model, s, a, th, op)
    } else {
        String::new()
    };
    s.actors[a].touched = true;
    s.in_flight += 1;
    let orphan =
        in_flight_effect(model, s, a, th, op, committed, foot, rec).map_err(unsupported)?;
    let world = if committed {
        World::Committed
    } else {
        World::NotCommitted
    };
    let attempt = s.actors[a].attempt;
    s.actors[a].ambiguous.push(Ambiguity {
        seq: orphan.unwrap_or(u32::MAX),
        op,
        world,
        possible,
        attempt,
        via: Via::ConnectionLost,
    });
    rec.add(
        a,
        th,
        EventKind::AckLost,
        Some(&src),
        || lost_text(&text, committed, Via::ConnectionLost),
        None,
        None,
    );
    let reason = format!(
        "connection terminated unexpectedly: the outcome of {} is unknown",
        match op {
            InFlightOp::Commit => "COMMIT",
            InFlightOp::Autocommit(_) => "the statement",
        }
    );
    fail_thread(model, s, a, th, reason, foot, rec);
    Ok(())
}

/// A process crashes with `in_flight_ops` of it in flight; bit *i* of `mask`
/// says that the *i*-th operation committed on the server.
fn crash_in_flight(
    model: &Model,
    s: &mut State,
    process: usize,
    mask: u16,
    foot: &mut Footprint,
    rec: &mut Recorder,
) -> Result<(), String> {
    let ops = in_flight_ops(model, s, process);
    if ops.is_empty() && mask != 0 {
        return Err("a crash with operations in flight was planned where none is".into());
    }
    let possible: Vec<OutcomeSet> = ops
        .iter()
        .map(|(a, th, op)| possible_worlds(model, s, *a, *th, *op))
        .collect();
    for (i, (a, th, op)) in ops.iter().enumerate() {
        let committed = (mask >> i) & 1 == 1;
        let src = op_at(model, s, *a, *th).src.clone();
        let text = if rec.on {
            op_text(model, s, *a, *th, *op)
        } else {
            String::new()
        };
        s.actors[*a].touched = true;
        let orphan = in_flight_effect(model, s, *a, *th, *op, committed, foot, rec)?;
        let attempt = s.actors[*a].attempt;
        s.actors[*a].ambiguous.push(Ambiguity {
            seq: orphan.unwrap_or(u32::MAX),
            op: *op,
            world: if committed {
                World::Committed
            } else {
                World::NotCommitted
            },
            possible: possible[i],
            attempt,
            via: Via::ProcessCrash,
        });
        rec.add(
            *a,
            *th,
            EventKind::AckLost,
            Some(&src),
            || lost_text(&text, committed, Via::ProcessCrash),
            None,
            None,
        );
    }
    s.in_flight += 1;
    crash_process(model, s, process, true, foot, rec);
    Ok(())
}

// ---------------------------------------------------------------------
// Crashes and retries
// ---------------------------------------------------------------------

fn crash_process(
    model: &Model,
    s: &mut State,
    process: usize,
    budgeted: bool,
    foot: &mut Footprint,
    rec: &mut Recorder,
) {
    if budgeted {
        s.crashes += 1;
    }
    foot.push(Access::Global);
    if model.pools.get(process).copied().flatten().is_some() {
        // The process takes its pool with it: every connection is closed
        // (the server then ends the sessions), every waiting request dies.
        s.pools[process] = PoolState::default();
        for a in 0..s.actors.len() {
            if model.process_of[a] != process {
                continue;
            }
            for t in &mut s.actors[a].threads {
                t.lease = Lease::None;
            }
            let ids: Vec<TxnId> = s
                .txns
                .iter()
                .filter(|(_, t)| t.actor as usize == a)
                .map(|(id, _)| *id)
                .collect();
            for id in ids {
                if let Some(t) = s.txns.get_mut(&id) {
                    t.leased = false;
                }
            }
        }
    }
    for a in 0..s.actors.len() {
        if model.process_of[a] != process {
            continue;
        }
        let running = s.actors[a]
            .threads
            .iter()
            .any(|t| t.status == TStatus::Running);
        if !running {
            continue;
        }
        rec.add(
            a,
            0,
            EventKind::Crash,
            None,
            || format!("PROCESS CRASH ({})", model.process_names[process]),
            None,
            None,
        );
        rec.act(|| Action::Crash { actor: a as u16 });
        // Open transactions are rolled back by the server when the
        // connection drops; committed data and performed external effects
        // are unaffected.
        let mut ids: Vec<TxnId> = s.actors[a].txn.take().into_iter().collect();
        for t in &mut s.actors[a].threads {
            if let Some(p) = t.pending.take()
                && !ids.contains(&p.txn)
            {
                ids.push(p.txn);
            }
            if t.status == TStatus::Running {
                t.status = TStatus::DoneFailed;
                t.failure = Some("process crashed".into());
            }
        }
        for id in ids {
            abort_txn(model, s, id, foot);
        }
        s.actors[a].outcome = Outcome::Crashed;
    }
    s.apps[process] = model.app_initial.clone();
}

fn retry_actor(model: &Model, s: &mut State, a: usize, foot: &mut Footprint, rec: &mut Recorder) {
    s.retries += 1;
    let spec = &model.actors[a];
    let st = &mut s.actors[a];
    st.locals = std::sync::Arc::new(spec.arguments.iter().cloned().collect());
    st.threads = vec![main_thread()];
    st.txn = None;
    st.begin = None;
    st.tx_attempts = 0;
    st.outcome = Outcome::Running;
    st.attempt += 1;
    st.touched = false;
    let attempt = st.attempt;
    rec.add(
        a,
        0,
        EventKind::Retry,
        None,
        || {
            format!(
                "RETRY: the operation is re-executed from the start (attempt {})",
                attempt + 1
            )
        },
        None,
        None,
    );
    settle_actor(model, s, a, foot, rec);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::testing::model_from_programs;

    const PURCHASE: &str = "\
let product = select products where @id == pid
if product == null || product.stock <= 0 {
  fail \"out of stock\"
}
update products where @id == pid set @stock = product.stock - 1
insert orders set @id = oid, @product = pid
";

    fn lost_update_model(iso: &str) -> Model {
        model_from_programs(
            iso,
            &[
                ("A", PURCHASE, &[("pid", "p1".into()), ("oid", "oA".into())]),
                ("B", PURCHASE, &[("pid", "p1".into()), ("oid", "oB".into())]),
            ],
        )
    }

    #[test]
    fn two_actors_have_enabled_transitions_and_initial_state_is_settled() {
        let model = lost_update_model("read-committed");
        let s = initial_state(&model);
        let en = enabled(&model, &s);
        assert_eq!(en.len(), 2);
        assert!(en.iter().all(|t| t.kind == TKind::Step));
    }

    #[test]
    fn different_schedules_produce_different_states() {
        let model = lost_update_model("read-committed");
        let s0 = initial_state(&model);
        let run = |order: &[u16]| {
            let mut s = s0.clone();
            for &actor in order {
                let t = enabled(&model, &s)
                    .into_iter()
                    .find(|t| t.actor == actor)
                    .expect("enabled");
                s = apply(&model, &s, t, &mut Recorder::off()).unwrap().state;
            }
            s
        };
        // Serial: A runs to completion (3 steps), then B (read -> fails).
        let serial = run(&[0, 0, 0, 1]);
        // Interleaved: both read before either writes.
        let interleaved = run(&[0, 1, 0, 0, 1, 1]);
        assert_ne!(serial.canonical(&model), interleaved.canonical(&model));
        assert_eq!(
            serial.actors[1].outcome,
            Outcome::Failed("out of stock".into())
        );
        assert_eq!(interleaved.actors[1].outcome, Outcome::Ok);
        assert_eq!(interleaved.actors[0].outcome, Outcome::Ok);
    }
}

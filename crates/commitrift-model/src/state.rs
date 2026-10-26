//! The global state of the transition system, its canonical encoding, and
//! transitions.
//!
//! A state contains: the multi-versioned store and row locks, the active
//! (and, for SERIALIZABLE, committed) transactions, every actor's program
//! counters, locals and pending statements, shared in-process variables, the
//! external effect log, crash/retry counters, and the serialization
//! dependency graph. The *canonical encoding* ([`State::canonical`]) is the
//! equality used for deduplication: it contains everything that can influence
//! future behavior and nothing else (commit sequence numbers are replaced by
//! per-snapshot visibility masks, wait clocks by ranks).

use std::collections::{BTreeMap, BTreeSet};

use crate::db::{RowId, Snapshot, Store, TxnId};
use crate::epistemic::Ambiguity;
use crate::ir::{Conn, Isolation, JoinKind, LockMode, OnConflict, ProjItem};
use crate::model::Model;
use crate::value::{Expr, Locals, Row, Value};

/// A statement whose operands are evaluated: ready to be sent to a backend.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ClosedStmt {
    Select {
        table: u16,
        pred: Expr,
        lock: LockMode,
        joins: Vec<ClosedJoin>,
        projection: Option<Vec<(String, ProjItem)>>,
    },
    Count {
        table: u16,
        pred: Expr,
    },
    Update {
        table: u16,
        pred: Expr,
        set: Vec<(String, Expr)>,
    },
    Insert {
        table: u16,
        values: Row,
        on_conflict: OnConflict,
        returning: bool,
    },
    Delete {
        table: u16,
        pred: Expr,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ClosedJoin {
    pub kind: JoinKind,
    pub table: u16,
    pub left: String,
    pub right: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub stmt: ClosedStmt,
    pub conn: Conn,
    pub txn: TxnId,
    /// The snapshot (commit sequence) the statement runs on.
    pub snapshot: u64,
    /// Candidate rows still to be processed (the first may be blocked).
    pub todo: Vec<RowId>,
    pub affected: i64,
    pub found: Option<Row>,
    pub blocked_on: Option<TxnId>,
    /// Whether candidate rows have been determined (the snapshot is used).
    pub started: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TStatus {
    Running,
    Done,
    DoneFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thread {
    pub code: u16,
    pub pc: u32,
    pub status: TStatus,
    pub parent: Option<u16>,
    pub detached: bool,
    pub pending: Option<Pending>,
    pub failure: Option<String>,
    /// Active `try` handlers, innermost last: (handler pc, error local,
    /// whether a transaction was open when the block was entered).
    pub handlers: Vec<(u32, Option<String>, bool)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Running,
    Ok,
    Failed(String),
    Crashed,
}

impl Outcome {
    pub fn label(&self) -> String {
        match self {
            Self::Running => "running".into(),
            Self::Ok => "ok".into(),
            Self::Failed(r) => format!("failed: {r}"),
            Self::Crashed => "crashed".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BeginInfo {
    pub code: u16,
    pub pc: u32,
    /// Shared until written (Phase 8: a transition writes the locals of at
    /// most one actor; the others are not copied).
    pub locals: std::sync::Arc<Locals>,
    pub retry: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActorState {
    /// Shared until written (Phase 8: a transition writes the locals of at
    /// most one actor; the others are not copied).
    pub locals: std::sync::Arc<Locals>,
    pub threads: Vec<Thread>,
    pub txn: Option<TxnId>,
    pub begin: Option<BeginInfo>,
    pub tx_attempts: u32,
    pub outcome: Outcome,
    /// Number of times the main task returned normally.
    pub completions: u32,
    pub attempt: u32,
    pub txn_seq: u32,
    pub insert_seq: u32,
    /// Whether the current attempt has performed a visible operation.
    pub touched: bool,
    /// Phase 9: operations of this actor whose acknowledgment was lost
    /// (physical outcome and what the application can exclude). Empty unless
    /// `Bounds::max_in_flight` is set; part of the canonical encoding.
    pub ambiguous: Vec<Ambiguity>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Txn {
    pub actor: u16,
    pub iso: Isolation,
    pub snapshot: Option<u64>,
    pub implicit: bool,
    pub writes: Vec<RowId>,
    pub committed: Option<u64>,
    /// Predicate reads (SERIALIZABLE only): (table, predicate).
    pub reads: Vec<(u16, Expr)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalEvent {
    pub channel: String,
    pub key: Option<Value>,
    pub payload: Row,
    pub actor: u16,
    pub attempt: u32,
    /// Whether the actor had an open, uncommitted transaction when the
    /// effect was performed.
    pub in_txn: bool,
    /// Suppressed by the channel's key-based deduplication.
    pub deduplicated: bool,
}

/// Phase 9: an operation the server received from a client that is gone (or
/// no longer listening) and has not completed yet: a COMMIT whose
/// transaction is still open on the server, or an autocommit write that has
/// not run. Its completion is the separate [`TKind::ServerComplete`]
/// transition, so it can happen before or after anything else the
/// application does in between.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Orphan {
    pub actor: u16,
    /// Identity within the actor.
    pub seq: u32,
    pub op: crate::epistemic::InFlightOp,
    /// The transaction whose COMMIT the server holds.
    pub txn: Option<TxnId>,
    /// The statement the server received (autocommit writes).
    pub stmt: Option<ClosedStmt>,
    pub attempt: u32,
}

#[derive(Clone, Debug)]
pub struct State {
    pub store: Store,
    pub commit_seq: u64,
    pub txns: BTreeMap<TxnId, Txn>,
    pub actors: Vec<ActorState>,
    /// Shared in-process variables, one map per process.
    pub apps: Vec<BTreeMap<String, Value>>,
    pub externals: Vec<ExternalEvent>,
    pub crashes: u32,
    pub retries: u32,
    /// Phase 9: acknowledgments lost so far on this schedule.
    pub in_flight: u32,
    /// Phase 9: operations received by the server and not completed yet.
    pub orphans: Vec<Orphan>,
    /// Serialization dependency edges between transactions (SERIALIZABLE).
    pub deps: BTreeSet<(TxnId, TxnId)>,
    /// Next value of each sequence-backed column (`Model::sequences` order).
    /// Not transactional: an aborted insert does not give its value back.
    pub seqs: Vec<i64>,
    // Path-dependent bookkeeping (not part of the canonical encoding unless
    // the context-switch bound is active).
    pub steps: u32,
    pub last_thread: Option<(u16, u16)>,
    pub switches: u32,
}

/// What a transition is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TKind {
    /// Execute a thread's next visible operation.
    Step,
    /// Resume a blocked statement whose blocker finished.
    Resume,
    /// Crash a process (index in `thread`'s place).
    Crash,
    /// Start the next attempt of an actor (client retry / redelivery).
    Retry,
    /// PostgreSQL's deadlock detector aborts one waiting transaction of a
    /// wait-for cycle (which one depends on timer order: any member).
    Deadlock,
    /// Phase 9: the operation a task submitted (COMMIT, or an autocommit
    /// write) reached the server, which will complete it (see
    /// [`TKind::ServerComplete`]), but its acknowledgment is lost: the
    /// application gets a connection error and cannot tell this from
    /// [`TKind::LostNotCommitted`].
    LostCommitted,
    /// Phase 9: the same observation for the application, but the server
    /// never executed the operation (or rolled it back).
    LostNotCommitted,
    /// Phase 9: a process crashes with operations in flight. `actor` holds
    /// the bit mask of which in-flight operations committed (bit *i* is the
    /// *i*-th operation of [`crate::engine::in_flight_ops`]); `thread` is the
    /// process index.
    CrashInFlight,
    /// Phase 9: the server completes an operation it received earlier from a
    /// client that is gone (`actor` the submitting actor, `thread` the
    /// operation's sequence number).
    ServerComplete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Transition {
    pub kind: TKind,
    pub actor: u16,
    /// Thread index; the process index for `Crash`; 0 for `Retry`.
    pub thread: u16,
}

impl Transition {
    pub fn is_crash_or_retry(&self) -> bool {
        matches!(
            self.kind,
            TKind::Crash | TKind::Retry | TKind::CrashInFlight
        )
    }
}

impl State {
    pub fn txn_active(&self, id: TxnId) -> bool {
        self.txns.get(&id).is_some_and(|t| t.committed.is_none())
    }

    pub fn snapshot_of(&self, txn: TxnId) -> Snapshot {
        Snapshot {
            seq: self
                .txns
                .get(&txn)
                .and_then(|t| t.snapshot)
                .unwrap_or(self.commit_seq),
            own: txn,
        }
    }

    /// The committed database state a *new* snapshot would see.
    pub fn committed_snapshot(&self) -> Snapshot {
        Snapshot {
            seq: self.commit_seq,
            own: crate::db::INITIAL_TXN,
        }
    }

    /// Committed, live rows of `table`, in key order.
    pub fn committed_rows(&self, model: &Model, table: u16) -> Vec<Row> {
        let snap = self.committed_snapshot();
        let schema = model.schema.table(table);
        let mut rows: Vec<Row> = self
            .store
            .table_rows(table)
            .filter_map(|id| self.store.visible_values(id, snap).cloned())
            .collect();
        rows.sort_by_key(|r| crate::db::pk_key(schema, r));
        rows
    }

    pub fn is_finished(&self) -> bool {
        self.actors
            .iter()
            .all(|a| a.threads.iter().all(|t| t.status != TStatus::Running))
    }

    /// Canonical encoding; see the module documentation.
    pub fn canonical(&self, model: &Model) -> Vec<u8> {
        let mut e = Enc::default();
        // Snapshot holders: transactions with a snapshot, and pending
        // statements (which hold the snapshot they started on).
        let mut holders: Vec<((u32, u16), u64)> = Vec::new();
        for (id, txn) in &self.txns {
            if txn.committed.is_none()
                && let Some(s) = txn.snapshot
            {
                holders.push(((id.0, 0), s));
            }
        }
        for (ai, a) in self.actors.iter().enumerate() {
            for (ti, t) in a.threads.iter().enumerate() {
                if let Some(p) = &t.pending {
                    holders.push(((p.txn.0, 1 + ti as u16 + ((ai as u16) << 8)), p.snapshot));
                }
            }
        }
        holders.sort();
        let ser = model.uses_serializable();
        e.u32(holders.len() as u32);
        for (k, _) in &holders {
            e.u32(k.0);
            e.u16(k.1);
        }
        e.u32(self.store.rows.len() as u32);
        for (id, chain) in &self.store.rows {
            e.row_id(*id);
            e.u32(chain.len() as u32);
            for v in chain.iter() {
                match v.commit {
                    Some(c) => {
                        e.u8(1);
                        if ser {
                            e.u32(v.creator.0);
                        }
                        for (_, s) in &holders {
                            e.u8(u8::from(c <= *s));
                        }
                    }
                    None => {
                        e.u8(0);
                        e.u32(v.creator.0);
                    }
                }
                match &v.values {
                    Some(r) => {
                        e.u8(1);
                        e.row(r);
                    }
                    None => e.u8(0),
                }
            }
        }
        e.u32(self.store.locks.len() as u32);
        for (r, t) in &self.store.locks {
            e.row_id(*r);
            e.u32(t.0);
        }
        // Transactions: committed ones matter only for SERIALIZABLE history.
        let txns: Vec<_> = self
            .txns
            .iter()
            .filter(|(_, t)| t.committed.is_none() || ser)
            .collect();
        e.u32(txns.len() as u32);
        for (id, t) in txns {
            e.u32(id.0);
            e.u16(t.actor);
            e.u8(t.iso as u8);
            e.u8(u8::from(t.snapshot.is_some()));
            e.u8(u8::from(t.implicit));
            e.u8(u8::from(t.committed.is_some()));
            e.u32(t.writes.len() as u32);
            for w in &t.writes {
                e.row_id(*w);
            }
            e.u32(t.reads.len() as u32);
            for (tab, p) in &t.reads {
                e.u16(*tab);
                e.str(&format!("{p:?}"));
            }
        }
        e.u32(self.deps.len() as u32);
        for (a, b) in &self.deps {
            e.u32(a.0);
            e.u32(b.0);
        }
        e.u32(self.actors.len() as u32);
        for a in &self.actors {
            e.locals(&a.locals);
            e.u32(a.threads.len() as u32);
            for t in &a.threads {
                e.u16(t.code);
                e.u32(t.pc);
                e.u8(t.status as u8);
                e.u32(t.parent.map_or(u32::MAX, u32::from));
                e.u8(u8::from(t.detached));
                e.str(t.failure.as_deref().unwrap_or(""));
                if !t.handlers.is_empty() {
                    e.u32(t.handlers.len() as u32);
                    for (pc, local, open) in &t.handlers {
                        e.u32(*pc);
                        e.str(local.as_deref().unwrap_or(""));
                        e.u8(u8::from(*open));
                    }
                }
                match &t.pending {
                    None => e.u8(0),
                    Some(p) => {
                        e.u8(1);
                        e.str(&format!("{:?}", p.stmt));
                        e.u8(p.conn as u8);
                        e.u8(u8::from(p.started));
                        e.u32(p.txn.0);
                        e.u32(p.todo.len() as u32);
                        for r in &p.todo {
                            e.row_id(*r);
                        }
                        e.u64(p.affected as u64);
                        match &p.found {
                            Some(r) => {
                                e.u8(1);
                                e.row(r);
                            }
                            None => e.u8(0),
                        }
                        e.u32(p.blocked_on.map_or(u32::MAX, |t| t.0));
                    }
                }
            }
            e.u32(a.txn.map_or(u32::MAX, |t| t.0));
            match &a.begin {
                None => e.u8(0),
                Some(b) => {
                    e.u8(1);
                    e.u16(b.code);
                    e.u32(b.pc);
                    e.locals(&b.locals);
                    e.u32(b.retry);
                }
            }
            e.u32(a.tx_attempts);
            match &a.outcome {
                Outcome::Running => e.u8(0),
                Outcome::Ok => e.u8(1),
                Outcome::Failed(r) => {
                    e.u8(2);
                    e.str(r);
                }
                Outcome::Crashed => e.u8(3),
            }
            e.u32(a.completions);
            e.u32(a.attempt);
            e.u32(a.txn_seq);
            e.u32(a.insert_seq);
            e.u8(u8::from(a.touched));
        }
        e.u32(self.apps.len() as u32);
        for app in &self.apps {
            e.locals(app);
        }
        e.u32(self.externals.len() as u32);
        for x in &self.externals {
            e.str(&x.channel);
            match &x.key {
                Some(k) => {
                    e.u8(1);
                    e.value(k);
                }
                None => e.u8(0),
            }
            e.row(&x.payload);
            e.u16(x.actor);
            e.u32(x.attempt);
            e.u8(u8::from(x.in_txn));
            e.u8(u8::from(x.deduplicated));
        }
        e.u32(self.crashes);
        e.u32(self.retries);
        if !self.seqs.is_empty() {
            e.u32(self.seqs.len() as u32);
            for n in &self.seqs {
                e.value(&Value::Int(*n));
            }
        }
        if model.bounds.max_context_switches.is_some() {
            let (a, t) = self.last_thread.unwrap_or((u16::MAX, u16::MAX));
            e.u16(a);
            e.u16(t);
            e.u32(self.switches);
        }
        // Phase 9: unresolved outcomes are part of the state. Written only
        // when present, so encodings of models without in-flight faults are
        // unchanged.
        if self.actors.iter().any(|a| !a.ambiguous.is_empty()) || !self.orphans.is_empty() {
            e.u8(0xA9);
            e.u32(self.in_flight);
            e.u32(self.orphans.len() as u32);
            for o in &self.orphans {
                e.u16(o.actor);
                e.u32(o.seq);
                e.u32(o.txn.map_or(u32::MAX, |t| t.0));
                match &o.stmt {
                    Some(st) => {
                        e.u8(1);
                        e.str(&format!("{st:?}"));
                    }
                    None => e.u8(0),
                }
                e.u32(o.attempt);
            }
            for a in &self.actors {
                e.u32(a.ambiguous.len() as u32);
                for g in &a.ambiguous {
                    e.u32(g.seq);
                    e.u8(match g.op {
                        crate::epistemic::InFlightOp::Commit => 0,
                        crate::epistemic::InFlightOp::Autocommit(_) => 1,
                    });
                    e.u16(match g.op {
                        crate::epistemic::InFlightOp::Autocommit(t) => t,
                        crate::epistemic::InFlightOp::Commit => 0,
                    });
                    e.u8(match g.world {
                        crate::epistemic::World::Committed => 1,
                        crate::epistemic::World::NotCommitted => 2,
                    });
                    e.u8(g.possible.bits());
                    e.u32(g.attempt);
                    e.u8(match g.via {
                        crate::epistemic::Via::ConnectionLost => 0,
                        crate::epistemic::Via::ProcessCrash => 1,
                    });
                }
            }
        }
        e.finish()
    }
}

#[derive(Default)]
struct Enc {
    buf: Vec<u8>,
}

impl Enc {
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.buf.extend_from_slice(s.as_bytes());
    }
    fn row_id(&mut self, r: RowId) {
        self.u16(r.table);
        self.u16(r.actor);
        self.u32(r.n);
    }
    fn value(&mut self, v: &Value) {
        match v {
            Value::Null => self.u8(0),
            Value::Bool(b) => {
                self.u8(1);
                self.u8(u8::from(*b));
            }
            Value::Int(n) => {
                self.u8(2);
                self.u64(*n as u64);
            }
            Value::Str(s) => {
                self.u8(3);
                self.str(s);
            }
            Value::Row(r) => {
                self.u8(4);
                self.row(r);
            }
        }
    }
    fn row(&mut self, r: &Row) {
        self.u32(r.len() as u32);
        for (k, v) in r {
            self.str(k);
            self.value(v);
        }
    }
    fn locals(&mut self, l: &Locals) {
        self.row(l);
    }
    fn finish(self) -> Vec<u8> {
        self.buf
    }
}

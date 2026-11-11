//! Transition footprints and the independence relation used by
//! partial-order reduction.
//!
//! Every executed transition records the abstract resources it read and
//! wrote ([`Access`]). Two transitions enabled in the same state are
//! *independent* when no pair of their accesses conflicts; the explorer's
//! sleep sets are only valid if independent transitions commute and do not
//! disable one another. The relation is therefore deliberately conservative:
//! whenever a conflict cannot be excluded (unknown predicate evaluation,
//! same actor, crash, retry) the transitions are dependent.

use crate::db::{RowId, key_tuple};
use crate::model::Model;
use crate::value::{Expr, Row, predicate_matches};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// All transitions of one actor are mutually dependent (shared locals,
    /// one connection per transaction).
    Actor(u16),
    /// A scan read: all rows of `table` matching `pred`, as of now.
    Pred {
        table: u16,
        pred: Expr,
    },
    /// A read of one specific row (re-reading the latest version).
    RowRead(RowId),
    /// A change of one row's image (a new uncommitted version, a commit that
    /// publishes it, or its removal on abort).
    RowWrite {
        row: RowId,
        old: Option<Row>,
        new: Option<Row>,
    },
    LockCheck(RowId),
    LockChange(RowId),
    /// A read of the commit stream (taking a snapshot that outlives the
    /// statement).
    Snapshot,
    /// A commit that publishes writes.
    CommitWrite,
    /// SERIALIZABLE bookkeeping (dependency graph): all such transitions are
    /// mutually dependent.
    Ser,
    /// The wait-for graph (blocking, deadlock victim choice).
    Wait,
    Ext(String),
    App(usize, String),
    /// Allocation from a sequence (index into `Model::sequences`): the value
    /// an insert gets depends on the order of allocations.
    Seq(u16),
    /// A connection pool (by process): acquisitions and releases
    /// of one pool are mutually dependent (who gets the connection).
    Pool(usize),
    /// The shared budget of in-flight faults: taking one can
    /// disable another.
    Budget,
    /// Conflicts with everything.
    Global,
}

#[derive(Clone, Debug, Default)]
pub struct Footprint {
    pub acc: Vec<Access>,
}

impl Access {
    /// The kind of access as a bit (for [`mutation::mute`]).
    pub fn kind_bit(&self) -> u32 {
        match self {
            Access::Actor(_) => 1 << 0,
            Access::Pred { .. } => 1 << 1,
            Access::RowRead(_) => 1 << 2,
            Access::RowWrite { .. } => 1 << 3,
            Access::LockCheck(_) => 1 << 4,
            Access::LockChange(_) => 1 << 5,
            Access::Snapshot => 1 << 6,
            Access::CommitWrite => 1 << 7,
            Access::Ser => 1 << 8,
            Access::Wait => 1 << 9,
            Access::Ext(_) => 1 << 10,
            Access::App(..) => 1 << 11,
            Access::Seq(_) => 1 << 12,
            Access::Pool(_) => 1 << 13,
            Access::Budget => 1 << 14,
            Access::Global => 1 << 15,
        }
    }

    /// The kinds, in `kind_bit` order, with their names.
    pub const KINDS: [(&'static str, u32); 16] = [
        ("Actor", 1 << 0),
        ("Pred", 1 << 1),
        ("RowRead", 1 << 2),
        ("RowWrite", 1 << 3),
        ("LockCheck", 1 << 4),
        ("LockChange", 1 << 5),
        ("Snapshot", 1 << 6),
        ("CommitWrite", 1 << 7),
        ("Ser", 1 << 8),
        ("Wait", 1 << 9),
        ("Ext", 1 << 10),
        ("App", 1 << 11),
        ("Seq", 1 << 12),
        ("Pool", 1 << 13),
        ("Budget", 1 << 14),
        ("Global", 1 << 15),
    ];
}

/// Mutation testing of the dependence relation: with the
/// `mutation-testing` feature, the calling thread can delete every access of
/// some kinds from the footprints it records. An explorer then believes
/// transitions independent that are not; a differential test that notices is
/// sensitive to that kind of dependency.
#[cfg(feature = "mutation-testing")]
pub mod mutation {
    use std::cell::Cell;

    thread_local! {
        static MUTED: Cell<u32> = const { Cell::new(0) };
        static CLAIM_POOL: Cell<bool> = const { Cell::new(false) };
    }

    /// Delete only the pool access of a task that takes over a connection
    /// (`claim_lease`): the defect Phase 9 found and fixed. The arrival and
    /// release accesses stay.
    pub fn mute_claim_pool(on: bool) {
        CLAIM_POOL.with(|m| m.set(on));
    }

    pub(crate) fn claim_pool_muted() -> bool {
        CLAIM_POOL.with(Cell::get)
    }

    /// Delete accesses whose `kind_bit` is in `mask` from footprints
    /// recorded by this thread from now on (0 restores them).
    pub fn mute(mask: u32) {
        MUTED.with(|m| m.set(mask));
    }

    pub(super) fn muted() -> u32 {
        MUTED.with(Cell::get)
    }
}

impl Footprint {
    pub fn push(&mut self, a: Access) {
        #[cfg(feature = "mutation-testing")]
        if mutation::muted() & a.kind_bit() != 0 {
            return;
        }
        if !self.acc.contains(&a) {
            self.acc.push(a);
        }
    }
}

fn key_overlap(model: &Model, table: u16, a: &[Option<&Row>], b: &[Option<&Row>]) -> bool {
    let schema = model.schema.table(table);
    for rule in schema.key_rules() {
        let cols = &rule.columns;
        for x in a.iter().flatten() {
            let kx = key_tuple(x, cols);
            // Under NULLS NOT DISTINCT two keys with NULLs in the
            // same columns are equal, so their inserts are dependent.
            if !rule.checks(&kx) {
                continue;
            }
            for y in b.iter().flatten() {
                if key_tuple(y, cols) == kx {
                    return true;
                }
            }
        }
    }
    false
}

fn pred_hits(pred: &Expr, row: &Option<Row>) -> bool {
    match row {
        // An evaluation error is treated as a conflict.
        Some(r) => predicate_matches(pred, r).unwrap_or(true),
        None => false,
    }
}

fn conflict(model: &Model, x: &Access, y: &Access) -> bool {
    use Access::*;
    match (x, y) {
        (Global, _) | (_, Global) => true,
        (Actor(a), Actor(b)) => a == b,
        (Ser, Ser) | (Wait, Wait) | (Budget, Budget) => true,
        (Pool(a), Pool(b)) => a == b,
        (Snapshot, CommitWrite) | (CommitWrite, Snapshot) => true,
        (Ext(a), Ext(b)) => a == b,
        (App(p, v), App(q, w)) => p == q && v == w,
        (Seq(a), Seq(b)) => a == b,
        (LockCheck(a), LockChange(b))
        | (LockChange(a), LockCheck(b))
        | (LockChange(a), LockChange(b)) => a == b,
        (RowRead(a), RowWrite { row: b, .. }) | (RowWrite { row: b, .. }, RowRead(a)) => a == b,
        (
            RowWrite {
                row: ra,
                old: oa,
                new: na,
            },
            RowWrite {
                row: rb,
                old: ob,
                new: nb,
            },
        ) => {
            ra == rb
                || (ra.table == rb.table
                    && key_overlap(
                        model,
                        ra.table,
                        &[oa.as_ref(), na.as_ref()],
                        &[ob.as_ref(), nb.as_ref()],
                    ))
        }
        (Pred { table, pred }, RowWrite { row, old, new })
        | (RowWrite { row, old, new }, Pred { table, pred }) => {
            *table == row.table && (pred_hits(pred, old) || pred_hits(pred, new))
        }
        _ => false,
    }
}

/// Whether two footprints conflict (their transitions are dependent).
pub fn dependent(model: &Model, a: &Footprint, b: &Footprint) -> bool {
    a.acc
        .iter()
        .any(|x| b.acc.iter().any(|y| conflict(model, x, y)))
}

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
use crate::value::{Expr, Row, Value, predicate_matches};

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
    /// Conflicts with everything.
    Global,
}

#[derive(Clone, Debug, Default)]
pub struct Footprint {
    pub acc: Vec<Access>,
}

impl Footprint {
    pub fn push(&mut self, a: Access) {
        if !self.acc.contains(&a) {
            self.acc.push(a);
        }
    }
}

fn key_overlap(model: &Model, table: u16, a: &[Option<&Row>], b: &[Option<&Row>]) -> bool {
    let schema = model.schema.table(table);
    for (_, cols) in schema.key_sets() {
        for x in a.iter().flatten() {
            let kx = key_tuple(x, &cols);
            if kx.iter().any(Value::is_null) {
                continue;
            }
            for y in b.iter().flatten() {
                if key_tuple(y, &cols) == kx {
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
        (Ser, Ser) | (Wait, Wait) => true,
        (Snapshot, CommitWrite) | (CommitWrite, Snapshot) => true,
        (Ext(a), Ext(b)) => a == b,
        (App(p, v), App(q, w)) => p == q && v == w,
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

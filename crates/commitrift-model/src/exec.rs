//! Statement execution against the abstract database: PostgreSQL's
//! snapshot visibility, row locking, `READ COMMITTED` re-evaluation
//! (EvalPlanQual), `REPEATABLE READ` serialization failures, unique
//! constraints (including waiting on in-progress writers), and the
//! serialization dependency graph used for `SERIALIZABLE`.
//!
//! A statement runs as one atomic step until it must wait for another
//! transaction; it then returns [`StmtStep::Blocked`] and is resumed later
//! with the *same snapshot* and the remaining candidate rows.

use crate::db::{
    DbError, INITIAL_TXN, RowId, Snapshot, TableSchema, TxnId, Version, check_row, key_tuple,
    matches_row, pk_key,
};
use crate::footprint::{Access, Footprint};
use crate::ir::{Isolation, LockMode, OnConflict};
use crate::model::Model;
use crate::state::{ClosedStmt, Pending, State};
use crate::value::{Expr, Locals, Mode, Row, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StmtResult {
    Row(Option<Row>),
    Count(i64),
    Affected(i64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StmtStep {
    Done(StmtResult),
    Blocked(TxnId),
    Failed(DbError),
}

fn is_ser(s: &State, txn: TxnId) -> bool {
    s.txns
        .get(&txn)
        .is_some_and(|t| t.iso == Isolation::Serializable)
}

fn tracked(s: &State, txn: TxnId) -> bool {
    txn != INITIAL_TXN && is_ser(s, txn)
}

fn add_edge(s: &mut State, from: TxnId, to: TxnId) {
    if from != to && tracked(s, from) && tracked(s, to) {
        s.deps.insert((from, to));
    }
}

/// The semantic read set of `SELECT ... WHERE pred ORDER BY pk LIMIT 1`: the
/// rows up to and including the returned one in key order (the whole
/// predicate when nothing was returned or the key is composite). A
/// serializable implementation must treat conflicts on exactly this set;
/// PostgreSQL's predicate locks may cover more (relation or page granularity)
/// but, as observed with `FOR UPDATE SKIP LOCKED` (docs/phase6/POSTGRES-VALIDATION.md),
/// do not cover rows a `LIMIT 1` scan never reached.
fn select_read_predicate(schema: &TableSchema, pred: &Expr, returned: Option<&Row>) -> Expr {
    match (returned, schema.primary_key.as_slice()) {
        (Some(row), [pk]) => {
            let bound = row.get(pk).cloned().unwrap_or(Value::Null);
            Expr::bin(
                crate::value::BinOp::And,
                pred.clone(),
                Expr::bin(
                    crate::value::BinOp::Le,
                    Expr::Col(pk.clone()),
                    Expr::Lit(bound),
                ),
            )
        }
        _ => pred.clone(),
    }
}

/// Candidate rows (visible in `snap` and matching `pred`), in key order.
fn scan(
    model: &Model,
    s: &State,
    table: u16,
    pred: &Expr,
    snap: Snapshot,
) -> Result<Vec<(RowId, Row)>, DbError> {
    let schema = model.schema.table(table);
    let mut out = Vec::new();
    for id in s.store.table_rows(table) {
        if let Some(values) = s.store.visible_values(id, snap)
            && matches_row(pred, values)?
        {
            out.push((id, values.clone()));
        }
    }
    out.sort_by(|a, b| {
        pk_key(schema, &a.1)
            .cmp(&pk_key(schema, &b.1))
            .then(a.0.cmp(&b.0))
    });
    Ok(out)
}

/// Record the read of a SERIALIZABLE transaction and add the
/// wr / rw dependency edges it creates with existing versions.
fn ser_read(
    s: &mut State,
    txn: TxnId,
    table: u16,
    pred: &Expr,
    snap: Snapshot,
    foot: &mut Footprint,
) {
    foot.push(Access::Ser);
    let mut edges: Vec<(TxnId, TxnId)> = Vec::new();
    for id in s.store.table_rows(table).collect::<Vec<_>>() {
        let chain = &s.store.rows[&id];
        let vis = chain.iter().rposition(|v| snap.sees(v));
        if let Some(i) = vis
            && let Some(img) = &chain[i].values
            && matches_row(pred, img).unwrap_or(true)
        {
            edges.push((chain[i].creator, txn));
        }
        let start = vis.map_or(0, |i| i + 1);
        for idx in start..chain.len() {
            let w = &chain[idx];
            if w.creator == txn {
                continue;
            }
            let prev = if idx > 0 {
                chain[idx - 1].values.as_ref()
            } else {
                None
            };
            let hits = |r: Option<&Row>| r.is_some_and(|r| matches_row(pred, r).unwrap_or(true));
            if hits(prev) || hits(w.values.as_ref()) {
                edges.push((txn, w.creator));
            }
        }
    }
    for (a, b) in edges {
        add_edge(s, a, b);
    }
    if let Some(t) = s.txns.get_mut(&txn)
        && !t.reads.iter().any(|(tb, p)| *tb == table && p == pred)
    {
        t.reads.push((table, pred.clone()));
    }
}

/// Edges created by `writer` replacing `old` with `new` in `row`.
fn ser_write(
    s: &mut State,
    writer: TxnId,
    row: RowId,
    replaced_creator: Option<TxnId>,
    old: Option<&Row>,
    new: Option<&Row>,
    foot: &mut Footprint,
) {
    if !is_ser(s, writer) {
        return;
    }
    foot.push(Access::Ser);
    if let Some(c) = replaced_creator {
        add_edge(s, c, writer);
    }
    let readers: Vec<TxnId> = s
        .txns
        .iter()
        .filter(|(id, t)| {
            **id != writer
                && t.iso == Isolation::Serializable
                && t.reads.iter().any(|(tb, p)| {
                    *tb == row.table
                        && (old.is_some_and(|r| matches_row(p, r).unwrap_or(true))
                            || new.is_some_and(|r| matches_row(p, r).unwrap_or(true)))
                })
        })
        .map(|(id, _)| *id)
        .collect();
    for r in readers {
        add_edge(s, r, writer);
    }
}

/// Whether committing `txn` would complete a cycle of committed
/// SERIALIZABLE transactions in the dependency graph.
pub fn ser_would_fail(s: &State, txn: TxnId) -> bool {
    if !is_ser(s, txn) {
        return false;
    }
    let committed = |t: TxnId| s.txns.get(&t).is_some_and(|x| x.committed.is_some());
    let mut stack: Vec<TxnId> = vec![txn];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(cur) = stack.pop() {
        for (a, b) in &s.deps {
            if *a != cur {
                continue;
            }
            if *b == txn {
                return true;
            }
            if committed(*b) && seen.insert(*b) {
                stack.push(*b);
            }
        }
    }
    false
}

enum UCheck {
    Ok,
    Violation(String),
    Block(TxnId),
}

fn unique_check(
    model: &Model,
    s: &State,
    own: TxnId,
    table: u16,
    new: &Row,
    exclude: Option<RowId>,
) -> UCheck {
    let schema = model.schema.table(table);
    for (name, cols) in schema.key_sets() {
        let key = key_tuple(new, &cols);
        if key.iter().any(Value::is_null) {
            continue;
        }
        for rid in s.store.table_rows(table) {
            if Some(rid) == exclude {
                continue;
            }
            let chain = &s.store.rows[&rid];
            let Some(last) = chain.last() else { continue };
            let has_key = |v: Option<&Version>| {
                v.and_then(|v| v.values.as_ref())
                    .is_some_and(|r| key_tuple(r, &cols) == key)
            };
            if last.commit.is_none() {
                if last.creator == own {
                    if has_key(Some(last)) {
                        return UCheck::Violation(name);
                    }
                } else {
                    let prev = chain.len().checked_sub(2).map(|i| &chain[i]);
                    if has_key(Some(last)) || has_key(prev) {
                        return UCheck::Block(last.creator);
                    }
                }
            } else if has_key(Some(last)) {
                return UCheck::Violation(name);
            }
        }
    }
    UCheck::Ok
}

fn write_version(s: &mut State, txn: TxnId, row: RowId, new: Option<Row>) {
    let chain = s.store.rows.entry(row).or_default();
    match chain.last_mut() {
        Some(v) if v.creator == txn && v.commit.is_none() => v.values = new,
        _ => chain.push(Version {
            values: new,
            creator: txn,
            commit: None,
        }),
    }
    if let Some(t) = s.txns.get_mut(&txn)
        && !t.writes.contains(&row)
    {
        t.writes.push(row);
    }
}

fn apply_set(schema: &TableSchema, target: &Row, set: &[(String, Expr)]) -> Result<Row, DbError> {
    let mut new = target.clone();
    let empty = Locals::new();
    for (col, expr) in set {
        if !schema.has_column(col) {
            return Err(DbError::new(
                "42703",
                format!("column \"{col}\" does not exist"),
            ));
        }
        let v = expr
            .eval(&empty, Some(target), Mode::Sql)
            .map_err(|e| DbError::new("22003", e.to_string()))?;
        new.insert(col.clone(), v);
    }
    Ok(new)
}

/// Run (or resume) a statement. `p` carries the progress between calls.
///
/// For SERIALIZABLE transactions the serialization graph is re-checked after
/// every statement: PostgreSQL raises SQLSTATE 40001 at the statement that
/// completes a dangerous structure (observed on 17.11: the second writer of a
/// write-skew pair fails at its `UPDATE`, not at `COMMIT`).
pub fn run_stmt(model: &Model, s: &mut State, p: &mut Pending, foot: &mut Footprint) -> StmtStep {
    let step = run_stmt_inner(model, s, p, foot);
    if let StmtStep::Done(_) = &step
        && s.txns
            .get(&p.txn)
            .is_some_and(|t| t.iso == Isolation::Serializable)
        && ser_would_fail(s, p.txn)
    {
        foot.push(Access::Ser);
        return StmtStep::Failed(DbError::new(
            "40001",
            "could not serialize access due to read/write dependencies among transactions",
        ));
    }
    step
}

fn run_stmt_inner(model: &Model, s: &mut State, p: &mut Pending, foot: &mut Footprint) -> StmtStep {
    let iso = s.txns[&p.txn].iso;
    let snap = Snapshot {
        seq: p.snapshot,
        own: p.txn,
    };
    match p.stmt.clone() {
        ClosedStmt::Insert {
            table,
            values,
            on_conflict,
        } => run_insert(model, s, p, foot, table, values, on_conflict, iso),
        ClosedStmt::Count { table, pred } => {
            let rows = match scan(model, s, table, &pred, snap) {
                Ok(r) => r,
                Err(e) => return StmtStep::Failed(e),
            };
            foot.push(Access::Pred {
                table,
                pred: pred.clone(),
            });
            if iso == Isolation::Serializable {
                ser_read(s, p.txn, table, &pred, snap, foot);
            }
            StmtStep::Done(StmtResult::Count(rows.len() as i64))
        }
        ClosedStmt::Select { table, pred, lock } => {
            if !p.started {
                p.started = true;
                let rows = match scan(model, s, table, &pred, snap) {
                    Ok(r) => r,
                    Err(e) => return StmtStep::Failed(e),
                };
                foot.push(Access::Pred {
                    table,
                    pred: pred.clone(),
                });
                if lock == LockMode::None {
                    let first = rows.into_iter().next().map(|r| r.1);
                    if iso == Isolation::Serializable {
                        let read =
                            select_read_predicate(model.schema.table(table), &pred, first.as_ref());
                        ser_read(s, p.txn, table, &read, snap, foot);
                    }
                    return StmtStep::Done(StmtResult::Row(first));
                }
                p.todo = rows.into_iter().map(|r| r.0).collect();
            }
            let step = run_rows(model, s, p, foot, table, &pred, Some(lock), &[], false, iso);
            if iso == Isolation::Serializable
                && let StmtStep::Done(StmtResult::Row(returned)) = &step
            {
                let read =
                    select_read_predicate(model.schema.table(table), &pred, returned.as_ref());
                ser_read(s, p.txn, table, &read, snap, foot);
            }
            step
        }
        ClosedStmt::Update { table, pred, set } => {
            if !p.started {
                p.started = true;
                let rows = match scan(model, s, table, &pred, snap) {
                    Ok(r) => r,
                    Err(e) => return StmtStep::Failed(e),
                };
                foot.push(Access::Pred {
                    table,
                    pred: pred.clone(),
                });
                if iso == Isolation::Serializable {
                    ser_read(s, p.txn, table, &pred, snap, foot);
                }
                p.todo = rows.into_iter().map(|r| r.0).collect();
            }
            run_rows(model, s, p, foot, table, &pred, None, &set, false, iso)
        }
        ClosedStmt::Delete { table, pred } => {
            if !p.started {
                p.started = true;
                let rows = match scan(model, s, table, &pred, snap) {
                    Ok(r) => r,
                    Err(e) => return StmtStep::Failed(e),
                };
                foot.push(Access::Pred {
                    table,
                    pred: pred.clone(),
                });
                if iso == Isolation::Serializable {
                    ser_read(s, p.txn, table, &pred, snap, foot);
                }
                p.todo = rows.into_iter().map(|r| r.0).collect();
            }
            run_rows(model, s, p, foot, table, &pred, None, &[], true, iso)
        }
    }
}

/// The row-processing loop shared by `SELECT ... FOR UPDATE`, `UPDATE` and
/// `DELETE`: lock, re-check, and write each candidate row in turn.
#[allow(clippy::too_many_arguments)]
fn run_rows(
    model: &Model,
    s: &mut State,
    p: &mut Pending,
    foot: &mut Footprint,
    table: u16,
    pred: &Expr,
    select_lock: Option<LockMode>,
    set: &[(String, Expr)],
    delete: bool,
    iso: Isolation,
) -> StmtStep {
    let schema = model.schema.table(table);
    let snap = Snapshot {
        seq: p.snapshot,
        own: p.txn,
    };
    while let Some(&row) = p.todo.first() {
        foot.push(Access::LockCheck(row));
        if let Some(&holder) = s.store.locks.get(&row)
            && holder != p.txn
            && s.txn_active(holder)
        {
            match select_lock {
                Some(LockMode::SkipLocked) => {
                    p.todo.remove(0);
                    continue;
                }
                Some(LockMode::NoWait) => {
                    return StmtStep::Failed(DbError::new("55P03", "could not obtain lock on row"));
                }
                _ => {
                    foot.push(Access::Wait);
                    return StmtStep::Blocked(holder);
                }
            }
        }
        let chain = &s.store.rows[&row];
        let vis_idx = chain.iter().rposition(|v| snap.sees(v));
        let last = chain.last().expect("a candidate row has versions");
        let (target, replaced_creator): (Row, Option<TxnId>);
        if last.creator == p.txn && last.commit.is_none() {
            match &last.values {
                Some(v) => {
                    target = v.clone();
                    replaced_creator = None;
                }
                None => {
                    p.todo.remove(0);
                    continue;
                }
            }
        } else {
            let (latest_idx, latest) = s
                .store
                .latest_committed(row)
                .expect("a visible row has a committed version");
            if Some(latest_idx) != vis_idx {
                foot.push(Access::RowRead(row));
                match iso {
                    Isolation::ReadCommitted => match &latest.values {
                        None => {
                            p.todo.remove(0);
                            continue;
                        }
                        Some(img) => {
                            // EvalPlanQual: re-evaluate the predicate on the
                            // newest version of the row.
                            match matches_row(pred, img) {
                                Ok(true) => {
                                    target = img.clone();
                                    replaced_creator = Some(latest.creator);
                                }
                                Ok(false) => {
                                    p.todo.remove(0);
                                    continue;
                                }
                                Err(e) => return StmtStep::Failed(e),
                            }
                        }
                    },
                    _ => return StmtStep::Failed(DbError::serialization()),
                }
            } else {
                target = chain[latest_idx]
                    .values
                    .clone()
                    .expect("a visible candidate is live");
                replaced_creator = Some(chain[latest_idx].creator);
            }
        }
        if select_lock.is_some() {
            s.store.locks.insert(row, p.txn);
            foot.push(Access::LockChange(row));
            p.found = Some(target);
            p.todo.clear();
            return StmtStep::Done(StmtResult::Row(p.found.clone()));
        }
        if delete {
            write_version(s, p.txn, row, None);
            s.store.locks.insert(row, p.txn);
            foot.push(Access::LockChange(row));
            foot.push(Access::RowWrite {
                row,
                old: Some(target.clone()),
                new: None,
            });
            ser_write(s, p.txn, row, replaced_creator, Some(&target), None, foot);
            p.affected += 1;
            p.todo.remove(0);
            continue;
        }
        let new = match apply_set(schema, &target, set) {
            Ok(n) => n,
            Err(e) => return StmtStep::Failed(e),
        };
        // As for inserts: the outcome depends on rows sharing a key.
        foot.push(Access::RowWrite {
            row,
            old: Some(target.clone()),
            new: Some(new.clone()),
        });
        if let Err(e) = check_row(schema, &new) {
            return StmtStep::Failed(e);
        }
        match unique_check(model, s, p.txn, table, &new, Some(row)) {
            UCheck::Ok => {}
            UCheck::Violation(name) => {
                return StmtStep::Failed(DbError::new(
                    "23505",
                    format!("duplicate key value violates unique constraint \"{name}\""),
                ));
            }
            UCheck::Block(c) => {
                foot.push(Access::Wait);
                foot.push(Access::RowWrite {
                    row,
                    old: Some(target.clone()),
                    new: Some(new.clone()),
                });
                return StmtStep::Blocked(c);
            }
        }
        write_version(s, p.txn, row, Some(new.clone()));
        s.store.locks.insert(row, p.txn);
        foot.push(Access::LockChange(row));
        foot.push(Access::RowWrite {
            row,
            old: Some(target.clone()),
            new: Some(new.clone()),
        });
        ser_write(
            s,
            p.txn,
            row,
            replaced_creator,
            Some(&target),
            Some(&new),
            foot,
        );
        p.affected += 1;
        p.todo.remove(0);
    }
    if select_lock.is_some() {
        StmtStep::Done(StmtResult::Row(p.found.clone()))
    } else {
        StmtStep::Done(StmtResult::Affected(p.affected))
    }
}

#[allow(clippy::too_many_arguments)]
fn run_insert(
    model: &Model,
    s: &mut State,
    p: &mut Pending,
    foot: &mut Footprint,
    table: u16,
    mut values: Row,
    on_conflict: OnConflict,
    iso: Isolation,
) -> StmtStep {
    let schema = model.schema.table(table);
    for c in &schema.columns {
        values.entry(c.name.clone()).or_insert(Value::Null);
    }
    // The outcome of an insert (new row, conflict, wait) depends on every
    // row sharing a unique key with it, so its footprint is its intent to
    // write this image whether or not the insert succeeds.
    foot.push(Access::RowWrite {
        row: RowId {
            table,
            actor: u16::MAX - 1,
            n: 0,
        },
        old: None,
        new: Some(values.clone()),
    });
    if let Err(e) = check_row(schema, &values) {
        return StmtStep::Failed(e);
    }
    match unique_check(model, s, p.txn, table, &values, None) {
        UCheck::Ok => {}
        UCheck::Violation(name) => {
            if on_conflict == OnConflict::DoNothing {
                if iso != Isolation::ReadCommitted {
                    // The conflicting row committed after this transaction's
                    // snapshot: PostgreSQL raises a serialization failure.
                    let snap = Snapshot {
                        seq: p.snapshot,
                        own: p.txn,
                    };
                    let cols: Vec<_> = schema
                        .key_sets()
                        .into_iter()
                        .find(|(n, _)| *n == name)
                        .map(|(_, c)| c)
                        .unwrap_or_default();
                    let key = key_tuple(&values, &cols);
                    let visible = s.store.table_rows(table).any(|rid| {
                        s.store
                            .visible_values(rid, snap)
                            .is_some_and(|r| key_tuple(r, &cols) == key)
                    });
                    if !visible {
                        return StmtStep::Failed(DbError::serialization());
                    }
                }
                return StmtStep::Done(StmtResult::Affected(0));
            }
            return StmtStep::Failed(DbError::new(
                "23505",
                format!("duplicate key value violates unique constraint \"{name}\""),
            ));
        }
        UCheck::Block(c) => {
            foot.push(Access::Wait);
            foot.push(Access::RowWrite {
                row: RowId {
                    table,
                    actor: u16::MAX - 1,
                    n: 0,
                },
                old: None,
                new: Some(values),
            });
            return StmtStep::Blocked(c);
        }
    }
    let actor = p.txn.actor();
    let a = &mut s.actors[actor as usize];
    let row = RowId {
        table,
        actor,
        n: a.insert_seq,
    };
    a.insert_seq += 1;
    write_version(s, p.txn, row, Some(values.clone()));
    foot.push(Access::RowWrite {
        row,
        old: None,
        new: Some(values.clone()),
    });
    ser_write(s, p.txn, row, None, None, Some(&values), foot);
    StmtStep::Done(StmtResult::Affected(1))
}

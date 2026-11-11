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
    DbError, INITIAL_TXN, RowId, RowLock, Snapshot, TableSchema, TxnId, Version, check_row,
    key_tuple, matches_row, pk_key,
};
use crate::footprint::{Access, Footprint};
use crate::ir::{Isolation, JoinKind, LockMode, OnConflict, ProjItem};
use crate::model::Model;
use crate::state::{ClosedJoin, ClosedStmt, FkCheck, Pending, State};
use crate::value::{Expr, Locals, Mode, Row, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StmtResult {
    Row(Option<Row>),
    Count(i64),
    Affected(i64),
    /// The rows a multi-row `INSERT ... RETURNING` returned, in
    /// the order the statement processed them.
    Rows(Vec<Row>),
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
/// but, as observed with `FOR UPDATE SKIP LOCKED` (docs/architecture/postgres-model.md),
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

/// The semantic read set of a multi-row `SELECT ... ORDER BY pk
/// [LIMIT n]` that returned `returned`: the whole predicate, unless the
/// limit stopped the scan (then the rows up to the last returned one, as for
/// `LIMIT 1`); `LIMIT 0` reads nothing.
fn many_read_predicate(
    schema: &TableSchema,
    pred: &Expr,
    limit: Option<u32>,
    returned: &[Row],
) -> Expr {
    match limit {
        Some(0) => Expr::lit(false),
        Some(n) if returned.len() >= n as usize => {
            select_read_predicate(schema, pred, returned.last())
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
    for rule in schema.key_rules() {
        let (name, cols) = (rule.name.clone(), &rule.columns);
        let key = key_tuple(new, cols);
        // NULLS DISTINCT skips a key with a NULL; NULLS NOT
        // DISTINCT checks it, NULL equal to NULL (`Value::Null == Null`).
        if !rule.checks(&key) {
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
                    .is_some_and(|r| key_tuple(r, cols) == key)
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
    let chain = std::sync::Arc::make_mut(s.store.rows.entry(row).or_default());
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
            .map_err(|e| crate::db::eval_db_error(&e, "22003", ""))?;
        new.insert(col.clone(), v);
    }
    Ok(new)
}

/// Rows of `SELECT ... FROM table JOIN ...` visible in `snap`, as flat rows
/// with qualified keys (`table.column`), in key order of the base table and
/// then of each joined table (the order the oracle requests with `ORDER BY`).
/// A `LEFT JOIN` without a match contributes `NULL` for every column of the
/// joined table. Only equality joins between columns are supported.
fn join_rows(
    model: &Model,
    s: &State,
    table: u16,
    joins: &[ClosedJoin],
    pred: &Expr,
    snap: Snapshot,
) -> Result<Vec<Row>, DbError> {
    let qualify = |t: u16, row: Option<&Row>, out: &mut Row| {
        let schema = model.schema.table(t);
        for c in &schema.columns {
            let v = row
                .and_then(|r| r.get(&c.name))
                .cloned()
                .unwrap_or(Value::Null);
            out.insert(format!("{}.{}", schema.name, c.name), v);
        }
    };
    let visible = |t: u16| -> Vec<Row> {
        let schema = model.schema.table(t);
        let mut rows: Vec<Row> = s
            .store
            .table_rows(t)
            .filter_map(|id| s.store.visible_values(id, snap).cloned())
            .collect();
        rows.sort_by_key(|r| pk_key(schema, r));
        rows
    };
    let mut partial: Vec<Row> = visible(table)
        .iter()
        .map(|r| {
            let mut out = Row::new();
            qualify(table, Some(r), &mut out);
            out
        })
        .collect();
    for j in joins {
        let candidates = visible(j.table);
        let mut next = Vec::new();
        for left in partial {
            let mut matched = false;
            for r in &candidates {
                let mut combined = left.clone();
                qualify(j.table, Some(r), &mut combined);
                let on = Expr::bin(
                    crate::value::BinOp::Eq,
                    Expr::Col(j.left.clone()),
                    Expr::Col(j.right.clone()),
                );
                if matches_row(&on, &combined)? {
                    matched = true;
                    next.push(combined);
                }
            }
            if !matched && j.kind == JoinKind::Left {
                let mut combined = left;
                qualify(j.table, None, &mut combined);
                next.push(combined);
            }
        }
        partial = next;
    }
    let mut out = Vec::new();
    for row in partial {
        if matches_row(pred, &row)? {
            out.push(row);
        }
    }
    Ok(out)
}

/// The nested row of table `t` in a flat joined row, or `null` when every
/// primary-key column is `NULL` (an unmatched `LEFT JOIN`).
fn nested(model: &Model, t: u16, flat: &Row) -> Value {
    let schema = model.schema.table(t);
    let mut row = Row::new();
    for c in &schema.columns {
        let v = flat
            .get(&format!("{}.{}", schema.name, c.name))
            .cloned()
            .unwrap_or(Value::Null);
        row.insert(c.name.clone(), v);
    }
    let key: Vec<&String> = if schema.primary_key.is_empty() {
        schema.columns.iter().map(|c| &c.name).collect()
    } else {
        schema.primary_key.iter().collect()
    };
    if key.iter().all(|k| row.get(*k).is_none_or(Value::is_null)) {
        Value::Null
    } else {
        Value::Row(row)
    }
}

/// The result row a projection yields. Without joins `flat` has plain
/// column names; with joins it has qualified ones and the default result
/// nests each table's row under its name (Drizzle's shape).
pub fn project(
    model: &Model,
    table: u16,
    joins: &[ClosedJoin],
    projection: Option<&[(String, ProjItem)]>,
    flat: &Row,
) -> Row {
    let joined = !joins.is_empty();
    let tables: Vec<u16> = std::iter::once(table)
        .chain(joins.iter().map(|j| j.table))
        .collect();
    let Some(projection) = projection else {
        if !joined {
            return flat.clone();
        }
        return tables
            .iter()
            .map(|t| (model.schema.table(*t).name.clone(), nested(model, *t, flat)))
            .collect();
    };
    let mut out = Row::new();
    for (field, item) in projection {
        let v = match item {
            ProjItem::Column(c) => flat.get(c).cloned().unwrap_or(Value::Null),
            ProjItem::Table(name) => match model.schema.index(name) {
                Some(t) if joined => nested(model, t, flat),
                Some(_) => Value::Row(flat.clone()),
                None => Value::Null,
            },
        };
        out.insert(field.clone(), v);
    }
    out
}

/// Assign column defaults to an insert's values the first time it runs:
/// constants, and the next value of each sequence. Sequences are advanced
/// before any constraint is checked (as PostgreSQL evaluates defaults before
/// inserting), so a failing or conflicting insert still consumes its value.
///
/// A generated default produces a new value for this execution of
/// the statement, identified by the inserting actor's generation counter. A
/// database default (`gen_random_uuid()`) is computed by PostgreSQL while it
/// executes the INSERT; an application default (`$defaultFn`) by Drizzle
/// when it builds the statement, just before sending it. Both happen once per
/// execution, here, where the statement starts: a statement that waits for a
/// lock and resumes keeps its values, a retried one (a new execution) gets
/// new ones.
fn apply_defaults(
    model: &Model,
    s: &mut State,
    txn: TxnId,
    table: u16,
    values: &mut Row,
    foot: &mut Footprint,
    pass: Defaults,
) {
    use crate::db::{ColumnDefault, GeneratedSide};
    let p_actor = txn.actor();
    let schema = model.schema.table(table);
    for c in &schema.columns {
        if values.contains_key(&c.name) {
            continue;
        }
        // Drizzle computes `$defaultFn` (and, for a column without
        // a default, `$onUpdate`) while it builds the statement; PostgreSQL
        // evaluates a row's defaults when it forms that row.
        let default = c.default.as_ref().or(c.on_update.as_ref());
        let application = matches!(
            default,
            Some(ColumnDefault::Clock)
                | Some(ColumnDefault::Generated(crate::db::GeneratedSpec {
                    side: GeneratedSide::Application,
                    ..
                }))
        );
        match pass {
            Defaults::All => {}
            Defaults::Application if !application => continue,
            Defaults::Database if application => continue,
            Defaults::Application | Defaults::Database => {}
        }
        match default {
            Some(ColumnDefault::Now) => {
                values.insert(c.name.clone(), crate::db::now_value(txn));
            }
            Some(ColumnDefault::Clock) => {
                let a = &mut s.actors[p_actor as usize];
                let id = crate::value::GenId {
                    generator: crate::db::CLOCK_APP,
                    actor: p_actor,
                    n: a.gen_seq,
                };
                a.gen_seq += 1;
                values.insert(c.name.clone(), Value::Gen(id));
            }
            Some(crate::db::ColumnDefault::Value(v)) => {
                values.insert(c.name.clone(), v.clone());
            }
            Some(crate::db::ColumnDefault::Sequence(_)) => {
                let index = model
                    .sequences()
                    .iter()
                    .position(|(t, col, _)| *t == table && *col == c.name)
                    .expect("every sequence column is listed");
                let n = s.seqs[index];
                s.seqs[index] = n.saturating_add(1);
                foot.push(Access::Seq(index as u16));
                values.insert(c.name.clone(), Value::Int(n));
            }
            Some(crate::db::ColumnDefault::Generated(_)) => {
                let generator = model
                    .generators()
                    .iter()
                    .position(|(t, col, _)| *t == table && *col == c.name)
                    .expect("every generated column is listed");
                let actor = p_actor;
                let a = &mut s.actors[actor as usize];
                let id = crate::value::GenId {
                    generator: generator as u16,
                    actor,
                    n: a.gen_seq,
                };
                a.gen_seq += 1;
                values.insert(c.name.clone(), Value::Gen(id));
            }
            None => {}
        }
    }
}

/// Which column defaults [`apply_defaults`] assigns.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Defaults {
    /// Every default (a single-row `INSERT`, as in Phase 7-13).
    All,
    /// Those Drizzle computes in the application, for every row of a
    /// multi-row `INSERT` before it is sent.
    Application,
    /// Those PostgreSQL computes, when it forms the row.
    Database,
}

/// Run (or resume) a statement. `p` carries the progress between calls.
///
/// For SERIALIZABLE transactions the serialization graph is re-checked after
/// every statement: PostgreSQL raises SQLSTATE 40001 at the statement that
/// completes a dangerous structure (observed on 17.11: the second writer of a
/// write-skew pair fails at its `UPDATE`, not at `COMMIT`).
pub fn run_stmt(model: &Model, s: &mut State, p: &mut Pending, foot: &mut Footprint) -> StmtStep {
    // A statement whose rows are done runs its foreign-key checks
    // (PostgreSQL's AFTER triggers, at the end of the statement) before it
    // completes; a check that waits resumes here.
    let step = match p.deferred.clone() {
        Some(result) => finish_fk(model, s, p, foot, result),
        None => match run_stmt_inner(model, s, p, foot) {
            StmtStep::Done(result) if !p.fk.is_empty() => finish_fk(model, s, p, foot, result),
            other => other,
        },
    };
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
    // A predicate comparing a JSON column is not modeled.
    if !p.started {
        let pred = match &p.stmt {
            ClosedStmt::Select { pred, joins, .. } if joins.is_empty() => Some(pred),
            ClosedStmt::SelectMany { pred, .. }
            | ClosedStmt::Count { pred, .. }
            | ClosedStmt::Update { pred, .. }
            | ClosedStmt::Delete { pred, .. } => Some(pred),
            ClosedStmt::Insert {
                upsert: Some(u), ..
            }
            | ClosedStmt::InsertMany {
                upsert: Some(u), ..
            } => u.filter.as_ref(),
            _ => None,
        };
        if let Some(why) =
            pred.and_then(|e| crate::db::json_comparison(model.schema.table(p.stmt.table()), e))
        {
            return StmtStep::Failed(DbError::unsupported(why));
        }
    }
    let snap = Snapshot {
        seq: p.snapshot,
        own: p.txn,
    };
    match p.stmt.clone() {
        ClosedStmt::Insert {
            table,
            mut values,
            on_conflict,
            returning,
            returning_columns,
            upsert,
        } => {
            if !p.started {
                p.started = true;
                apply_defaults(model, s, p.txn, table, &mut values, foot, Defaults::All);
                if let ClosedStmt::Insert { values: v, .. } = &mut p.stmt {
                    v.clone_from(&values);
                }
            }
            let step = run_insert(
                model,
                s,
                p,
                foot,
                table,
                values,
                on_conflict,
                upsert.as_ref(),
                iso,
                &mut InsertProgress::default(),
            );
            match step {
                StmtStep::Done(StmtResult::Affected(n)) if returning => {
                    let row = if n > 0 { p.found.take() } else { None };
                    // `RETURNING col, ...` returns those columns.
                    let row = match (&returning_columns, row) {
                        (Some(cols), Some(r)) => Some(
                            cols.iter()
                                .map(|c| (c.clone(), r.get(c).cloned().unwrap_or(Value::Null)))
                                .collect(),
                        ),
                        (_, r) => r,
                    };
                    StmtStep::Done(StmtResult::Row(row))
                }
                other => other,
            }
        }
        ClosedStmt::InsertMany { .. } => run_insert_many(model, s, p, foot, iso),
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
        ClosedStmt::Select {
            table,
            pred,
            joins,
            projection,
            ..
        } if !joins.is_empty() => {
            // Joins are read-only here (row locks on joins are not modeled;
            // the extractor refuses them). Every joined table is read as a
            // whole: a conservative footprint (any write to a joined table
            // is dependent) and, under SERIALIZABLE, a whole-table read set.
            let rows = match join_rows(model, s, table, &joins, &pred, snap) {
                Ok(r) => r,
                Err(e) => return StmtStep::Failed(e),
            };
            for t in std::iter::once(table).chain(joins.iter().map(|j| j.table)) {
                foot.push(Access::Pred {
                    table: t,
                    pred: Expr::lit(true),
                });
                if iso == Isolation::Serializable {
                    ser_read(s, p.txn, t, &Expr::lit(true), snap, foot);
                }
            }
            let first = rows
                .first()
                .map(|r| project(model, table, &joins, projection.as_deref(), r));
            StmtStep::Done(StmtResult::Row(first))
        }
        ClosedStmt::Select {
            table,
            pred,
            lock,
            projection,
            ..
        } => {
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
                    let first =
                        first.map(|r| project(model, table, &[], projection.as_deref(), &r));
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
            match step {
                StmtStep::Done(StmtResult::Row(Some(r))) if projection.is_some() => StmtStep::Done(
                    StmtResult::Row(Some(project(model, table, &[], projection.as_deref(), &r))),
                ),
                other => other,
            }
        }
        // A multi-row SELECT. Without a lock it returns the
        // matching rows of its snapshot; with one it locks them one at a
        // time in scan order (`run_rows`): a lock it cannot take makes it
        // wait while holding the locks it already took.
        ClosedStmt::SelectMany {
            table,
            pred,
            lock,
            projection,
            limit,
            ..
        } => {
            let schema = model.schema.table(table);
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
                    let mut rows: Vec<Row> = rows.into_iter().map(|r| r.1).collect();
                    if let Some(n) = limit {
                        rows.truncate(n as usize);
                    }
                    if iso == Isolation::Serializable {
                        let read = many_read_predicate(schema, &pred, limit, &rows);
                        ser_read(s, p.txn, table, &read, snap, foot);
                    }
                    return StmtStep::Done(StmtResult::Rows(
                        rows.iter()
                            .map(|r| project(model, table, &[], projection.as_deref(), r))
                            .collect(),
                    ));
                }
                p.todo = rows.into_iter().map(|r| r.0).collect();
                if limit == Some(0) {
                    p.todo.clear();
                }
            }
            let step = run_rows(model, s, p, foot, table, &pred, Some(lock), &[], false, iso);
            match step {
                StmtStep::Done(StmtResult::Rows(rows)) => {
                    if iso == Isolation::Serializable {
                        let read = many_read_predicate(schema, &pred, limit, &rows);
                        ser_read(s, p.txn, table, &read, snap, foot);
                    }
                    StmtStep::Done(StmtResult::Rows(
                        rows.iter()
                            .map(|r| project(model, table, &[], projection.as_deref(), r))
                            .collect(),
                    ))
                }
                other => other,
            }
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
    let referencing = model.referencing(table);
    while let Some(&row) = p.todo.first() {
        // The row-lock mode. Without shared modes in the model
        // every two locks conflict, so the mode changes nothing.
        let mode = match select_lock {
            Some(l) => l.row_lock().unwrap_or(RowLock::Update),
            None if delete => RowLock::Update,
            None => update_lock_mode(schema, s, row, snap, p.txn, set, iso),
        };
        foot.push(Access::LockCheck(row));
        if let Some(holder) = s
            .store
            .conflicting_holder(row, p.txn, mode, |t| s.txn_active(t))
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
                // `FOR KEY SHARE` is not disturbed by an update
                // that left the key alone.
                let tolerated = mode == RowLock::KeyShare
                    && latest.values.as_ref().is_some_and(|new| {
                        vis_idx
                            .and_then(|i| chain[i].values.as_ref())
                            .is_some_and(|old| !key_columns_changed(schema, old, new))
                    });
                match iso {
                    _ if tolerated => {
                        target = latest.values.clone().expect("checked above");
                        replaced_creator = Some(latest.creator);
                    }
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
            s.store.grant(row, p.txn, mode);
            foot.push(Access::LockChange(row));
            // A multi-row SELECT keeps the row (and its lock) and
            // goes on with the next candidate until its limit is reached.
            if let ClosedStmt::SelectMany { out, limit, .. } = &mut p.stmt {
                out.push(target);
                p.todo.remove(0);
                if limit.is_some_and(|n| out.len() >= n as usize) {
                    p.todo.clear();
                }
                continue;
            }
            p.found = Some(target);
            p.todo.clear();
            return StmtStep::Done(StmtResult::Row(p.found.clone()));
        }
        if delete {
            write_version(s, p.txn, row, None);
            s.store.grant(row, p.txn, mode);
            foot.push(Access::LockChange(row));
            // Referencing rows must not remain (NO ACTION).
            for &(ct, fi) in &referencing {
                let fk = &model.schema.table(ct).foreign_keys[fi as usize];
                let key = key_tuple(&target, &fk.referenced_columns);
                if !key.iter().any(Value::is_null) {
                    p.fk.push(FkCheck::Children {
                        table: ct,
                        fk: fi,
                        key,
                        todo: None,
                    });
                }
            }
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
        if let Some(why) = crate::db::typed_row_problem(schema, &new) {
            return StmtStep::Failed(DbError::unsupported(why));
        }
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
        s.store.grant(row, p.txn, mode);
        foot.push(Access::LockChange(row));
        // The foreign-key checks the update fires.
        enqueue_update_fk(model, p, table, &target, &new, replaced_creator.is_none());
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
    if let ClosedStmt::SelectMany { out, .. } = &p.stmt {
        return StmtStep::Done(StmtResult::Rows(out.clone()));
    }
    if select_lock.is_some() {
        StmtStep::Done(StmtResult::Row(p.found.clone()))
    } else {
        StmtStep::Done(StmtResult::Affected(p.affected))
    }
}

/// What one row of an `INSERT` reports to its statement: the
/// rows the statement wrote before (an `ON CONFLICT DO UPDATE` reaching one
/// of them again is SQLSTATE 21000), and the row this one wrote.
#[derive(Default)]
struct InsertProgress {
    touched: Vec<RowId>,
    wrote: Option<RowId>,
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
    upsert: Option<&crate::ir::Upsert>,
    iso: Isolation,
    progress: &mut InsertProgress,
) -> StmtStep {
    let schema = model.schema.table(table);
    for c in &schema.columns {
        values.entry(c.name.clone()).or_insert(Value::Null);
    }
    // A value of a typed column the model does not represent.
    if let Some(why) = crate::db::typed_row_problem(schema, &values) {
        return StmtStep::Failed(DbError::unsupported(why));
    }
    // `ON CONFLICT (target) DO UPDATE` checks its arbiter first.
    if let Some(u) = upsert {
        foot.push(Access::RowWrite {
            row: RowId {
                table,
                actor: u16::MAX - 1,
                n: 0,
            },
            old: None,
            new: Some(values.clone()),
        });
        let nulls_not_distinct = schema.arbiter_nulls_not_distinct(&u.target);
        match arbiter(s, p.txn, table, &values, &u.target, nulls_not_distinct) {
            Arbiter::None => {}
            Arbiter::Block(c) => {
                foot.push(Access::Wait);
                return StmtStep::Blocked(c);
            }
            Arbiter::Row(r) => {
                // A row this statement inserted or updated
                // (PostgreSQL: the tuple is invisible to the command, or
                // updated by it).
                if progress.touched.contains(&r) {
                    return StmtStep::Failed(DbError::new(
                        "21000",
                        "ON CONFLICT DO UPDATE command cannot affect row a second time",
                    ));
                }
                let step = upsert_row(model, s, p, foot, table, &values, u, r, iso);
                if matches!(step, StmtStep::Done(StmtResult::Affected(1))) {
                    progress.wrote = Some(r);
                }
                return step;
            }
        }
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
    progress.wrote = Some(row);
    foot.push(Access::RowWrite {
        row,
        old: None,
        new: Some(values.clone()),
    });
    ser_write(s, p.txn, row, None, None, Some(&values), foot);
    // The referenced rows must exist (`MATCH SIMPLE`: a NULL in
    // the key skips the check).
    for (fi, fk) in schema.foreign_keys.iter().enumerate() {
        let key = key_tuple(&values, &fk.columns);
        if !key.iter().any(Value::is_null) {
            p.fk.push(FkCheck::Parent {
                table,
                fk: fi as u16,
                key,
                row: None,
            });
        }
    }
    p.found = Some(values);
    StmtStep::Done(StmtResult::Affected(1))
}

/// A multi-row `INSERT`, as PostgreSQL's `ModifyTable` runs it:
/// the rows one after the other, each formed (its database defaults
/// evaluated) when the previous one is done, inserted with the single-row
/// semantics (unique checks and waits, `ON CONFLICT`); the statement's
/// foreign-key checks run after the last row (AFTER ROW triggers fire at the
/// end of the statement, in row order). Drizzle's application defaults are
/// computed for every row when the statement starts. A failure of any row
/// fails the statement, and with it its transaction: no row stays.
fn run_insert_many(
    model: &Model,
    s: &mut State,
    p: &mut Pending,
    foot: &mut Footprint,
    iso: Isolation,
) -> StmtStep {
    let ClosedStmt::InsertMany {
        table,
        mut rows,
        on_conflict,
        returning,
        returning_columns,
        upsert,
        mut cursor,
        mut row_started,
        mut out,
        mut touched,
    } = p.stmt.clone()
    else {
        unreachable!("run_insert_many on another statement");
    };
    if !p.started {
        p.started = true;
        for row in &mut rows {
            apply_defaults(model, s, p.txn, table, row, foot, Defaults::Application);
        }
    }
    let mut outcome = None;
    while cursor < rows.len() {
        if !row_started {
            row_started = true;
            apply_defaults(
                model,
                s,
                p.txn,
                table,
                &mut rows[cursor],
                foot,
                Defaults::Database,
            );
        }
        let mut progress = InsertProgress {
            touched: touched.clone(),
            wrote: None,
        };
        p.found = None;
        let step = run_insert(
            model,
            s,
            p,
            foot,
            table,
            rows[cursor].clone(),
            on_conflict,
            upsert.as_ref(),
            iso,
            &mut progress,
        );
        match step {
            StmtStep::Done(StmtResult::Affected(n)) => {
                if n > 0 {
                    p.affected += n;
                    if let Some(r) = progress.wrote {
                        touched.push(r);
                    }
                    if returning && let Some(r) = p.found.take() {
                        out.push(match &returning_columns {
                            Some(cols) => cols
                                .iter()
                                .map(|c| (c.clone(), r.get(c).cloned().unwrap_or(Value::Null)))
                                .collect(),
                            None => r,
                        });
                    }
                }
                p.found = None;
                cursor += 1;
                row_started = false;
            }
            other => {
                outcome = Some(other);
                break;
            }
        }
    }
    let result = if returning {
        StmtResult::Rows(out.clone())
    } else {
        StmtResult::Affected(p.affected)
    };
    p.stmt = ClosedStmt::InsertMany {
        table,
        rows,
        on_conflict,
        returning,
        returning_columns,
        upsert,
        cursor,
        row_started,
        out,
        touched,
    };
    outcome.unwrap_or(StmtStep::Done(result))
}

enum Arbiter {
    None,
    Block(TxnId),
    Row(RowId),
}

/// The row of `table` that conflicts with `new` on the arbiter columns, or
/// the transaction to wait for (as [`unique_check`] decides it). Phase 16:
/// with a `NULLS NOT DISTINCT` arbiter a key with NULLs is looked up too
/// (NULL equal to NULL); otherwise it never conflicts.
fn arbiter(
    s: &State,
    own: TxnId,
    table: u16,
    new: &Row,
    cols: &[String],
    nulls_not_distinct: bool,
) -> Arbiter {
    let key = key_tuple(new, cols);
    if !nulls_not_distinct && key.iter().any(Value::is_null) {
        return Arbiter::None;
    }
    for rid in s.store.table_rows(table) {
        let chain = &s.store.rows[&rid];
        let Some(last) = chain.last() else { continue };
        let has_key = |v: Option<&Version>| {
            v.and_then(|v| v.values.as_ref())
                .is_some_and(|r| key_tuple(r, cols) == key)
        };
        if last.commit.is_none() && last.creator != own {
            let prev = chain.len().checked_sub(2).map(|i| &chain[i]);
            if has_key(Some(last)) || has_key(prev) {
                return Arbiter::Block(last.creator);
            }
        } else if has_key(Some(last)) {
            return Arbiter::Row(rid);
        }
    }
    Arbiter::None
}

/// The `DO UPDATE` arm on the conflicting row `r`, as PostgreSQL's
/// `ExecOnConflictUpdate`: under REPEATABLE READ / SERIALIZABLE a row the
/// snapshot does not see as it is now is a serialization failure; the row is
/// locked (`FOR UPDATE` when a key column changes, else `FOR NO KEY
/// UPDATE`), even when `WHERE` then rejects the update; `SET` and `WHERE`
/// read the existing row and `excluded`.
#[allow(clippy::too_many_arguments)]
fn upsert_row(
    model: &Model,
    s: &mut State,
    p: &mut Pending,
    foot: &mut Footprint,
    table: u16,
    proposed: &Row,
    u: &crate::ir::Upsert,
    r: RowId,
    iso: Isolation,
) -> StmtStep {
    let schema = model.schema.table(table);
    foot.push(Access::RowRead(r));
    let chain = &s.store.rows[&r];
    let own = chain
        .last()
        .is_some_and(|v| v.creator == p.txn && v.commit.is_none());
    let replaced_creator = (!own)
        .then(|| s.store.latest_committed(r).map(|(_, v)| v.creator))
        .flatten();
    if iso != Isolation::ReadCommitted && !own {
        let snap = Snapshot {
            seq: p.snapshot,
            own: p.txn,
        };
        let vis = chain.iter().rposition(|v| snap.sees(v));
        let latest = s.store.latest_committed(r).map(|(i, _)| i);
        if vis != latest {
            return StmtStep::Failed(DbError::serialization());
        }
    }
    let Some(target) = newest_image(s, r, p.txn) else {
        return StmtStep::Failed(DbError::new("XX000", "the conflicting row is gone"));
    };
    let mut env = target.clone();
    for (c, v) in proposed {
        env.insert(format!("excluded.{c}"), v.clone());
    }
    let new = match apply_set(schema, &env, &u.set) {
        Ok(n) => n
            .into_iter()
            .filter(|(c, _)| !c.starts_with("excluded."))
            .collect::<Row>(),
        Err(e) => return StmtStep::Failed(e),
    };
    if let Some(why) = crate::db::typed_row_problem(schema, &new) {
        return StmtStep::Failed(DbError::unsupported(why));
    }
    let mode = if key_columns_changed(schema, &target, &new) {
        RowLock::Update
    } else {
        RowLock::NoKeyUpdate
    };
    foot.push(Access::LockCheck(r));
    if let Some(h) = s
        .store
        .conflicting_holder(r, p.txn, mode, |t| s.txn_active(t))
    {
        foot.push(Access::Wait);
        return StmtStep::Blocked(h);
    }
    s.store.grant(r, p.txn, mode);
    foot.push(Access::LockChange(r));
    if iso == Isolation::Serializable {
        let snap = Snapshot {
            seq: p.snapshot,
            own: p.txn,
        };
        ser_read(
            s,
            p.txn,
            table,
            &key_pred(&u.target, &key_tuple(&target, &u.target)),
            snap,
            foot,
        );
    }
    if let Some(f) = &u.filter {
        match f.eval(&Locals::new(), Some(&env), Mode::Sql) {
            Ok(Value::Bool(true)) => {}
            Ok(_) => return StmtStep::Done(StmtResult::Affected(0)),
            Err(e) => return StmtStep::Failed(crate::db::eval_db_error(&e, "XX000", "")),
        }
    }
    foot.push(Access::RowWrite {
        row: r,
        old: Some(target.clone()),
        new: Some(new.clone()),
    });
    if let Err(e) = check_row(schema, &new) {
        return StmtStep::Failed(e);
    }
    match unique_check(model, s, p.txn, table, &new, Some(r)) {
        UCheck::Ok => {}
        UCheck::Violation(name) => {
            return StmtStep::Failed(DbError::new(
                "23505",
                format!("duplicate key value violates unique constraint \"{name}\""),
            ));
        }
        UCheck::Block(c) => {
            foot.push(Access::Wait);
            return StmtStep::Blocked(c);
        }
    }
    write_version(s, p.txn, r, Some(new.clone()));
    ser_write(
        s,
        p.txn,
        r,
        replaced_creator,
        Some(&target),
        Some(&new),
        foot,
    );
    enqueue_update_fk(model, p, table, &target, &new, own);
    p.found = Some(new);
    StmtStep::Done(StmtResult::Affected(1))
}

/// The foreign-key checks an update of `old` to `new` in `table`
/// fires: its own foreign keys whose key changed (or whose old version this
/// transaction wrote, as `RI_FKey_fk_upd_check_required` decides), and the
/// referencing rows of a changed referenced key.
fn enqueue_update_fk(model: &Model, p: &mut Pending, table: u16, old: &Row, new: &Row, own: bool) {
    let schema = model.schema.table(table);
    for (fi, fk) in schema.foreign_keys.iter().enumerate() {
        let key = key_tuple(new, &fk.columns);
        if !key.iter().any(Value::is_null) && (own || key != key_tuple(old, &fk.columns)) {
            p.fk.push(FkCheck::Parent {
                table,
                fk: fi as u16,
                key,
                row: None,
            });
        }
    }
    for (ct, fi) in model.referencing(table) {
        let fk = &model.schema.table(ct).foreign_keys[fi as usize];
        let old_key = key_tuple(old, &fk.referenced_columns);
        if !old_key.iter().any(Value::is_null) && old_key != key_tuple(new, &fk.referenced_columns)
        {
            p.fk.push(FkCheck::Children {
                table: ct,
                fk: fi,
                key: old_key,
                todo: None,
            });
        }
    }
}

/// Whether an update from `old` to `new` changes a column of a unique key
/// (PostgreSQL's "key columns": those of unique indexes a foreign key can
/// reference). Such an update locks the row `FOR UPDATE`, other updates
/// `FOR NO KEY UPDATE`.
fn key_columns_changed(schema: &TableSchema, old: &Row, new: &Row) -> bool {
    schema
        .key_sets()
        .iter()
        .flat_map(|(_, cols)| cols)
        .any(|c| old.get(c) != new.get(c))
}

/// The lock an `UPDATE` takes on `row`: computed from the
/// version it would change (the newest under READ COMMITTED, as after
/// EvalPlanQual; the snapshot's otherwise).
fn update_lock_mode(
    schema: &TableSchema,
    s: &State,
    row: RowId,
    snap: Snapshot,
    own: TxnId,
    set: &[(String, Expr)],
    iso: Isolation,
) -> RowLock {
    let Some(chain) = s.store.rows.get(&row) else {
        return RowLock::NoKeyUpdate;
    };
    let base = match chain.last() {
        Some(v) if v.creator == own && v.commit.is_none() => v.values.as_ref(),
        _ if iso == Isolation::ReadCommitted => s
            .store
            .latest_committed(row)
            .and_then(|(_, v)| v.values.as_ref()),
        _ => s.store.visible_values(row, snap),
    };
    match base.map(|b| (b, apply_set(schema, b, set))) {
        Some((b, Ok(new))) if key_columns_changed(schema, b, &new) => RowLock::Update,
        _ => RowLock::NoKeyUpdate,
    }
}

enum FkStep {
    Ok,
    Blocked(TxnId),
    Failed(DbError),
}

/// Run the pending foreign-key checks of a statement whose rows are done,
/// then complete it with `result`.
fn finish_fk(
    model: &Model,
    s: &mut State,
    p: &mut Pending,
    foot: &mut Footprint,
    result: StmtResult,
) -> StmtStep {
    p.deferred = Some(result);
    match run_fk(model, s, p, foot) {
        FkStep::Ok => StmtStep::Done(p.deferred.take().expect("set above")),
        FkStep::Blocked(t) => StmtStep::Blocked(t),
        FkStep::Failed(e) => StmtStep::Failed(e),
    }
}

fn key_pred(cols: &[String], key: &[Value]) -> Expr {
    cols.iter()
        .zip(key)
        .map(|(c, v)| Expr::eq(Expr::Col(c.clone()), Expr::Lit(v.clone())))
        .reduce(|a, b| Expr::bin(crate::value::BinOp::And, a, b))
        .unwrap_or_else(|| Expr::lit(true))
}

/// The newest image of a row as this transaction's lock-rows node sees it
/// after locking: its own uncommitted version, else the latest committed
/// one (`None`: deleted or absent).
fn newest_image(s: &State, row: RowId, own: TxnId) -> Option<Row> {
    let chain = s.store.rows.get(&row)?;
    match chain.last() {
        Some(v) if v.creator == own && v.commit.is_none() => v.values.clone(),
        _ => s
            .store
            .latest_committed(row)
            .and_then(|(_, v)| v.values.clone()),
    }
}

/// PostgreSQL's RI triggers for `NO ACTION`/`RESTRICT` foreign keys
/// (`docs/architecture/postgres-model.md`):
///
/// * referencing side (`RI_FKey_check`): `SELECT 1 FROM parent WHERE key =
///   $1 FOR KEY SHARE`, on a new snapshot under READ COMMITTED and on the
///   transaction's snapshot otherwise; no row is SQLSTATE 23503. The lock
///   waits for a holder of `FOR UPDATE` (a `SELECT ... FOR UPDATE`, a
///   `DELETE`, a key-changing `UPDATE`); afterwards the row's newest version
///   decides: gone or re-keyed is 23503 under READ COMMITTED and 40001
///   otherwise; an update that kept the key does not matter.
/// * referenced side (`RI_FKey_noaction_del/upd`): `SELECT 1 FROM child
///   WHERE fk = $1 FOR KEY SHARE` on the newest snapshot; a remaining row is
///   23503, and under REPEATABLE READ / SERIALIZABLE a row the transaction's
///   own snapshot does not see is 40001.
fn run_fk(model: &Model, s: &mut State, p: &mut Pending, foot: &mut Footprint) -> FkStep {
    let iso = s.txns[&p.txn].iso;
    let txn_snap = Snapshot {
        seq: p.snapshot,
        own: p.txn,
    };
    while let Some(check) = p.fk.first().cloned() {
        match check {
            FkCheck::Parent {
                table,
                fk,
                key,
                row,
            } => {
                let child = model.schema.table(table);
                let def = &child.foreign_keys[fk as usize];
                let parent = model
                    .schema
                    .index(&def.references)
                    .expect("validated: the referenced table is modeled");
                let pschema = model.schema.table(parent);
                let cols = &def.referenced_columns;
                let violation = || {
                    DbError::new(
                        "23503",
                        format!(
                            "insert or update on table \"{}\" violates foreign key constraint \"{}\"",
                            child.name,
                            def.name(&child.name)
                        ),
                    )
                };
                let rid = match row {
                    Some(r) => r,
                    None => {
                        let snap = if iso == Isolation::ReadCommitted {
                            Snapshot {
                                seq: s.commit_seq,
                                own: p.txn,
                            }
                        } else {
                            txn_snap
                        };
                        let pred = key_pred(cols, &key);
                        foot.push(Access::Pred {
                            table: parent,
                            pred: pred.clone(),
                        });
                        if iso == Isolation::Serializable {
                            ser_read(s, p.txn, parent, &pred, snap, foot);
                        }
                        let found = s.store.table_rows(parent).find(|id| {
                            s.store
                                .visible_values(*id, snap)
                                .is_some_and(|r| key_tuple(r, cols) == key)
                        });
                        let Some(r) = found else {
                            return FkStep::Failed(violation());
                        };
                        if let Some(FkCheck::Parent { row, .. }) = p.fk.first_mut() {
                            *row = Some(r);
                        }
                        r
                    }
                };
                foot.push(Access::LockCheck(rid));
                if let Some(h) = s
                    .store
                    .conflicting_holder(rid, p.txn, RowLock::KeyShare, |t| s.txn_active(t))
                {
                    foot.push(Access::Wait);
                    return FkStep::Blocked(h);
                }
                foot.push(Access::RowRead(rid));
                let newest = newest_image(s, rid, p.txn);
                if iso == Isolation::ReadCommitted {
                    // EvalPlanQual: the newest version must still carry the
                    // key.
                    if !newest.as_ref().is_some_and(|r| key_tuple(r, cols) == key) {
                        return FkStep::Failed(violation());
                    }
                } else {
                    // The snapshot's version must be the newest, or differ
                    // from it by an update that changed no key column (of
                    // ANY unique key, not only the referenced one: such an
                    // update sets PostgreSQL's "keys updated" bit).
                    let seen = s.store.visible_values(rid, txn_snap);
                    let disturbed = match (seen, newest.as_ref()) {
                        (Some(old), Some(new)) => {
                            old != new && key_columns_changed(pschema, old, new)
                        }
                        (Some(_), None) => true,
                        _ => false,
                    };
                    if disturbed {
                        return FkStep::Failed(DbError::serialization());
                    }
                }
                s.store.grant(rid, p.txn, RowLock::KeyShare);
                foot.push(Access::LockChange(rid));
                p.fk.remove(0);
            }
            FkCheck::Children {
                table,
                fk,
                key,
                todo,
            } => {
                let child = model.schema.table(table);
                let def = &child.foreign_keys[fk as usize];
                let cols = &def.columns;
                let mut todo = match todo {
                    Some(t) => t,
                    None => {
                        let test = Snapshot {
                            seq: s.commit_seq,
                            own: p.txn,
                        };
                        let pred = key_pred(cols, &key);
                        foot.push(Access::Pred {
                            table,
                            pred: pred.clone(),
                        });
                        if iso == Isolation::Serializable {
                            ser_read(s, p.txn, table, &pred, txn_snap, foot);
                        }
                        match scan(model, s, table, &pred, test) {
                            Ok(rows) => rows.into_iter().map(|r| r.0).collect(),
                            Err(e) => return FkStep::Failed(e),
                        }
                    }
                };
                let parent_name = &def.references;
                let mut outcome = None;
                while let Some(&cid) = todo.first() {
                    foot.push(Access::LockCheck(cid));
                    if let Some(h) =
                        s.store
                            .conflicting_holder(cid, p.txn, RowLock::KeyShare, |t| s.txn_active(t))
                    {
                        foot.push(Access::Wait);
                        outcome = Some(FkStep::Blocked(h));
                        break;
                    }
                    foot.push(Access::RowRead(cid));
                    let still =
                        newest_image(s, cid, p.txn).is_some_and(|r| key_tuple(&r, cols) == key);
                    if !still {
                        todo.remove(0);
                        continue;
                    }
                    // A referencing row the transaction's snapshot does not
                    // see is a violation too: the cross-check snapshot that
                    // `ri_PerformCheck` passes applies to row updates and
                    // deletions (cascading actions), not to this probe's
                    // `FOR KEY SHARE` (measured: `LOCK-DIFFERENTIAL.md`).
                    outcome = Some(FkStep::Failed(DbError::new(
                        "23503",
                        format!(
                            "update or delete on table \"{parent_name}\" violates foreign key constraint \"{}\" on table \"{}\"",
                            def.name(&child.name),
                            child.name
                        ),
                    )));
                    break;
                }
                match outcome {
                    Some(o) => {
                        if let Some(FkCheck::Children { todo: t, .. }) = p.fk.first_mut() {
                            *t = Some(todo);
                        }
                        return o;
                    }
                    None => {
                        p.fk.remove(0);
                    }
                }
            }
        }
    }
    FkStep::Ok
}

/// A read on the read replica: the versions committed at or
/// before the replica's replayed prefix, without locks (a replica is read
/// only; the loader refuses locking reads there).
pub fn replica_read(
    model: &Model,
    s: &State,
    stmt: &ClosedStmt,
    _foot: &mut Footprint,
) -> Result<StmtResult, DbError> {
    let snap = Snapshot {
        seq: s.replica_seq,
        own: INITIAL_TXN,
    };
    match stmt {
        ClosedStmt::Select {
            table,
            pred,
            joins,
            projection,
            ..
        } if !joins.is_empty() => {
            let rows = join_rows(model, s, *table, joins, pred, snap)?;
            Ok(StmtResult::Row(rows.first().map(|r| {
                project(model, *table, joins, projection.as_deref(), r)
            })))
        }
        ClosedStmt::Select {
            table,
            pred,
            projection,
            ..
        } => {
            let rows = scan(model, s, *table, pred, snap)?;
            Ok(StmtResult::Row(rows.into_iter().next().map(|r| {
                project(model, *table, &[], projection.as_deref(), &r.1)
            })))
        }
        ClosedStmt::Count { table, pred } => Ok(StmtResult::Count(
            scan(model, s, *table, pred, snap)?.len() as i64,
        )),
        // A multi-row read (the loader refuses locks on a
        // replica).
        ClosedStmt::SelectMany {
            table,
            pred,
            projection,
            limit,
            ..
        } => {
            let rows = scan(model, s, *table, pred, snap)?;
            Ok(StmtResult::Rows(
                rows.iter()
                    .take(limit.map_or(usize::MAX, |n| n as usize))
                    .map(|r| project(model, *table, &[], projection.as_deref(), &r.1))
                    .collect(),
            ))
        }
        _ => Err(DbError::new(
            "25006",
            "cannot execute a write in a read-only transaction (replica)",
        )),
    }
}

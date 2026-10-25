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

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value as Json;

use commitrift_replay::source::{
    self, ActorLaunch, Expected, LoadedSource, Observed, PlanStep, Region, RunRequest, SourceRun,
    SqlKind,
};

use crate::db::{ColumnDefault, INITIAL_TXN, RowId, TableSchema, Version};
use crate::engine::{Action, Expect, Recorder, apply, initial_state};
use crate::exec::StmtResult;
use crate::invariant::Invariant;
use crate::ir::{Conn, Origin, ProjItem, SourceRef};
use crate::model::Model;
use crate::provider::EntryInfo;
pub use crate::source_types::{
    RunSummary, SourceActor, SourceSpec, SourceVerification, StatementComparison,
};
use crate::state::{ClosedStmt, State, TKind, Transition};
use crate::value::{Row, Value};

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
        }
    }
}

const NOTE: &str = "Each actor's ORIGINAL application code ran in its own worker process (an authorized harness) against a private, disposable PostgreSQL database; every SQL statement the application sent was held until the coordinator released it in the order of the model's counterexample, and PostgreSQL's answers (rows, row counts, SQLSTATEs, lock waits reported by pg_stat_activity) were recorded. The invariant was evaluated on the rows the run left in the database. This is evidence about this application, this input and this schedule on this PostgreSQL; it is not a claim about production traffic.";

// ---------------------------------------------------------------------------
// Plan
// ---------------------------------------------------------------------------

/// What the model predicts for one released statement.
#[derive(Clone, Debug)]
struct Prediction {
    transition: usize,
    actor: usize,
    stmt: Option<ClosedStmt>,
    expect: Expect,
    src: Option<SourceRef>,
    label: String,
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
        ClosedStmt::Insert { .. } => SqlKind::Insert,
        ClosedStmt::Update { .. } => SqlKind::Update,
        ClosedStmt::Delete { .. } => SqlKind::Delete,
    }
}

fn table_of(stmt: &ClosedStmt) -> u16 {
    match stmt {
        ClosedStmt::Select { table, .. }
        | ClosedStmt::Count { table, .. }
        | ClosedStmt::Insert { table, .. }
        | ClosedStmt::Update { table, .. }
        | ClosedStmt::Delete { table, .. } => *table,
    }
}

/// The plan for `schedule`, and what the model predicts for each released
/// statement.
fn plan(
    model: &Model,
    schedule: &[Transition],
) -> Result<(Vec<PlanStep>, Vec<Prediction>), String> {
    let mut state = initial_state(model);
    let mut steps = Vec::new();
    let mut predictions = Vec::new();
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
            TKind::Crash => return Err("the counterexample crashes a process: crashes are not executed against original code (yet)".to_owned()),
            TKind::Retry => return Err("the counterexample retries an operation: retries are not executed against original code (yet)".to_owned()),
            TKind::Deadlock => return Err("the counterexample picks a deadlock victim: victim steering is not done against original code".to_owned()),
            TKind::Step | TKind::Resume => {}
        }
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
                            label: String| {
                let a = actor as usize;
                steps.push(PlanStep::Release(Expected {
                    actor: a,
                    kind,
                    table,
                    region: src.as_ref().and_then(region),
                    expect_block: matches!(expect, Expect::Blocked(_)),
                    transition: i + 1,
                    label: label.clone(),
                }));
                predictions.push(Prediction {
                    transition: i + 1,
                    actor: a,
                    stmt,
                    expect,
                    src: src.clone(),
                    label,
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
                    push(kind_of(&stmt), table, actor, Some(stmt), expect, label);
                }
                Action::Stmt {
                    actor,
                    resumed: true,
                    ..
                } => steps.push(PlanStep::Complete {
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
                        );
                    }
                }
                Action::Victim { .. } | Action::Crash { .. } => {
                    return Err(
                        "deadlock victims and crashes are not executed against original code"
                            .to_owned(),
                    );
                }
            }
        }
    }
    Ok((steps, predictions))
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
        Value::Row(_) => return Err("a row value in the initial state".to_owned()),
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
        Json::Array(_) => Value::Str(v.to_string()),
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
        ClosedStmt::Insert { table, .. } => table_cells(*table, &Value::Row(row.clone()), &mut out),
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

fn compare_statement(model: &Model, p: &Prediction, o: Option<&Observed>) -> StatementComparison {
    let predicted = match &p.expect {
        Expect::Done(StmtResult::Row(None)) => "no row".to_owned(),
        Expect::Done(StmtResult::Row(Some(r))) => Value::Row(r.clone()).render(),
        Expect::Done(StmtResult::Count(n)) => format!("count {n}"),
        Expect::Done(StmtResult::Affected(n)) => format!("{n} row(s)"),
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
    let Some(o) = o else { return c };
    c.sql = o.sql.split_whitespace().collect::<Vec<_>>().join(" ");
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
                                Some(v) if json_to_value(v) == value => {}
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

/// A state whose committed rows are `tables` (for evaluating an invariant
/// on the observed database).
fn state_with(model: &Model, tables: &BTreeMap<String, Vec<Row>>) -> State {
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
                vec![Version {
                    values: Some(row.clone()),
                    creator: INITIAL_TXN,
                    commit: Some(0),
                }],
            );
        }
    }
    s
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
}

/// Verify a counterexample against the original code.
pub fn verify(req: &VerifyRequest<'_>) -> SourceVerification {
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
        launches.push(ActorLaunch {
            name: actor.name.clone(),
            entry: spec.entry.clone(),
            input: spec.input.clone(),
            scope,
        });
    }
    let (steps, predictions) = match plan(model, req.schedule) {
        Ok(p) => p,
        Err(e) => return SourceVerification::refused("unsupported", &harness_name, e),
    };
    let seed = match seed_sql(model) {
        Ok(s) => s,
        Err(e) => return SourceVerification::refused("unsupported", &harness_name, e),
    };
    let queries = final_queries(model);
    let mut model_tables = BTreeMap::new();
    for (i, t) in model.schema.tables.iter().enumerate() {
        model_tables.insert(
            t.name.clone(),
            req.final_state.committed_rows(model, i as u16),
        );
    }
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
            keep_artifacts: false,
            actors: &launches,
            plan: &steps,
            seed_sql: &seed,
            final_queries: &queries,
            settle: Duration::from_millis(150),
        });
        if run.status == "blocked" {
            let mut refused =
                SourceVerification::refused("blocked", &harness_name, run.problems.join("; "));
            refused.performed = false;
            return refused;
        }
        // Statement comparison.
        let mut statements = Vec::new();
        for (k, p) in predictions.iter().enumerate() {
            // Plan step index of the k-th release.
            let step_index = steps
                .iter()
                .enumerate()
                .filter(|(_, s)| matches!(s, PlanStep::Release(_)))
                .nth(k)
                .map(|(i, _)| i);
            let observed = run
                .statements
                .iter()
                .find(|o| o.plan.is_some() && o.plan == step_index);
            statements.push(compare_statement(model, p, observed));
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
            match o.role.as_str() {
                "outside-model" => outside.push(format!("{}: {flat}{at}", launches[o.actor].name)),
                "beyond-plan" | "unexpected" | "after-divergence" => {
                    unexpected.push(format!(
                        "{} ({}): {flat}{at}",
                        launches[o.actor].name, o.role
                    ));
                }
                _ => {}
            }
        }
        // Final rows.
        let runtime_tables: BTreeMap<String, Vec<Row>> = run
            .tables
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
            .collect();
        let tables_agree = runtime_tables == model_tables;
        // Outcomes.
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
            (m == "ok" && r == "returned") || (m.starts_with("failed") && r.starts_with("threw"))
        });
        // The invariant on the observed rows.
        let invariant = if !req.invariant.formula.reads_only_database() {
            "not-evaluated".to_owned()
        } else if req
            .invariant
            .holds(model, &state_with(model, &runtime_tables))
        {
            "holds".to_owned()
        } else {
            "violated".to_owned()
        };
        let reproduced = run.status == "followed"
            && statements_agree
            && tables_agree
            && outcomes_agree
            && unexpected.is_empty()
            && invariant == "violated";
        out.runs.push(RunSummary {
            run: run_index,
            coordination: run.status.clone(),
            statements_agree,
            tables_agree,
            outcomes_agree,
            invariant,
            reproduced,
            elapsed_ms: run.elapsed_ms,
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
        }
    }
    out.status = if out.runs.iter().all(|r| r.reproduced) {
        "reproduced"
    } else if out.runs.iter().any(|r| r.coordination == "diverged") {
        "diverged"
    } else if out.runs.iter().any(|r| r.coordination == "inconclusive") {
        "inconclusive"
    } else if out.runs.iter().any(|r| r.reproduced) {
        "unstable"
    } else {
        "not-reproduced"
    }
    .to_owned();
    out
}

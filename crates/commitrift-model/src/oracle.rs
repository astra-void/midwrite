//! Differential validation against a real PostgreSQL.
//!
//! The oracle replays a model *schedule* in lockstep against a disposable,
//! private PostgreSQL cluster: for every transition the model executed, the
//! physical actions it implies (`BEGIN`, a statement, `COMMIT`, a crash...)
//! are sent to real server sessions (one `psql` process per session, so each
//! actor owns a separate connection), and what PostgreSQL does (result,
//! SQLSTATE, whether the statement waits for a lock and for whom) is compared
//! with what the model predicted. Nothing is forced into the model's
//! categories: the first disagreement is recorded as it was observed.
//!
//! Coordination is explicit, not timing-based: the schedule decides which
//! session sends its next statement; a statement is classified as *blocked*
//! only when the server's own wait state says so (`pg_stat_activity`,
//! `pg_blocking_pids`). The only timing dependence is PostgreSQL's deadlock
//! detector, which the oracle steers by delaying a statement relative to the
//! deadlock timeout (documented in `docs/phase6/POSTGRES-VALIDATION.md`).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use commitrift_replay::postgres::{Binaries, Cluster, USER};
use commitrift_replay::safety::ReplayRoot;
use serde::{Deserialize, Serialize};

use crate::db::ColumnDefault;
use crate::engine::{Action, Expect, Recorder, apply, initial_state};
use crate::exec::StmtResult;
use crate::ir::{Conn, JoinKind, OnConflict, Op, Origin, ProjItem, StmtKind};
use crate::model::Model;
pub use crate::oracle_types::Observation;
use crate::state::{ClosedJoin, ClosedStmt, TKind, Transition};
use crate::value::{BinOp, Expr, Row, Value};

/// PostgreSQL's `deadlock_timeout` used by the oracle's sessions.
pub const DEADLOCK_TIMEOUT_MS: u64 = 500;
const PORT: &str = "5432";

// ---------------------------------------------------------------------
// SQL generation
// ---------------------------------------------------------------------

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `table.column` (a joined query's qualified column) or a plain column.
/// Phase 13: columns of the existing row qualified with the table name (in
/// `ON CONFLICT ... DO UPDATE`, an unqualified column is ambiguous with
/// `excluded`).
fn qualify(e: &Expr, table: &str) -> Expr {
    match e {
        Expr::Col(c) if !c.contains('.') => Expr::Col(format!("{table}.{c}")),
        Expr::Not(x) => Expr::Not(Box::new(qualify(x, table))),
        Expr::IsNull(x) => Expr::IsNull(Box::new(qualify(x, table))),
        Expr::Bin(op, a, b) => Expr::Bin(
            *op,
            Box::new(qualify(a, table)),
            Box::new(qualify(b, table)),
        ),
        // Phase 14: SQL built-ins (`inArray`).
        Expr::Call { func, args } => Expr::Call {
            func: *func,
            args: args.iter().map(|a| qualify(a, table)).collect(),
        },
        other => other.clone(),
    }
}

fn qualified_ident(name: &str) -> String {
    match name.split_once('.') {
        Some((t, c)) => format!("{}.{}", quote_ident(t), quote_ident(c)),
        None => quote_ident(name),
    }
}

/// `to_json(table)`, or `NULL` for the unmatched side of a `LEFT JOIN`.
fn nested_json(model: &Model, t: u16) -> String {
    let schema = model.schema.table(t);
    let key: Vec<&String> = if schema.primary_key.is_empty() {
        schema.columns.iter().map(|c| &c.name).collect()
    } else {
        schema.primary_key.iter().collect()
    };
    let null = key
        .iter()
        .map(|k| format!("{}.{} IS NULL", quote_ident(&schema.name), quote_ident(k)))
        .collect::<Vec<_>>()
        .join(" AND ");
    format!(
        "CASE WHEN {null} THEN NULL ELSE to_json({}) END",
        quote_ident(&schema.name)
    )
}

/// The JSON object a (joined or projected) select returns per row.
fn select_json(
    model: &Model,
    table: u16,
    joins: &[ClosedJoin],
    projection: Option<&[(String, ProjItem)]>,
) -> String {
    let name = |t: u16| model.schema.table(t).name.clone();
    let fields: Vec<String> = match projection {
        None => std::iter::once(table)
            .chain(joins.iter().map(|j| j.table))
            .map(|t| format!("{}, {}", quote_literal(&name(t)), nested_json(model, t)))
            .collect(),
        Some(items) => items
            .iter()
            .map(|(field, item)| {
                let value = match item {
                    ProjItem::Column(c) => qualified_ident(c),
                    ProjItem::Table(t) => match model.schema.index(t) {
                        Some(ti) if !joins.is_empty() => nested_json(model, ti),
                        _ => format!("to_json({})", quote_ident(t)),
                    },
                };
                format!("{}, {value}", quote_literal(field))
            })
            .collect(),
    };
    format!("json_build_object({})", fields.join(", "))
}

fn quote_literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Phase 12: the text a generated value stands for in the oracle's database.
/// The model decides every generated value when the INSERT starts, so the
/// oracle sends it explicitly (a UUID that encodes the generation event)
/// and reads it back as the same event: the oracle checks constraints,
/// visibility and blocking with generated keys, not generation itself (the
/// Drizzle differential tests do that).
fn gen_text(g: crate::value::GenId) -> String {
    format!(
        "00000000-0000-4000-8{:03x}-{:04x}{:08x}",
        g.generator & 0xfff,
        g.actor,
        g.n
    )
}

fn gen_of_text(s: &str) -> Option<crate::value::GenId> {
    let rest = s.strip_prefix("00000000-0000-4000-8")?;
    let (generator, tail) = rest.split_once('-')?;
    if generator.len() != 3 || tail.len() != 12 {
        return None;
    }
    Some(crate::value::GenId {
        generator: u16::from_str_radix(generator, 16).ok()?,
        actor: u16::from_str_radix(&tail[..4], 16).ok()?,
        n: u32::from_str_radix(&tail[4..], 16).ok()?,
    })
}

// ---------------------------------------------------------------------
// Phase 14: clock readings and explicit time values
// ---------------------------------------------------------------------

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day count since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Seconds of the clock encodings: `now()` of a transaction in 2001-2013,
/// application readings in 2051-2063, one second per (actor, event), with
/// the microsecond digit `1` that no millisecond-precision explicit value
/// has.
const CLOCK_DB_BASE: i64 = 978_307_200; // 2001-01-01T00:00:00Z
const CLOCK_APP_BASE: i64 = 2_556_144_000; // 2051-01-01T00:00:00Z
const CLOCK_SPAN: i64 = 100_000;

/// The timestamp a clock reading stands for in the oracle's database: the
/// model decides every reading, the oracle sends it explicitly and reads it
/// back as the same reading (as for generated values, `gen_text`).
fn clock_text(g: crate::value::GenId) -> String {
    let base = if g.generator == crate::db::CLOCK_DB {
        CLOCK_DB_BASE
    } else {
        CLOCK_APP_BASE
    };
    let secs = base + i64::from(g.actor) * CLOCK_SPAN + i64::from(g.n);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}.000001+00",
        t / 3600,
        t % 3600 / 60,
        t % 60
    )
}

/// `YYYY-MM-DD[T ]HH:MM:SS[.ffffff][+00[:00]|Z]` → (seconds since the
/// epoch, fraction digits), for UTC texts only.
fn parse_utc(text: &str) -> Option<(i64, String)> {
    let b = text.as_bytes();
    if b.len() < 19 || !(b[10] == b'T' || b[10] == b' ') {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> { text.get(r)?.parse::<i64>().ok() };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut rest = &text[19..];
    let mut frac = String::new();
    if let Some(r) = rest.strip_prefix('.') {
        let n = r.bytes().take_while(u8::is_ascii_digit).count();
        frac = r[..n].to_owned();
        rest = &r[n..];
    }
    if !matches!(rest, "" | "Z" | "+00" | "+00:00") {
        return None;
    }
    Some((
        days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se,
        frac,
    ))
}

/// A clock reading read back from PostgreSQL.
fn clock_of_text(text: &str) -> Option<crate::value::GenId> {
    let (secs, frac) = parse_utc(text)?;
    if frac != "000001" {
        return None;
    }
    let (generator, base) = if (CLOCK_DB_BASE..CLOCK_APP_BASE).contains(&secs) {
        (crate::db::CLOCK_DB, CLOCK_DB_BASE)
    } else if secs >= CLOCK_APP_BASE {
        (crate::db::CLOCK_APP, CLOCK_APP_BASE)
    } else {
        return None;
    };
    let off = secs - base;
    Some(crate::value::GenId {
        generator,
        actor: u16::try_from(off / CLOCK_SPAN).ok()?,
        n: u32::try_from(off % CLOCK_SPAN).ok()?,
    })
}

/// An explicit time value read back from PostgreSQL, in the model's
/// canonical form (`YYYY-MM-DDTHH:MM:SS.sssZ`); `None` when the text is not
/// a UTC timestamp with millisecond precision.
fn canonical_time(text: &str) -> Option<String> {
    let (secs, frac) = parse_utc(text)?;
    if frac.len() > 3 && frac[3..].bytes().any(|c| c != b'0') {
        return None;
    }
    let ms: String = format!("{frac:0<3}").chars().take(3).collect();
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ms}Z",
        t / 3600,
        t % 3600 / 60,
        t % 60
    ))
}

/// Phase 14: a model row read back from PostgreSQL, with the values of time
/// columns in the model's form (other columns are left as read).
fn fix_row(model: &Model, table: u16, row: &mut Row) {
    for c in &model.schema.table(table).columns {
        if !crate::db::is_time_type(c.sql_type.as_deref()) || c.sql_type.as_deref() == Some("date")
        {
            continue;
        }
        if let Some(Value::Str(text)) = row.get(&c.name)
            && let Some(canonical) = canonical_time(text)
        {
            row.insert(c.name.clone(), Value::Str(canonical));
        }
    }
}

/// The JSON text of a model value stored in a `json`/`jsonb` column.
fn json_text(v: &Value) -> Result<serde_json::Value, String> {
    Ok(match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(n) => serde_json::Value::from(*n),
        Value::Str(s) => serde_json::Value::String(s.clone()),
        Value::Row(r) => serde_json::Value::Object(
            r.iter()
                .map(|(k, v)| Ok((k.clone(), json_text(v)?)))
                .collect::<Result<_, String>>()?,
        ),
        Value::List(items) => {
            serde_json::Value::Array(items.iter().map(json_text).collect::<Result<_, _>>()?)
        }
        other => return Err(format!("{} is not a JSON value", other.render())),
    })
}

/// Phase 14: a value written to column `col` of `table`: JSON columns take
/// a JSON literal; other columns `value_sql`.
fn column_value_sql(model: &Model, table: u16, col: &str, v: &Value) -> Result<String, String> {
    let ty = model
        .schema
        .table(table)
        .columns
        .iter()
        .find(|c| c.name == col)
        .and_then(|c| c.sql_type.clone());
    match (ty.as_deref(), v) {
        (Some(t @ ("json" | "jsonb")), v) if !v.is_null() => Ok(format!(
            "{}::{t}",
            quote_literal(&json_text(v)?.to_string())
        )),
        _ => value_sql(v),
    }
}

fn column_expr_sql(model: &Model, table: u16, col: &str, e: &Expr) -> Result<String, String> {
    match e {
        Expr::Lit(v) => column_value_sql(model, table, col, v),
        other => expr_sql(other),
    }
}

fn value_sql(v: &Value) -> Result<String, String> {
    Ok(match v {
        Value::Null => "NULL".into(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        Value::Int(n) => n.to_string(),
        Value::Str(s) => quote_literal(s),
        // Phase 14: a clock reading is a timestamp.
        Value::Gen(g) if crate::db::is_clock(g) => quote_literal(&clock_text(*g)),
        Value::Gen(g) => quote_literal(&gen_text(*g)),
        Value::Row(_) => return Err("a row value cannot be used in SQL".into()),
        Value::List(_) | Value::Map(_) | Value::Set(_) | Value::Undefined(_) => {
            return Err(format!("the value {} cannot be used in SQL", v.render()));
        }
    })
}

pub fn expr_sql(e: &Expr) -> Result<String, String> {
    Ok(match e {
        Expr::Lit(v) => value_sql(v)?,
        Expr::Col(c) => qualified_ident(c),
        Expr::Not(x) => format!("(NOT {})", expr_sql(x)?),
        Expr::IsNull(x) => format!("({} IS NULL)", expr_sql(x)?),
        Expr::Bin(op, a, b) => {
            let sym = match op {
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                BinOp::Eq => "=",
                BinOp::Ne => "<>",
                BinOp::Lt => "<",
                BinOp::Le => "<=",
                BinOp::Gt => ">",
                BinOp::Ge => ">=",
                BinOp::And => "AND",
                BinOp::Or => "OR",
            };
            format!("({} {sym} {})", expr_sql(a)?, expr_sql(b)?)
        }
        Expr::Local(n) => return Err(format!("unresolved local `{n}`")),
        Expr::Field(..) => return Err("property access in SQL".into()),
        Expr::Object(..) => return Err("an object value in SQL".into()),
        Expr::Call { func, args } => crate::sqlfn::render(*func, args)?,
        Expr::Array(..) | Expr::Each { .. } => {
            return Err("an array value in SQL".into());
        }
        Expr::Unknown(d) => return Err(format!("unsupported expression: {d}")),
    })
}

fn qualified_order(model: &Model, table: u16) -> String {
    let t = model.schema.table(table);
    let cols: Vec<&String> = if t.primary_key.is_empty() {
        t.columns.iter().map(|c| &c.name).collect()
    } else {
        t.primary_key.iter().collect()
    };
    cols.iter()
        .map(|c| format!("{}.{}", quote_ident(&t.name), quote_ident(c)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn pk_order(model: &Model, table: u16) -> String {
    let t = model.schema.table(table);
    let cols: Vec<String> = if t.primary_key.is_empty() {
        t.columns.iter().map(|c| quote_ident(&c.name)).collect()
    } else {
        t.primary_key.iter().map(|c| quote_ident(c)).collect()
    };
    cols.join(", ")
}

/// The SQL text a statement is sent as. Reads wrap their result so that the
/// oracle can parse it uniformly (a JSON row, or a count).
pub fn stmt_sql(model: &Model, stmt: &ClosedStmt) -> Result<String, String> {
    let name = |t: u16| quote_ident(&model.schema.table(t).name);
    Ok(match stmt {
        ClosedStmt::Select {
            table,
            pred,
            joins,
            projection,
            ..
        } if !joins.is_empty() => {
            let mut from = name(*table);
            let mut order = vec![qualified_order(model, *table)];
            for j in joins {
                from.push_str(&format!(
                    " {} {} ON {} = {}",
                    match j.kind {
                        JoinKind::Inner => "JOIN",
                        JoinKind::Left => "LEFT JOIN",
                    },
                    name(j.table),
                    qualified_ident(&j.left),
                    qualified_ident(&j.right)
                ));
                order.push(qualified_order(model, j.table));
            }
            format!(
                "SELECT {} FROM {from} WHERE {} ORDER BY {} LIMIT 1;",
                select_json(model, *table, joins, projection.as_deref()),
                expr_sql(pred)?,
                order.join(", ")
            )
        }
        ClosedStmt::Select {
            table,
            pred,
            lock,
            projection: Some(projection),
            ..
        } => format!(
            "SELECT {} FROM {} WHERE {} ORDER BY {} LIMIT 1{};",
            select_json(model, *table, &[], Some(projection)),
            name(*table),
            expr_sql(pred)?,
            pk_order(model, *table),
            lock.sql()
        ),
        ClosedStmt::Select {
            table, pred, lock, ..
        } => format!(
            "SELECT row_to_json(t) FROM (SELECT * FROM {} WHERE {} ORDER BY {} LIMIT 1{}) t;",
            name(*table),
            expr_sql(pred)?,
            pk_order(model, *table),
            lock.sql()
        ),
        ClosedStmt::Count { table, pred } => {
            format!(
                "SELECT count(*) FROM {} WHERE {};",
                name(*table),
                expr_sql(pred)?
            )
        }
        ClosedStmt::Update { table, pred, set } => {
            let sets: Result<Vec<String>, String> = set
                .iter()
                .map(|(c, e)| {
                    Ok(format!(
                        "{} = {}",
                        quote_ident(c),
                        column_expr_sql(model, *table, c, e)?
                    ))
                })
                .collect();
            format!(
                "WITH r AS (UPDATE {} SET {} WHERE {} RETURNING 1) SELECT count(*) FROM r;",
                name(*table),
                sets?.join(", "),
                expr_sql(pred)?
            )
        }
        ClosedStmt::Insert {
            table,
            values,
            on_conflict,
            returning,
            returning_columns,
            upsert,
        } => {
            let cols: Vec<String> = values.keys().map(|c| quote_ident(c)).collect();
            let vals: Result<Vec<String>, String> = values
                .iter()
                .map(|(c, v)| column_value_sql(model, *table, c, v))
                .collect();
            let insert = if values.is_empty() {
                format!("INSERT INTO {} DEFAULT VALUES", name(*table))
            } else {
                format!(
                    "INSERT INTO {} ({}) VALUES ({})",
                    name(*table),
                    cols.join(", "),
                    vals?.join(", ")
                )
            };
            let conflict = conflict_sql(model, *table, *on_conflict, upsert.as_ref())?;
            if *returning {
                let list = returning_list(returning_columns.as_deref());
                format!(
                    "WITH r AS ({insert}{conflict} RETURNING {list}) SELECT row_to_json(r) FROM r;"
                )
            } else {
                format!("WITH r AS ({insert}{conflict} RETURNING 1) SELECT count(*) FROM r;")
            }
        }
        // Phase 14: one statement with every row; a column a row does not
        // (yet) carry is `DEFAULT` (the lockstep run sends the rows as the
        // model completed them: a row the statement never formed is never
        // formed by PostgreSQL either).
        ClosedStmt::InsertMany {
            table,
            rows,
            on_conflict,
            returning,
            returning_columns,
            upsert,
            ..
        } => {
            let mut cols: Vec<String> = Vec::new();
            for r in rows {
                for c in r.keys() {
                    if !cols.contains(c) {
                        cols.push(c.clone());
                    }
                }
            }
            cols.sort();
            let tuples = rows
                .iter()
                .map(|r| {
                    let vals = cols
                        .iter()
                        .map(|c| match r.get(c) {
                            Some(v) => column_value_sql(model, *table, c, v),
                            None => Ok("DEFAULT".to_owned()),
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    Ok(format!("({})", vals.join(", ")))
                })
                .collect::<Result<Vec<_>, String>>()?;
            let insert = if cols.is_empty() {
                // `INSERT ... VALUES (DEFAULT), (DEFAULT)` needs a column.
                let first = &model.schema.table(*table).columns[0].name;
                format!(
                    "INSERT INTO {} ({}) VALUES {}",
                    name(*table),
                    quote_ident(first),
                    vec!["(DEFAULT)"; rows.len()].join(", ")
                )
            } else {
                format!(
                    "INSERT INTO {} ({}) VALUES {}",
                    name(*table),
                    cols.iter()
                        .map(|c| quote_ident(c))
                        .collect::<Vec<_>>()
                        .join(", "),
                    tuples.join(", ")
                )
            };
            let conflict = conflict_sql(model, *table, *on_conflict, upsert.as_ref())?;
            if *returning {
                let list = returning_list(returning_columns.as_deref());
                format!(
                    "WITH r AS ({insert}{conflict} RETURNING {list}) SELECT row_to_json(r) FROM r;"
                )
            } else {
                format!("WITH r AS ({insert}{conflict} RETURNING 1) SELECT count(*) FROM r;")
            }
        }
        ClosedStmt::Delete { table, pred } => format!(
            "WITH r AS (DELETE FROM {} WHERE {} RETURNING 1) SELECT count(*) FROM r;",
            name(*table),
            expr_sql(pred)?
        ),
    })
}

/// The `ON CONFLICT` clause of an insert.
fn conflict_sql(
    model: &Model,
    table: u16,
    on_conflict: OnConflict,
    upsert: Option<&crate::ir::Upsert>,
) -> Result<String, String> {
    let tname = &model.schema.table(table).name;
    Ok(match upsert {
        // Phase 13.
        Some(u) => format!(
            " ON CONFLICT ({}) DO UPDATE SET {}{}",
            u.target
                .iter()
                .map(|c| quote_ident(c))
                .collect::<Vec<_>>()
                .join(", "),
            u.set
                .iter()
                .map(|(c, e)| {
                    Ok(format!(
                        "{} = {}",
                        quote_ident(c),
                        column_expr_sql(model, table, c, &qualify(e, tname))?
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?
                .join(", "),
            match &u.filter {
                Some(f) => format!(" WHERE {}", expr_sql(&qualify(f, tname))?),
                None => String::new(),
            }
        ),
        None if on_conflict == OnConflict::DoNothing => " ON CONFLICT DO NOTHING".to_owned(),
        None => String::new(),
    })
}

fn returning_list(cols: Option<&[String]>) -> String {
    cols.map_or_else(
        || "*".to_owned(),
        |cols| {
            cols.iter()
                .map(|c| quote_ident(c))
                .collect::<Vec<_>>()
                .join(", ")
        },
    )
}

fn value_type(v: &Value) -> Option<&'static str> {
    match v {
        Value::Int(_) => Some("bigint"),
        Value::Str(_) | Value::Gen(_) => Some("text"),
        Value::Bool(_) => Some("boolean"),
        _ => None,
    }
}

fn expr_type(e: &Expr, args: &BTreeMap<String, Value>) -> Option<&'static str> {
    match e {
        Expr::Lit(v) => value_type(v),
        Expr::Local(n) => args.get(n).and_then(value_type),
        Expr::Bin(BinOp::Add | BinOp::Sub | BinOp::Mul, ..) => Some("bigint"),
        _ => None,
    }
}

fn predicate_hints(
    e: &Expr,
    args: &BTreeMap<String, Value>,
    hint: &mut dyn FnMut(&str, Option<&'static str>),
) {
    match e {
        Expr::Bin(op, a, b) => {
            if matches!(
                op,
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
            ) {
                if let Expr::Col(c) = &**a {
                    hint(c, expr_type(b, args));
                }
                if let Expr::Col(c) = &**b {
                    hint(c, expr_type(a, args));
                }
            }
            predicate_hints(a, args, hint);
            predicate_hints(b, args, hint);
        }
        Expr::Not(x) | Expr::IsNull(x) => predicate_hints(x, args, hint),
        _ => {}
    }
}

/// Column types for the oracle's DDL: declared, else inferred from initial
/// rows, literals and actor arguments, else `text`.
fn column_types(model: &Model) -> Vec<Vec<&'static str>> {
    let mut out = Vec::new();
    for (ti, t) in model.schema.tables.iter().enumerate() {
        let mut types: BTreeMap<String, &'static str> = BTreeMap::new();
        for c in &t.columns {
            match c.sql_type.as_deref() {
                Some("int") => {
                    types.insert(c.name.clone(), "bigint");
                }
                Some("text") => {
                    types.insert(c.name.clone(), "text");
                }
                Some("bool") => {
                    types.insert(c.name.clone(), "boolean");
                }
                // Phase 14.
                Some("timestamp") => {
                    types.insert(c.name.clone(), "timestamp");
                }
                Some("timestamptz") => {
                    types.insert(c.name.clone(), "timestamptz");
                }
                Some("date") => {
                    types.insert(c.name.clone(), "date");
                }
                Some("json") => {
                    types.insert(c.name.clone(), "json");
                }
                Some("jsonb") => {
                    types.insert(c.name.clone(), "jsonb");
                }
                _ => {}
            }
        }
        let mut hint = |col: &str, ty: Option<&'static str>| {
            if let Some(ty) = ty {
                types.entry(col.to_owned()).or_insert(ty);
            }
        };
        for row in &model.initial_rows[ti] {
            for (c, v) in row {
                hint(c, value_type(v));
            }
        }
        for actor in &model.actors {
            let args: BTreeMap<String, Value> = actor.arguments.iter().cloned().collect();
            for step in actor.codes.iter().flatten() {
                let Op::Db { stmt } = &step.op else { continue };
                if model.schema.index(stmt.kind.table()) != Some(ti as u16) {
                    continue;
                }
                match &stmt.kind {
                    StmtKind::Insert {
                        values, more_rows, ..
                    } => {
                        for (c, e) in values.iter().chain(more_rows.iter().flatten()) {
                            hint(c, expr_type(e, &args));
                        }
                    }
                    StmtKind::Update { pred, set, .. } => {
                        for (c, e) in set {
                            hint(c, expr_type(e, &args));
                        }
                        predicate_hints(pred, &args, &mut hint);
                    }
                    StmtKind::Select { pred, .. }
                    | StmtKind::SelectCount { pred, .. }
                    | StmtKind::Delete { pred, .. } => predicate_hints(pred, &args, &mut hint),
                }
            }
        }
        out.push(
            t.columns
                .iter()
                .map(|c| types.get(&c.name).copied().unwrap_or("text"))
                .collect(),
        );
    }
    out
}

fn ddl(model: &Model) -> Result<String, String> {
    let types = column_types(model);
    let sequences = model.sequences();
    let mut out = String::new();
    for (ti, t) in model.schema.tables.iter().enumerate() {
        let mut parts: Vec<String> = Vec::new();
        for (ci, c) in t.columns.iter().enumerate() {
            let not_null = c.not_null || t.primary_key.contains(&c.name);
            let default = match &c.default {
                None => String::new(),
                Some(ColumnDefault::Value(v)) => format!(" DEFAULT {}", value_sql(v)?),
                Some(ColumnDefault::Sequence(_)) => {
                    let start = sequences
                        .iter()
                        .find(|(st, sc, _)| *st as usize == ti && *sc == c.name)
                        .map_or(1, |(_, _, n)| *n);
                    format!(" GENERATED BY DEFAULT AS IDENTITY (START WITH {start})")
                }
                // The model supplies generated values explicitly (`gen_text`),
                // Phase 14: and clock readings (`clock_text`).
                Some(ColumnDefault::Generated(_) | ColumnDefault::Now | ColumnDefault::Clock) => {
                    String::new()
                }
            };
            parts.push(format!(
                "{} {}{}{default}",
                quote_ident(&c.name),
                types[ti][ci],
                if not_null { " NOT NULL" } else { "" }
            ));
        }
        if !t.primary_key.is_empty() {
            parts.push(format!(
                "PRIMARY KEY ({})",
                t.primary_key
                    .iter()
                    .map(|c| quote_ident(c))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for u in &t.unique {
            parts.push(format!(
                "UNIQUE ({})",
                u.iter()
                    .map(|c| quote_ident(c))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for check in &t.checks {
            parts.push(format!("CHECK {}", expr_sql(check)?));
        }
        out.push_str(&format!(
            "CREATE TABLE {} ({});\n",
            quote_ident(&t.name),
            parts.join(", ")
        ));
    }
    // Phase 13: foreign keys, once every table exists.
    for t in &model.schema.tables {
        for fk in &t.foreign_keys {
            let cols = |c: &[String]| {
                c.iter()
                    .map(|c| quote_ident(c))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            out.push_str(&format!(
                "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({}) ON DELETE {} ON UPDATE {}{};\n",
                quote_ident(&t.name),
                quote_ident(&fk.name(&t.name)),
                cols(&fk.columns),
                quote_ident(&fk.references),
                cols(&fk.referenced_columns),
                fk.on_delete.sql(),
                fk.on_update.sql(),
                if fk.initially_deferred {
                    " DEFERRABLE INITIALLY DEFERRED"
                } else {
                    ""
                }
            ));
        }
    }
    Ok(out)
}

/// Tables in an order that inserts referenced rows first (Phase 13).
fn seed_order(model: &Model) -> Vec<usize> {
    let n = model.schema.tables.len();
    let mut out: Vec<usize> = Vec::new();
    while out.len() < n {
        let before = out.len();
        for (ti, t) in model.schema.tables.iter().enumerate() {
            if out.contains(&ti) {
                continue;
            }
            let ready = t.foreign_keys.iter().all(|fk| {
                fk.references == t.name
                    || model
                        .schema
                        .index(&fk.references)
                        .is_none_or(|p| out.contains(&(p as usize)))
            });
            if ready {
                out.push(ti);
            }
        }
        if out.len() == before {
            // A cycle: the remaining tables in schema order.
            out.extend((0..n).filter(|i| !out.contains(i)).collect::<Vec<_>>());
        }
    }
    out
}

fn seed_sql(model: &Model) -> Result<String, String> {
    let mut out = String::new();
    let names: Vec<String> = model
        .schema
        .tables
        .iter()
        .map(|t| quote_ident(&t.name))
        .collect();
    out.push_str(&format!("TRUNCATE {};\n", names.join(", ")));
    for ti in seed_order(model) {
        let t = &model.schema.tables[ti];
        for row in &model.initial_rows[ti] {
            let cols: Vec<String> = row.keys().map(|c| quote_ident(c)).collect();
            let vals: Result<Vec<String>, String> = row
                .iter()
                .map(|(c, v)| column_value_sql(model, ti as u16, c, v))
                .collect();
            out.push_str(&format!(
                "INSERT INTO {} ({}) VALUES ({});\n",
                quote_ident(&t.name),
                cols.join(", "),
                vals?.join(", ")
            ));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------

struct Session {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
    pid: i64,
    sent: u64,
    inflight: Option<(String, Vec<String>)>,
    waiting_since: Option<Instant>,
    actor: Option<u16>,
}

impl Session {
    fn open(
        bins: &Binaries,
        socket_dir: &Path,
        db: &str,
        default_iso: Option<&str>,
        actor: Option<u16>,
    ) -> Result<Self, String> {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("exec \"$0\" \"$@\" 2>&1")
            .arg(bins.dir.join("psql"))
            .args([
                "-X",
                "-q",
                "-A",
                "-t",
                "-F",
                "\u{1f}",
                "-v",
                "ON_ERROR_STOP=0",
            ])
            .arg("-h")
            .arg(socket_dir)
            .args(["-p", PORT, "-U", USER, "-d", db])
            .env_clear()
            .env("LC_ALL", "C")
            .env("PGCONNECT_TIMEOUT", "10")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("cannot start psql: {e}"))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let mut s = Self {
            child,
            stdin,
            rx,
            pid: 0,
            sent: 0,
            inflight: None,
            waiting_since: None,
            actor,
        };
        let mut init = String::from("\\set VERBOSITY verbose\n");
        init.push_str(&format!(
            "SET deadlock_timeout = '{DEADLOCK_TIMEOUT_MS}ms';\n"
        ));
        // Phase 14: time values are read back in UTC.
        init.push_str("SET TimeZone = 'UTC';\n");
        if let Some(iso) = default_iso {
            init.push_str(&format!("SET default_transaction_isolation = '{iso}';\n"));
        }
        init.push_str("SELECT pg_backend_pid();\n");
        let lines = s.exec(&init, Duration::from_secs(10))?;
        s.pid = lines
            .iter()
            .find_map(|l| l.trim().parse::<i64>().ok())
            .ok_or_else(|| format!("no backend pid from psql: {lines:?}"))?;
        Ok(s)
    }

    fn send(&mut self, text: &str) -> Result<(), String> {
        self.sent += 1;
        let sentinel = format!("@@END-{}@@", self.sent);
        self.inflight = Some((sentinel.clone(), Vec::new()));
        self.waiting_since = None;
        let payload = format!("{text}\n\\echo {sentinel}\n");
        self.stdin
            .write_all(payload.as_bytes())
            .and_then(|()| self.stdin.flush())
            .map_err(|e| format!("cannot write to psql: {e}"))
    }

    /// Wait up to `wait` for the in-flight statement; its output lines.
    fn try_complete(&mut self, wait: Duration) -> Option<Vec<String>> {
        let deadline = Instant::now() + wait;
        loop {
            let (sentinel, _) = self.inflight.as_ref()?;
            let sentinel = sentinel.clone();
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(remaining) {
                Ok(line) => {
                    if line == sentinel {
                        return self.inflight.take().map(|(_, buf)| buf);
                    }
                    if let Some((_, buf)) = &mut self.inflight {
                        buf.push(line);
                    }
                }
                Err(RecvTimeoutError::Timeout) => return None,
                Err(RecvTimeoutError::Disconnected) => {
                    return self.inflight.take().map(|(_, mut buf)| {
                        buf.push("ERROR:  08006: connection to psql lost".into());
                        buf
                    });
                }
            }
        }
    }

    fn exec(&mut self, text: &str, timeout: Duration) -> Result<Vec<String>, String> {
        self.send(text)?;
        self.try_complete(timeout)
            .ok_or_else(|| format!("timeout waiting for PostgreSQL: {text}"))
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.kill();
    }
}

/// SQLSTATE of the first `ERROR:` line of verbose psql output.
fn error_code(lines: &[String]) -> Option<String> {
    lines.iter().find_map(|l| {
        let rest = l.strip_prefix("ERROR:")?;
        let rest = rest.trim_start();
        let code = rest.split(':').next()?.trim();
        (code.len() == 5).then(|| code.to_owned())
    })
}

fn data_lines(lines: &[String]) -> Vec<&String> {
    lines
        .iter()
        .filter(|l| {
            !l.is_empty()
                && !l.starts_with("ERROR:")
                && !l.starts_with("WARNING:")
                && !l.starts_with("NOTICE:")
                && !l.starts_with("LOCATION:")
                && !l.starts_with("DETAIL:")
                && !l.starts_with("HINT:")
                && !l.starts_with("CONTEXT:")
                && !l.starts_with("STATEMENT:")
                && !l.starts_with("QUERY:")
        })
        .collect()
}

fn json_to_value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => Value::Int(n.as_i64().unwrap_or(i64::MAX)),
        serde_json::Value::String(s) => gen_of_text(s)
            .or_else(|| clock_of_text(s))
            .map_or_else(|| Value::Str(s.clone()), Value::Gen),
        serde_json::Value::Object(o) => Value::Row(
            o.iter()
                .map(|(k, v)| (k.clone(), json_to_value(v)))
                .collect(),
        ),
        // Phase 14: a JSON array (a `json`/`jsonb` value).
        serde_json::Value::Array(items) => Value::List(items.iter().map(json_to_value).collect()),
    }
}

fn parse_result(model: &Model, stmt: &ClosedStmt, lines: &[String]) -> Result<StmtResult, String> {
    let data = data_lines(lines);
    let row = |line: &String| -> Result<Row, String> {
        let json: serde_json::Value =
            serde_json::from_str(line).map_err(|e| format!("bad row JSON `{line}`: {e}"))?;
        match json_to_value(&json) {
            Value::Row(mut r) => {
                fix_row(model, stmt.table(), &mut r);
                Ok(r)
            }
            other => Err(format!("unexpected row value {other:?}")),
        }
    };
    match stmt {
        // Phase 14.
        ClosedStmt::InsertMany {
            returning: true, ..
        } => Ok(StmtResult::Rows(
            data.iter().map(|l| row(l)).collect::<Result<_, _>>()?,
        )),
        ClosedStmt::Select {
            joins, projection, ..
        } if joins.is_empty() && projection.is_none() => match data.as_slice() {
            [] => Ok(StmtResult::Row(None)),
            [one] => Ok(StmtResult::Row(Some(row(one)?))),
            more => Err(format!("{} rows for LIMIT 1", more.len())),
        },
        ClosedStmt::Select { .. } => match data.as_slice() {
            [] => Ok(StmtResult::Row(None)),
            [one] => {
                let json: serde_json::Value =
                    serde_json::from_str(one).map_err(|e| format!("bad row JSON `{one}`: {e}"))?;
                match json_to_value(&json) {
                    Value::Row(r) => Ok(StmtResult::Row(Some(r))),
                    other => Err(format!("unexpected row value {other:?}")),
                }
            }
            more => Err(format!("{} rows for LIMIT 1", more.len())),
        },
        ClosedStmt::Count { .. } => parse_count(&data).map(StmtResult::Count),
        ClosedStmt::Insert {
            returning: true, ..
        } => match data.as_slice() {
            [] => Ok(StmtResult::Row(None)),
            [one] => Ok(StmtResult::Row(Some(row(one)?))),
            more => Err(format!("{} rows for a single-row INSERT", more.len())),
        },
        _ => parse_count(&data).map(StmtResult::Affected),
    }
}

fn parse_count(data: &[&String]) -> Result<i64, String> {
    match data {
        [one] => one
            .trim()
            .parse()
            .map_err(|_| format!("not a count: `{one}`")),
        other => Err(format!("expected one count row, got {other:?}")),
    }
}

// ---------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------

pub struct PgEnv {
    _root: ReplayRoot,
    cluster: Cluster,
    bins: Binaries,
    counter: u32,
}

pub struct Database {
    name: String,
    seed: String,
}

impl PgEnv {
    /// Start a private cluster. `Err` means PostgreSQL is unavailable (the
    /// caller reports a blocked validation, never a passed one).
    pub fn start() -> Result<Self, String> {
        let bins = Binaries::discover()?;
        let root = ReplayRoot::create(false)?;
        // No background statistics: autovacuum's ANALYZE can change a plan
        // (index scan to sequential scan) between two replays of the same
        // schedule, and with it the granularity of SERIALIZABLE predicate
        // locks, which decides some 40001 failures.
        let cluster =
            Cluster::start_with(bins.clone(), &root.path, &["autovacuum=off".to_owned()])?;
        Ok(Self {
            _root: root,
            cluster,
            bins,
            counter: 0,
        })
    }

    pub fn version(&self) -> &str {
        self.cluster.version()
    }

    fn open(&self, db: &str, iso: Option<&str>, actor: Option<u16>) -> Result<Session, String> {
        Session::open(&self.bins, self.cluster.socket_dir(), db, iso, actor)
    }

    /// Create a database with the model's schema.
    pub fn database(&mut self, model: &Model) -> Result<Database, String> {
        self.counter += 1;
        let name = format!("model{}", self.counter);
        self.cluster.create_database(&name)?;
        let mut s = self.open(&name, None, None)?;
        let lines = s.exec(&ddl(model)?, Duration::from_secs(10))?;
        if let Some(code) = error_code(&lines) {
            return Err(format!("DDL failed with SQLSTATE {code}: {lines:?}"));
        }
        Ok(Database {
            name,
            seed: seed_sql(model)?,
        })
    }

    pub fn table_contents(
        &self,
        db: &Database,
        model: &Model,
    ) -> Result<BTreeMap<String, Vec<Row>>, String> {
        let mut s = self.open(&db.name, None, None)?;
        read_tables(&mut s, model)
    }

    /// Phase 14: restore the model's initial rows (what every lockstep run
    /// starts from).
    pub fn reset(&self, db: &Database) -> Result<(), String> {
        let mut s = self.open(&db.name, None, None)?;
        let lines = s.exec(&db.seed, Duration::from_secs(10))?;
        match error_code(&lines) {
            Some(code) => Err(format!("cannot seed the database: SQLSTATE {code}")),
            None => Ok(()),
        }
    }

    /// Phase 14: run one SQL text (tests comparing SQL forms): the data
    /// lines (fields separated by U+001F), or the SQLSTATE of an error.
    pub fn query(&self, db: &Database, sql: &str) -> Result<Vec<String>, String> {
        let mut s = self.open(&db.name, None, None)?;
        let lines = s.exec(sql, Duration::from_secs(10))?;
        if let Some(code) = error_code(&lines) {
            return Err(format!("SQLSTATE {code}: {lines:?}"));
        }
        Ok(data_lines(&lines).into_iter().cloned().collect())
    }
}

fn read_tables(s: &mut Session, model: &Model) -> Result<BTreeMap<String, Vec<Row>>, String> {
    let mut out = BTreeMap::new();
    for (i, t) in model.schema.tables.iter().enumerate() {
        let lines = s.exec(
            &format!(
                "SELECT row_to_json(t) FROM (SELECT * FROM {} ORDER BY {}) t;",
                quote_ident(&t.name),
                pk_order(model, i as u16)
            ),
            Duration::from_secs(10),
        )?;
        let mut rows = Vec::new();
        for l in data_lines(&lines) {
            let json: serde_json::Value =
                serde_json::from_str(l).map_err(|e| format!("bad row JSON `{l}`: {e}"))?;
            if let Value::Row(mut r) = json_to_value(&json) {
                fix_row(model, i as u16, &mut r);
                rows.push(r);
            }
        }
        out.insert(t.name.clone(), rows);
    }
    Ok(out)
}

// ---------------------------------------------------------------------
// Lockstep run
// ---------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OracleRun {
    pub agree: bool,
    pub observations: Vec<Observation>,
    /// SQLSTATE codes PostgreSQL returned, in order.
    pub sqlstates: Vec<String>,
    pub model_tables: BTreeMap<String, Vec<Row>>,
    pub postgres_tables: BTreeMap<String, Vec<Row>>,
    pub tables_agree: bool,
    /// Actors whose statements came from extraction versus the scenario.
    pub sql_origin: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Tx(u16),
    Pool(u16, u16),
    /// Phase 9: a connection whose COMMIT the server holds for a client
    /// that is gone (actor, operation).
    Orphan(u16, u32),
}

struct Run<'a> {
    env: &'a PgEnv,
    db: &'a Database,
    model: &'a Model,
    sessions: BTreeMap<Key, Session>,
    monitor: Session,
    observations: Vec<Observation>,
    sqlstates: Vec<String>,
    /// Transition indexes at which a deadlock victim is chosen, with the
    /// victim, for steering the detector's timers.
    victims: Vec<(usize, u16, u16)>,
}

fn describe_result(r: &StmtResult) -> String {
    match r {
        StmtResult::Row(Some(row)) => Value::Row(row.clone()).render(),
        StmtResult::Row(None) => "no row".into(),
        StmtResult::Count(n) => format!("count {n}"),
        StmtResult::Affected(n) => format!("{n} row(s)"),
        StmtResult::Rows(rows) => format!(
            "rows [{}]",
            rows.iter()
                .map(|r| Value::Row(r.clone()).render())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn describe_expect(e: &Expect, model: &Model) -> String {
    match e {
        Expect::Done(r) => describe_result(r),
        Expect::Blocked(a) => format!("blocked by {}", model.actors[*a as usize].name),
        Expect::Error(c) => format!("error {c}"),
        Expect::Ok => "ok".into(),
    }
}

impl Run<'_> {
    fn session(&mut self, key: Key, actor: u16) -> Result<&mut Session, String> {
        if !self.sessions.contains_key(&key) {
            let iso = match key {
                Key::Pool(..) => Some(self.model.isolation.sql().to_ascii_lowercase()),
                Key::Tx(_) | Key::Orphan(..) => None,
            };
            let s = self.env.open(&self.db.name, iso.as_deref(), Some(actor))?;
            self.sessions.insert(key, s);
        }
        Ok(self.sessions.get_mut(&key).expect("just inserted"))
    }

    /// (waiting for a lock, blocking backends) of a backend.
    fn wait_state(&mut self, pid: i64) -> Result<(bool, Vec<i64>), String> {
        let lines = self.monitor.exec(
            &format!(
                "SELECT coalesce(wait_event_type, ''), array_to_string(pg_blocking_pids(pid), ',') FROM pg_stat_activity WHERE pid = {pid};"
            ),
            Duration::from_secs(5),
        )?;
        let Some(line) = data_lines(&lines).first().map(|l| l.to_string()) else {
            return Ok((false, Vec::new()));
        };
        let mut parts = line.split('\u{1f}');
        let waiting = parts.next() == Some("Lock");
        let blockers = parts
            .next()
            .unwrap_or("")
            .split(',')
            .filter_map(|p| p.trim().parse().ok())
            .collect();
        Ok((waiting, blockers))
    }

    fn actors_of(&self, pids: &[i64]) -> Vec<u16> {
        let mut out: Vec<u16> = self
            .sessions
            .values()
            .filter(|s| pids.contains(&s.pid))
            .filter_map(|s| s.actor)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Wait for a statement outcome. `expect_block` selects the wait policy:
    /// a blocked statement is recognized as soon as the server reports a
    /// lock wait; for statements expected to finish a lock wait only counts
    /// if it persists.
    fn observe(&mut self, key: Key, expect_block: bool) -> Result<Observed, String> {
        let deadline = Instant::now() + Duration::from_secs(if expect_block { 4 } else { 8 });
        let mut waiting_since: Option<Instant> = None;
        loop {
            let (done, pid) = {
                let s = self.sessions.get_mut(&key).expect("session exists");
                (s.try_complete(Duration::from_millis(20)), s.pid)
            };
            if let Some(lines) = done {
                if let Some(s) = self.sessions.get_mut(&key) {
                    s.waiting_since = None;
                }
                return Ok(Observed::Completed(lines));
            }
            let (waiting, blockers) = self.wait_state(pid)?;
            if waiting {
                let since = *waiting_since.get_or_insert_with(Instant::now);
                let stable =
                    since.elapsed() >= Duration::from_millis(if expect_block { 0 } else { 250 });
                if stable {
                    if let Some(s) = self.sessions.get_mut(&key) {
                        s.waiting_since.get_or_insert_with(Instant::now);
                    }
                    return Ok(Observed::Blocked(self.actors_of(&blockers)));
                }
            } else {
                waiting_since = None;
            }
            if Instant::now() > deadline {
                return Ok(Observed::Timeout);
            }
        }
    }

    fn record(
        &mut self,
        i: usize,
        actor: u16,
        sql: &str,
        predicted: String,
        observed: String,
        agree: bool,
    ) {
        self.observations.push(Observation {
            transition: i + 1,
            actor: self.model.actors[actor as usize].name.clone(),
            sql: sql.to_owned(),
            predicted,
            observed,
            agree,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn stmt(
        &mut self,
        i: usize,
        actor: u16,
        thread: u16,
        conn: Conn,
        stmt: &ClosedStmt,
        resumed: bool,
        expect: &Expect,
    ) -> Result<bool, String> {
        let key = match conn {
            Conn::Tx => Key::Tx(actor),
            Conn::Pool | Conn::Replica => Key::Pool(actor, thread),
        };
        let sql = stmt_sql(self.model, stmt)?;
        self.session(key, actor)?;
        let expect_block = matches!(expect, Expect::Blocked(_));
        if !resumed {
            // Steer PostgreSQL's deadlock detector when this statement is
            // the one the model later aborts as the victim: the earlier
            // waiter's timer must already have fired.
            if expect_block
                && self
                    .victims
                    .iter()
                    .any(|(j, a, t)| *j > i && (*a, *t) == (actor, thread))
            {
                let oldest = self.sessions.values().filter_map(|s| s.waiting_since).min();
                if let Some(since) = oldest {
                    let need = Duration::from_millis(DEADLOCK_TIMEOUT_MS * 3 / 2);
                    let spent = since.elapsed();
                    if spent < need {
                        std::thread::sleep(need - spent);
                    }
                }
            }
            self.sessions.get_mut(&key).expect("exists").send(&sql)?;
        }
        let observed = self.observe(key, expect_block)?;
        let (text, agree) = match (&observed, expect) {
            (Observed::Blocked(by), Expect::Blocked(a)) => (
                format!(
                    "blocked by {}",
                    by.iter()
                        .map(|x| self.model.actors[*x as usize].name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                by.contains(a),
            ),
            (Observed::Blocked(by), _) => (
                format!(
                    "blocked by {}",
                    by.iter()
                        .map(|x| self.model.actors[*x as usize].name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                false,
            ),
            (Observed::Timeout, _) => ("no result and no lock wait (timeout)".into(), false),
            (Observed::Completed(lines), exp) => {
                let code = error_code(lines);
                if let Some(c) = &code {
                    self.sqlstates.push(c.clone());
                }
                match (exp, code) {
                    (Expect::Error(want), Some(got)) => (format!("error {got}"), *want == got),
                    (Expect::Error(_), None) => ("completed without error".into(), false),
                    (_, Some(got)) => (format!("error {got}"), false),
                    (Expect::Done(want), None) => match parse_result(self.model, stmt, lines) {
                        Ok(got) => (describe_result(&got), got == *want),
                        Err(e) => (format!("unparsable: {e}"), false),
                    },
                    (Expect::Blocked(_), None) => ("completed without waiting".into(), false),
                    (Expect::Ok, None) => ("ok".into(), true),
                }
            }
        };
        let predicted = describe_expect(expect, self.model);
        self.record(i, actor, &sql, predicted, text, agree);
        Ok(agree)
    }

    fn simple(&mut self, key: Key, actor: u16, sql: &str) -> Result<Vec<String>, String> {
        self.session(key, actor)?;
        let s = self.sessions.get_mut(&key).expect("exists");
        s.exec(sql, Duration::from_secs(10))
    }

    fn action(&mut self, i: usize, action: &Action) -> Result<bool, String> {
        match action {
            Action::Begin { actor, iso } => {
                let sql = format!("BEGIN ISOLATION LEVEL {};", iso.sql());
                let lines = self.simple(Key::Tx(*actor), *actor, &sql)?;
                let code = error_code(&lines);
                let agree = code.is_none();
                self.record(
                    i,
                    *actor,
                    &sql,
                    "ok".into(),
                    code.map_or("ok".into(), |c| format!("error {c}")),
                    agree,
                );
                Ok(agree)
            }
            Action::Stmt {
                actor,
                thread,
                conn,
                stmt,
                resumed,
                expect,
            } => self.stmt(i, *actor, *thread, *conn, stmt, *resumed, expect),
            Action::Commit { actor, expect, .. } => {
                let lines = self.simple(Key::Tx(*actor), *actor, "COMMIT;")?;
                let code = error_code(&lines);
                if let Some(c) = &code {
                    self.sqlstates.push(c.clone());
                }
                let (observed, agree) = match (expect, &code) {
                    (Expect::Ok, None) => ("ok".to_owned(), true),
                    (Expect::Error(want), Some(got)) => (format!("error {got}"), want == got),
                    (_, Some(got)) => (format!("error {got}"), false),
                    (_, None) => ("ok".to_owned(), false),
                };
                self.record(
                    i,
                    *actor,
                    "COMMIT;",
                    describe_expect(expect, self.model),
                    observed,
                    agree,
                );
                Ok(agree)
            }
            Action::Rollback { actor } => {
                self.simple(Key::Tx(*actor), *actor, "ROLLBACK;")?;
                Ok(true)
            }
            Action::Orphan { actor, seq } => {
                // The client is gone but its COMMIT is on the server: the
                // connection stays open (idle in transaction) until the
                // model's server completes it.
                let moved = match self.sessions.remove(&Key::Tx(*actor)) {
                    Some(s) => {
                        self.sessions.insert(Key::Orphan(*actor, *seq), s);
                        true
                    }
                    None => false,
                };
                self.record(
                    i,
                    *actor,
                    if moved {
                        "(COMMIT held by the server)"
                    } else {
                        "(statement held by the server)"
                    },
                    if moved {
                        "transaction stays open"
                    } else {
                        "statement not yet executed"
                    }
                    .into(),
                    if moved {
                        "connection kept open"
                    } else {
                        "nothing sent"
                    }
                    .into(),
                    true,
                );
                Ok(true)
            }
            Action::OrphanCommit { actor, seq, expect } => {
                let key = Key::Orphan(*actor, *seq);
                if !self.sessions.contains_key(&key) {
                    return Ok(false);
                }
                let lines = self.simple(key, *actor, "COMMIT;")?;
                let code = error_code(&lines);
                if let Some(c) = &code {
                    self.sqlstates.push(c.clone());
                }
                let (observed, agree) = match (expect, &code) {
                    (Expect::Ok, None) => ("ok".to_owned(), true),
                    (Expect::Error(want), Some(got)) => (format!("error {got}"), want == got),
                    (_, Some(got)) => (format!("error {got}"), false),
                    (_, None) => ("ok".to_owned(), false),
                };
                self.record(
                    i,
                    *actor,
                    "COMMIT; (completed by the server)",
                    describe_expect(expect, self.model),
                    observed,
                    agree,
                );
                if let Some(s) = self.sessions.remove(&key) {
                    drop(s);
                }
                Ok(agree)
            }
            Action::Abort { actor, conn } => {
                if *conn == Conn::Tx && self.sessions.contains_key(&Key::Tx(*actor)) {
                    self.simple(Key::Tx(*actor), *actor, "ROLLBACK;")?;
                }
                Ok(true)
            }
            Action::Victim {
                actor,
                thread,
                conn,
            } => {
                let key = match conn {
                    Conn::Tx => Key::Tx(*actor),
                    Conn::Pool | Conn::Replica => Key::Pool(*actor, *thread),
                };
                let Some(s) = self.sessions.get_mut(&key) else {
                    return Ok(false);
                };
                let done = s.try_complete(Duration::from_millis(DEADLOCK_TIMEOUT_MS * 4 + 3000));
                let (observed, agree) = match done {
                    Some(lines) => {
                        let code = error_code(&lines);
                        if let Some(c) = &code {
                            self.sqlstates.push(c.clone());
                        }
                        let agree = code.as_deref() == Some("40P01");
                        (
                            code.map_or("completed without error".into(), |c| format!("error {c}")),
                            agree,
                        )
                    }
                    None => ("no deadlock error within the timeout".into(), false),
                };
                self.record(
                    i,
                    *actor,
                    "(deadlock detection)",
                    "error 40P01 (victim)".into(),
                    observed,
                    agree,
                );
                Ok(agree)
            }
            // Effects are not database operations: the model SQL replay
            // has nothing to run for them.
            Action::External { .. } | Action::Respond { .. } => Ok(true),
            Action::Crash { actor } => {
                let keys: Vec<Key> = self
                    .sessions
                    .iter()
                    .filter(|(k, s)| s.actor == Some(*actor) && !matches!(k, Key::Orphan(..)))
                    .map(|(k, _)| *k)
                    .collect();
                for k in keys {
                    if let Some(mut s) = self.sessions.remove(&k) {
                        let pid = s.pid;
                        s.kill();
                        // Wait until the server noticed the lost connection
                        // and rolled the transaction back.
                        let deadline = Instant::now() + Duration::from_secs(5);
                        loop {
                            let lines = self.monitor.exec(
                                &format!(
                                    "SELECT count(*) FROM pg_stat_activity WHERE pid = {pid};"
                                ),
                                Duration::from_secs(5),
                            )?;
                            if data_lines(&lines).first().is_some_and(|l| l.trim() == "0") {
                                break;
                            }
                            if Instant::now() > deadline {
                                return Err("backend of a crashed session did not exit".into());
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    }
                }
                self.record(
                    i,
                    *actor,
                    "(process killed)",
                    "connection lost, transaction rolled back".into(),
                    "backend exited".into(),
                    true,
                );
                Ok(true)
            }
        }
    }
}

enum Observed {
    Completed(Vec<String>),
    Blocked(Vec<u16>),
    Timeout,
}

/// Replay `schedule` on a fresh copy of the model's initial state in real
/// PostgreSQL, comparing every physical action with the model's prediction.
/// `Err` is an infrastructure failure (PostgreSQL could not be driven); a
/// model/PostgreSQL disagreement is reported in the `OracleRun`.
pub fn run_schedule(
    env: &PgEnv,
    db: &Database,
    model: &Model,
    schedule: &[Transition],
) -> Result<OracleRun, String> {
    if model.features.replicas {
        return Err(
            "the lockstep oracle has one PostgreSQL server: a model with a read replica is not replayed"
                .to_owned(),
        );
    }
    // Execute the model first to learn every transition's physical actions.
    let mut state = initial_state(model);
    let mut per_transition: Vec<Vec<Action>> = Vec::new();
    let mut victims = Vec::new();
    for (i, t) in schedule.iter().enumerate() {
        let mut rec = Recorder::on();
        let applied = apply(model, &state, *t, &mut rec)
            .map_err(|u| format!("the schedule reaches unsupported behavior: {}", u.detail))?;
        state = applied.state;
        if t.kind == TKind::Deadlock {
            victims.push((i, t.actor, t.thread));
        }
        per_transition.push(rec.actions);
    }
    // Phase 14: a multi-row INSERT is sent once, with its rows as the model
    // completed them (a row the model formed only after a lock wait gets its
    // database defaults then; PostgreSQL forms it then too, and the oracle
    // sends the values the model chose, as it does for single-row inserts).
    let mut open: BTreeMap<(u16, u16), (usize, usize)> = BTreeMap::new();
    let mut completed: Vec<((usize, usize), ClosedStmt)> = Vec::new();
    for (i, actions) in per_transition.iter().enumerate() {
        for (j, a) in actions.iter().enumerate() {
            if let Action::Stmt {
                actor,
                thread,
                stmt: stmt @ ClosedStmt::InsertMany { .. },
                resumed,
                ..
            } = a
            {
                if *resumed {
                    if let Some(at) = open.get(&(*actor, *thread)) {
                        completed.push((*at, stmt.clone()));
                    }
                } else {
                    open.insert((*actor, *thread), (i, j));
                }
            }
        }
    }
    for ((i, j), done) in completed {
        if let Action::Stmt { stmt, .. } = &mut per_transition[i][j] {
            *stmt = done;
        }
    }
    // Reset the data.
    let mut setup = env.open(&db.name, None, None)?;
    let lines = setup.exec(&db.seed, Duration::from_secs(10))?;
    if let Some(code) = error_code(&lines) {
        return Err(format!("cannot seed the database: SQLSTATE {code}"));
    }
    drop(setup);
    let monitor = env.open("postgres", None, None)?;
    let mut run = Run {
        env,
        db,
        model,
        sessions: BTreeMap::new(),
        monitor,
        observations: Vec::new(),
        sqlstates: Vec::new(),
        victims,
    };
    let mut agree = true;
    'outer: for (i, actions) in per_transition.iter().enumerate() {
        for a in actions {
            if !run.action(i, a)? {
                agree = false;
                break 'outer;
            }
        }
    }
    let mut reader = env.open(&db.name, None, None)?;
    let postgres_tables = read_tables(&mut reader, model)?;
    let mut model_tables = BTreeMap::new();
    for (i, t) in model.schema.tables.iter().enumerate() {
        model_tables.insert(t.name.clone(), state.committed_rows(model, i as u16));
    }
    // Rows are compared as sets of images in key order; the final tables are
    // only meaningful when every prior action agreed.
    let tables_agree = agree && model_tables == postgres_tables;
    let mut sql_origin = BTreeMap::new();
    for a in &model.actors {
        let extracted = a
            .codes
            .iter()
            .flatten()
            .any(|s| matches!(s.op, Op::Db { .. }) && s.src.origin == Origin::Extracted);
        sql_origin.insert(
            a.name.clone(),
            if extracted {
                "model-derived (extracted from source)"
            } else {
                "fixture-declared"
            }
            .to_owned(),
        );
    }
    let observations = std::mem::take(&mut run.observations);
    let sqlstates = std::mem::take(&mut run.sqlstates);
    drop(run);
    Ok(OracleRun {
        agree: agree && tables_agree,
        observations,
        sqlstates,
        model_tables,
        postgres_tables,
        tables_agree,
        sql_origin,
    })
}

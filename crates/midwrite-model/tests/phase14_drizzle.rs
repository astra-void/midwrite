//! Phase 14 Track D: multi-row inserts, SQL predicates with NULLs, timestamp
//! and JSON columns in the model (semantics), and the reductions on the new
//! statements. The PostgreSQL differential runs these workloads in
//! `pg_matrix.rs`.

mod common;
#[path = "phase14/workloads.rs"]
mod workloads;

use common::por_diff::{Alg, discrepancy, full, run};
use midwrite_model::engine::{Recorder, apply, enabled, initial_state};
use midwrite_model::explore::{ExploreOptions, SdporConfig, audit_independence, explore_with};
use midwrite_model::ir::{Conn, OnConflict, Op, StmtKind};
use midwrite_model::model::{Model, Strategy};
use midwrite_model::scenario::load_str;
use midwrite_model::state::{State, TKind};
use midwrite_model::value::{BinOp, Expr, GenId, Value};
use serde_json::json;
use workloads::*;

const RC: &str = "read-committed";

/// Step actor `who` (a Step or a Resume).
fn step_actor(model: &Model, s: &State, who: u16) -> State {
    let t = enabled(model, s)
        .into_iter()
        .find(|t| t.actor == who && matches!(t.kind, TKind::Step | TKind::Resume))
        .unwrap_or_else(|| panic!("actor {who} cannot move"));
    apply(model, s, t, &mut Recorder::off())
        .expect("supported")
        .state
}

/// Run actor `who` until it finished or waits.
fn run_actor(model: &Model, mut s: State, who: u16) -> State {
    for _ in 0..32 {
        let movable = enabled(model, &s)
            .into_iter()
            .any(|t| t.actor == who && matches!(t.kind, TKind::Step | TKind::Resume));
        if !movable {
            return s;
        }
        s = step_actor(model, &s, who);
    }
    s
}

fn blocked(s: &State, who: usize) -> bool {
    s.actors[who].threads[0]
        .pending
        .as_ref()
        .is_some_and(|p| p.blocked_on.is_some())
}

fn failure(s: &State, who: usize) -> Option<String> {
    s.actors[who].threads[0].failure.clone()
}

fn local(s: &State, who: usize, name: &str) -> Option<Value> {
    s.actors[who].locals.get(name).cloned()
}

fn table(model: &Model, s: &State, name: &str) -> Vec<midwrite_model::value::Row> {
    s.committed_rows(model, model.schema.index(name).expect("table"))
}

fn single(model: Model) -> (Model, State) {
    let s = run_actor(&model, initial_state(&model), 0);
    (model, s)
}

fn one(name: &str, code: Vec<midwrite_model::ir::Step>) -> Model {
    model(
        name,
        RC,
        insert_tables(),
        insert_initial(),
        vec![("A", code)],
    )
}

fn tags_of(model: &Model, s: &State) -> Vec<(i64, String)> {
    table(model, s, "tags")
        .into_iter()
        .map(|r| {
            let id = match r.get("id") {
                Some(Value::Int(n)) => *n,
                other => panic!("{other:?}"),
            };
            let t = match r.get("tag") {
                Some(Value::Str(t)) => t.clone(),
                other => panic!("{other:?}"),
            };
            (id, t)
        })
        .collect()
}

// ---------------------------------------------------------------------
// D1: multi-row inserts
// ---------------------------------------------------------------------

#[test]
fn a_multi_row_insert_inserts_every_row_and_returns_them_in_order() {
    let (m, s) = single(one("rows", insert_program("tags-xy-dn")));
    assert_eq!(failure(&s, 0), None);
    assert_eq!(tags_of(&m, &s), vec![(1, "x".into()), (2, "y".into())]);
    let Some(Value::List(rows)) = local(&s, 0, "r") else {
        panic!(
            "RETURNING of a multi-row insert is a list: {:?}",
            local(&s, 0, "r")
        );
    };
    assert_eq!(rows.len(), 2);
    assert_eq!(local(&s, 0, "n"), Some(Value::Int(2)));
    let ids: Vec<Value> = rows
        .iter()
        .map(|r| match r {
            Value::Row(r) => r["id"].clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(ids, vec![Value::Int(1), Value::Int(2)]);
}

#[test]
fn a_conflict_between_new_rows_aborts_the_whole_statement() {
    // Without ON CONFLICT: the second row's key equals the first's: 23505,
    // and the first row does not stay.
    let code = vec![db(
        Conn::Pool,
        insert(
            "tags",
            vec![
                row(&[("id", lit(1)), ("post", lit(1)), ("tag", lit("x"))]),
                row(&[("id", lit(2)), ("post", lit(1)), ("tag", lit("x"))]),
            ],
            OnConflict::Error,
            None,
            None,
            None,
        ),
    )];
    let (m, s) = single(one("dup", code));
    assert!(
        failure(&s, 0).is_some_and(|f| f.starts_with("23505")),
        "{:?}",
        failure(&s, 0)
    );
    assert!(tags_of(&m, &s).is_empty());
}

#[test]
fn on_conflict_do_nothing_skips_a_row_that_conflicts_with_an_earlier_row_of_the_statement() {
    // PostgreSQL 17: `INSERT ... VALUES (1,'x'),(2,'x') ON CONFLICT DO
    // NOTHING` inserts the first row only (measured, docs/phase14).
    let (m, s) = single(one("dup-dn", insert_program("tags-dup-dn")));
    assert_eq!(failure(&s, 0), None);
    assert_eq!(tags_of(&m, &s), vec![(1, "x".into())]);
    assert_eq!(local(&s, 0, "n"), Some(Value::Int(1)));
    assert!(matches!(local(&s, 0, "r"), Some(Value::List(r)) if r.len() == 1));
}

#[test]
fn an_upsert_reaching_a_row_of_its_own_statement_again_is_21000() {
    let (m, s) = single(one("bump-11", insert_program("bump-11")));
    assert!(
        failure(&s, 0).is_some_and(|f| f.starts_with("21000")),
        "{:?}",
        failure(&s, 0)
    );
    // Nothing changed: counters(1) keeps 5.
    assert_eq!(table(&m, &s, "counters")[0]["hits"], Value::Int(5));
    // Two different keys: one updated (row 1 exists), one inserted.
    let (m, s) = single(one("bump-12", insert_program("bump-12")));
    assert_eq!(failure(&s, 0), None);
    let hits: Vec<Value> = table(&m, &s, "counters")
        .into_iter()
        .map(|r| r["hits"].clone())
        .collect();
    assert_eq!(hits, vec![Value::Int(6), Value::Int(1)]);
    assert!(matches!(local(&s, 0, "r"), Some(Value::List(r)) if r.len() == 2));
}

#[test]
fn every_row_is_checked_against_its_foreign_key_and_a_violation_leaves_nothing() {
    let (m, s) = single(one("fk", insert_program("items-missing-order")));
    assert!(
        failure(&s, 0).is_some_and(|f| f.starts_with("23503")),
        "{:?}",
        failure(&s, 0)
    );
    assert_eq!(table(&m, &s, "items").len(), 1);
}

#[test]
fn sequence_values_are_drawn_per_row_when_the_row_is_formed() {
    // PostgreSQL 17 (measured): a failing first row draws one value; a
    // statement failing at its second row has drawn two.
    let doc = |rows: &str| {
        json!({
            "schemaVersion": 1, "name": "seq",
            "database": { "isolation": RC,
                "tables": [{ "name": "s", "columns": [
                    {"name":"id","notNull":true,"type":"int","default":{"sequence":{}}},
                    {"name":"u","type":"text"}], "primaryKey": ["id"], "unique": [["u"]] }],
                "initialState": { "s": [{"id": 1, "u": "k"}] } },
            "actors": [{ "name": "A", "function": "a", "program": "return\n", "arguments": {} }],
            "invariants": [{ "name": "true", "assert": { "eq": [0, 0] } }],
            "description": rows
        })
    };
    for (rows, drawn) in [
        (&["k", "l"][..], 1),
        (&["l", "l"][..], 2),
        (&["a", "b", "k"][..], 3),
    ] {
        let mut m = midwrite_model::scenario::testing::model_from_json(doc(""));
        m.actors[0].codes = vec![vec![
            db(
                Conn::Pool,
                insert(
                    "s",
                    rows.iter().map(|u| row(&[("u", lit(*u))])).collect(),
                    OnConflict::Error,
                    None,
                    None,
                    None,
                ),
            ),
            ret(),
        ]];
        let s = run_actor(&m, initial_state(&m), 0);
        assert!(failure(&s, 0).is_some_and(|f| f.starts_with("23505")));
        assert_eq!(s.seqs, vec![2 + drawn], "{rows:?}");
    }
}

#[test]
fn a_statement_waiting_in_the_middle_keeps_its_first_rows_and_resumes_at_the_waiting_row() {
    let m = insert_pair(RC, "tag-y-tx", "tags-xyz");
    let mut s = initial_state(&m);
    s = step_actor(&m, &s, 0); // BEGIN
    s = step_actor(&m, &s, 0); // insert y (uncommitted)
    s = step_actor(&m, &s, 1); // x inserted, waits at y
    assert!(blocked(&s, 1));
    let x_is_held = s.store.rows.values().any(|chain| {
        chain.last().is_some_and(|v| {
            v.commit.is_none()
                && v.values
                    .as_ref()
                    .is_some_and(|r| r["tag"] == Value::str("x"))
        })
    });
    assert!(
        x_is_held,
        "the first row is written (uncommitted) while the statement waits"
    );
    s = step_actor(&m, &s, 0); // COMMIT
    s = run_actor(&m, s, 1);
    assert!(failure(&s, 1).is_some_and(|f| f.starts_with("23505")));
    assert_eq!(tags_of(&m, &s), vec![(5, "y".into())]);
    // With a rollback the waiting statement inserts its three rows.
    let m = insert_pair(RC, "tag-y-rollback", "tags-xyz");
    let mut s = initial_state(&m);
    for who in [0, 0, 1, 0] {
        s = step_actor(&m, &s, who);
    }
    s = run_actor(&m, s, 1);
    assert_eq!(failure(&s, 1), None);
    assert_eq!(
        tags_of(&m, &s),
        vec![(1, "x".into()), (2, "y".into()), (3, "z".into())]
    );
}

#[test]
fn generated_keys_are_one_generation_event_per_row_in_row_order() {
    let mut m = midwrite_model::scenario::testing::model_from_json(json!({
        "schemaVersion": 1, "name": "gen",
        "database": { "isolation": RC,
            "tables": [{ "name": "notes", "columns": [
                {"name":"id","notNull":true,"type":"text",
                 "default":{"generated":{"side":"database","generator":"gen_random_uuid()"}}},
                {"name":"body","type":"text"}], "primaryKey": ["id"] }] },
        "actors": [{ "name": "A", "function": "a", "program": "return\n", "arguments": {} }],
        "invariants": [{ "name": "true", "assert": { "eq": [0, 0] } }]
    }));
    m.actors[0].codes = vec![vec![
        db(
            Conn::Pool,
            insert(
                "notes",
                vec![row(&[("body", lit("a"))]), row(&[("body", lit("b"))])],
                OnConflict::Error,
                Some("r"),
                None,
                None,
            ),
        ),
        ret(),
    ]];
    let s = run_actor(&m, initial_state(&m), 0);
    let Some(Value::List(rows)) = local(&s, 0, "r") else {
        panic!()
    };
    let ids: Vec<Value> = rows
        .iter()
        .map(|r| match r {
            Value::Row(r) => r["id"].clone(),
            _ => panic!(),
        })
        .collect();
    assert_eq!(
        ids,
        vec![
            Value::Gen(GenId {
                generator: 0,
                actor: 0,
                n: 0
            }),
            Value::Gen(GenId {
                generator: 0,
                actor: 0,
                n: 1
            }),
        ]
    );
}

#[test]
fn multi_row_inserts_are_refused_beyond_the_bound() {
    let rows: Vec<_> = (0..9)
        .map(|i| row(&[("id", lit(i)), ("post", lit(9)), ("tag", lit("t"))]))
        .collect();
    let mut m = one("big", vec![]);
    m.actors[0].codes = vec![vec![
        db(
            Conn::Pool,
            insert("tags", rows, OnConflict::DoNothing, None, None, None),
        ),
        ret(),
    ]];
    let err = midwrite_model::scenario::validate(&m).unwrap_err();
    assert!(err.to_string().contains("exceeds the bound"), "{err}");
}

// ---------------------------------------------------------------------
// D4: predicates with NULLs
// ---------------------------------------------------------------------

fn statuses(m: &Model, s: &State) -> Vec<(i64, Value, Value)> {
    table(m, s, "comments")
        .into_iter()
        .map(|r| {
            let Value::Int(id) = r["id"] else { panic!() };
            (id, r["parent"].clone(), r["status"].clone())
        })
        .collect()
}

#[test]
fn in_list_not_or_and_is_null_follow_three_valued_logic() {
    let one_pred = |name: &str| {
        let m = model(
            "pred",
            RC,
            comment_tables(),
            comment_initial(),
            vec![("A", predicate_program(name))],
        );
        let s = run_actor(&m, initial_state(&m), 0);
        assert_eq!(failure(&s, 0), None, "{name}");
        (m, s)
    };
    // id IN (1, NULL, 2) AND (status IS NULL OR status = 'visible'):
    // comments 1 and 2.
    let (m, s) = one_pred("hide");
    assert_eq!(local(&s, 0, "n"), Some(Value::Int(2)));
    let st = statuses(&m, &s);
    assert_eq!(st[0].2, Value::str("hidden"));
    assert_eq!(st[1].2, Value::str("hidden"));
    assert_eq!(st[3].2, Value::Null);
    // NOT (status = 'hidden'): NULL statuses are not counted.
    let (_, s) = one_pred("count-visible");
    assert_eq!(local(&s, 0, "c"), Some(Value::Int(1)));
    // NOT (id IN (2, NULL)) is NULL or false for every row.
    let (_, s) = one_pred("count-not-in-null");
    assert_eq!(local(&s, 0, "c"), Some(Value::Int(0)));
    // inArray(id, []) is `false`; notInArray(id, []) is `true`.
    let (_, s) = one_pred("hide-none");
    assert_eq!(local(&s, 0, "n"), Some(Value::Int(0)));
    let (m, s) = one_pred("prune-replies");
    assert_eq!(local(&s, 0, "n"), Some(Value::Int(2)));
    assert_eq!(statuses(&m, &s).len(), 2);
    // The conditional predicate with `parentId === null`: top-level
    // comments except 2.
    let (m, s) = one_pred("prune-top-tx");
    assert_eq!(local(&s, 0, "n"), Some(Value::Int(1)));
    let ids: Vec<i64> = statuses(&m, &s).iter().map(|x| x.0).collect();
    assert_eq!(ids, vec![2, 3, 4]);
}

#[test]
fn in_list_over_values_it_does_not_model_is_unsupported_not_a_database_error() {
    // `inArray(id, [{...}])`: comparing an object is not modeled.
    let pred = Expr::Call {
        func: midwrite_model::value::Builtin::InList,
        args: vec![
            col("id"),
            Expr::Array(vec![Expr::Object(vec![("a".into(), lit(1))])]),
        ],
    };
    let m = model(
        "pred",
        RC,
        comment_tables(),
        comment_initial(),
        vec![("A", vec![db(Conn::Pool, count("comments", pred, "c"))])],
    );
    let r = explore_with(&m, &opts(false));
    assert!(!r.unsupported.is_empty(), "the path is unsupported");
    assert!(
        r.unsupported[0].detail.contains("not modeled"),
        "{}",
        r.unsupported[0].detail
    );
}

// ---------------------------------------------------------------------
// D2: timestamps (clock readings)
// ---------------------------------------------------------------------

fn time_model(
    programs: Vec<(&str, Vec<midwrite_model::ir::Step>)>,
    extra_tables: serde_json::Value,
) -> Model {
    let mut tables = json!([
        { "name": "users", "columns": [
            {"name":"id","notNull":true,"type":"int"},
            {"name":"createdAt","notNull":true,"type":"timestamptz","default":"now"},
            {"name":"welcomedAt","type":"timestamptz"},
            {"name":"updatedAt","type":"timestamptz","onUpdate":"clock"},
            {"name":"day","type":"date"}
          ], "primaryKey": ["id"] }
    ]);
    if let (Some(t), Some(extra)) = (tables.as_array_mut(), extra_tables.as_array()) {
        t.extend(extra.iter().cloned());
    }
    let actors: Vec<serde_json::Value> = programs
        .iter()
        .map(|(n, _)| json!({ "name": n, "function": n, "program": "return\n", "arguments": {} }))
        .collect();
    let (mut m, _) = load_str(
        &json!({
            "schemaVersion": 1, "name": "clocks", "features": ["timestamps"],
            "database": { "isolation": RC, "tables": tables,
                "initialState": { "users": [{"id": 9, "createdAt": "2024-01-01T00:00:00.000Z", "day": "2024-01-01"}] } },
            "actors": actors,
            "invariants": [{ "name": "true", "assert": { "eq": [0, 0] } }]
        })
        .to_string(),
        std::path::Path::new("."),
        &midwrite_model::provider::NoProvider,
    )
    .expect("valid");
    for (i, (_, mut code)) in programs.into_iter().enumerate() {
        code.push(ret());
        m.actors[i].codes = vec![code];
    }
    m
}

fn user(id: i64) -> Vec<(String, Expr)> {
    row(&[("id", lit(id))])
}

#[test]
fn now_is_the_same_value_in_one_transaction_and_another_in_the_next() {
    let ins = |id| {
        db(
            Conn::Tx,
            insert("users", vec![user(id)], OnConflict::Error, None, None, None),
        )
    };
    let m = time_model(
        vec![(
            "A",
            vec![
                begin(),
                ins(1),
                ins(2),
                commit(),
                db(
                    Conn::Pool,
                    insert("users", vec![user(3)], OnConflict::Error, None, None, None),
                ),
            ],
        )],
        json!([]),
    );
    let s = run_actor(&m, initial_state(&m), 0);
    assert_eq!(failure(&s, 0), None);
    let rows = table(&m, &s, "users");
    let created: Vec<&Value> = rows.iter().map(|r| &r["createdAt"]).collect();
    assert_eq!(
        created[0], created[1],
        "now() is stable within a transaction"
    );
    assert_ne!(
        created[1], created[2],
        "another transaction reads another now()"
    );
    assert!(matches!(created[0], Value::Gen(g) if g.generator == midwrite_model::db::CLOCK_DB));
    // `$onUpdate(() => new Date())` on INSERT (no default): one application
    // reading per row.
    assert!(
        matches!(&rows[0]["updatedAt"], Value::Gen(g) if g.generator == midwrite_model::db::CLOCK_APP)
    );
    assert_ne!(rows[0]["updatedAt"], rows[1]["updatedAt"]);
}

#[test]
fn update_sets_the_on_update_column_and_new_date_once_per_execution() {
    // `set({ welcomedAt: new Date() }).where(and(eq(id, 9), isNull(welcomedAt)))`
    let welcome = |aff: &str| {
        db(
            Conn::Pool,
            update(
                "users",
                and(eq(col("id"), lit(9)), is_null(col("welcomedAt"))),
                &[(
                    "welcomedAt",
                    Expr::Lit(midwrite_model::db::clock_placeholder(7)),
                )],
                aff,
            ),
        )
    };
    let m = time_model(vec![("A", vec![welcome("a"), welcome("b")])], json!([]));
    let s = run_actor(&m, initial_state(&m), 0);
    assert_eq!(failure(&s, 0), None);
    assert_eq!(local(&s, 0, "a"), Some(Value::Int(1)));
    assert_eq!(
        local(&s, 0, "b"),
        Some(Value::Int(0)),
        "isNull(welcomedAt) is false now"
    );
    let u = &table(&m, &s, "users")[0];
    let (Value::Gen(w), Value::Gen(up)) = (&u["welcomedAt"], &u["updatedAt"]) else {
        panic!("{u:?}")
    };
    assert_eq!(w.generator, midwrite_model::db::CLOCK_APP);
    assert_ne!(w, up, "the $onUpdate reading is its own reading");
    assert_eq!(u["createdAt"], Value::str("2024-01-01T00:00:00.000Z"));
}

#[test]
fn ordering_a_clock_reading_is_unsupported_and_explicit_values_compare_by_value() {
    // lt(createdAt, sql`now()`): the order of an opaque reading.
    let m = time_model(
        vec![(
            "A",
            vec![db(
                Conn::Pool,
                count(
                    "users",
                    Expr::bin(
                        BinOp::Lt,
                        col("createdAt"),
                        Expr::Lit(midwrite_model::db::now_placeholder()),
                    ),
                    "c",
                ),
            )],
        )],
        json!([]),
    );
    let r = explore_with(&m, &opts(false));
    assert!(!r.unsupported.is_empty());
    assert!(
        r.unsupported[0].detail.contains("order"),
        "{}",
        r.unsupported[0].detail
    );
    // Explicit canonical values compare by value.
    let m = time_model(
        vec![(
            "A",
            vec![db(
                Conn::Pool,
                count(
                    "users",
                    Expr::bin(BinOp::Lt, col("createdAt"), lit("2024-06-01T00:00:00.000Z")),
                    "c",
                ),
            )],
        )],
        json!([]),
    );
    let s = run_actor(&m, initial_state(&m), 0);
    assert_eq!(local(&s, 0, "c"), Some(Value::Int(1)));
}

#[test]
fn time_columns_hold_only_the_values_the_model_represents() {
    // A non-canonical explicit value is unsupported (PostgreSQL would
    // normalize it; Drizzle would not even send a string for a Date column).
    let m = time_model(
        vec![(
            "A",
            vec![db(
                Conn::Pool,
                insert(
                    "users",
                    vec![row(&[
                        ("id", lit(1)),
                        ("welcomedAt", lit("2024-01-01 10:00")),
                    ])],
                    OnConflict::Error,
                    None,
                    None,
                    None,
                ),
            )],
        )],
        json!([]),
    );
    let r = explore_with(&m, &opts(false));
    assert!(!r.unsupported.is_empty());
    // A clock reading into a date column (truncated to its day) is refused.
    let m = time_model(
        vec![(
            "A",
            vec![db(
                Conn::Pool,
                update(
                    "users",
                    eq(col("id"), lit(9)),
                    &[("day", Expr::Lit(midwrite_model::db::now_placeholder()))],
                    "n",
                ),
            )],
        )],
        json!([]),
    );
    let r = explore_with(&m, &opts(false));
    assert!(r.unsupported.iter().any(|u| u.detail.contains("date")));
    // The loader refuses time columns without the feature, and a clock
    // default on a date column.
    let doc = |features: serde_json::Value, column: serde_json::Value| {
        json!({
            "schemaVersion": 1, "name": "t", "features": features,
            "database": { "tables": [{ "name": "t", "columns": [{"name":"id","type":"int"}, column], "primaryKey": ["id"] }] },
            "actors": [{ "name": "A", "function": "a", "program": "return\n", "arguments": {} }],
            "invariants": [{ "name": "true", "assert": { "eq": [0, 0] } }]
        })
        .to_string()
    };
    let load = |d: String| {
        load_str(
            &d,
            std::path::Path::new("."),
            &midwrite_model::provider::NoProvider,
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    };
    assert!(load(doc(json!([]), json!({"name":"at","type":"timestamptz"}))).is_err());
    assert!(
        load(doc(
            json!(["timestamps"]),
            json!({"name":"at","type":"timestamptz"})
        ))
        .is_ok()
    );
    assert!(
        load(doc(
            json!(["timestamps"]),
            json!({"name":"at","type":"date","default":"now"})
        ))
        .is_err()
    );
    assert!(load(doc(json!(["json"]), json!({"name":"at","type":"jsonb"}))).is_ok());
}

// ---------------------------------------------------------------------
// D3: JSON columns
// ---------------------------------------------------------------------

#[test]
fn json_values_are_stored_and_read_back_and_unbounded_ones_are_refused() {
    let tables = json!([{ "name": "settings", "columns": [
        {"name":"userId","notNull":true,"type":"int"},
        {"name":"prefs","type":"jsonb"}], "primaryKey": ["userId"] }]);
    let build = |prefs: Expr| {
        let (mut m, _) = load_str(
            &json!({
                "schemaVersion": 1, "name": "json", "features": ["json"],
                "database": { "isolation": RC, "tables": tables.clone() },
                "actors": [{ "name": "A", "function": "a", "program": "return\n", "arguments": {} }],
                "invariants": [{ "name": "true", "assert": { "eq": [0, 0] } }]
            })
            .to_string(),
            std::path::Path::new("."),
            &midwrite_model::provider::NoProvider,
        )
        .expect("valid");
        m.actors[0].codes = vec![vec![
            db(
                Conn::Pool,
                insert(
                    "settings",
                    vec![row(&[("userId", lit(1)), ("prefs", prefs)])],
                    OnConflict::Error,
                    None,
                    None,
                    None,
                ),
            ),
            db(
                Conn::Pool,
                StmtKind::Select {
                    table: "settings".into(),
                    pred: Expr::not(is_null(col("prefs"))),
                    lock: midwrite_model::ir::LockMode::None,
                    into: "row".into(),
                    joins: Vec::new(),
                    projection: None,
                    many: None,
                },
            ),
            ret(),
        ]];
        m
    };
    let prefs = Expr::Object(vec![
        ("theme".into(), lit("dark")),
        ("tags".into(), Expr::Array(vec![lit(1), lit(2)])),
    ]);
    let m = build(prefs);
    let s = run_actor(&m, initial_state(&m), 0);
    assert_eq!(failure(&s, 0), None);
    let Some(Value::Row(r)) = local(&s, 0, "row") else {
        panic!()
    };
    let Value::Row(p) = &r["prefs"] else { panic!() };
    assert_eq!(p["tags"], Value::List(vec![Value::Int(1), Value::Int(2)]));
    // Nesting beyond the bound.
    let mut deep = lit(1);
    for _ in 0..6 {
        deep = Expr::Array(vec![deep]);
    }
    let r = explore_with(&build(deep), &opts(false));
    assert!(!r.unsupported.is_empty());
}

// ---------------------------------------------------------------------
// D6: reductions
// ---------------------------------------------------------------------

fn opts(por: bool) -> ExploreOptions {
    ExploreOptions {
        por,
        strategy: Strategy::Dfs,
        collect_states: true,
        stop_at_first: false,
    }
}

#[test]
fn reductions_agree_on_multi_row_insert_and_predicate_workloads() {
    let algs = [
        Alg::Sleep,
        Alg::Classic,
        Alg::Sdpor(SdporConfig {
            cache: true,
            sleep: true,
            no_enabling_edges: false,
            defects: 0,
        }),
        Alg::Sdpor(SdporConfig {
            cache: false,
            sleep: true,
            no_enabling_edges: false,
            defects: 0,
        }),
    ];
    let mut models: Vec<(String, Model)> = Vec::new();
    for iso in ["read-committed", "repeatable-read", "serializable"] {
        for (label, a, b) in insert_workloads() {
            models.push((format!("insert/{label}/{iso}"), insert_pair(iso, a, b)));
        }
        for (label, a, b) in predicate_workloads() {
            models.push((format!("pred/{label}/{iso}"), predicate_pair(iso, a, b)));
        }
        for (label, a, b) in value_workloads() {
            models.push((format!("values/{label}/{iso}"), value_pair(iso, a, b)));
        }
    }
    // Three actors: two multi-row inserts and a single-row one.
    for iso in ["read-committed", "repeatable-read"] {
        models.push((
            format!("insert/three/{iso}"),
            model(
                "three",
                iso,
                insert_tables(),
                insert_initial(),
                vec![
                    ("A", insert_program("tags-xy-dn")),
                    ("B", insert_program("tags-yz-dn")),
                    ("C", insert_program("tag-y-tx")),
                ],
            ),
        ));
    }
    let mut compared = 0;
    let mut audited_states = 0u64;
    let mut unsupported_models = 0;
    for (label, m) in &models {
        let violations = audit_independence(m, 100_000);
        assert!(
            violations.is_empty(),
            "{label}: independence audit: {:?}",
            violations
                .iter()
                .map(|v| format!("{:?} vs {:?}: {}", v.a, v.b, v.reason))
                .collect::<Vec<_>>()
        );
        let f = full(m).unwrap_or_else(|| panic!("{label}: the unreduced search is truncated"));
        // The order of a clock reading is unsupported by design: a schedule
        // where `stale-tx` compares the reading `touch-ab` wrote ends there.
        if label.contains("stale-tx") {
            unsupported_models += usize::from(!f.unsupported.is_empty());
        } else {
            assert!(f.unsupported.is_empty(), "{label}: {:?}", f.unsupported);
        }
        audited_states += f.metrics.states;
        for alg in algs {
            let r = run(alg, m);
            if let Some(d) = discrepancy(&f, &r, m) {
                panic!(
                    "{label}: {} differs from the unreduced search: {d}",
                    alg.name()
                );
            }
            compared += 1;
        }
    }
    eprintln!(
        "{} models, {compared} reduced searches compared, {audited_states} states in the unreduced searches, {unsupported_models} with an unsupported (clock order) path",
        models.len()
    );
    assert_eq!(unsupported_models, 3);
    assert_eq!(compared, models.len() * algs.len());
}

// ---------------------------------------------------------------------
// D7: earlier IR is unchanged
// ---------------------------------------------------------------------

#[test]
fn a_single_row_insert_serializes_as_before() {
    let kind = insert(
        "tags",
        vec![row(&[("id", lit(1))])],
        OnConflict::Error,
        None,
        None,
        None,
    );
    let text = serde_json::to_string(&kind).unwrap();
    assert!(!text.contains("more_rows"), "{text}");
    let back: StmtKind = serde_json::from_str(&text).unwrap();
    assert_eq!(back, kind);
    let many = insert(
        "tags",
        vec![row(&[("id", lit(1))]), row(&[("id", lit(2))])],
        OnConflict::Error,
        None,
        None,
        None,
    );
    let text = serde_json::to_string(&many).unwrap();
    assert!(
        text.contains("\"more_rows\":[[[\"id\",{\"lit\":2}]]]"),
        "{text}"
    );
    let op: Op = serde_json::from_str(&format!(
        "{{\"op\":\"db\",\"conn\":\"pool\",{}}}",
        &text[1..text.len() - 1]
    ))
    .unwrap();
    assert!(matches!(op, Op::Db { stmt } if stmt.kind == many));
}

// ---------------------------------------------------------------------
// D5: the SQL the pinned Drizzle generates (`fixtures/phase14/drizzle-app/
// probe/sql-probe.mjs`, recorded in `reports/phase14/drizzle-sql/probe.json`)
// and the predicate the extractor produces for the same call (recorded by
// the analyzer test `extracted_probe_predicates_are_the_recorded_ones`)
// select the same rows: on PostgreSQL (Drizzle's text, the oracle's
// rendering) and in the model.
// ---------------------------------------------------------------------

fn sql_literal(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "NULL".into(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => format!("'{}'", s.replace('\'', "''")),
        serde_json::Value::Bool(b) => b.to_string(),
        other => panic!("parameter {other}"),
    }
}

/// The WHERE clause of a Drizzle statement with its parameters inlined
/// (`TRUE` without one); the model's column names.
fn drizzle_where(sql: &str, params: &[serde_json::Value]) -> String {
    let Some((_, clause)) = sql.split_once(" where ") else {
        return "TRUE".into();
    };
    let mut out = clause.to_owned();
    for (i, p) in params.iter().enumerate().rev() {
        out = out.replace(&format!("${}", i + 1), &sql_literal(p));
    }
    out.replace("\"parent_id\"", "\"parentId\"")
}

#[test]
fn drizzle_sql_and_the_oracle_rendering_select_the_same_rows() {
    use midwrite_model::oracle::PgEnv;
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let read = |p: &str| -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(repo.join(p)).unwrap()).unwrap()
    };
    let probe = read("reports/phase14/drizzle-sql/probe.json");
    assert_eq!(probe["drizzleOrm"], "0.45.4");
    let extracted = read("fixtures/phase14/drizzle-app/probe/extracted-predicates.json");
    let env = match PgEnv::start() {
        Ok(env) => env,
        Err(reason) => {
            assert!(
                std::env::var_os("MIDWRITE_REQUIRE_REPLAY").is_none(),
                "PostgreSQL is required but unavailable: {reason}"
            );
            eprintln!("skipping: {reason}");
            return;
        }
    };
    let mut env = env;
    let model = midwrite_model::scenario::testing::model_from_json(json!({
        "schemaVersion": 1, "name": "comments",
        "database": {
            "tables": [{ "name": "comments", "columns": [
                {"name":"id","notNull":true,"type":"int"},
                {"name":"parentId","type":"int"},
                {"name":"body","type":"text"},
                {"name":"status","type":"text"}], "primaryKey": ["id"] }],
            "initialState": { "comments": [
                {"id": 1, "parentId": null, "body": "a", "status": "visible"},
                {"id": 2, "parentId": null, "body": "b", "status": null},
                {"id": 3, "parentId": 1, "body": "c", "status": "hidden"},
                {"id": 4, "parentId": 4, "body": "d", "status": null},
                {"id": 5, "parentId": null, "body": "e", "status": "hidden"}
            ] }
        },
        "actors": [{ "name": "A", "function": "a", "program": "return\n", "arguments": {} }],
        "invariants": [{ "name": "true", "assert": { "eq": [0, 0] } }]
    }));
    let db = env.database(&model).unwrap();
    env.reset(&db).unwrap();
    let rows = model.initial_rows[0].clone();
    let ids = |lines: Vec<String>| -> Vec<i64> {
        lines.iter().map(|l| l.trim().parse().unwrap()).collect()
    };
    let select = |w: &str| format!("SELECT \"id\" FROM \"comments\" WHERE {w} ORDER BY \"id\";");
    let mut compared = 0;
    for (name, pred) in extracted.as_object().unwrap() {
        let p = probe["probes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == *name)
            .unwrap_or_else(|| panic!("probe {name}"));
        let drizzle = drizzle_where(p["sql"].as_str().unwrap(), p["params"].as_array().unwrap());
        let closed: Expr = serde_json::from_value(pred.clone()).unwrap();
        let rendered = midwrite_model::oracle::expr_sql(&closed).unwrap();
        let by_drizzle = ids(env.query(&db, &select(&drizzle)).unwrap());
        let by_oracle = ids(env.query(&db, &select(&rendered)).unwrap());
        let by_model: Vec<i64> = rows
            .iter()
            .filter(|r| midwrite_model::value::predicate_matches(&closed, r).unwrap())
            .map(|r| match r["id"] {
                Value::Int(n) => n,
                _ => unreachable!(),
            })
            .collect();
        eprintln!("{name}: {by_drizzle:?}\n  drizzle: {drizzle}\n  oracle:  {rendered}");
        assert_eq!(
            by_drizzle, by_oracle,
            "{name}: Drizzle `{drizzle}` vs oracle `{rendered}`"
        );
        assert_eq!(
            by_drizzle, by_model,
            "{name}: Drizzle `{drizzle}` vs the model"
        );
        compared += 1;
    }
    assert_eq!(compared, 11);
}

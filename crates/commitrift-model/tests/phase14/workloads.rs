//! Phase 14 Track D workloads shared by the semantic, reduction and
//! PostgreSQL tests: multi-row inserts (unique keys, foreign keys,
//! sequences, `ON CONFLICT DO NOTHING`/`DO UPDATE`, `RETURNING`) and SQL
//! predicates with NULLs (`IN`, `NOT`, `OR`, `IS NULL`, a conditional
//! predicate). Programs are written as IR (the declared-scenario DSL has no
//! syntax for these statements).
#![allow(dead_code)]

use commitrift_model::ir::{Conn, OnConflict, Op, SourceRef, Step, Stmt, StmtKind, Upsert};
use commitrift_model::model::Model;
use commitrift_model::scenario::testing::model_from_json;
use commitrift_model::value::{BinOp, Builtin, Expr, Value};
use serde_json::{Value as Json, json};

pub fn step(op: Op) -> Step {
    Step {
        op,
        src: SourceRef::declared(0, ""),
    }
}

pub fn begin() -> Step {
    step(Op::Begin {
        isolation: None,
        retry: 0,
    })
}

pub fn commit() -> Step {
    step(Op::Commit)
}

pub fn rollback() -> Step {
    step(Op::Rollback)
}

pub fn ret() -> Step {
    step(Op::Return)
}

pub fn db(conn: Conn, kind: StmtKind) -> Step {
    step(Op::Db {
        stmt: Stmt { conn, kind },
    })
}

pub fn lit(v: impl Into<Value>) -> Expr {
    Expr::lit(v)
}

pub fn null() -> Expr {
    Expr::Lit(Value::Null)
}

pub fn col(c: &str) -> Expr {
    Expr::col(c)
}

pub fn row(cells: &[(&str, Expr)]) -> Vec<(String, Expr)> {
    cells
        .iter()
        .map(|(c, e)| ((*c).to_owned(), e.clone()))
        .collect()
}

/// `INSERT INTO table VALUES (rows...)`.
pub fn insert(
    table: &str,
    rows: Vec<Vec<(String, Expr)>>,
    on_conflict: OnConflict,
    returning: Option<&str>,
    affected: Option<&str>,
    upsert: Option<Upsert>,
) -> StmtKind {
    let mut rows = rows.into_iter();
    let first = rows.next().expect("a row");
    StmtKind::Insert {
        table: table.to_owned(),
        values: first,
        on_conflict,
        affected: affected.map(str::to_owned),
        returning: returning.map(str::to_owned),
        returning_columns: None,
        upsert,
        more_rows: rows.collect(),
    }
}

pub fn update(table: &str, pred: Expr, set: &[(&str, Expr)], affected: &str) -> StmtKind {
    StmtKind::Update {
        table: table.to_owned(),
        pred,
        set: row(set),
        affected: Some(affected.to_owned()),
    }
}

pub fn delete(table: &str, pred: Expr, affected: &str) -> StmtKind {
    StmtKind::Delete {
        table: table.to_owned(),
        pred,
        affected: Some(affected.to_owned()),
    }
}

pub fn count(table: &str, pred: Expr, into: &str) -> StmtKind {
    StmtKind::SelectCount {
        table: table.to_owned(),
        pred,
        into: into.to_owned(),
    }
}

pub fn select_for_update(table: &str, pred: Expr, into: &str) -> StmtKind {
    StmtKind::Select {
        table: table.to_owned(),
        pred,
        lock: commitrift_model::ir::LockMode::ForUpdate,
        into: into.to_owned(),
        joins: Vec::new(),
        projection: None,
    }
}

/// `col IN (items...)` (Drizzle `inArray`).
pub fn in_list(c: &str, items: Vec<Expr>) -> Expr {
    Expr::Call {
        func: Builtin::InList,
        args: vec![col(c), Expr::Array(items)],
    }
}

pub fn and(a: Expr, b: Expr) -> Expr {
    Expr::bin(BinOp::And, a, b)
}

pub fn or(a: Expr, b: Expr) -> Expr {
    Expr::bin(BinOp::Or, a, b)
}

pub fn eq(a: Expr, b: Expr) -> Expr {
    Expr::bin(BinOp::Eq, a, b)
}

pub fn is_null(e: Expr) -> Expr {
    Expr::IsNull(Box::new(e))
}

/// A model with the given tables and initial rows whose actors run the given
/// IR programs (`Return` is appended).
pub fn model(
    name: &str,
    iso: &str,
    tables: Json,
    initial: Json,
    programs: Vec<(&str, Vec<Step>)>,
) -> Model {
    model_with(name, iso, tables, initial, programs, json!([]))
}

pub fn model_with(
    name: &str,
    iso: &str,
    tables: Json,
    initial: Json,
    programs: Vec<(&str, Vec<Step>)>,
    features: Json,
) -> Model {
    let actors: Vec<Json> = programs
        .iter()
        .map(|(n, _)| json!({ "name": n, "function": n, "program": "return\n", "arguments": {} }))
        .collect();
    let mut m = model_from_json(json!({
        "schemaVersion": 1,
        "name": name,
        "features": features,
        "database": { "isolation": iso, "tables": tables, "initialState": initial },
        "actors": actors,
        "invariants": [{ "name": "true", "assert": { "eq": [0, 0] } }]
    }));
    for (i, (_, mut code)) in programs.into_iter().enumerate() {
        code.push(ret());
        m.actors[i].codes = vec![code];
    }
    commitrift_model::scenario::validate(&m).expect("valid IR program");
    m
}

// ---------------------------------------------------------------------
// Multi-row inserts
// ---------------------------------------------------------------------

pub fn insert_tables() -> Json {
    json!([
        { "name": "orders", "columns": [
            {"name":"id","notNull":true,"type":"int"},
            {"name":"customer","type":"text"}
          ], "primaryKey": ["id"] },
        { "name": "items", "columns": [
            {"name":"id","notNull":true,"type":"int","default":{"sequence":{}}},
            {"name":"orderId","notNull":true,"type":"int"},
            {"name":"sku","type":"text"}
          ], "primaryKey": ["id"],
          "foreignKeys": [{ "columns": ["orderId"], "references": "orders", "referencedColumns": ["id"] }] },
        { "name": "tags", "columns": [
            {"name":"id","notNull":true,"type":"int"},
            {"name":"post","type":"int"},
            {"name":"tag","type":"text"}
          ], "primaryKey": ["id"], "unique": [["post", "tag"]] },
        { "name": "counters", "columns": [
            {"name":"id","notNull":true,"type":"int"},
            {"name":"hits","type":"int"}
          ], "primaryKey": ["id"] }
    ])
}

pub fn insert_initial() -> Json {
    json!({
        "orders": [{"id": 1, "customer": "c"}],
        "items": [{"id": 1, "orderId": 1, "sku": "z"}],
        "counters": [{"id": 1, "hits": 5}]
    })
}

fn tag(id: i64, post: i64, t: &str) -> Vec<(String, Expr)> {
    row(&[("id", lit(id)), ("post", lit(post)), ("tag", lit(t))])
}

fn item(order: i64, sku: &str) -> Vec<(String, Expr)> {
    row(&[("orderId", lit(order)), ("sku", lit(sku))])
}

fn bump() -> Upsert {
    Upsert {
        target: vec!["id".into()],
        set: vec![("hits".into(), Expr::bin(BinOp::Add, col("hits"), lit(1)))],
        filter: None,
    }
}

/// A program by name (see [`insert_workloads`]).
pub fn insert_program(name: &str) -> Vec<Step> {
    use Conn::{Pool, Tx};
    use OnConflict::{DoNothing, Error};
    match name {
        // Two tags in one statement (autocommit), the second conflicting
        // with the other actor's first.
        "tags-xy" => vec![db(
            Pool,
            insert(
                "tags",
                vec![tag(1, 1, "x"), tag(2, 1, "y")],
                Error,
                None,
                Some("n"),
                None,
            ),
        )],
        "tags-yz" => vec![db(
            Pool,
            insert(
                "tags",
                vec![tag(3, 1, "y"), tag(4, 1, "z")],
                Error,
                None,
                Some("n"),
                None,
            ),
        )],
        "tags-xy-dn" => vec![db(
            Pool,
            insert(
                "tags",
                vec![tag(1, 1, "x"), tag(2, 1, "y")],
                DoNothing,
                Some("r"),
                Some("n"),
                None,
            ),
        )],
        "tags-yz-dn" => vec![db(
            Pool,
            insert(
                "tags",
                vec![tag(3, 1, "y"), tag(4, 1, "z")],
                DoNothing,
                Some("r"),
                Some("n"),
                None,
            ),
        )],
        "tags-xy-tx" => vec![
            begin(),
            db(
                Tx,
                insert(
                    "tags",
                    vec![tag(1, 1, "x"), tag(2, 1, "y")],
                    Error,
                    None,
                    Some("n"),
                    None,
                ),
            ),
            commit(),
        ],
        // Two rows of one statement with the same key: the second is skipped.
        "tags-dup-dn" => vec![db(
            Pool,
            insert(
                "tags",
                vec![tag(1, 1, "x"), tag(2, 1, "x")],
                DoNothing,
                Some("r"),
                Some("n"),
                None,
            ),
        )],
        "tag-x-tx" => vec![
            begin(),
            db(
                Tx,
                insert("tags", vec![tag(5, 1, "x")], Error, None, Some("n"), None),
            ),
            commit(),
        ],
        "tag-y-tx" => vec![
            begin(),
            db(
                Tx,
                insert("tags", vec![tag(5, 1, "y")], Error, None, Some("n"), None),
            ),
            commit(),
        ],
        "tag-y-rollback" => vec![
            begin(),
            db(
                Tx,
                insert("tags", vec![tag(5, 1, "y")], Error, None, Some("n"), None),
            ),
            rollback(),
        ],
        // Three rows: the second waits for the other actor's uncommitted
        // `y` (a wait in the middle of the statement).
        "tags-xyz" => vec![db(
            Pool,
            insert(
                "tags",
                vec![tag(1, 1, "x"), tag(2, 1, "y"), tag(3, 1, "z")],
                Error,
                None,
                Some("n"),
                None,
            ),
        )],
        "tags-xyz-dn" => vec![db(
            Pool,
            insert(
                "tags",
                vec![tag(1, 1, "x"), tag(2, 1, "y"), tag(3, 1, "z")],
                DoNothing,
                Some("r"),
                Some("n"),
                None,
            ),
        )],
        // Order lines (a sequence key, a foreign key per row).
        "items-tx" => vec![
            begin(),
            db(
                Tx,
                insert(
                    "items",
                    vec![item(1, "a"), item(1, "b")],
                    Error,
                    Some("r"),
                    Some("n"),
                    None,
                ),
            ),
            commit(),
        ],
        "items" => vec![db(
            Pool,
            insert(
                "items",
                vec![item(1, "a"), item(1, "b")],
                Error,
                Some("r"),
                Some("n"),
                None,
            ),
        )],
        "item-tx" => vec![
            begin(),
            db(
                Tx,
                insert(
                    "items",
                    vec![item(1, "c")],
                    Error,
                    Some("r"),
                    Some("n"),
                    None,
                ),
            ),
            commit(),
        ],
        "items-missing-order" => vec![db(
            Pool,
            insert(
                "items",
                vec![item(1, "a"), item(2, "b")],
                Error,
                None,
                Some("n"),
                None,
            ),
        )],
        "order-delete" => vec![db(Pool, delete("orders", eq(col("id"), lit(1)), "d"))],
        "order-lock" => vec![
            begin(),
            db(Tx, select_for_update("orders", eq(col("id"), lit(1)), "o")),
            commit(),
        ],
        // Multi-row upserts on `counters` (row 1 exists, row 2 does not).
        "bump-12" => vec![db(
            Pool,
            insert(
                "counters",
                vec![
                    row(&[("id", lit(1)), ("hits", lit(1))]),
                    row(&[("id", lit(2)), ("hits", lit(1))]),
                ],
                Error,
                Some("r"),
                Some("n"),
                Some(bump()),
            ),
        )],
        "bump-21-tx" => vec![
            begin(),
            db(
                Tx,
                insert(
                    "counters",
                    vec![
                        row(&[("id", lit(2)), ("hits", lit(1))]),
                        row(&[("id", lit(1)), ("hits", lit(1))]),
                    ],
                    Error,
                    Some("r"),
                    Some("n"),
                    Some(bump()),
                ),
            ),
            commit(),
        ],
        // The same key twice in one upsert: SQLSTATE 21000.
        "bump-11" => vec![db(
            Pool,
            insert(
                "counters",
                vec![
                    row(&[("id", lit(1)), ("hits", lit(1))]),
                    row(&[("id", lit(1)), ("hits", lit(1))]),
                ],
                Error,
                None,
                Some("n"),
                Some(bump()),
            ),
        )],
        "bump-1-tx" => vec![
            begin(),
            db(
                Tx,
                insert(
                    "counters",
                    vec![row(&[("id", lit(1)), ("hits", lit(1))])],
                    Error,
                    None,
                    Some("n"),
                    Some(bump()),
                ),
            ),
            commit(),
        ],
        // Two statements per transaction: the second, multi-row, waits in
        // the middle for the other transaction's first (a deadlock).
        "x-then-yz-tx" => vec![
            begin(),
            db(
                Tx,
                insert("tags", vec![tag(1, 1, "x")], Error, None, Some("m"), None),
            ),
            db(
                Tx,
                insert(
                    "tags",
                    vec![tag(3, 1, "y"), tag(4, 1, "z")],
                    Error,
                    None,
                    Some("n"),
                    None,
                ),
            ),
            commit(),
        ],
        "y-then-wx-tx" => vec![
            begin(),
            db(
                Tx,
                insert("tags", vec![tag(5, 1, "y")], Error, None, Some("m"), None),
            ),
            db(
                Tx,
                insert(
                    "tags",
                    vec![tag(6, 1, "w"), tag(7, 1, "x")],
                    Error,
                    None,
                    Some("n"),
                    None,
                ),
            ),
            commit(),
        ],
        "y-then-wx-dn-tx" => vec![
            begin(),
            db(
                Tx,
                insert("tags", vec![tag(5, 1, "y")], Error, None, Some("m"), None),
            ),
            db(
                Tx,
                insert(
                    "tags",
                    vec![tag(6, 1, "w"), tag(7, 1, "x")],
                    DoNothing,
                    Some("r"),
                    Some("n"),
                    None,
                ),
            ),
            commit(),
        ],
        "order-delete-tx" => vec![
            begin(),
            db(Tx, delete("orders", eq(col("id"), lit(1)), "d")),
            commit(),
        ],
        other => panic!("unknown program {other}"),
    }
}

/// The multi-row workload pairs: (label, A, B).
pub fn insert_workloads() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("xy×yz", "tags-xy", "tags-yz"),
        ("xy-dn×yz-dn", "tags-xy-dn", "tags-yz-dn"),
        ("xy-tx×yz-dn", "tags-xy-tx", "tags-yz-dn"),
        ("dup-dn×x-tx", "tags-dup-dn", "tag-x-tx"),
        ("y-tx×xyz", "tag-y-tx", "tags-xyz"),
        ("y-rollback×xyz", "tag-y-rollback", "tags-xyz"),
        ("y-tx×xyz-dn", "tag-y-tx", "tags-xyz-dn"),
        ("items-tx×order-delete", "items-tx", "order-delete"),
        ("order-lock×items", "order-lock", "items"),
        ("items×item-tx", "items", "item-tx"),
        ("missing-order×items", "items-missing-order", "items"),
        ("bump-12×bump-21-tx", "bump-12", "bump-21-tx"),
        ("bump-11×bump-1-tx", "bump-11", "bump-1-tx"),
        ("x-then-yz×y-then-wx", "x-then-yz-tx", "y-then-wx-tx"),
        ("x-then-yz×y-then-wx-dn", "x-then-yz-tx", "y-then-wx-dn-tx"),
        ("items-tx×order-delete-tx", "items-tx", "order-delete-tx"),
    ]
}

pub fn insert_pair(iso: &str, a: &str, b: &str) -> Model {
    model(
        "multi-row-inserts",
        iso,
        insert_tables(),
        insert_initial(),
        vec![("A", insert_program(a)), ("B", insert_program(b))],
    )
}

// ---------------------------------------------------------------------
// Predicates with NULLs
// ---------------------------------------------------------------------

pub fn comment_tables() -> Json {
    json!([
        { "name": "comments", "columns": [
            {"name":"id","notNull":true,"type":"int"},
            {"name":"parent","type":"int"},
            {"name":"status","type":"text"}
          ], "primaryKey": ["id"] }
    ])
}

pub fn comment_initial() -> Json {
    json!({ "comments": [
        {"id": 1, "parent": null, "status": "visible"},
        {"id": 2, "parent": null, "status": null},
        {"id": 3, "parent": 1, "status": "hidden"},
        {"id": 4, "parent": 1, "status": null}
    ]})
}

/// `parentId === null ? isNull(t.parent) : eq(t.parent, parentId)` with the
/// test already evaluated (`which`), as the extractor lowers it.
pub fn conditional_parent(which: bool, parent: Option<i64>) -> Expr {
    or(
        and(lit(which), is_null(col("parent"))),
        and(
            Expr::not(lit(which)),
            eq(col("parent"), parent.map_or_else(null, lit)),
        ),
    )
}

pub fn predicate_program(name: &str) -> Vec<Step> {
    use Conn::{Pool, Tx};
    match name {
        // Drizzle: and(inArray(id, [1, null, 2]), or(isNull(status), eq(status, "visible")))
        "hide" => vec![db(
            Pool,
            update(
                "comments",
                and(
                    in_list("id", vec![lit(1), null(), lit(2)]),
                    or(is_null(col("status")), eq(col("status"), lit("visible"))),
                ),
                &[("status", lit("hidden"))],
                "n",
            ),
        )],
        "hide-tx" => {
            let mut p = vec![begin()];
            p.extend(predicate_program("hide").into_iter().map(|s| match s.op {
                Op::Db { stmt } => db(Tx, stmt.kind),
                other => step(other),
            }));
            p.push(commit());
            p
        }
        // not(eq(status, "hidden")): NULL statuses are not counted.
        "count-visible" => vec![db(
            Pool,
            count("comments", Expr::not(eq(col("status"), lit("hidden"))), "c"),
        )],
        "count-visible-tx" => vec![
            begin(),
            db(
                Tx,
                count("comments", Expr::not(eq(col("status"), lit("hidden"))), "c"),
            ),
            db(Tx, count("comments", is_null(col("status")), "d")),
            commit(),
        ],
        // notInArray(id, [2, null]): NULL for every row, nothing counted.
        "count-not-in-null" => vec![db(
            Pool,
            count(
                "comments",
                Expr::not(in_list("id", vec![lit(2), null()])),
                "c",
            ),
        )],
        // Detach the replies of comment 1: parent IN (1).
        "detach" => vec![db(
            Pool,
            update(
                "comments",
                in_list("parent", vec![lit(1)]),
                &[("parent", null())],
                "n",
            ),
        )],
        // prune(null, [2]): delete top-level comments except 2.
        "prune-top-tx" => vec![
            begin(),
            db(
                Tx,
                delete(
                    "comments",
                    and(
                        conditional_parent(true, None),
                        Expr::not(in_list("id", vec![lit(2)])),
                    ),
                    "n",
                ),
            ),
            commit(),
        ],
        // prune(1, []): delete the replies of comment 1 (empty notInArray).
        "prune-replies" => vec![db(
            Pool,
            delete(
                "comments",
                and(
                    conditional_parent(false, Some(1)),
                    Expr::not(in_list("id", vec![])),
                ),
                "n",
            ),
        )],
        // inArray(id, []): nothing.
        "hide-none" => vec![db(
            Pool,
            update(
                "comments",
                in_list("id", vec![]),
                &[("status", lit("hidden"))],
                "n",
            ),
        )],
        other => panic!("unknown program {other}"),
    }
}

pub fn predicate_workloads() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("hide×count-visible", "hide", "count-visible"),
        ("hide-tx×count-visible-tx", "hide-tx", "count-visible-tx"),
        ("hide-tx×detach", "hide-tx", "detach"),
        ("prune-top-tx×hide", "prune-top-tx", "hide"),
        (
            "prune-replies×count-not-in-null",
            "prune-replies",
            "count-not-in-null",
        ),
        ("detach×prune-top-tx", "detach", "prune-top-tx"),
        ("hide-none×count-visible", "hide-none", "count-visible"),
    ]
}

pub fn predicate_pair(iso: &str, a: &str, b: &str) -> Model {
    model(
        "predicates",
        iso,
        comment_tables(),
        comment_initial(),
        vec![("A", predicate_program(a)), ("B", predicate_program(b))],
    )
}

// ---------------------------------------------------------------------
// Timestamp and JSON columns (features `timestamps`, `json`)
// ---------------------------------------------------------------------

pub fn value_tables() -> Json {
    json!([
        { "name": "users", "columns": [
            {"name":"id","notNull":true,"type":"int"},
            {"name":"createdAt","notNull":true,"type":"timestamptz","default":"now"},
            {"name":"welcomedAt","type":"timestamptz"},
            {"name":"updatedAt","type":"timestamptz","onUpdate":"clock"}
          ], "primaryKey": ["id"] },
        { "name": "settings", "columns": [
            {"name":"userId","notNull":true,"type":"int"},
            {"name":"prefs","type":"jsonb"}
          ], "primaryKey": ["userId"] },
        { "name": "counters", "columns": [
            {"name":"name","notNull":true,"type":"text"},
            {"name":"hits","type":"int"},
            {"name":"updatedAt","type":"timestamp"}
          ], "primaryKey": ["name"] }
    ])
}

pub fn value_initial() -> Json {
    json!({
        "users": [{"id": 9, "createdAt": "2024-01-01T00:00:00.000Z"}],
        "settings": [{"userId": 9, "prefs": {"theme": "light", "tags": [1, 2]}}],
        "counters": [{"name": "a", "hits": 1, "updatedAt": "2024-01-02T03:04:05.678Z"}]
    })
}

fn clock(k: u32) -> Expr {
    Expr::Lit(commitrift_model::db::clock_placeholder(k))
}

pub fn value_program(name: &str) -> Vec<Step> {
    use Conn::{Pool, Tx};
    let welcome = |conn| {
        db(
            conn,
            update(
                "users",
                and(eq(col("id"), lit(9)), is_null(col("welcomedAt"))),
                &[("welcomedAt", clock(1))],
                "n",
            ),
        )
    };
    let user = |id: i64| row(&[("id", lit(id))]);
    match name {
        // The job handler: `set({ welcomedAt: new Date() })` where
        // `isNull(welcomedAt)`; `$onUpdate` stamps `updatedAt`.
        "welcome" => vec![welcome(Pool)],
        "welcome-tx" => vec![
            begin(),
            db(Tx, count("users", is_null(col("welcomedAt")), "c")),
            welcome(Tx),
            commit(),
        ],
        // Two users in one transaction: one `now()`.
        "signup-tx" => vec![
            begin(),
            db(
                Tx,
                insert("users", vec![user(1)], OnConflict::Error, None, None, None),
            ),
            db(
                Tx,
                insert(
                    "users",
                    vec![user(2), user(3)],
                    OnConflict::DoNothing,
                    Some("r"),
                    None,
                    None,
                ),
            ),
            commit(),
        ],
        "signup-2" => vec![db(
            Pool,
            insert(
                "users",
                vec![user(2)],
                OnConflict::DoNothing,
                Some("r"),
                None,
                None,
            ),
        )],
        // A JSON value written and read back.
        "settings" => vec![
            db(
                Pool,
                update(
                    "settings",
                    eq(col("userId"), lit(9)),
                    &[(
                        "prefs",
                        Expr::Object(vec![
                            ("theme".into(), lit("dark")),
                            ("tags".into(), Expr::Array(vec![lit(3)])),
                        ]),
                    )],
                    "n",
                ),
            ),
            db(
                Pool,
                insert(
                    "settings",
                    vec![row(&[("userId", lit(1))])],
                    OnConflict::DoNothing,
                    None,
                    None,
                    None,
                ),
            ),
        ],
        "settings-read-tx" => vec![
            begin(),
            db(
                Tx,
                StmtKind::Select {
                    table: "settings".into(),
                    pred: Expr::not(is_null(col("prefs"))),
                    lock: commitrift_model::ir::LockMode::None,
                    into: "s".into(),
                    joins: Vec::new(),
                    projection: None,
                },
            ),
            db(Tx, count("settings", is_null(col("prefs")), "c")),
            commit(),
        ],
        // A multi-row upsert whose `set` reads the clock once per statement.
        "touch-ab" => vec![db(
            Pool,
            insert(
                "counters",
                vec![
                    row(&[("name", lit("a")), ("hits", lit(1))]),
                    row(&[("name", lit("b")), ("hits", lit(1))]),
                ],
                OnConflict::Error,
                Some("r"),
                None,
                Some(Upsert {
                    target: vec!["name".into()],
                    set: vec![
                        ("hits".into(), Expr::bin(BinOp::Add, col("hits"), lit(1))),
                        ("updatedAt".into(), clock(2)),
                    ],
                    filter: None,
                }),
            ),
        )],
        "stale-tx" => vec![
            begin(),
            db(
                Tx,
                count(
                    "counters",
                    Expr::bin(BinOp::Lt, col("updatedAt"), lit("2024-06-01T00:00:00.000Z")),
                    "c",
                ),
            ),
            db(
                Tx,
                update(
                    "counters",
                    eq(col("name"), lit("a")),
                    &[("hits", lit(0))],
                    "n",
                ),
            ),
            commit(),
        ],
        other => panic!("unknown program {other}"),
    }
}

pub fn value_workloads() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("welcome×welcome", "welcome", "welcome"),
        ("welcome-tx×welcome", "welcome-tx", "welcome"),
        ("signup-tx×signup-2", "signup-tx", "signup-2"),
        ("settings×settings-read-tx", "settings", "settings-read-tx"),
        ("touch-ab×stale-tx", "touch-ab", "stale-tx"),
    ]
}

pub fn value_pair(iso: &str, a: &str, b: &str) -> Model {
    model_with(
        "values",
        iso,
        value_tables(),
        value_initial(),
        vec![("A", value_program(a)), ("B", value_program(b))],
        json!(["timestamps", "json"]),
    )
}

//! Foreign-key workloads (a `users` parent, a `workspaces` child
//! referencing it) shared by the semantic and PostgreSQL tests.

use midwrite_model::model::Model;
use midwrite_model::scenario::testing::model_from_json;
use serde_json::{Value, json};

pub fn fk_tables() -> Value {
    json!([
        { "name": "users", "columns": [
            {"name":"id","notNull":true,"type":"int"},
            {"name":"name","type":"text"},
            {"name":"email","type":"text"}
          ], "primaryKey": ["id"], "unique": [["email"]] },
        { "name": "workspaces", "columns": [
            {"name":"id","notNull":true,"type":"int"},
            {"name":"owner","type":"int"}
          ], "primaryKey": ["id"],
          "foreignKeys": [{ "columns": ["owner"], "references": "users", "referencedColumns": ["id"] }] }
    ])
}

/// Two actors over one parent row `users(1)` (and `users(3)`, owner of the
/// existing workspace 5).
pub fn fk_pair(iso: &str, a: &str, b: &str) -> Model {
    model_from_json(json!({
        "schemaVersion": 1,
        "name": "foreign-keys",
        "database": {
            "isolation": iso,
            "tables": fk_tables(),
            "initialState": {
                "users": [{"id": 1, "name": "a", "email": "a@x"}, {"id": 3, "name": "c", "email": "c@x"}],
                "workspaces": [{"id": 5, "owner": 3}]
            }
        },
        "actors": [
            { "name": "A", "function": "a", "program": a, "arguments": {} },
            { "name": "B", "function": "b", "program": b, "arguments": {} }
        ],
        "invariants": [{ "name": "true", "assert": { "eq": [0, 0] } }]
    }))
}

/// The Phase 12 `createWorkspaceLocked`: lock the owner, then insert.
pub const PARENT_FOR_UPDATE_TX: &str = "\
begin
let u = select users where @id == 1 for update
insert workspaces set @id = 10, @owner = 1
commit
";
pub const PARENT_FOR_UPDATE_ROLLBACK: &str = "\
begin
let u = select users where @id == 1 for update
rollback
";
/// The Phase 12 `createWorkspace` (unlocked): one autocommit insert.
pub const CHILD_AUTOCOMMIT: &str = "insert workspaces set @id = 20, @owner = 1\n";
pub const CHILD_TX: &str = "\
begin
insert workspaces set @id = 20, @owner = 1
commit
";
pub const CHILD_ROLLBACK: &str = "\
begin
insert workspaces set @id = 20, @owner = 1
rollback
";
pub const CHILD_REPOINT_TX: &str = "\
begin
update workspaces where @id == 5 set @owner = 1
commit
";
pub const PARENT_DELETE_TX: &str = "\
begin
delete users where @id == 1
commit
";
pub const PARENT_DELETE_ROLLBACK: &str = "\
begin
delete users where @id == 1
rollback
";
pub const PARENT_DELETE: &str = "delete users where @id == 1\n";
pub const PARENT_RENAME_TX: &str = "\
begin
update users where @id == 1 set @name = \"b\"
commit
";
pub const PARENT_RENAME: &str = "update users where @id == 1 set @name = \"b\"\n";
pub const PARENT_REKEY_TX: &str = "\
begin
update users where @id == 1 set @id = 2
commit
";
pub const PARENT_REKEY: &str = "update users where @id == 1 set @id = 2\n";
/// An update of another unique column (no foreign key refers to it): still
/// a key update for PostgreSQL (FOR UPDATE).
pub const PARENT_REEMAIL_TX: &str = "\
begin
update users where @id == 1 set @email = \"z@x\"
commit
";
pub const PARENT_REEMAIL: &str = "update users where @id == 1 set @email = \"z@x\"\n";
/// The same value: no key changes (FOR NO KEY UPDATE).
pub const PARENT_SAME_EMAIL_TX: &str = "\
begin
update users where @id == 1 set @email = \"a@x\"
commit
";
pub const PARENT_SHARE_TX: &str = "\
begin
let u = select users where @id == 1 for share
commit
";
pub const PARENT_KEY_SHARE_TX: &str = "\
begin
let u = select users where @id == 1 for key share
commit
";
pub const PARENT_NO_KEY_UPDATE_TX: &str = "\
begin
let u = select users where @id == 1 for no key update
commit
";
/// The referenced row is deleted before the transaction inserts (a
/// REPEATABLE READ snapshot taken first still sees it).
pub const SNAPSHOT_THEN_CHILD_TX: &str = "\
begin
let w = select workspaces where @id == 5
insert workspaces set @id = 20, @owner = 1
commit
";

/// The workload pairs of the matrix: (label, A, B).
pub fn fk_workloads() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("for-update×child", PARENT_FOR_UPDATE_TX, CHILD_AUTOCOMMIT),
        ("for-update×child-tx", PARENT_FOR_UPDATE_TX, CHILD_TX),
        (
            "for-update-rollback×child-tx",
            PARENT_FOR_UPDATE_ROLLBACK,
            CHILD_TX,
        ),
        ("delete×child-tx", PARENT_DELETE_TX, CHILD_TX),
        ("delete-rollback×child-tx", PARENT_DELETE_ROLLBACK, CHILD_TX),
        ("child-tx×delete", CHILD_TX, PARENT_DELETE),
        ("child-rollback×delete", CHILD_ROLLBACK, PARENT_DELETE),
        ("rename×child-tx", PARENT_RENAME_TX, CHILD_TX),
        ("rekey×child-tx", PARENT_REKEY_TX, CHILD_TX),
        ("child-tx×rekey", CHILD_TX, PARENT_REKEY),
        ("repoint×delete", CHILD_REPOINT_TX, PARENT_DELETE),
        ("share×rename", PARENT_SHARE_TX, PARENT_RENAME),
        ("key-share×rename", PARENT_KEY_SHARE_TX, PARENT_RENAME),
        ("key-share×rekey", PARENT_KEY_SHARE_TX, PARENT_REKEY),
        (
            "no-key-update×child",
            PARENT_NO_KEY_UPDATE_TX,
            CHILD_AUTOCOMMIT,
        ),
        (
            "snapshot-child×delete",
            SNAPSHOT_THEN_CHILD_TX,
            PARENT_DELETE,
        ),
        (
            "snapshot-child×rename",
            SNAPSHOT_THEN_CHILD_TX,
            PARENT_RENAME,
        ),
        ("reemail×child-tx", PARENT_REEMAIL_TX, CHILD_TX),
        ("same-email×child-tx", PARENT_SAME_EMAIL_TX, CHILD_TX),
        (
            "snapshot-child×reemail",
            SNAPSHOT_THEN_CHILD_TX,
            PARENT_REEMAIL,
        ),
    ]
}

/// `INSERT ... ON CONFLICT (target) DO UPDATE` workloads.
pub fn upsert_pair(iso: &str, a: &str, b: &str, existing: bool) -> Model {
    model_from_json(json!({
        "schemaVersion": 1,
        "name": "upserts",
        "database": {
            "isolation": iso,
            "tables": [
                { "name": "counters", "columns": [
                    {"name":"id","notNull":true,"type":"int"},
                    {"name":"hits","type":"int"}
                  ], "primaryKey": ["id"] },
                { "name": "handles", "columns": [
                    {"name":"handle","notNull":true,"type":"text"},
                    {"name":"owner","type":"text"}
                  ], "primaryKey": ["handle"] }
            ],
            "initialState": if existing {
                json!({ "counters": [{"id": 1, "hits": 5}], "handles": [{"handle": "x", "owner": null}] })
            } else {
                json!({})
            }
        },
        "actors": [
            { "name": "A", "function": "a", "program": a, "arguments": {"me": "a"} },
            { "name": "B", "function": "b", "program": b, "arguments": {"me": "b"} }
        ],
        "invariants": [{ "name": "true", "assert": { "eq": [0, 0] } }]
    }))
}

pub const COUNT_VIEW: &str =
    "insert counters set @id = 1, @hits = 1 on conflict (@id) do update set @hits = @hits + 1\n";
pub const COUNT_VIEW_TX: &str = "\
begin
let c = select counters where @id == 1
insert counters set @id = 1, @hits = 1 on conflict (@id) do update set @hits = @hits + excluded.hits
commit
";
pub const RESERVE: &str = "\
insert handles set @handle = \"x\", @owner = me on conflict (@handle) do update set @owner = excluded.owner returning r
";
pub const RESERVE_IF_FREE: &str = "\
insert handles set @handle = \"x\", @owner = me on conflict (@handle) do update set @owner = excluded.owner where @owner is null into n
";
pub const RESERVE_IF_FREE_TX: &str = "\
begin
let h = select handles where @handle == \"x\"
insert handles set @handle = \"x\", @owner = me on conflict (@handle) do update set @owner = excluded.owner where @owner is null into n
commit
";

pub fn upsert_workloads() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("count×count", COUNT_VIEW, COUNT_VIEW),
        ("count-tx×count", COUNT_VIEW_TX, COUNT_VIEW),
        ("count-tx×count-tx", COUNT_VIEW_TX, COUNT_VIEW_TX),
        ("reserve×reserve", RESERVE, RESERVE),
        (
            "reserve-if-free×reserve-if-free",
            RESERVE_IF_FREE,
            RESERVE_IF_FREE,
        ),
        ("reserve-if-free-tx×reserve", RESERVE_IF_FREE_TX, RESERVE),
    ]
}

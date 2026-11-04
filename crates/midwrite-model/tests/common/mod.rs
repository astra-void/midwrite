//! Workloads shared by the semantic, reduction and PostgreSQL tests.
#![allow(dead_code)]

pub mod adversarial;
pub mod fk;
pub mod minimize;
pub mod por_diff;
pub mod por_workloads;
pub mod scenarios_gen;

use midwrite_model::model::Model;
use midwrite_model::scenario::testing::model_from_json;
use serde_json::{Value, json};

pub struct Actor {
    pub name: &'static str,
    pub program: &'static str,
    pub args: Value,
    pub retry: Option<&'static str>,
    pub process: Option<&'static str>,
}

pub fn actor(name: &'static str, program: &'static str, args: Value) -> Actor {
    Actor {
        name,
        program,
        args,
        retry: None,
        process: None,
    }
}

pub fn build(
    iso: &str,
    tables: Value,
    initial: Value,
    actors: Vec<Actor>,
    invariants: Value,
    bounds: Value,
    extra: Value,
) -> Model {
    let actors: Vec<Value> = actors
        .into_iter()
        .map(|a| {
            let mut v = json!({
                "name": a.name, "function": "work", "program": a.program, "arguments": a.args
            });
            if let Some(r) = a.retry {
                v["retry"] = json!({ "trigger": r });
            }
            if let Some(p) = a.process {
                v["process"] = json!(p);
            }
            v
        })
        .collect();
    let mut doc = json!({
        "schemaVersion": 1,
        "name": "workload",
        "database": { "isolation": iso, "tables": tables, "initialState": initial },
        "actors": actors,
        "bounds": bounds,
        "invariants": invariants,
    });
    if let (Some(d), Some(e)) = (doc.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            d.insert(k.clone(), v.clone());
        }
    }
    model_from_json(doc)
}

pub const PURCHASE_AUTOCOMMIT: &str = "\
let product = select products where @id == pid
if product == null || product.stock <= 0 {
  fail \"out of stock\"
}
update products where @id == pid set @stock = product.stock - 1
insert orders set @id = oid, @product = pid
";

pub const PURCHASE_TX: &str = "\
begin
let product = select products where @id == pid
if product == null || product.stock <= 0 {
  fail \"out of stock\"
}
update products where @id == pid set @stock = product.stock - 1
insert orders set @id = oid, @product = pid
commit
";

pub const PURCHASE_FOR_UPDATE: &str = "\
begin
let product = select products where @id == pid for update
if product == null || product.stock <= 0 {
  fail \"out of stock\"
}
update products where @id == pid set @stock = product.stock - 1
insert orders set @id = oid, @product = pid
commit
";

pub const PURCHASE_ATOMIC: &str = "\
update products where @id == pid && @stock > 0 set @stock = @stock - 1 into n
if n == 0 {
  fail \"out of stock\"
}
insert orders set @id = oid, @product = pid
";

pub const PURCHASE_ATOMIC_TX: &str = "\
begin
update products where @id == pid && @stock > 0 set @stock = @stock - 1 into n
if n == 0 {
  fail \"out of stock\"
}
insert orders set @id = oid, @product = pid
commit
";

pub fn products_tables() -> Value {
    json!([
        { "name": "products", "columns": [{"name":"id","notNull":true,"type":"text"}, {"name":"stock","type":"int"}], "primaryKey": ["id"] },
        { "name": "orders", "columns": [{"name":"id","notNull":true,"type":"text"}, {"name":"product","type":"text"}], "primaryKey": ["id"] }
    ])
}

pub fn no_oversell() -> Value {
    json!([{
        "name": "no-oversell",
        "assert": { "le": [
            { "actors": { "function": "work", "outcome": "ok" } },
            { "initial": { "table": "products", "where": {"id": "p1"}, "column": "stock" } }
        ] }
    }])
}

/// Two purchasers of one unit of stock.
pub fn purchase(iso: &str, program: &'static str, bounds: Value) -> Model {
    build(
        iso,
        products_tables(),
        json!({ "products": [{"id": "p1", "stock": 1}], "orders": [] }),
        vec![
            actor("A", program, json!({"pid": "p1", "oid": "oA"})),
            actor("B", program, json!({"pid": "p1", "oid": "oB"})),
        ],
        no_oversell(),
        bounds,
        json!({}),
    )
}

pub const WRITE_SKEW: &str = "\
begin
let others = count doctors where @on_call == true && @id != me
if others < 1 {
  fail \"no other doctor on call\"
}
update doctors where @id == me set @on_call = false
commit
";

pub fn write_skew(iso: &str, bounds: Value) -> Model {
    build(
        iso,
        json!([{ "name": "doctors", "columns": [{"name":"id","notNull":true,"type":"text"}, {"name":"on_call","type":"bool"}], "primaryKey": ["id"] }]),
        json!({ "doctors": [{"id": "d1", "on_call": true}, {"id": "d2", "on_call": true}] }),
        vec![
            actor("A", WRITE_SKEW, json!({"me": "d1"})),
            actor("B", WRITE_SKEW, json!({"me": "d2"})),
        ],
        json!([{
            "name": "someone-on-call",
            "assert": { "ge": [ { "count": { "table": "doctors", "where": {"on_call": true} } }, 1 ] }
        }]),
        bounds,
        json!({}),
    )
}

pub const REDEEM_CHECK_THEN_ACT: &str = "\
let used = select redemptions where @code == code
if used != null {
  fail \"already redeemed\"
}
insert redemptions set @id = rid, @code = code
";

pub fn redeem(iso: &str, unique_code: bool, bounds: Value) -> Model {
    let table = if unique_code {
        json!({ "name": "redemptions", "columns": [{"name":"id","notNull":true,"type":"text"}, {"name":"code","type":"text"}], "primaryKey": ["id"], "unique": [["code"]] })
    } else {
        json!({ "name": "redemptions", "columns": [{"name":"id","notNull":true,"type":"text"}, {"name":"code","type":"text"}], "primaryKey": ["id"] })
    };
    build(
        iso,
        json!([table]),
        json!({ "redemptions": [] }),
        vec![
            actor(
                "A",
                REDEEM_CHECK_THEN_ACT,
                json!({"code": "SAVE10", "rid": "rA"}),
            ),
            actor(
                "B",
                REDEEM_CHECK_THEN_ACT,
                json!({"code": "SAVE10", "rid": "rB"}),
            ),
        ],
        json!([{
            "name": "redeemed-at-most-once",
            "assert": { "le": [ { "count": { "table": "redemptions", "where": {"code": "SAVE10"} } }, 1 ] }
        }]),
        bounds,
        json!({}),
    )
}

pub const DEADLOCK_AB: &str = "\
begin
update t where @id == 1 set @v = @v + 1
update t where @id == 2 set @v = @v + 1
commit
";
pub const DEADLOCK_BA: &str = "\
begin
update t where @id == 2 set @v = @v + 10
update t where @id == 1 set @v = @v + 10
commit
";

pub fn deadlock(iso: &str, bounds: Value) -> Model {
    build(
        iso,
        json!([{ "name": "t", "columns": [{"name":"id","notNull":true,"type":"int"}, {"name":"v","type":"int"}], "primaryKey": ["id"] }]),
        json!({ "t": [{"id": 1, "v": 0}, {"id": 2, "v": 0}] }),
        vec![
            actor("A", DEADLOCK_AB, json!({})),
            actor("B", DEADLOCK_BA, json!({})),
        ],
        json!([{
            "name": "both-rows-exist",
            "assert": { "eq": [ { "count": { "table": "t", "where": {} } }, 2 ] }
        }]),
        bounds,
        json!({}),
    )
}

// ---- insert / unique / constraint workloads ---------------------------

pub const INSERT_DO_NOTHING_AFTER_READ: &str = "\
begin
let seen = count redemptions where @code == code
insert redemptions set @id = code, @code = code on conflict do nothing into n
commit
";

pub const INSERT_DO_NOTHING: &str = "\
begin
insert redemptions set @id = code, @code = code on conflict do nothing into n
commit
";

pub const INSERT_THEN_ROLLBACK: &str = "\
begin
insert redemptions set @id = rid, @code = code
rollback
";

pub const INSERT_THEN_COMMIT: &str = "\
begin
insert redemptions set @id = rid, @code = code
commit
";

pub fn redemptions_tables(unique_code: bool) -> serde_json::Value {
    let mut t = json!({ "name": "redemptions", "columns": [{"name":"id","notNull":true,"type":"text"}, {"name":"code","type":"text"}], "primaryKey": ["id"] });
    if unique_code {
        t["unique"] = json!([["code"]]);
    }
    json!([t])
}

pub fn redemption_count_invariant() -> serde_json::Value {
    json!([{
        "name": "at-most-one",
        "assert": { "le": [ { "count": { "table": "redemptions", "where": {"code": "X"} } }, 1 ] }
    }])
}

pub fn insert_race(iso: &str, a: &'static str, b: &'static str, unique: bool) -> Model {
    build(
        iso,
        redemptions_tables(unique),
        json!({ "redemptions": [] }),
        vec![
            actor("A", a, json!({"code": "X", "rid": "rA"})),
            actor("B", b, json!({"code": "X", "rid": "rB"})),
        ],
        redemption_count_invariant(),
        json!({}),
        json!({}),
    )
}

pub const DECREMENT_UNGUARDED: &str = "\
begin
update products where @id == pid set @stock = @stock - 1
commit
";

pub fn check_constraint(iso: &str) -> Model {
    build(
        iso,
        json!([{ "name": "products", "columns": [{"name":"id","notNull":true,"type":"text"}, {"name":"stock","type":"int"}], "primaryKey": ["id"], "checks": ["@stock >= 0"] }]),
        json!({ "products": [{"id": "p1", "stock": 1}] }),
        vec![
            actor("A", DECREMENT_UNGUARDED, json!({"pid": "p1"})),
            actor("B", DECREMENT_UNGUARDED, json!({"pid": "p1"})),
        ],
        json!([{ "name": "stock-nonnegative", "assert": { "ge": [ { "row": { "table": "products", "where": {"id": "p1"}, "column": "stock" } }, 0 ] } }]),
        json!({}),
        json!({}),
    )
}

pub const DELETE_ROW: &str = "\
begin
delete products where @id == pid into n
commit
";

pub const UPDATE_ROW: &str = "\
begin
update products where @id == pid && @stock > 0 set @stock = @stock + 5 into n
commit
";

pub fn delete_vs_update(iso: &str) -> Model {
    build(
        iso,
        json!([{ "name": "products", "columns": [{"name":"id","notNull":true,"type":"text"}, {"name":"stock","type":"int"}], "primaryKey": ["id"] }]),
        json!({ "products": [{"id": "p1", "stock": 1}] }),
        vec![
            actor("A", DELETE_ROW, json!({"pid": "p1"})),
            actor("B", UPDATE_ROW, json!({"pid": "p1"})),
        ],
        json!([{ "name": "trivial", "assert": { "ge": [ { "count": { "table": "products", "where": {} } }, 0 ] } }]),
        json!({}),
        json!({}),
    )
}

pub const CLAIM_SKIP_LOCKED: &str = "\
begin
let job = select jobs where @status == 'pending' for update skip locked
if job == null {
  fail \"no job\"
}
update jobs where @id == job.id set @status = 'done'
commit
";

pub const CLAIM_NOWAIT: &str = "\
begin
let job = select jobs where @status == 'pending' for update nowait
if job == null {
  fail \"no job\"
}
update jobs where @id == job.id set @status = 'done'
commit
";

pub const CLAIM_PLAIN: &str = "\
begin
let job = select jobs where @status == 'pending' for update
if job == null {
  fail \"no job\"
}
update jobs where @id == job.id set @status = 'done'
commit
";

pub fn jobs(iso: &str, program: &'static str, n_jobs: usize) -> Model {
    let rows: Vec<_> = (1..=n_jobs)
        .map(|i| json!({"id": format!("j{i}"), "status": "pending"}))
        .collect();
    build(
        iso,
        json!([{ "name": "jobs", "columns": [{"name":"id","notNull":true,"type":"text"}, {"name":"status","type":"text"}], "primaryKey": ["id"] }]),
        json!({ "jobs": rows }),
        vec![
            actor("A", program, json!({})),
            actor("B", program, json!({})),
        ],
        json!([{ "name": "each-job-claimed-once", "assert": { "le": [ { "actors": { "function": "work", "outcome": "ok" } }, n_jobs ] } }]),
        json!({}),
        json!({}),
    )
}

pub const PAY_AND_PUBLISH: &str = "\
begin
update accounts where @id == acct set @balance = @balance - 10
commit
external payment key req set amount = 10
";

pub fn crash_after_commit(
    max_crashes: u32,
    retry: Option<&'static str>,
    max_retries: u32,
) -> Model {
    let mut a = actor("A", PAY_AND_PUBLISH, json!({"acct": "a1", "req": "r1"}));
    a.retry = retry;
    build(
        "read-committed",
        json!([{ "name": "accounts", "columns": [{"name":"id","notNull":true,"type":"text"}, {"name":"balance","type":"int"}], "primaryKey": ["id"] }]),
        json!({ "accounts": [{"id": "a1", "balance": 100}] }),
        vec![a],
        json!([{ "name": "charged-once", "assert": { "le": [ { "externals": { "channel": "payment" } }, 1 ] } }]),
        json!({ "maxCrashes": max_crashes, "maxRetries": max_retries }),
        json!({ "channels": { "payment": { "dedup": "none" } } }),
    )
}

pub const TX_RETRY: &str = "\
begin isolation repeatable-read retry 2
let product = select products where @id == pid
update products where @id == pid set @stock = product.stock - 1
commit
";

pub fn tx_retry(max_retries: u32) -> Model {
    build(
        "read-committed",
        json!([{ "name": "products", "columns": [{"name":"id","notNull":true,"type":"text"}, {"name":"stock","type":"int"}], "primaryKey": ["id"] }]),
        json!({ "products": [{"id": "p1", "stock": 5}] }),
        vec![
            actor("A", TX_RETRY, json!({"pid": "p1"})),
            actor("B", TX_RETRY, json!({"pid": "p1"})),
        ],
        json!([{ "name": "both-decrements-applied", "assert": { "eq": [ { "row": { "table": "products", "where": {"id": "p1"}, "column": "stock" } }, 3 ] } }]),
        json!({ "maxRetries": max_retries }),
        json!({}),
    )
}

pub const SPAWN_TWO_UPDATES: &str = "\
spawn {
  update t where @id == 1 set @v = @v + 1
} {
  update t where @id == 1 set @v = @v + 10
}
join
";

pub fn concurrent_tasks() -> Model {
    build(
        "read-committed",
        json!([{ "name": "t", "columns": [{"name":"id","notNull":true,"type":"int"}, {"name":"v","type":"int"}], "primaryKey": ["id"] }]),
        json!({ "t": [{"id": 1, "v": 0}] }),
        vec![actor("A", SPAWN_TWO_UPDATES, json!({}))],
        json!([{ "name": "both-applied", "assert": { "eq": [ { "row": { "table": "t", "where": {"id": 1}, "column": "v" } }, 11 ] } }]),
        json!({}),
        json!({}),
    )
}

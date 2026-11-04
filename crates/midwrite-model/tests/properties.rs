//! Transition invariants, exploration properties and metamorphic tests of
//! the model checker itself.

mod common;

use common::*;
use midwrite_model::engine::{Recorder, apply, enabled, initial_state};
use midwrite_model::explore::{ExploreOptions, Verdict, explore, explore_with};
use midwrite_model::model::{Model, Strategy};
use midwrite_model::state::{Outcome, State, TKind, Transition};
use midwrite_model::value::Value;
use midwrite_model::witness::{build_witness, check_schedule, context_switches, replay};
use serde_json::json;

/// Run the next Step/Resume of the named actors in order.
fn run(model: &Model, actors: &[usize]) -> State {
    let mut s = initial_state(model);
    for &a in actors {
        let t = enabled(model, &s)
            .into_iter()
            .find(|t| t.actor as usize == a && matches!(t.kind, TKind::Step | TKind::Resume))
            .unwrap_or_else(|| panic!("actor {a} has no enabled step"));
        s = apply(model, &s, t, &mut Recorder::off()).unwrap().state;
    }
    s
}

fn committed(model: &Model, s: &State, table: &str, id: &str, col: &str) -> Value {
    let t = model.schema.index(table).unwrap();
    s.committed_rows(model, t)
        .into_iter()
        .find(|r| r.get("id") == Some(&Value::str(id)))
        .and_then(|r| r.get(col).cloned())
        .unwrap_or(Value::Null)
}

fn two(iso: &str, a: &'static str, b: &'static str) -> Model {
    build(
        iso,
        products_tables(),
        json!({ "products": [{"id": "p1", "stock": 5}], "orders": [] }),
        vec![
            actor("A", a, json!({"pid": "p1"})),
            actor("B", b, json!({"pid": "p1"})),
        ],
        json!([{ "name": "t", "assert": { "ge": [ { "count": { "table": "products", "where": {} } }, 0 ] } }]),
        json!({ "maxCrashes": 1, "maxRetries": 1 }),
        json!({ "channels": { "ch": { "dedup": "none" } } }),
    )
}

const WRITE_THEN_COMMIT: &str = "begin\nupdate products where @id == pid set @stock = 9\ncommit\n";
const WRITE_THEN_ROLLBACK: &str =
    "begin\nupdate products where @id == pid set @stock = 9\nrollback\n";
const READ_TWICE: &str = "begin\nlet x = select products where @id == pid\nlet y = select products where @id == pid\ncommit\n";

#[test]
fn transaction_local_writes_are_not_visible_before_commit() {
    let m = two("read-committed", WRITE_THEN_COMMIT, READ_TWICE);
    // A: begin, update (uncommitted). B: begin, read.
    let s = run(&m, &[0, 0, 1, 1]);
    assert_eq!(
        s.actors[1].locals["x"],
        Value::Row(
            [
                ("id".into(), Value::str("p1")),
                ("stock".into(), Value::Int(5))
            ]
            .into()
        )
    );
    assert_eq!(committed(&m, &s, "products", "p1", "stock"), Value::Int(5));
}

#[test]
fn commit_persists_and_releases_locks() {
    let m = two("read-committed", WRITE_THEN_COMMIT, READ_TWICE);
    let s = run(&m, &[0, 0]);
    assert!(!s.store.locks.is_empty(), "the update holds a row lock");
    let s = run(&m, &[0, 0, 0]);
    assert!(s.store.locks.is_empty(), "commit releases the lock");
    assert_eq!(committed(&m, &s, "products", "p1", "stock"), Value::Int(9));
}

#[test]
fn rollback_discards_writes_and_releases_locks() {
    let m = two("read-committed", WRITE_THEN_ROLLBACK, READ_TWICE);
    let s = run(&m, &[0, 0, 0]);
    assert!(s.store.locks.is_empty());
    assert_eq!(committed(&m, &s, "products", "p1", "stock"), Value::Int(5));
    assert!(s.txns.is_empty());
}

#[test]
fn snapshot_visibility_follows_the_isolation_level() {
    // B reads, A commits a write, B reads again.
    for (iso, second) in [("read-committed", 9), ("repeatable-read", 5)] {
        let m = two(iso, WRITE_THEN_COMMIT, READ_TWICE);
        let s = run(&m, &[1, 1, 0, 0, 0, 1]);
        let stock = |v: &Value| match v {
            Value::Row(r) => r["stock"].clone(),
            _ => Value::Null,
        };
        assert_eq!(stock(&s.actors[1].locals["x"]), Value::Int(5), "{iso}");
        assert_eq!(stock(&s.actors[1].locals["y"]), Value::Int(second), "{iso}");
    }
}

#[test]
fn program_counters_advance_one_visible_operation_per_step() {
    let m = two("read-committed", WRITE_THEN_COMMIT, READ_TWICE);
    let s0 = initial_state(&m);
    let s1 = run(&m, &[0]);
    assert_eq!(s0.actors[0].threads[0].pc, 0);
    assert_eq!(s1.actors[0].threads[0].pc, 1);
    assert_eq!(s1.actors[1].threads[0].pc, 0);
}

#[test]
fn a_crash_keeps_committed_state_and_performed_effects() {
    let program = "begin\nupdate products where @id == pid set @stock = 9\ncommit\nexternal ch\nbegin\nupdate products where @id == pid set @stock = 1\ncommit\n";
    let m = two("read-committed", program, READ_TWICE);
    // commit, external, begin, update (uncommitted), then crash
    let mut s = run(&m, &[0, 0, 0, 0, 0, 0]);
    let crash = enabled(&m, &s)
        .into_iter()
        .find(|t| t.kind == TKind::Crash && t.actor == 0)
        .unwrap();
    s = apply(&m, &s, crash, &mut Recorder::off()).unwrap().state;
    assert_eq!(s.actors[0].outcome, Outcome::Crashed);
    assert_eq!(
        committed(&m, &s, "products", "p1", "stock"),
        Value::Int(9),
        "committed data survives"
    );
    assert_eq!(
        s.externals.len(),
        1,
        "a performed external effect is not undone"
    );
    assert!(
        s.store.locks.is_empty(),
        "the uncommitted transaction was rolled back"
    );
}

#[test]
fn a_retry_does_not_erase_earlier_external_effects() {
    let mut m = two("read-committed", "external ch\nfail \"boom\"\n", READ_TWICE);
    m.actors[0].retry = Some(midwrite_model::ir::OperationRetry {
        trigger: midwrite_model::ir::RetryTrigger::OnFailure,
        guaranteed: false,
    });
    let s = run(&m, &[0]);
    assert!(matches!(s.actors[0].outcome, Outcome::Failed(_)));
    let retry = enabled(&m, &s)
        .into_iter()
        .find(|t| t.kind == TKind::Retry)
        .unwrap();
    let s = apply(&m, &s, retry, &mut Recorder::off()).unwrap().state;
    let s = apply(
        &m,
        &s,
        Transition {
            kind: TKind::Step,
            actor: 0,
            thread: 0,
        },
        &mut Recorder::off(),
    )
    .unwrap()
    .state;
    assert_eq!(s.externals.len(), 2);
    assert_eq!(s.externals[0].attempt, 0);
    assert_eq!(s.externals[1].attempt, 1);
}

#[test]
fn exploration_is_deterministic() {
    let m = purchase("read-committed", PURCHASE_TX, json!({}));
    let a = explore(&m);
    let b = explore(&m);
    assert_eq!(a.metrics.states, b.metrics.states);
    assert_eq!(a.metrics.transitions, b.metrics.transitions);
    assert_eq!(
        a.counterexample.unwrap().schedule,
        b.counterexample.unwrap().schedule
    );
}

#[test]
fn state_hashing_is_stable_and_merges_commuted_independent_commits() {
    let m = build(
        "read-committed",
        json!([{ "name": "t", "columns": [{"name":"id","notNull":true,"type":"int"}, {"name":"v","type":"int"}], "primaryKey": ["id"] }]),
        json!({ "t": [{"id": 1, "v": 0}, {"id": 2, "v": 0}] }),
        vec![
            actor(
                "A",
                "begin\nupdate t where @id == 1 set @v = 1\ncommit\n",
                json!({}),
            ),
            actor(
                "B",
                "begin\nupdate t where @id == 2 set @v = 2\ncommit\n",
                json!({}),
            ),
        ],
        json!([{ "name": "t", "assert": { "ge": [ { "count": { "table": "t", "where": {} } }, 0 ] } }]),
        json!({}),
        json!({}),
    );
    let ab = run(&m, &[0, 0, 0, 1, 1, 1]);
    let ba = run(&m, &[1, 1, 1, 0, 0, 0]);
    assert_eq!(ab.canonical(&m), ab.clone().canonical(&m));
    // Different commit sequence numbers, same future behavior: one state.
    assert_ne!(
        ab.store
            .rows
            .values()
            .map(|c| c.last().unwrap().commit)
            .collect::<Vec<_>>(),
        ba.store
            .rows
            .values()
            .map(|c| c.last().unwrap().commit)
            .collect::<Vec<_>>()
    );
    assert_eq!(ab.canonical(&m), ba.canonical(&m));
}

#[test]
fn states_with_equal_data_but_different_futures_are_not_merged() {
    // Same database values, but one actor has not yet run its update.
    let m = two("read-committed", WRITE_THEN_COMMIT, READ_TWICE);
    let a = run(&m, &[0]);
    let b = run(&m, &[1]);
    assert_eq!(
        committed(&m, &a, "products", "p1", "stock"),
        committed(&m, &b, "products", "p1", "stock")
    );
    assert_ne!(a.canonical(&m), b.canonical(&m));
}

#[test]
fn hitting_a_state_limit_is_never_reported_as_exhaustive() {
    let mut m = purchase("read-committed", PURCHASE_ATOMIC_TX, json!({}));
    let full = explore(&m);
    assert_eq!(full.verdict, Verdict::NoCounterexampleWithinBounds);
    m.bounds.max_states = 3;
    let cut = explore(&m);
    assert_eq!(cut.verdict, Verdict::Inconclusive);
    assert!(cut.truncation.max_states);
}

#[test]
fn counterexamples_replay_and_minimized_witnesses_still_violate() {
    for (por, strategy) in [
        (true, Strategy::Dfs),
        (false, Strategy::Bfs),
        (false, Strategy::Dfs),
    ] {
        let m = purchase("read-committed", PURCHASE_TX, json!({}));
        let mut o = ExploreOptions::from_model(&m);
        o.por = por;
        o.strategy = strategy;
        let ce = explore_with(&m, &o).counterexample.unwrap();
        assert!(replay(&m, &ce.schedule, false).is_ok());
        let w = build_witness(&m, &ce, true).unwrap();
        assert!(check_schedule(&m, &w.schedule, &ce.invariant).is_some());
        let unmin = check_schedule(&m, &ce.schedule, &ce.invariant).unwrap();
        assert!(w.schedule.len() <= unmin.len());
        assert!(context_switches(&w.schedule) <= context_switches(&unmin));
    }
}

#[test]
fn an_invalid_schedule_is_rejected_by_replay() {
    let m = purchase("read-committed", PURCHASE_FOR_UPDATE, json!({}));
    // B cannot resume before it is blocked.
    let bad = [Transition {
        kind: TKind::Resume,
        actor: 1,
        thread: 0,
    }];
    assert!(replay(&m, &bad, false).is_err());
}

// ---- metamorphic tests ------------------------------------------------

fn summary(m: &Model) -> (Verdict, u64) {
    let r = explore(m);
    (r.verdict, r.metrics.states)
}

#[test]
fn swapping_actor_identities_preserves_the_result() {
    let m1 = purchase("read-committed", PURCHASE_TX, json!({}));
    let m2 = build(
        "read-committed",
        products_tables(),
        json!({ "products": [{"id": "p1", "stock": 1}], "orders": [] }),
        vec![
            actor("B", PURCHASE_TX, json!({"pid": "p1", "oid": "oB"})),
            actor("A", PURCHASE_TX, json!({"pid": "p1", "oid": "oA"})),
        ],
        no_oversell(),
        json!({}),
        json!({}),
    );
    assert_eq!(summary(&m1), summary(&m2));
}

#[test]
fn renaming_resources_preserves_the_result() {
    let renamed = "\
begin
let item = select items where @key == k
if item == null || item.qty <= 0 {
  fail \"out of stock\"
}
update items where @key == k set @qty = item.qty - 1
insert sales set @key = o, @item = k
commit
";
    let m2 = build(
        "read-committed",
        json!([
            { "name": "items", "columns": [{"name":"key","notNull":true}, "qty"], "primaryKey": ["key"] },
            { "name": "sales", "columns": [{"name":"key","notNull":true}, "item"], "primaryKey": ["key"] }
        ]),
        json!({ "items": [{"key": "p1", "qty": 1}], "sales": [] }),
        vec![
            actor("A", renamed, json!({"k": "p1", "o": "oA"})),
            actor("B", renamed, json!({"k": "p1", "o": "oB"})),
        ],
        json!([{ "name": "n", "assert": { "le": [ { "actors": { "function": "work", "outcome": "ok" } },
            { "initial": { "table": "items", "where": {"key": "p1"}, "column": "qty" } } ] } }]),
        json!({}),
        json!({}),
    );
    assert_eq!(
        summary(&purchase("read-committed", PURCHASE_TX, json!({}))),
        summary(&m2)
    );
}

#[test]
fn adding_pure_computation_or_splitting_it_preserves_the_verdict() {
    let with_pure = "\
begin
let product = select products where @id == pid
let unused = 1 + 2
let low = product == null || product.stock <= 0
if low {
  fail \"out of stock\"
}
let next = product.stock - 1
update products where @id == pid set @stock = next
insert orders set @id = oid, @product = pid
commit
";
    let base = summary(&purchase("read-committed", PURCHASE_TX, json!({})));
    let other = summary(&purchase("read-committed", with_pure, json!({})));
    assert_eq!(base.0, other.0);
    // Local computation is not a scheduling point: same number of states.
    assert_eq!(base.1, other.1);
}

#[test]
fn reordering_independent_reads_preserves_the_verdict() {
    let ab = "begin\nlet a = count orders where @id == oid\nlet product = select products where @id == pid\nif product == null || product.stock <= 0 {\n  fail \"x\"\n}\nupdate products where @id == pid set @stock = product.stock - 1\ninsert orders set @id = oid, @product = pid\ncommit\n";
    let ba = "begin\nlet product = select products where @id == pid\nlet a = count orders where @id == oid\nif product == null || product.stock <= 0 {\n  fail \"x\"\n}\nupdate products where @id == pid set @stock = product.stock - 1\ninsert orders set @id = oid, @product = pid\ncommit\n";
    assert_eq!(
        summary(&purchase("read-committed", ab, json!({}))).0,
        summary(&purchase("read-committed", ba, json!({}))).0
    );
}

#[test]
fn behavior_changing_transformations_change_the_verdict_as_expected() {
    // Adding a unique constraint removes the duplicate redemption.
    assert_eq!(
        explore(&redeem("read-committed", false, json!({}))).verdict,
        Verdict::CounterexampleFound
    );
    assert_eq!(
        explore(&redeem("read-committed", true, json!({}))).verdict,
        Verdict::NoCounterexampleWithinBounds
    );
    // Raising the isolation level removes the lost update (serialization failure).
    assert_eq!(
        explore(&purchase("read-committed", PURCHASE_TX, json!({}))).verdict,
        Verdict::CounterexampleFound
    );
    assert_eq!(
        explore(&purchase("repeatable-read", PURCHASE_TX, json!({}))).verdict,
        Verdict::NoCounterexampleWithinBounds
    );
    // Replacing read-modify-write with an atomic conditional update.
    assert_eq!(
        explore(&purchase("read-committed", PURCHASE_ATOMIC, json!({}))).verdict,
        Verdict::NoCounterexampleWithinBounds
    );
    // Write skew: present under REPEATABLE READ, prevented by SERIALIZABLE.
    assert_eq!(
        explore(&write_skew("repeatable-read", json!({}))).verdict,
        Verdict::CounterexampleFound
    );
    assert_eq!(
        explore(&write_skew("serializable", json!({}))).verdict,
        Verdict::NoCounterexampleWithinBounds
    );
}

#[test]
fn outbox_versus_dual_write_under_crashes() {
    // Dual write: commit the order, then publish; a crash in between loses
    // the event even though the relay is retried.
    let direct =
        "begin\ninsert orders set @id = oid, @product = 'p1'\ncommit\nexternal events key oid\n";
    // Outbox: the event row commits with the order; a relay publishes it.
    let producer = "begin\ninsert orders set @id = oid, @product = 'p1'\ninsert outbox set @id = oid, @sent = false\ncommit\n";
    let relay = "begin\nlet e = select outbox where @sent == false for update skip locked\nif e == null {\n  fail \"nothing to relay\"\n}\nexternal events key e.id\nupdate outbox where @id == e.id set @sent = true\ncommit\n";
    let tables = json!([
        { "name": "orders", "columns": [{"name":"id","notNull":true}, "product"], "primaryKey": ["id"] },
        { "name": "outbox", "columns": [{"name":"id","notNull":true}, {"name":"sent","type":"bool"}], "primaryKey": ["id"] }
    ]);
    // What an outbox guarantees: an event of a committed order is never
    // lost — it was published or is still pending in the outbox. (Whether
    // the relay ever runs late enough to publish it is a liveness question
    // the bounded model does not answer: a bounded relay may give up.)
    let invariant = json!([{ "name": "no-lost-event", "assert": { "implies": [
        { "ge": [ { "count": { "table": "orders", "where": {} } }, 1 ] },
        { "or": [
            { "ge": [ { "externals": { "channel": "events" } }, 1 ] },
            { "ge": [ { "count": { "table": "outbox", "where": { "sent": false } } }, 1 ] }
        ] } ] } }]);
    let mut d = actor("P", direct, json!({"oid": "o1"}));
    d.retry = None;
    let dual = build(
        "read-committed",
        tables.clone(),
        json!({}),
        vec![d],
        invariant.clone(),
        json!({ "maxCrashes": 1 }),
        json!({ "channels": { "events": { "dedup": "none" } } }),
    );
    assert_eq!(explore(&dual).verdict, Verdict::CounterexampleFound);
    let mut r = actor("Relay", relay, json!({}));
    r.process = Some("relay");
    let mut p = actor("P", producer, json!({"oid": "o1"}));
    p.process = Some("api");
    // The relay runs after the producer here (a guaranteed, retried job):
    // modeled by giving it a guaranteed retry on failure or crash.
    let mut doc_relay = r;
    doc_relay.retry = Some("onFailureOrCrash");
    let outbox = build_guaranteed(tables, vec![p, doc_relay], invariant);
    let r = explore(&outbox);
    assert_eq!(r.verdict, Verdict::NoCounterexampleWithinBounds);
}

fn build_guaranteed(
    tables: serde_json::Value,
    actors: Vec<Actor>,
    invariant: serde_json::Value,
) -> Model {
    let mut m = build(
        "read-committed",
        tables,
        json!({}),
        actors,
        invariant,
        json!({ "maxCrashes": 1, "maxRetries": 3 }),
        json!({ "channels": { "events": { "dedup": "none" } } }),
    );
    for a in &mut m.actors {
        if let Some(r) = &mut a.retry {
            r.guaranteed = true;
        }
    }
    m
}

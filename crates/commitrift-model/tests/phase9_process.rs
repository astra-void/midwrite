//! Phase 9: tasks that share a process and a bounded connection pool.
//!
//! * a process crash ends every task of the process;
//! * a pool of N connections serves requests first come first served; a
//!   task that needs a connection and finds none waits;
//! * a state in which nothing can proceed is classified: pool starvation,
//!   an application-level wait cycle (no database-level deadlock), versus a
//!   PostgreSQL deadlock, which the server resolves.

mod common;

use commitrift_model::engine::{Recorder, apply, enabled, initial_state};
use commitrift_model::explore::{
    ExploreOptions, StuckKind, Verdict, audit_independence, explore, explore_with,
};
use commitrift_model::model::{Model, Strategy};
use commitrift_model::state::{Lease, Outcome, State, TKind};
use common::*;
use serde_json::json;

fn t_table() -> serde_json::Value {
    json!([{
        "name": "t",
        "columns": [{"name":"id","notNull":true,"type":"int"}, {"name":"v","type":"int"}],
        "primaryKey": ["id"]
    }])
}

fn rows() -> serde_json::Value {
    json!({ "t": [{"id": 1, "v": 0}, {"id": 2, "v": 0}] })
}

fn unconstrained() -> serde_json::Value {
    json!([{ "name": "any", "when": "final", "assert": { "ge": [ { "count": { "table": "t", "where": {} } }, 0 ] } }])
}

fn model(actors: Vec<Actor>, extra: serde_json::Value, bounds: serde_json::Value) -> Model {
    build(
        "read-committed",
        t_table(),
        rows(),
        actors,
        unconstrained(),
        bounds,
        extra,
    )
}

fn step_of(m: &Model, s: &State, actor: u16) -> State {
    let t = enabled(m, s)
        .into_iter()
        .find(|t| t.kind == TKind::Step && t.actor == actor)
        .unwrap_or_else(|| panic!("actor {actor} has no enabled step"));
    apply(m, s, t, &mut Recorder::off()).unwrap().state
}

const TX_ONLY: &str = "begin\nupdate t where @id == 1 set @v = @v + 1\ncommit\n";

/// The parent holds the only connection in its transaction and waits for a
/// child that needs a connection of its own.
const PARENT_CHILD: &str = "begin\nupdate t where @id == 1 set @v = 1\nspawn {\n  pool update t where @id == 2 set @v = 5\n}\njoin\ncommit\n";

fn shared(name: &'static str, program: &'static str) -> Actor {
    let mut a = actor(name, program, json!({}));
    a.process = Some("P");
    a
}

#[test]
fn a_pool_of_one_starves_a_task_that_waits_for_its_own_parent() {
    let m = model(
        vec![shared("A", PARENT_CHILD)],
        json!({ "pools": { "P": 1 } }),
        json!({"maxSteps": 30}),
    );
    let r = explore(&m);
    assert_eq!(r.verdict, Verdict::CounterexampleFound, "{:?}", r.stuck);
    assert_eq!(r.counterexample.as_ref().unwrap().invariant, "termination");
    assert!(r.stuck.iter().all(|s| s.kind == StuckKind::PoolStarvation));
    // The witness replays to the stuck state.
    let ce = r.counterexample.unwrap();
    let w = commitrift_model::witness::build_witness(&m, &ce, true).expect("witness");
    assert!(
        !w.final_state.quiescent,
        "the witness ends in a stuck state"
    );
    // With a second connection the same code terminates.
    let m2 = model(
        vec![shared("A", PARENT_CHILD)],
        json!({ "pools": { "P": 2 } }),
        json!({"maxSteps": 30}),
    );
    let r2 = explore(&m2);
    assert_eq!(r2.verdict, Verdict::NoCounterexampleWithinBounds);
    assert!(r2.stuck.is_empty());
    // And without a bounded pool (the Phase 8 semantics).
    let m3 = model(
        vec![shared("A", PARENT_CHILD)],
        json!({}),
        json!({"maxSteps": 30}),
    );
    assert_eq!(explore(&m3).verdict, Verdict::NoCounterexampleWithinBounds);
}

#[test]
fn a_database_deadlock_is_not_pool_starvation() {
    // Two transactions locking two rows in opposite orders: PostgreSQL's
    // detector aborts one; nothing is stuck.
    let a =
        "begin\nupdate t where @id == 1 set @v = 1\nupdate t where @id == 2 set @v = 1\ncommit\n";
    let b =
        "begin\nupdate t where @id == 2 set @v = 2\nupdate t where @id == 1 set @v = 2\ncommit\n";
    let m = model(
        vec![shared("A", a), shared("B", b)],
        json!({ "pools": { "P": 2 } }),
        json!({"maxSteps": 40}),
    );
    let r = explore(&m);
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    let with_deadlock = explore_with(
        &m,
        &ExploreOptions {
            por: false,
            strategy: Strategy::Dfs,
            collect_states: false,
            stop_at_first: false,
        },
    );
    assert!(with_deadlock.metrics.terminal_states > 0);
}

#[test]
fn a_lock_wait_on_the_applications_own_transaction_is_an_application_wait_cycle() {
    // The child writes (outside the transaction) the row the parent holds
    // locked, and the parent waits for the child: PostgreSQL sees one waiter.
    let program = "begin\nupdate t where @id == 1 set @v = 1\nspawn {\n  pool update t where @id == 1 set @v = 5\n}\njoin\ncommit\n";
    let m = model(
        vec![shared("A", program)],
        json!({ "options": { "termination": true } }),
        json!({"maxSteps": 30}),
    );
    let r = explore(&m);
    assert_eq!(r.verdict, Verdict::CounterexampleFound);
    assert_eq!(r.counterexample.as_ref().unwrap().invariant, "termination");
    assert!(
        r.stuck
            .iter()
            .all(|s| s.kind == StuckKind::ApplicationWaitCycle),
        "{:?}",
        r.stuck
    );
}

#[test]
fn connections_are_granted_in_arrival_order() {
    // Capacity 1; A holds it; B asks first, then C.
    let m = model(
        vec![
            shared("A", TX_ONLY),
            shared("B", TX_ONLY),
            shared("C", TX_ONLY),
        ],
        json!({ "pools": { "P": 1 } }),
        json!({"maxSteps": 30}),
    );
    let s = initial_state(&m);
    // Arrival order is the order of settling in the initial state: A, B, C.
    let lease = |s: &State, a: usize| s.actors[a].threads[0].lease;
    assert_eq!(
        [lease(&s, 0), lease(&s, 1), lease(&s, 2)],
        [Lease::Held, Lease::Waiting, Lease::Waiting]
    );
    assert_eq!(s.pools[0].queue, vec![(1, 0), (2, 0)]);
    // Only A can run.
    let steps: Vec<u16> = enabled(&m, &s)
        .iter()
        .filter(|t| t.kind == TKind::Step)
        .map(|t| t.actor)
        .collect();
    assert_eq!(steps, vec![0]);
    // A begins, updates, commits: the connection goes to B, not C.
    let mut s = s;
    for _ in 0..3 {
        s = step_of(&m, &s, 0);
    }
    assert_eq!(lease(&s, 1), Lease::Held);
    assert_eq!(lease(&s, 2), Lease::Waiting);
    assert_eq!(s.pools[0].queue, vec![(2, 0)]);
    assert_eq!(s.pools[0].leased, 1, "the connection changed hands");
    // Every schedule ends with all three committed.
    let r = explore(&m);
    assert_eq!(r.verdict, Verdict::NoCounterexampleWithinBounds);
}

#[test]
fn a_crash_ends_every_task_of_the_process_and_empties_its_pool() {
    let m = model(
        vec![shared("A", TX_ONLY), shared("B", TX_ONLY)],
        json!({ "pools": { "P": 1 } }),
        json!({"maxSteps": 30, "maxCrashes": 1}),
    );
    let mut s = initial_state(&m);
    s = step_of(&m, &s, 0); // A: begin (B is queued)
    let crash = enabled(&m, &s)
        .into_iter()
        .find(|t| t.kind == TKind::Crash)
        .expect("a crash is possible");
    let s = apply(&m, &s, crash, &mut Recorder::off()).unwrap().state;
    // A was running; B was waiting for a connection: both are gone.
    assert_eq!(s.actors[0].outcome, Outcome::Crashed);
    assert_eq!(s.actors[1].outcome, Outcome::Crashed);
    assert_eq!(s.pools[0].leased, 0);
    assert!(s.pools[0].queue.is_empty());
    assert!(s.txns.is_empty());
    assert!(
        s.actors
            .iter()
            .flat_map(|a| &a.threads)
            .all(|t| t.lease == Lease::None)
    );
}

#[test]
fn a_lost_acknowledgment_frees_the_broken_connection() {
    let m = model(
        vec![shared("A", TX_ONLY), shared("B", TX_ONLY)],
        json!({ "pools": { "P": 1 } }),
        json!({"maxSteps": 30, "maxInFlight": 1}),
    );
    let mut s = initial_state(&m);
    s = step_of(&m, &s, 0); // begin
    s = step_of(&m, &s, 0); // update
    let lost = enabled(&m, &s)
        .into_iter()
        .find(|t| t.kind == TKind::LostCommitted && t.actor == 0)
        .expect("a lost acknowledgment is possible");
    let s = apply(&m, &s, lost, &mut Recorder::off()).unwrap().state;
    assert_eq!(
        s.actors[1].threads[0].lease,
        Lease::Held,
        "the pool dropped the broken connection; B got a new one"
    );
    // The server still holds A's COMMIT.
    assert_eq!(s.orphans.len(), 1);
}

#[test]
fn pool_state_is_part_of_the_state_and_reduction_stays_sound() {
    let m = model(
        vec![shared("A", TX_ONLY), shared("B", TX_ONLY)],
        json!({ "pools": { "P": 1 } }),
        json!({"maxSteps": 30}),
    );
    let s = initial_state(&m);
    let mut other = s.clone();
    other.pools[0].queue.clear();
    assert_ne!(s.canonical(&m), other.canonical(&m));
    assert!(audit_independence(&m, 100_000).is_empty());
    let mut full = ExploreOptions::from_model(&m);
    full.por = false;
    full.collect_states = true;
    full.stop_at_first = false;
    let mut reduced = full.clone();
    reduced.por = true;
    assert_eq!(
        explore_with(&m, &full).reached,
        explore_with(&m, &reduced).reached
    );
}

#[test]
fn an_unbounded_pool_changes_nothing() {
    // The Phase 8 transition system: no leases anywhere.
    let m = model(
        vec![shared("A", TX_ONLY), shared("B", TX_ONLY)],
        json!({}),
        json!({"maxSteps": 30}),
    );
    let s = initial_state(&m);
    assert!(s.pools.is_empty());
    assert!(
        s.actors
            .iter()
            .flat_map(|a| &a.threads)
            .all(|t| t.lease == Lease::None)
    );
    assert_eq!(
        enabled(&m, &s)
            .iter()
            .filter(|t| t.kind == TKind::Step)
            .count(),
        2
    );
}

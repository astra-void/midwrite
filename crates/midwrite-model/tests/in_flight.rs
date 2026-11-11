//! Operations whose acknowledgment is lost in flight.
//!
//! Small exhaustive checks of the transition system: both legal physical
//! outcomes are explored (the server never gets the operation, or receives
//! it and completes it at *some* later point), the application's observation
//! is the same in both, the unresolved outcome is part of the canonical
//! state, and the reduction is still sound with the new transitions.

mod common;

use std::collections::BTreeSet;

use common::*;
use midwrite_model::engine::{Recorder, apply, enabled, initial_state};
use midwrite_model::epistemic::{InFlightOp, OutcomeKnowledge, OutcomeSet, Via, World};
use midwrite_model::explore::{ExploreOptions, Verdict, audit_independence, explore_with};
use midwrite_model::model::{Model, Strategy};
use midwrite_model::state::{Outcome, State, TKind, Transition};
use midwrite_model::value::Value;
use midwrite_model::witness::{enumerate_schedules, replay};
use serde_json::json;

fn payments_tables(unique_req: bool) -> serde_json::Value {
    let mut t = json!({
        "name": "payments",
        "columns": [
            {"name": "id", "notNull": true, "type": "text"},
            {"name": "req", "type": "text"}
        ],
        "primaryKey": ["id"]
    });
    if unique_req {
        t["unique"] = json!([["req"]]);
    }
    json!([t])
}

fn serial_payments() -> serde_json::Value {
    json!([{
        "name": "payments",
        "columns": [
            {"name": "id", "notNull": true, "type": "int", "default": {"sequence": {}}},
            {"name": "req", "type": "text"}
        ],
        "primaryKey": ["id"]
    }])
}

/// A client records a payment in a transaction. `id` is the row id the
/// client chooses per attempt (blind) or per logical request (idempotent).
const BLIND: &str = "begin\ninsert payments set @id = rid, @req = rid\ncommit\n";
/// Idempotent: the unique logical request id makes a repeated insert a no-op.
const IDEMPOTENT: &str =
    "begin\ninsert payments set @id = rid, @req = rid on conflict do nothing\ncommit\n";
/// Autocommit variant.
const AUTOCOMMIT: &str = "insert payments set @id = rid, @req = rid on conflict do nothing\n";

fn at_most_one_payment() -> serde_json::Value {
    json!([{
        "name": "at-most-one-payment",
        "assert": { "le": [ { "count": { "table": "payments", "where": {} } }, 1 ] }
    }])
}

fn one_client(
    program: &'static str,
    unique: bool,
    bounds: serde_json::Value,
    retry: &'static str,
) -> Model {
    let mut a = actor("Client", program, json!({"rid": "r1"}));
    a.retry = Some(retry);
    build(
        "read-committed",
        payments_tables(unique),
        json!({ "payments": [] }),
        vec![a],
        at_most_one_payment(),
        bounds,
        json!({ "channels": { "confirm": { "dedup": "none" } } }),
    )
}

fn rows(m: &Model, s: &State) -> usize {
    s.committed_rows(m, m.schema.index("payments").unwrap())
        .len()
}

fn take(m: &Model, s: &State, kind: TKind, actor: u16) -> State {
    let t = enabled(m, s)
        .into_iter()
        .find(|t| t.kind == kind && t.actor == actor)
        .unwrap_or_else(|| panic!("{kind:?} of actor {actor} is not enabled"));
    apply(m, s, t, &mut Recorder::off()).unwrap().state
}

fn step(m: &Model, s: &State, actor: u16) -> State {
    take(m, s, TKind::Step, actor)
}

fn complete_orphans(m: &Model, mut s: State) -> State {
    while let Some(t) = enabled(m, &s)
        .into_iter()
        .find(|t| t.kind == TKind::ServerComplete)
    {
        s = apply(m, &s, t, &mut Recorder::off()).unwrap().state;
    }
    s
}

#[test]
fn without_the_bound_there_are_no_in_flight_transitions() {
    let m = one_client(
        BLIND,
        false,
        json!({"maxCrashes": 1, "maxRetries": 1}),
        "onFailureOrCrash",
    );
    let mut s = initial_state(&m);
    s = step(&m, &s, 0); // begin
    s = step(&m, &s, 0); // insert
    let kinds: BTreeSet<_> = enabled(&m, &s).into_iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        BTreeSet::from([TKind::Step, TKind::Crash]),
        "Phase 8 transition system"
    );
}

#[test]
fn a_commit_in_flight_has_both_physical_outcomes_and_one_observation() {
    let m = one_client(
        BLIND,
        false,
        json!({"maxInFlight": 1, "maxRetries": 1}),
        "onFailure",
    );
    let mut s = initial_state(&m);
    s = step(&m, &s, 0); // begin
    s = step(&m, &s, 0); // insert (uncommitted)
    let kinds: BTreeSet<_> = enabled(&m, &s).into_iter().map(|t| t.kind).collect();
    assert!(kinds.contains(&TKind::LostCommitted) && kinds.contains(&TKind::LostNotCommitted));
    assert!(
        !kinds.contains(&TKind::Crash),
        "no process crash is allowed"
    );

    let reached = take(&m, &s, TKind::LostCommitted, 0);
    let aborted = take(&m, &s, TKind::LostNotCommitted, 0);
    // The server holds the COMMIT: nothing is durable yet, and it is not
    // aborted either: the transaction is still open on the server.
    assert_eq!(rows(&m, &reached), 0);
    assert_eq!(reached.orphans.len(), 1);
    assert!(
        !reached.txns.is_empty(),
        "the transaction is open on the server"
    );
    // It completes at some later point.
    let committed = complete_orphans(&m, reached.clone());
    assert_eq!(rows(&m, &committed), 1);
    assert_eq!(rows(&m, &aborted), 0);
    assert!(aborted.orphans.is_empty());
    // The application observed exactly the same thing in both worlds.
    assert_eq!(reached.actors[0].outcome, aborted.actors[0].outcome);
    let Outcome::Failed(reason) = &reached.actors[0].outcome else {
        panic!("the application got no error");
    };
    assert!(reason.contains("outcome of COMMIT is unknown"), "{reason}");
    assert_eq!(reached.actors[0].locals, aborted.actors[0].locals);
    // The unresolved outcome is recorded with every possible world.
    let g = reached.actors[0].ambiguous[0];
    assert_eq!(g.op, InFlightOp::Commit);
    assert_eq!(g.world, World::Committed);
    assert_eq!(g.via, Via::ConnectionLost);
    assert_eq!(g.possible.len(), 2);
    assert_eq!(aborted.actors[0].ambiguous[0].world, World::NotCommitted);
    assert_eq!(
        OutcomeKnowledge::of(&g).label(),
        "ambiguous: committed or not committed"
    );
}

#[test]
fn blind_retry_after_a_lost_commit_acknowledgment_can_record_the_payment_twice() {
    // With one row id per logical payment the retried insert hits the primary
    // key: accidentally safe.
    let m = one_client(
        BLIND,
        false,
        json!({"maxInFlight": 1, "maxRetries": 1, "maxSteps": 40}),
        "onFailure",
    );
    let r = explore_with(&m, &ExploreOptions::from_model(&m));
    assert_eq!(r.verdict, Verdict::NoCounterexampleWithinBounds);

    // A client that numbers its attempts (a fresh payment id per attempt).
    let m = build(
        "read-committed",
        serial_payments(),
        json!({ "payments": [] }),
        vec![{
            let mut a = actor(
                "Client",
                "begin\ninsert payments set @req = rid\ncommit\n",
                json!({"rid": "r1"}),
            );
            a.retry = Some("onFailure");
            a
        }],
        at_most_one_payment(),
        json!({"maxInFlight": 1, "maxRetries": 1, "maxSteps": 40}),
        json!({}),
    );
    let r = explore_with(&m, &ExploreOptions::from_model(&m));
    assert_eq!(r.verdict, Verdict::CounterexampleFound);
    let ce = r.counterexample.unwrap();
    assert!(
        ce.schedule.iter().any(|t| t.kind == TKind::LostCommitted),
        "the duplicate needs the commit that the client did not hear about: {:?}",
        ce.schedule
    );
    assert!(
        !ce.schedule
            .iter()
            .any(|t| t.kind == TKind::LostNotCommitted),
        "the aborted world cannot duplicate"
    );
    assert!(replay(&m, &ce.schedule, false).is_ok());
    // The Phase 8 transition system has no counterexample for this client
    // under a lost response: it can only crash before or after a statement.
    let mut phase8 = m.clone();
    phase8.bounds.max_in_flight = 0;
    phase8.bounds.max_crashes = 1;
    let r8 = explore_with(&phase8, &ExploreOptions::from_model(&phase8));
    assert_eq!(
        r8.verdict,
        Verdict::NoCounterexampleWithinBounds,
        "with onFailure (no retry after a crash) Phase 8 cannot express the failure"
    );
}

#[test]
fn an_idempotent_retry_is_safe_under_every_physical_outcome() {
    for crashes in [0, 1] {
        let m = one_client(
            IDEMPOTENT,
            true,
            json!({"maxInFlight": 1, "maxCrashes": crashes, "maxRetries": 1, "maxSteps": 40}),
            "onFailureOrCrash",
        );
        let r = explore_with(&m, &ExploreOptions::from_model(&m));
        assert_eq!(
            r.verdict,
            Verdict::NoCounterexampleWithinBounds,
            "crashes {crashes}"
        );
    }
}

#[test]
fn reading_durable_state_does_not_make_the_retry_safe() {
    // After the lost COMMIT the client looks for its payment and inserts
    // only when there is none. The server may still be holding the COMMIT
    // when the client reads, so the read proves nothing about the future.
    let lookup = "let p = select payments where @req == rid\nif p == null {\n  begin\n  insert payments set @req = rid\n  commit\n}\n";
    let mut a = actor("Client", lookup, json!({"rid": "r1"}));
    a.retry = Some("onFailureOrCrash");
    let m = build(
        "read-committed",
        serial_payments(),
        json!({ "payments": [] }),
        vec![a],
        at_most_one_payment(),
        json!({"maxInFlight": 1, "maxRetries": 1, "maxSteps": 40}),
        json!({}),
    );
    let r = explore_with(&m, &ExploreOptions::from_model(&m));
    assert_eq!(r.verdict, Verdict::CounterexampleFound);
    let ce = r.counterexample.unwrap();
    let kinds: Vec<TKind> = ce.schedule.iter().map(|t| t.kind).collect();
    let lost = kinds
        .iter()
        .position(|k| *k == TKind::LostCommitted)
        .expect("a commit the server received");
    let complete = kinds
        .iter()
        .position(|k| *k == TKind::ServerComplete)
        .expect("the server completes it");
    let retry = kinds.iter().position(|k| *k == TKind::Retry).unwrap();
    assert!(
        lost < retry && retry < complete,
        "the server completes the COMMIT after the client already retried: {kinds:?}"
    );
}

#[test]
fn states_that_differ_only_in_unresolved_outcomes_are_not_merged() {
    let m = one_client(
        IDEMPOTENT,
        true,
        json!({"maxInFlight": 1, "maxRetries": 1, "maxSteps": 40}),
        "onFailure",
    );
    let finish = |lost: TKind| {
        let mut s = initial_state(&m);
        s = step(&m, &s, 0); // begin
        s = step(&m, &s, 0); // insert
        s = take(&m, &s, lost, 0);
        s = take(&m, &s, TKind::Retry, 0);
        while let Some(t) = enabled(&m, &s)
            .into_iter()
            .find(|t| matches!(t.kind, TKind::Step | TKind::Resume | TKind::ServerComplete))
        {
            s = apply(&m, &s, t, &mut Recorder::off()).unwrap().state;
        }
        s
    };
    let c = finish(TKind::LostCommitted);
    let a = finish(TKind::LostNotCommitted);
    assert_eq!(rows(&m, &c), 1);
    assert_eq!(rows(&m, &a), 1);
    assert_eq!(c.actors[0].outcome, a.actors[0].outcome);
    assert_ne!(c.canonical(&m), a.canonical(&m));
    // The two worlds also differ in row identity (which transaction wrote
    // the row), so isolate the knowledge itself: the same physical state
    // with a different (or no) record of the unresolved outcome is a
    // different state.
    let base = c.canonical(&m);
    let mut flipped = c.clone();
    flipped.actors[0].ambiguous[0].world = World::NotCommitted;
    assert_ne!(
        flipped.canonical(&m),
        base,
        "the physical world is part of the state"
    );
    let mut narrowed = c.clone();
    narrowed.actors[0].ambiguous[0].possible = OutcomeSet::single(World::Committed);
    assert_ne!(
        narrowed.canonical(&m),
        base,
        "the set of possible worlds is part of the state"
    );
    let mut forgotten = c.clone();
    forgotten.actors[0].ambiguous.clear();
    forgotten.in_flight = 0;
    assert_ne!(
        forgotten.canonical(&m),
        base,
        "an unresolved outcome is never silently dropped"
    );
    assert_eq!(c.clone().canonical(&m), base, "encoding is deterministic");
}

#[test]
fn a_held_commit_is_part_of_the_state_until_the_server_completes_it() {
    let m = one_client(
        BLIND,
        false,
        json!({"maxInFlight": 1, "maxRetries": 0}),
        "onFailure",
    );
    let mut s = initial_state(&m);
    s = step(&m, &s, 0);
    s = step(&m, &s, 0);
    let held = take(&m, &s, TKind::LostCommitted, 0);
    let done = complete_orphans(&m, held.clone());
    assert_ne!(held.canonical(&m), done.canonical(&m));
    assert!(done.orphans.is_empty() && done.txns.is_empty());
    // A state with a held COMMIT is not terminal: the final invariants are
    // evaluated after the server has completed it.
    let en = enabled(&m, &held);
    assert!(en.iter().any(|t| t.kind == TKind::ServerComplete), "{en:?}");
}

#[test]
fn a_crash_with_a_commit_in_flight_reaches_the_server_or_not() {
    let m = one_client(
        BLIND,
        false,
        json!({"maxInFlight": 1, "maxCrashes": 1, "maxRetries": 1}),
        "onCrash",
    );
    let mut s = initial_state(&m);
    s = step(&m, &s, 0);
    s = step(&m, &s, 0);
    let crashes: Vec<Transition> = enabled(&m, &s)
        .into_iter()
        .filter(|t| matches!(t.kind, TKind::Crash | TKind::CrashInFlight))
        .collect();
    assert_eq!(
        crashes
            .iter()
            .map(|t| (t.kind, t.actor))
            .collect::<Vec<_>>(),
        vec![(TKind::CrashInFlight, 0), (TKind::CrashInFlight, 1)],
        "one transition per fate of the COMMIT; the plain crash (nothing was submitted) is fate 0"
    );
    let after = |t: &Transition| apply(&m, &s, *t, &mut Recorder::off()).unwrap().state;
    let lost = after(&crashes[0]);
    let held = after(&crashes[1]);
    assert_eq!((rows(&m, &lost), rows(&m, &held)), (0, 0));
    assert!(lost.orphans.is_empty() && held.orphans.len() == 1);
    assert_eq!(rows(&m, &complete_orphans(&m, held.clone())), 1);
    for x in [&lost, &held] {
        assert_eq!(x.actors[0].outcome, Outcome::Crashed);
        assert_eq!(x.actors[0].ambiguous[0].via, Via::ProcessCrash);
    }
    assert!(lost.txns.is_empty() && lost.store.locks.is_empty());
    // The dead client's transaction is still open on the server.
    assert!(!held.txns.is_empty());
}

#[test]
fn every_task_of_a_process_crashes_with_its_own_operation_in_flight() {
    let mut a = actor("A", BLIND, json!({"rid": "ra"}));
    let mut b = actor("B", BLIND, json!({"rid": "rb"}));
    a.process = Some("P");
    b.process = Some("P");
    let m = build(
        "read-committed",
        payments_tables(false),
        json!({ "payments": [] }),
        vec![a, b],
        at_most_one_payment(),
        json!({"maxInFlight": 1, "maxCrashes": 1}),
        json!({}),
    );
    let mut s = initial_state(&m);
    for who in [0, 0, 1, 1] {
        s = step(&m, &s, who);
    }
    let masks: Vec<u16> = enabled(&m, &s)
        .into_iter()
        .filter(|t| t.kind == TKind::CrashInFlight)
        .map(|t| t.actor)
        .collect();
    assert_eq!(
        masks,
        vec![0, 1, 2, 3],
        "2 operations in flight: 4 combinations"
    );
    let t = Transition {
        kind: TKind::CrashInFlight,
        actor: 0b10, // only B's COMMIT reached the server
        thread: 0,
    };
    let s = apply(&m, &s, t, &mut Recorder::off()).unwrap().state;
    assert_eq!(s.orphans.len(), 1);
    let s = complete_orphans(&m, s);
    assert_eq!(rows(&m, &s), 1);
    let committed = s.committed_rows(&m, 0);
    assert_eq!(committed[0].get("id"), Some(&Value::str("rb")));
    assert!(
        s.actors
            .iter()
            .all(|a| a.outcome == Outcome::Crashed && a.ambiguous.len() == 1),
        "both actors share the failure domain and each keeps its own unresolved outcome"
    );
    assert_eq!(s.actors[0].ambiguous[0].world, World::NotCommitted);
    assert_eq!(s.actors[1].ambiguous[0].world, World::Committed);
    assert_eq!(s.in_flight, 1, "one fault event, however many operations");
}

#[test]
fn an_autocommit_write_can_be_lost_after_it_reached_the_server() {
    let m = one_client(
        AUTOCOMMIT,
        true,
        json!({"maxInFlight": 1, "maxCrashes": 1, "maxRetries": 1, "maxSteps": 30}),
        "onFailureOrCrash",
    );
    let s = initial_state(&m);
    let kinds: BTreeSet<_> = enabled(&m, &s).into_iter().map(|t| t.kind).collect();
    assert!(kinds.contains(&TKind::LostCommitted));
    assert!(
        kinds.contains(&TKind::CrashInFlight),
        "a crash is possible with the very first statement in flight"
    );
    let held = take(&m, &s, TKind::LostCommitted, 0);
    assert_eq!(rows(&m, &held), 0, "the statement has not run yet");
    assert!(matches!(held.actors[0].outcome, Outcome::Failed(_)));
    assert_eq!(
        held.actors[0].ambiguous[0].op,
        InFlightOp::Autocommit(m.schema.index("payments").unwrap())
    );
    let done = complete_orphans(&m, held);
    assert_eq!(
        rows(&m, &done),
        1,
        "committed although the client saw an error"
    );
}

#[test]
fn a_commit_the_server_rejects_later_is_not_a_committed_world() {
    // Under SERIALIZABLE the held COMMIT of B can be rejected (40001) when
    // it completes after A's: the physical outcome then is "not committed".
    let a_prog = "begin isolation serializable\nlet n = count payments where @req == \"rb\"\ninsert payments set @id = \"ra\", @req = \"ra\"\ncommit\n";
    let b_prog = "begin isolation serializable\nlet n = count payments where @req == \"ra\"\ninsert payments set @id = \"rb\", @req = \"rb\"\ncommit\n";
    let m = build(
        "serializable",
        payments_tables(false),
        json!({ "payments": [] }),
        vec![actor("A", a_prog, json!({})), actor("B", b_prog, json!({}))],
        at_most_one_payment(),
        json!({"maxInFlight": 2, "maxSteps": 30}),
        json!({}),
    );
    // A: begin, count, insert; B: begin, count, insert; A commits (ok).
    let mut s = initial_state(&m);
    for who in [0, 0, 0, 1, 1, 1, 0] {
        s = step(&m, &s, who);
    }
    let held = take(&m, &s, TKind::LostCommitted, 1);
    assert_eq!(held.actors[1].ambiguous[0].world, World::Committed);
    let done = complete_orphans(&m, held);
    assert_eq!(rows(&m, &done), 1, "B's COMMIT was rejected by the server");
    assert_eq!(
        done.actors[1].ambiguous[0].world,
        World::NotCommitted,
        "the record follows the physical outcome"
    );
}

#[test]
fn the_reduction_is_sound_with_in_flight_transitions() {
    // Two clients with independent rows: lost acknowledgments of different
    // clients share the fault budget and must not be treated as independent.
    let mut a = actor("A", IDEMPOTENT, json!({"rid": "ra"}));
    let mut b = actor("B", IDEMPOTENT, json!({"rid": "rb"}));
    a.retry = Some("onFailureOrCrash");
    b.retry = Some("onFailureOrCrash");
    let m = build(
        "read-committed",
        payments_tables(true),
        json!({ "payments": [] }),
        vec![a, b],
        json!([{
            "name": "two-payments-at-most",
            "assert": { "le": [ { "count": { "table": "payments", "where": {} } }, 2 ] }
        }]),
        json!({"maxInFlight": 1, "maxCrashes": 1, "maxRetries": 2, "maxSteps": 40}),
        json!({}),
    );
    assert!(
        audit_independence(&m, 200_000).is_empty(),
        "independence relation is unsound with in-flight transitions"
    );
    let mut full = ExploreOptions::from_model(&m);
    full.por = false;
    full.strategy = Strategy::Dfs;
    full.collect_states = true;
    let mut reduced = full.clone();
    reduced.por = true;
    let f = explore_with(&m, &full);
    let r = explore_with(&m, &reduced);
    assert_eq!(f.verdict, r.verdict);
    assert_eq!(f.reached, r.reached, "reduction must reach every state");
    assert!(r.metrics.transitions <= f.metrics.transitions);
}

#[test]
fn schedules_enumerate_both_worlds() {
    let m = one_client(
        IDEMPOTENT,
        true,
        json!({"maxInFlight": 1, "maxRetries": 1, "maxSteps": 30}),
        "onFailure",
    );
    let (schedules, truncated) = enumerate_schedules(&m, 100_000);
    assert!(!truncated);
    let has = |k: TKind| schedules.iter().any(|s| s.iter().any(|t| t.kind == k));
    assert!(
        has(TKind::LostCommitted) && has(TKind::LostNotCommitted) && has(TKind::ServerComplete)
    );
    // Every complete schedule ends with the payment recorded at most once.
    for s in &schedules {
        let r = replay(&m, s, false).unwrap();
        assert!(rows(&m, &r.state) <= 1);
    }
}

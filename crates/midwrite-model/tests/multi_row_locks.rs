//! Multi-row `SELECT` and row locks taken one row at a time
//! (`docs/architecture/postgres-model.md`).
//!
//! * Semantics: chosen schedules of two transactions over `t(id, v, w)`
//!   (prefix locks held while waiting, wait-for deadlocks, `NOWAIT`, `SKIP
//!   LOCKED`, READ COMMITTED re-evaluation, REPEATABLE READ failures, `LIMIT`,
//!   zero rows, lock strengths).
//! * Reduction: unreduced exploration, sleep sets, classic DPOR and
//!   source-set DPOR agree on terminal states and counterexamples for the
//!   differential workloads and for generated multi-row lock scenarios.
//! * PostgreSQL: every schedule of every workload × isolation level is
//!   replayed in lockstep (statements sent as the oracle writes them, with
//!   `ORDER BY` the key), and the workloads whose predicates are on the
//!   primary key are replayed again as Drizzle sends them (no `ORDER BY`).
//!   Skipped without PostgreSQL unless `MIDWRITE_REQUIRE_REPLAY` is set.

mod common;

use common::por_diff::{Alg, discrepancy, full, run};
use common::{actor, build};
use midwrite_model::engine::{Recorder, apply, enabled, initial_state};
use midwrite_model::explore::SdporConfig;
use midwrite_model::model::Model;
use midwrite_model::state::{Outcome, State, TKind};
use midwrite_model::value::Value;
use serde_json::{Value as Json, json};

const ISOLATIONS: [&str; 3] = ["read-committed", "repeatable-read", "serializable"];

fn tables() -> Json {
    json!([{ "name": "t", "columns": [
        {"name":"id","notNull":true,"type":"int"},
        {"name":"v","type":"int"},
        {"name":"w","type":"int"}
    ], "primaryKey": ["id"] }])
}

fn rows() -> Json {
    json!({ "t": [
        {"id": 1, "v": 0, "w": 0},
        {"id": 2, "v": 0, "w": 0},
        {"id": 3, "v": 0, "w": 0}
    ] })
}

/// No transaction fails (a deadlock victim, `NOWAIT`, a serialization
/// failure all violate it): the property the reduction tests must keep
/// counterexamples of.
fn none_failed() -> Json {
    json!([{ "name": "none-failed", "assert": { "eq": [ { "actors": { "outcome": "failed" } }, 0 ] } }])
}

fn pair(iso: &str, a: &'static str, b: &'static str) -> Model {
    pair_on(iso, a, b, rows())
}

/// The same rows inserted out of key order (heap order 3, 1, 2): a plan
/// that returned rows in physical order would lock them in that order.
fn shuffled_rows() -> Json {
    json!({ "t": [
        {"id": 3, "v": 0, "w": 0},
        {"id": 1, "v": 0, "w": 0},
        {"id": 2, "v": 0, "w": 0}
    ] })
}

fn pair_on(iso: &str, a: &'static str, b: &'static str, initial: Json) -> Model {
    build(
        iso,
        tables(),
        initial,
        vec![actor("A", a, json!({})), actor("B", b, json!({}))],
        none_failed(),
        json!({}),
        json!({}),
    )
}

/// The differential workloads: (name, A, B, predicates on the primary key
/// only — those are also replayed without `ORDER BY`).
pub fn workloads() -> Vec<(&'static str, &'static str, &'static str, bool)> {
    vec![
        (
            "overlapping-sets",
            "begin\nlet r = select all t where inArray(@id, [1, 2]) for update\nupdate t where inArray(@id, [1, 2]) set @v = @v + 1\ncommit\n",
            "begin\nlet r = select all t where inArray(@id, [3, 2]) for update\nupdate t where inArray(@id, [2, 3]) set @v = @v + 10\ncommit\n",
            true,
        ),
        (
            "reversed-order",
            "begin\nlet r = select all t where inArray(@id, [2, 1]) for update\nupdate t where @id == 1 set @v = @v + 1\ncommit\n",
            "begin\nlet y = select t where @id == 2 for update\nlet x = select t where @id == 1 for update\ncommit\n",
            true,
        ),
        (
            "missing-row",
            "begin\nlet r = select all t where inArray(@id, [1, 4]) for update\nupdate t where inArray(@id, [1, 4]) set @v = @v + 1\ncommit\n",
            "begin\ninsert t set @id = 4, @v = 0, @w = 0\nlet x = select t where @id == 1 for update\ncommit\n",
            true,
        ),
        (
            "concurrent-update-out",
            "begin\nlet r = select all t where inArray(@id, [1, 2]) && @v == 0 for update\ncommit\n",
            "begin\nupdate t where @id == 2 set @v = @v + 1\ncommit\n",
            false,
        ),
        (
            "concurrent-update-in",
            "begin\nlet r = select all t where inArray(@id, [1, 2]) && @v == 0 for update\ncommit\n",
            "begin\nupdate t where @id == 2 set @w = @w + 1\ncommit\n",
            false,
        ),
        (
            "concurrent-delete",
            "begin\nlet r = select all t where inArray(@id, [1, 2, 3]) for update\ncommit\n",
            "begin\ndelete t where @id == 2\ncommit\n",
            true,
        ),
        (
            "nowait",
            "begin\nlet r = select all t where inArray(@id, [1, 2, 3]) for update nowait\ncommit\n",
            "begin\nlet y = select t where @id == 2 for update\nlet x = select t where @id == 1 for update\ncommit\n",
            true,
        ),
        (
            "skip-locked",
            "begin\nlet r = select all t where inArray(@id, [1, 2, 3]) for update skip locked\nupdate t where inArray(@id, [1, 2, 3]) set @w = @w + 1\ncommit\n",
            "begin\nlet y = select t where @id == 2 for update\ncommit\n",
            true,
        ),
        (
            "share-then-update",
            "begin\nlet r = select all t where inArray(@id, [1, 2]) for share\nupdate t where @id == 2 set @v = @v + 1\ncommit\n",
            "begin\nlet r = select all t where inArray(@id, [2, 3]) for share\nupdate t where @id == 2 set @v = @v + 10\ncommit\n",
            true,
        ),
        (
            "share-vs-update",
            "begin\nlet r = select all t where inArray(@id, [1, 2]) for share\ncommit\n",
            "begin\nlet r = select all t where inArray(@id, [2, 3]) for update\ncommit\n",
            true,
        ),
        (
            "limit-after-wait",
            "begin\nlet r = select all t where @v == 0 limit 2 for update\ncommit\n",
            "begin\nupdate t where @id == 1 set @v = 1\ncommit\n",
            false,
        ),
        (
            "zero-rows",
            "begin\nlet r = select all t where inArray(@id, [7, 8]) for update\ncommit\n",
            "begin\ninsert t set @id = 7, @v = 0, @w = 0\ncommit\n",
            true,
        ),
    ]
}

fn workload(name: &str) -> (&'static str, &'static str) {
    let (_, a, b, _) = workloads()
        .into_iter()
        .find(|w| w.0 == name)
        .expect("workload");
    (a, b)
}

// ---------------------------------------------------------------------
// Chosen schedules
// ---------------------------------------------------------------------

/// Apply the next step (or resumption) of `actor`.
fn go(m: &Model, s: &State, actor: u16) -> State {
    let t = enabled(m, s)
        .into_iter()
        .find(|t| t.actor == actor && matches!(t.kind, TKind::Step | TKind::Resume))
        .unwrap_or_else(|| panic!("actor {actor} cannot move"));
    apply(m, s, t, &mut Recorder::off())
        .expect("supported")
        .state
}

/// Apply `actor`'s steps until it waits or ends.
fn run_actor(m: &Model, mut s: State, actor: u16) -> State {
    while enabled(m, &s)
        .iter()
        .any(|t| t.actor == actor && matches!(t.kind, TKind::Step | TKind::Resume))
        && !waiting(&s, actor)
    {
        s = go(m, &s, actor);
    }
    s
}

fn waiting(s: &State, a: u16) -> bool {
    s.actors[a as usize].threads[0]
        .pending
        .as_ref()
        .is_some_and(|p| p.blocked_on.is_some_and(|b| s.txn_active(b)))
}

/// Row locks held by `a`'s transaction, as primary keys.
fn locked_ids(s: &State, a: u16) -> Vec<i64> {
    let Some(txn) = s.actors[a as usize].txn else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (row, holders) in &s.store.locks {
        if holders.iter().any(|(t, _)| *t == txn)
            && let Some(Value::Int(id)) = s
                .store
                .rows
                .get(row)
                .and_then(|c| c.iter().rev().find_map(|v| v.values.as_ref()))
                .and_then(|r| r.get("id"))
        {
            out.push(*id);
        }
    }
    out.sort_unstable();
    out
}

fn ids(v: Option<&Value>) -> Vec<i64> {
    match v {
        Some(Value::List(items)) => items
            .iter()
            .map(|r| match r {
                Value::Row(r) => match r.get("id") {
                    Some(Value::Int(n)) => *n,
                    other => panic!("id {other:?}"),
                },
                other => panic!("row {other:?}"),
            })
            .collect(),
        other => panic!("not a list of rows: {other:?}"),
    }
}

fn failure(s: &State, a: u16) -> Option<String> {
    match &s.actors[a as usize].outcome {
        Outcome::Failed(r) => Some(r.clone()),
        _ => None,
    }
}

#[test]
fn a_waiting_statement_keeps_the_locks_it_already_took() {
    // B locks row 2; A's multi-row lock takes row 1 and waits on row 2
    // while holding row 1; B then asks for row 1: a wait-for cycle.
    let (a, b) = workload("reversed-order");
    let m = pair("read-committed", a, b);
    let s = initial_state(&m);
    let s = go(&m, &s, 1); // B: begin
    let s = go(&m, &s, 1); // B: lock 2
    let s = go(&m, &s, 0); // A: begin
    let s = go(&m, &s, 0); // A: lock 1, wait on 2
    assert!(waiting(&s, 0));
    assert_eq!(locked_ids(&s, 0), vec![1], "the prefix is held");
    let s = go(&m, &s, 1); // B: lock 1 -> waits on A
    assert!(waiting(&s, 1));
    let cycle = midwrite_model::engine::deadlock_members(&s);
    assert_eq!(cycle.len(), 2, "a wait-for cycle: {cycle:?}");
    // The detector aborts one member (40P01); the other proceeds.
    let t = enabled(&m, &s)
        .into_iter()
        .find(|t| t.kind == TKind::Deadlock && t.actor == 1)
        .expect("B can be the victim");
    let s = apply(&m, &s, t, &mut Recorder::off()).unwrap().state;
    assert!(
        failure(&s, 1).is_some_and(|r| r.contains("40P01")),
        "{:?}",
        failure(&s, 1)
    );
    let s = run_actor(&m, s, 0);
    assert!(matches!(s.actors[0].outcome, Outcome::Ok));
    assert_eq!(ids(s.actors[0].locals.get("r")), vec![1, 2]);
}

#[test]
fn locks_are_taken_in_key_order_whatever_the_list_order() {
    // `inArray(@id, [2, 1])` locks row 1 first: with B holding row 1, A
    // waits holding nothing.
    let (a, _) = workload("reversed-order");
    let m = pair(
        "read-committed",
        a,
        "begin\nlet x = select t where @id == 1 for update\ncommit\n",
    );
    let s = initial_state(&m);
    let s = run_actor(&m, go(&m, &go(&m, &s, 1), 1), 0);
    assert!(waiting(&s, 0));
    assert!(locked_ids(&s, 0).is_empty(), "{:?}", locked_ids(&s, 0));
}

#[test]
fn nowait_fails_at_the_first_conflicting_row() {
    let (a, b) = workload("nowait");
    let m = pair("read-committed", a, b);
    let s = initial_state(&m);
    let s = go(&m, &go(&m, &s, 1), 1); // B locks 2
    let s = run_actor(&m, s, 0);
    assert!(
        failure(&s, 0).is_some_and(|r| r.contains("55P03")),
        "{:?}",
        failure(&s, 0)
    );
    // The failed transaction released row 1 (rolled back): B takes it.
    let s = run_actor(&m, s, 1);
    assert!(matches!(s.actors[1].outcome, Outcome::Ok));
}

#[test]
fn skip_locked_skips_rows_locked_by_others_and_never_waits() {
    let (a, b) = workload("skip-locked");
    let m = pair("read-committed", a, b);
    let s = initial_state(&m);
    let s = go(&m, &go(&m, &s, 1), 1); // B locks 2
    let s = go(&m, &go(&m, &s, 0), 0); // A: begin, select
    assert!(!waiting(&s, 0));
    assert_eq!(ids(s.actors[0].locals.get("r")), vec![1, 3]);
    assert_eq!(locked_ids(&s, 0), vec![1, 3]);
    // Its update of every row then waits for row 2.
    let s = go(&m, &s, 0);
    assert!(waiting(&s, 0));
}

#[test]
fn read_committed_rechecks_a_row_updated_by_the_lock_holder() {
    for (name, expected) in [
        ("concurrent-update-out", vec![1]),
        ("concurrent-update-in", vec![1, 2]),
        ("concurrent-delete", vec![1, 3]),
    ] {
        let (a, b) = workload(name);
        let m = pair("read-committed", a, b);
        let s = initial_state(&m);
        let s = go(&m, &go(&m, &s, 1), 1); // B writes row 2 (uncommitted)
        let s = run_actor(&m, s, 0);
        assert!(waiting(&s, 0), "{name}");
        let s = run_actor(&m, s, 1); // B commits
        let s = run_actor(&m, s, 0);
        assert!(matches!(s.actors[0].outcome, Outcome::Ok), "{name}");
        assert_eq!(ids(s.actors[0].locals.get("r")), expected, "{name}");
        // REPEATABLE READ / SERIALIZABLE: the locked row changed after the
        // snapshot: 40001.
        for iso in ["repeatable-read", "serializable"] {
            let m = pair(iso, a, b);
            let s = initial_state(&m);
            let s = go(&m, &s, 0); // A: begin (snapshot at its first statement)
            let s = go(&m, &go(&m, &s, 1), 1); // B writes row 2
            let s = go(&m, &s, 0); // A: snapshot, lock 1, wait on 2
            assert!(waiting(&s, 0), "{name}/{iso}");
            let s = run_actor(&m, s, 1);
            let s = run_actor(&m, s, 0);
            assert!(
                failure(&s, 0).is_some_and(|r| r.contains("40001")),
                "{name}/{iso}: {:?}",
                failure(&s, 0)
            );
        }
    }
}

#[test]
fn a_limit_counts_only_rows_that_still_match_after_a_wait() {
    let (a, b) = workload("limit-after-wait");
    let m = pair("read-committed", a, b);
    let s = initial_state(&m);
    let s = go(&m, &go(&m, &s, 1), 1); // B: v of row 1 := 1 (uncommitted)
    let s = run_actor(&m, s, 0);
    assert!(waiting(&s, 0));
    let s = run_actor(&m, run_actor(&m, s, 1), 0);
    assert_eq!(ids(s.actors[0].locals.get("r")), vec![2, 3]);
}

#[test]
fn zero_rows_lock_nothing_and_bind_an_empty_list() {
    let (a, b) = workload("zero-rows");
    let m = pair("read-committed", a, b);
    let s = initial_state(&m);
    let s = go(&m, &go(&m, &s, 0), 0);
    assert_eq!(ids(s.actors[0].locals.get("r")), Vec::<i64>::new());
    assert!(locked_ids(&s, 0).is_empty());
}

#[test]
fn shared_locks_coexist_and_exclude_an_update_lock() {
    let (a, b) = workload("share-vs-update");
    let m = pair("read-committed", a, b);
    let s = initial_state(&m);
    let s = go(&m, &go(&m, &s, 0), 0); // A shares 1, 2
    assert_eq!(locked_ids(&s, 0), vec![1, 2]);
    let s = run_actor(&m, s, 1); // B: FOR UPDATE on 2 waits
    assert!(waiting(&s, 1));
    // Two FOR SHARE readers do not wait for each other.
    let (a, b) = workload("share-then-update");
    let m = pair("read-committed", a, b);
    let s = initial_state(&m);
    let s = go(&m, &go(&m, &s, 0), 0);
    let s = go(&m, &go(&m, &s, 1), 1);
    assert!(!waiting(&s, 1));
    assert_eq!(locked_ids(&s, 1), vec![2, 3]);
}

#[test]
fn single_row_selects_are_unchanged_by_the_multi_row_mode() {
    // The Phase 6-15 statement: the first row in key order, or null.
    let m = pair(
        "read-committed",
        "begin\nlet r = select t where inArray(@id, [3, 2]) for update\ncommit\n",
        "commit\n",
    );
    let s = go(&m, &go(&m, &initial_state(&m), 0), 0);
    match s.actors[0].locals.get("r") {
        Some(Value::Row(r)) => assert_eq!(r.get("id"), Some(&Value::Int(2))),
        other => panic!("{other:?}"),
    }
    assert_eq!(locked_ids(&s, 0), vec![2]);
}

// ---------------------------------------------------------------------
// Partial-order reduction
// ---------------------------------------------------------------------

const SDPOR: [SdporConfig; 3] = [
    SdporConfig::STATEFUL_SLEEP,
    SdporConfig::STATELESS_SLEEP,
    SdporConfig::STATEFUL,
];

/// Compare every reduction with the unreduced search; returns how many
/// (model, reduction) pairs were compared.
fn por_agrees(label: &str, model: &Model, failures: &mut Vec<String>) -> usize {
    let Some(reference) = full(model) else {
        return 0;
    };
    let mut algs = vec![Alg::Sleep, Alg::Classic];
    algs.extend(SDPOR.iter().map(|c| Alg::Sdpor(*c)));
    let mut n = 0;
    for alg in algs {
        let r = run(alg, model);
        if r.truncation.resource_limit() && !r.truncation.max_steps {
            continue;
        }
        n += 1;
        if let Some(why) = discrepancy(&reference, &r, model) {
            failures.push(format!("{label}: {}: {why}", alg.name()));
        }
    }
    n
}

#[test]
fn reductions_agree_on_the_differential_workloads() {
    let mut failures = Vec::new();
    let mut compared = 0;
    let mut with_counterexample = 0;
    for (name, a, b, _) in workloads() {
        for iso in ISOLATIONS {
            let m = pair(iso, a, b);
            if full(&m).is_some_and(|r| r.counterexample.is_some()) {
                with_counterexample += 1;
            }
            compared += por_agrees(&format!("{name}/{iso}"), &m, &mut failures);
        }
    }
    eprintln!(
        "reductions: {compared} comparison(s) over {} model(s), {with_counterexample} with a counterexample, {} discrepancy(ies)",
        workloads().len() * ISOLATIONS.len(),
        failures.len()
    );
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(
        with_counterexample > 0,
        "some workload must fail (deadlock, NOWAIT)"
    );
}

/// A small deterministic generator of multi-row lock scenarios: two or
/// three transactions over `t(1..=4)` and a child table referencing it (a
/// foreign-key check locks the parent `FOR KEY SHARE`), each with up to
/// three statements: multi-row locks of every strength and option over a
/// random key set (with `LIMIT` sometimes), single-row locks, updates
/// (key and non-key columns), deletes, inserts of children.
struct Gen(u64);

impl Gen {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn generated(seed: u64) -> Model {
    let mut g = Gen(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let iso = ISOLATIONS[g.below(3) as usize];
    let actors = if g.below(4) == 0 { 3 } else { 2 };
    let mut programs = Vec::new();
    for a in 0..actors {
        let mut p = String::from("begin\n");
        for k in 0..1 + g.below(3) {
            let set: Vec<String> = (1..=4)
                .filter(|_| g.below(2) == 0)
                .map(|i: u64| i.to_string())
                .collect();
            let list = format!("[{}]", set.join(", "));
            let id = 1 + g.below(4);
            let stmt = match g.below(9) {
                0..=3 => {
                    let lock = [
                        "for update",
                        "for share",
                        "for key share",
                        "for no key update",
                        "for update nowait",
                        "for update skip locked",
                    ][g.below(6) as usize];
                    let limit = if g.below(4) == 0 {
                        format!(" limit {}", 1 + g.below(2))
                    } else {
                        String::new()
                    };
                    format!("let r{k} = select all t where inArray(@id, {list}){limit} {lock}")
                }
                4 => format!("let x{k} = select t where @id == {id} for update"),
                5 => format!("update t where @id == {id} set @v = @v + 1"),
                6 => format!("update t where @id == {id} set @id = {}", 10 + id),
                7 => format!("delete t where @id == {id}"),
                _ => format!(
                    "insert c set @id = {}, @t = {id}",
                    100 + a * 10 + k as usize
                ),
            };
            p.push_str(&stmt);
            p.push('\n');
        }
        p.push_str("commit\n");
        programs.push(p);
    }
    let names = ["A", "B", "C"];
    let actors_json: Vec<Json> = programs
        .iter()
        .enumerate()
        .map(
            |(i, p)| json!({ "name": names[i], "function": "work", "program": p, "arguments": {} }),
        )
        .collect();
    midwrite_model::scenario::testing::model_from_json(json!({
        "schemaVersion": 1,
        "name": format!("generated-{seed}"),
        "database": {
            "isolation": iso,
            "tables": [
                { "name": "t", "columns": [
                    {"name":"id","notNull":true,"type":"int"},
                    {"name":"v","type":"int"},
                    {"name":"w","type":"int"}
                ], "primaryKey": ["id"] },
                { "name": "c", "columns": [
                    {"name":"id","notNull":true,"type":"int"},
                    {"name":"t","type":"int"}
                ], "primaryKey": ["id"],
                  "foreignKeys": [{ "columns": ["t"], "references": "t", "referencedColumns": ["id"] }] }
            ],
            "initialState": { "t": [
                {"id": 1, "v": 0, "w": 0}, {"id": 2, "v": 0, "w": 0},
                {"id": 3, "v": 0, "w": 0}, {"id": 4, "v": 0, "w": 0}
            ] }
        },
        "actors": actors_json,
        "invariants": none_failed(),
        "bounds": { "maxStates": 200000 }
    }))
}

#[test]
fn reductions_agree_on_generated_multi_row_lock_scenarios() {
    let mut failures = Vec::new();
    let mut compared = 0;
    let mut models = 0;
    let mut skipped = 0;
    let mut with_counterexample = 0;
    for seed in 1..=120u64 {
        let m = generated(seed);
        let Some(reference) = full(&m) else {
            skipped += 1;
            continue;
        };
        models += 1;
        if reference.counterexample.is_some() {
            with_counterexample += 1;
        }
        compared += por_agrees(&format!("seed {seed}"), &m, &mut failures);
    }
    eprintln!(
        "generated: {models} model(s) compared ({skipped} over the state bound), {with_counterexample} with a counterexample, {compared} comparison(s), {} discrepancy(ies)",
        failures.len()
    );
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(models >= 100, "{models}");
    assert!(with_counterexample > 0);
}

#[test]
fn the_workloads_notice_a_missing_lock_dependency() {
    // Delete every lock access (`LockCheck`, `LockChange`) from the
    // footprints: transitions that take or test the same row's lock look
    // independent. The differential workloads must then make some
    // reduction lose a terminal state or a counterexample: they are
    // sensitive to the dependency the multi-row lock steps rely on.
    use midwrite_model::footprint::{Access, mutation};
    let mask = Access::KINDS
        .iter()
        .filter(|(n, _)| matches!(*n, "LockCheck" | "LockChange"))
        .fold(0, |m, (_, b)| m | b);
    let mut killed = Vec::new();
    for (name, a, b, _) in workloads() {
        let m = pair("read-committed", a, b);
        let Some(reference) = full(&m) else { continue };
        for alg in [Alg::Sleep, Alg::Sdpor(SdporConfig::STATEFUL_SLEEP)] {
            mutation::mute(mask);
            let r = run(alg, &m);
            mutation::mute(0);
            if discrepancy(&reference, &r, &m).is_some() {
                killed.push(format!("{name}: {}", alg.name()));
            }
        }
    }
    eprintln!("lock accesses deleted: noticed by {killed:?}");
    assert!(!killed.is_empty());
}

// ---------------------------------------------------------------------
// PostgreSQL differential matrix
// ---------------------------------------------------------------------

use midwrite_model::oracle::{PgEnv, SelectForm, run_schedule};
use midwrite_model::witness::enumerate_schedules;

fn env_or_skip() -> Option<PgEnv> {
    match PgEnv::start() {
        Ok(env) => Some(env),
        Err(reason) => {
            assert!(
                std::env::var_os("MIDWRITE_REQUIRE_REPLAY").is_none(),
                "PostgreSQL is required but unavailable: {reason}"
            );
            eprintln!("skipping: {reason}");
            None
        }
    }
}

/// Attempts for a schedule whose deadlock victim PostgreSQL chose
/// differently from the one the schedule steers to (as in `pg_matrix.rs`).
const VICTIM_ATTEMPTS: usize = 3;

#[derive(Default)]
struct Tally {
    configs: usize,
    schedules: usize,
    agreeing: usize,
    statements: usize,
    blocked: usize,
    victim_retries: usize,
    sqlstates: std::collections::BTreeMap<String, usize>,
    sql: std::collections::BTreeSet<String>,
    mismatches: Vec<String>,
}

/// Replay every schedule of `model` against PostgreSQL in lockstep.
fn differential(env: &mut PgEnv, label: &str, model: &Model, tally: &mut Tally) {
    let (all, truncated) = enumerate_schedules(model, 20_000);
    assert!(!all.is_empty() && !truncated, "{label}: schedules");
    let db = env
        .database(model)
        .unwrap_or_else(|e| panic!("{label}: {e}"));
    tally.configs += 1;
    for schedule in &all {
        let mut attempt = 1;
        let result = loop {
            let r = run_schedule(env, &db, model, schedule);
            let victim_differs = matches!(&r, Ok(run) if !run.agree
                && run
                    .observations
                    .iter()
                    .filter(|o| !o.agree)
                    .all(|o| o.sql == "(deadlock detection)"));
            if victim_differs && attempt < VICTIM_ATTEMPTS {
                attempt += 1;
                tally.victim_retries += 1;
                continue;
            }
            break r;
        };
        tally.schedules += 1;
        match result {
            Ok(run) => {
                for o in &run.observations {
                    if o.sql.starts_with("SELECT") || o.sql.starts_with("WITH") {
                        tally.statements += 1;
                        tally.sql.insert(o.sql.clone());
                    }
                    if o.observed.starts_with("blocked") {
                        tally.blocked += 1;
                    }
                }
                for c in &run.sqlstates {
                    *tally.sqlstates.entry(c.clone()).or_default() += 1;
                }
                if run.agree {
                    tally.agreeing += 1;
                } else {
                    let bad: Vec<String> = run
                        .observations
                        .iter()
                        .filter(|o| !o.agree)
                        .map(|o| {
                            format!(
                                "#{} {}: {} → predicted {} observed {}",
                                o.transition, o.actor, o.sql, o.predicted, o.observed
                            )
                        })
                        .collect();
                    tally.mismatches.push(format!(
                        "{label} schedule {:?}\n   {}{}",
                        schedule
                            .iter()
                            .map(|t| format!("{:?}{}.{}", t.kind, t.actor, t.thread))
                            .collect::<Vec<_>>(),
                        bad.join("\n   "),
                        if run.tables_agree {
                            String::new()
                        } else {
                            format!(
                                "\n   tables: model {:?} postgres {:?}",
                                run.model_tables, run.postgres_tables
                            )
                        }
                    ));
                }
            }
            Err(e) => tally
                .mismatches
                .push(format!("{label}: infrastructure error: {e}")),
        }
    }
}

/// `program` with `order by @id` in its multi-row selects (Drizzle
/// `.orderBy(t.id)`).
fn order_by_id(program: &str) -> &'static str {
    let lines: Vec<String> = program
        .lines()
        .map(|l| {
            if l.contains("select all") {
                l.replacen(") for ", ") order by @id for ", 1)
            } else {
                l.to_owned()
            }
        })
        .collect();
    Box::leak(format!("{}\n", lines.join("\n")).into_boxed_str())
}

#[test]
fn without_order_by_postgresql_locks_in_physical_order_which_the_model_does_not_follow() {
    // The approximation, measured: Drizzle's statement without `ORDER BY`
    // over rows inserted out of key order. PostgreSQL's scan of the tiny
    // table returns (and locks) rows in physical order; the model locks in
    // key order. Some schedules then disagree (row order, which row a
    // statement waits on, a deadlock).
    let Some(mut env) = env_or_skip() else { return };
    env.set_select_form(SelectForm::Drizzle);
    let mut tally = Tally::default();
    for name in ["reversed-order", "overlapping-sets"] {
        let (a, b) = workload(name);
        differential(
            &mut env,
            &format!("{name}/read-committed/drizzle-sql/shuffled-heap"),
            &pair_on("read-committed", a, b, shuffled_rows()),
            &mut tally,
        );
    }
    eprintln!(
        "without ORDER BY over a heap in order 3, 1, 2: {} of {} schedule(s) disagree",
        tally.schedules - tally.agreeing,
        tally.schedules
    );
    for m in tally.mismatches.iter().take(3) {
        eprintln!("{m}");
    }
    assert!(tally.agreeing < tally.schedules);
}

#[test]
fn the_model_agrees_with_postgresql_on_multi_row_locks() {
    let Some(mut env) = env_or_skip() else { return };
    eprintln!("PostgreSQL: {}", env.version());
    let mut tally = Tally::default();
    for (name, a, b, _) in workloads() {
        for iso in ISOLATIONS {
            differential(
                &mut env,
                &format!("{name}/{iso}"),
                &pair(iso, a, b),
                &mut tally,
            );
        }
    }
    let oracle_form = tally.schedules;
    // The statements as Drizzle sends them, for the workloads whose
    // predicates are on the primary key:
    env.set_select_form(SelectForm::Drizzle);
    // (1) without `ORDER BY`, over rows whose physical order is key order
    // (inserted in key order): PostgreSQL's bitmap or sequential scan then
    // returns key order too;
    for (name, a, b, key_only) in workloads() {
        if !key_only {
            continue;
        }
        for iso in ISOLATIONS {
            differential(
                &mut env,
                &format!("{name}/{iso}/drizzle-sql"),
                &pair(iso, a, b),
                &mut tally,
            );
        }
    }
    let drizzle_plain = tally.schedules - oracle_form;
    // (2) with `.orderBy(t.id)` (`ORDER BY id` sent), over rows inserted
    // out of key order (physical order 3, 1, 2): locks follow the sort.
    for (name, a, b, key_only) in workloads() {
        if !key_only {
            continue;
        }
        for iso in ISOLATIONS {
            differential(
                &mut env,
                &format!("{name}/{iso}/drizzle-sql/order-by-id/shuffled-heap"),
                &pair_on(iso, order_by_id(a), order_by_id(b), shuffled_rows()),
                &mut tally,
            );
        }
    }
    let drizzle_ordered = tally.schedules - oracle_form - drizzle_plain;
    env.set_select_form(SelectForm::Ordered);
    eprintln!(
        "multi-row lock matrix: {} configuration(s), {} schedule(s) ({} in the oracle's form, {} as Drizzle sends them without orderBy, {} with orderBy(id) over an out-of-order heap), {} agreeing; {} statement(s) compared, {} lock wait(s) observed, {} deadlock-victim retry(ies); SQLSTATEs {:?}",
        tally.configs,
        tally.schedules,
        oracle_form,
        drizzle_plain,
        drizzle_ordered,
        tally.agreeing,
        tally.statements,
        tally.blocked,
        tally.victim_retries,
        tally.sqlstates
    );
    eprintln!("distinct SQL texts of multi-row selects:");
    for sql in tally.sql.iter().filter(|s| s.contains("row_to_json(t)")) {
        eprintln!("  {sql}");
    }
    assert!(
        tally.mismatches.is_empty(),
        "{} disagreement(s):\n{}",
        tally.mismatches.len(),
        tally.mismatches.join("\n")
    );
    assert_eq!(tally.agreeing, tally.schedules);
}

// ---------------------------------------------------------------------
// PostgreSQL facts the model relies on
// ---------------------------------------------------------------------

/// A private cluster with `t(id, v)` holding rows 3, 1, 2 (heap order out
/// of key order); scripts run in their own `psql` sessions with `\set
/// VERBOSITY verbose` (SQLSTATEs in the output).
struct Facts {
    _root: midwrite_replay::safety::ReplayRoot,
    cluster: midwrite_replay::postgres::Cluster,
    bins: midwrite_replay::postgres::Binaries,
}

impl Facts {
    fn start() -> Option<Self> {
        use midwrite_replay::postgres::{Binaries, Cluster};
        let started = (|| {
            let bins = Binaries::discover()?;
            let root = midwrite_replay::safety::ReplayRoot::create(false)?;
            let cluster =
                Cluster::start_with(bins.clone(), &root.path, &["autovacuum=off".to_owned()])?;
            cluster.create_database("facts")?;
            Ok::<_, String>(Self {
                _root: root,
                cluster,
                bins,
            })
        })();
        match started {
            Ok(f) => {
                let out = output(f.spawn(
                    "CREATE TABLE t (id int PRIMARY KEY, v int);\nINSERT INTO t VALUES (3, 0), (1, 0), (2, 0);\n",
                ));
                assert!(!out.contains("ERROR"), "{out}");
                Some(f)
            }
            Err(reason) => {
                assert!(
                    std::env::var_os("MIDWRITE_REQUIRE_REPLAY").is_none(),
                    "PostgreSQL is required but unavailable: {reason}"
                );
                eprintln!("skipping: {reason}");
                None
            }
        }
    }

    /// Start `script` in its own session (output collected by [`output`]).
    fn spawn(&self, script: &str) -> std::process::Child {
        use std::io::Write;
        let mut c = std::process::Command::new(self.bins.dir.join("psql"));
        c.args(["-X", "-q", "-A", "-t", "-v", "ON_ERROR_STOP=0"])
            .arg("-h")
            .arg(self.cluster.socket_dir())
            .args([
                "-p",
                "5432",
                "-U",
                midwrite_replay::postgres::USER,
                "-d",
                "facts",
            ])
            .env_clear()
            .env("LC_ALL", "C")
            .env("PGPASSWORD", self.cluster.superuser_env().1)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(path) = std::env::var_os("PATH") {
            c.env("PATH", path);
        }
        let mut child = c.spawn().expect("psql");
        child
            .stdin
            .take()
            .expect("piped")
            .write_all(format!("\\set VERBOSITY verbose\n{script}").as_bytes())
            .expect("psql input");
        child
    }

    fn script(&self, script: &str) -> String {
        output(self.spawn(script))
    }
}

fn output(child: std::process::Child) -> String {
    let out = child.wait_with_output().expect("psql output");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn first_column(rows: &str) -> Vec<String> {
    rows.lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.split('|').next().unwrap_or_default().to_owned())
        .collect()
}

/// Rows a statement returned (and locked), and its plan.
fn locked_order(f: &Facts, sql: &str) -> (Vec<String>, String) {
    let plan = f.script(&format!("EXPLAIN (COSTS OFF) {sql};\n"));
    let rows = f.script(&format!("BEGIN;\n{sql};\nROLLBACK;\n"));
    eprintln!("{sql}\n  -> {:?}\n{plan}", first_column(&rows));
    (first_column(&rows), plan)
}

#[test]
fn postgresql_locks_in_the_plan_order_without_order_by_and_in_key_order_with_it() {
    let Some(f) = Facts::start() else { return };
    // Drizzle's statements for `inArray(t.id, [2, 3, 1])` on the 3-row
    // table whose physical order is 3, 1, 2 (no statistics, as in a fresh
    // test database): a bitmap or sequential scan, physical order.
    let (rows, plan) = locked_order(&f, "SELECT * FROM t WHERE id IN (2, 3, 1) FOR UPDATE");
    assert!(
        plan.contains("Bitmap Heap Scan") || plan.contains("Seq Scan"),
        "{plan}"
    );
    assert_eq!(rows, ["3", "1", "2"]);
    let (rows, _) = locked_order(&f, "SELECT * FROM t WHERE v = 0 FOR UPDATE");
    assert_eq!(rows, ["3", "1", "2"]);
    // `.orderBy(t.id)`: LockRows above the sort, key order.
    for sql in [
        "SELECT * FROM t WHERE id IN (2, 3, 1) ORDER BY id FOR UPDATE",
        "SELECT * FROM t WHERE v = 0 ORDER BY id FOR UPDATE",
        "SELECT * FROM t WHERE id IN (2, 3, 1) ORDER BY id LIMIT 2 FOR UPDATE",
    ] {
        let (rows, plan) = locked_order(&f, sql);
        assert!(
            plan.starts_with("LockRows") || plan.contains("LockRows"),
            "{plan}"
        );
        let want: &[&str] = if sql.contains("LIMIT 2") {
            &["1", "2"]
        } else {
            &["1", "2", "3"]
        };
        assert_eq!(rows, want, "{sql}");
    }
    // A larger table with statistics, inserted in descending key order: the
    // plan decides the order (an index scan returns key order, a bitmap
    // scan physical order); whichever it is, the observed order follows it.
    let out = f.script(
        "CREATE TABLE big (id int PRIMARY KEY, v int);\nINSERT INTO big SELECT g, 0 FROM generate_series(10000, 1, -1) g;\nANALYZE big;\n",
    );
    assert!(!out.contains("ERROR"), "{out}");
    let (rows, plan) = locked_order(&f, "SELECT * FROM big WHERE id IN (2, 3, 1) FOR UPDATE");
    if plan.contains("Bitmap Heap Scan") || plan.contains("Seq Scan") {
        assert_eq!(rows, ["3", "2", "1"], "physical order: {plan}");
    } else {
        assert!(plan.contains("Index Scan using big_pkey"), "{plan}");
        assert_eq!(rows, ["1", "2", "3"], "key order: {plan}");
    }
}

#[test]
fn postgresql_releases_the_locks_of_a_failed_nowait_statement_at_the_error() {
    // PostgreSQL aborts the transaction when the statement fails (the
    // session then waits for ROLLBACK in state "idle in transaction
    // (aborted)"): the rows the statement had locked are free at once. The
    // model releases them in the failing step too.
    let Some(f) = Facts::start() else { return };
    let step = std::time::Duration::from_millis(800);
    // B holds row 2.
    let b = f.spawn("BEGIN;\nSELECT id FROM t WHERE id = 2 FOR UPDATE;\n\\! sleep 4\nCOMMIT;\n");
    std::thread::sleep(step);
    // A locks rows 3 and 1 (physical order), then fails at row 2 (55P03);
    // it sends ROLLBACK only two seconds later.
    let a = f.spawn(
        "BEGIN;\nSELECT id FROM t WHERE id IN (1, 2, 3) FOR UPDATE NOWAIT;\n\\! sleep 2\nROLLBACK;\n",
    );
    std::thread::sleep(step);
    let probe = f.script(
        "SELECT state FROM pg_stat_activity WHERE state LIKE 'idle in transaction%' ORDER BY state;\nSELECT id FROM t WHERE id IN (1, 3) ORDER BY id FOR UPDATE NOWAIT;\n",
    );
    let a = output(a);
    let b = output(b);
    eprintln!("A: {a}\nprobe while A waits for its ROLLBACK: {probe}\nB: {b}");
    assert!(a.contains("55P03"), "A fails with 55P03: {a}");
    assert!(
        probe.contains("idle in transaction (aborted)"),
        "A had not ended: {probe}"
    );
    assert!(!probe.contains("55P03"), "{probe}");
    let rows = first_column(&probe);
    assert!(
        rows.ends_with(&["1".to_owned(), "3".to_owned()]),
        "rows 1 and 3 are free: {probe}"
    );
}

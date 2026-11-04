//! Phase 13: foreign keys and row-lock modes in the model (semantics,
//! loader refusals, reductions). The PostgreSQL lockstep matrix of the same
//! workloads is `pg_matrix::foreign_key_lock_workloads`.

mod common;

use common::fk::*;
use common::por_diff::{Alg, discrepancy, full, run};
use midwrite_model::db::RowLock;
use midwrite_model::engine::{Recorder, apply, enabled, initial_state};
use midwrite_model::explore::{SdporConfig, audit_independence};
use midwrite_model::model::Model;
use midwrite_model::scenario::load_str;
use midwrite_model::state::{State, TKind};
use serde_json::json;

#[test]
fn row_lock_modes_conflict_as_postgresql_documents() {
    use RowLock::*;
    let modes = [KeyShare, Share, NoKeyUpdate, Update];
    // PostgreSQL 17 documentation, "Conflicting Row-Level Locks".
    let expected = [
        [false, false, false, true],
        [false, false, true, true],
        [false, true, true, true],
        [true, true, true, true],
    ];
    for (i, a) in modes.iter().enumerate() {
        for (j, b) in modes.iter().enumerate() {
            assert_eq!(a.conflicts(*b), expected[i][j], "{a:?} vs {b:?}");
        }
    }
}

/// Step actor `who` (a Step or a Resume) and return the next state.
fn step(model: &Model, s: &State, who: u16) -> State {
    let t = enabled(model, s)
        .into_iter()
        .find(|t| t.actor == who && matches!(t.kind, TKind::Step | TKind::Resume))
        .unwrap_or_else(|| panic!("actor {who} cannot move"));
    apply(model, s, t, &mut Recorder::off())
        .expect("supported")
        .state
}

fn blocked(s: &State, who: usize) -> bool {
    s.actors[who].threads[0]
        .pending
        .as_ref()
        .is_some_and(|p| p.blocked_on.is_some())
}

fn failure(s: &State, who: usize) -> Option<String> {
    match &s.actors[who].outcome {
        midwrite_model::state::Outcome::Failed(r) => Some(r.clone()),
        _ => None,
    }
}

#[test]
fn a_child_insert_waits_for_the_parent_row_locked_for_update() {
    // The Phase 12 divergence (`createWorkspaceLocked` × `createWorkspace`):
    // PostgreSQL's foreign-key check locks the owner FOR KEY SHARE, which
    // waits for the other transaction's FOR UPDATE.
    for iso in ["read-committed", "repeatable-read", "serializable"] {
        let m = fk_pair(iso, PARENT_FOR_UPDATE_TX, CHILD_AUTOCOMMIT);
        let s = initial_state(&m);
        let s = step(&m, &s, 0); // BEGIN
        let s = step(&m, &s, 0); // SELECT ... FOR UPDATE
        let s = step(&m, &s, 1); // INSERT: the check waits
        assert!(blocked(&s, 1), "{iso}: the insert must wait");
        let s = step(&m, &s, 0); // A's INSERT
        let s = step(&m, &s, 0); // COMMIT
        let s = step(&m, &s, 1); // resumes, succeeds
        assert_eq!(failure(&s, 1), None, "{iso}");
        assert_eq!(s.committed_rows(&m, 1).len(), 3, "{iso}");
    }
}

#[test]
fn a_child_insert_does_not_wait_for_a_non_key_update_of_the_parent() {
    let m = fk_pair("read-committed", PARENT_RENAME_TX, CHILD_AUTOCOMMIT);
    let s = initial_state(&m);
    let s = step(&m, &s, 0); // BEGIN
    let s = step(&m, &s, 0); // UPDATE users SET name (FOR NO KEY UPDATE)
    let s = step(&m, &s, 1); // INSERT: FOR KEY SHARE is compatible
    assert!(!blocked(&s, 1));
    assert_eq!(failure(&s, 1), None);
}

#[test]
fn a_deleted_parent_fails_the_waiting_child_insert() {
    // READ COMMITTED: 23503 after the wait; REPEATABLE READ: 40001.
    for (iso, sqlstate) in [("read-committed", "23503"), ("repeatable-read", "40001")] {
        let m = fk_pair(iso, PARENT_DELETE_TX, SNAPSHOT_THEN_CHILD_TX);
        let s = initial_state(&m);
        let s = step(&m, &s, 1); // B: BEGIN
        let s = step(&m, &s, 1); // B: SELECT (takes the snapshot under RR)
        let s = step(&m, &s, 0); // A: BEGIN
        let s = step(&m, &s, 0); // A: DELETE users(1)
        let s = step(&m, &s, 1); // B: INSERT, check waits for A's FOR UPDATE
        assert!(blocked(&s, 1), "{iso}");
        let s = step(&m, &s, 0); // A: COMMIT
        let s = step(&m, &s, 1); // B resumes
        let reason = failure(&s, 1).unwrap_or_default();
        assert!(reason.contains(sqlstate), "{iso}: {reason}");
    }
}

#[test]
fn deleting_a_parent_waits_for_an_uncommitted_child_and_then_fails() {
    let m = fk_pair("read-committed", CHILD_TX, PARENT_DELETE);
    let s = initial_state(&m);
    let s = step(&m, &s, 0); // BEGIN
    let s = step(&m, &s, 0); // INSERT child (holds FOR KEY SHARE on users(1))
    let s = step(&m, &s, 1); // DELETE users(1): FOR UPDATE waits
    assert!(blocked(&s, 1));
    let s = step(&m, &s, 0); // COMMIT
    let s = step(&m, &s, 1); // the NO ACTION check finds the child
    let reason = failure(&s, 1).unwrap_or_default();
    assert!(reason.contains("23503"), "{reason}");
    // After a rollback of the child, the delete succeeds.
    let m = fk_pair("read-committed", CHILD_ROLLBACK, PARENT_DELETE);
    let s = initial_state(&m);
    let s = step(&m, &s, 0);
    let s = step(&m, &s, 0);
    let s = step(&m, &s, 1);
    assert!(blocked(&s, 1));
    let s = step(&m, &s, 0); // ROLLBACK
    let s = step(&m, &s, 1);
    assert_eq!(failure(&s, 1), None);
}

#[test]
fn models_without_shared_lock_modes_keep_the_phase_12_encoding() {
    // No foreign keys, no FOR [KEY] SHARE: lock modes are not part of the
    // state (every two locks conflict), so state identity is unchanged.
    let m = common::purchase("read-committed", common::PURCHASE_FOR_UPDATE, json!({}));
    assert!(!m.uses_lock_modes());
    let m = fk_pair("read-committed", PARENT_RENAME_TX, CHILD_AUTOCOMMIT);
    assert!(m.uses_lock_modes());
}

fn load(doc: serde_json::Value) -> Result<Model, String> {
    load_str(
        &doc.to_string(),
        std::path::Path::new("."),
        &midwrite_model::provider::NoProvider,
    )
    .map(|(m, _)| m)
    .map_err(|e| e.to_string())
}

fn fk_doc(fk: serde_json::Value, program: &str) -> serde_json::Value {
    let mut tables = fk_tables();
    tables[1]["foreignKeys"] = json!([fk]);
    json!({
        "schemaVersion": 1, "name": "fk",
        "database": { "tables": tables, "initialState": { "users": [{"id": 1}] } },
        "actors": [{ "name": "A", "function": "a", "program": program, "arguments": {} }],
        "invariants": [{ "name": "true", "assert": { "eq": [0, 0] } }]
    })
}

#[test]
fn foreign_key_behavior_the_model_does_not_execute_is_refused() {
    let base = json!({ "columns": ["owner"], "references": "users", "referencedColumns": ["id"] });
    // ON DELETE CASCADE with a parent DELETE.
    let mut cascade = base.clone();
    cascade["onDelete"] = json!("cascade");
    let e = load(fk_doc(cascade.clone(), PARENT_DELETE)).unwrap_err();
    assert!(e.contains("ON DELETE CASCADE"), "{e}");
    // ... is fine when nothing deletes a parent.
    assert!(load(fk_doc(cascade, CHILD_AUTOCOMMIT)).is_ok());
    // A check deferred to COMMIT.
    let mut deferred = base.clone();
    deferred["initiallyDeferred"] = json!(true);
    let e = load(fk_doc(deferred, CHILD_AUTOCOMMIT)).unwrap_err();
    assert!(e.contains("INITIALLY DEFERRED"), "{e}");
    // A reference to a column that is not a key (PostgreSQL refuses it).
    let mut not_key = base.clone();
    not_key["referencedColumns"] = json!(["name"]);
    let e = load(fk_doc(not_key, CHILD_AUTOCOMMIT)).unwrap_err();
    assert!(e.contains("not the primary key"), "{e}");
    // An unknown table.
    let mut unknown = base;
    unknown["references"] = json!("accounts");
    assert!(load(fk_doc(unknown, CHILD_AUTOCOMMIT)).is_err());
}

#[test]
fn an_unknown_feature_is_refused() {
    let mut doc = fk_doc(
        json!({ "columns": ["owner"], "references": "users", "referencedColumns": ["id"] }),
        CHILD_AUTOCOMMIT,
    );
    doc["features"] = json!(["time-travel"]);
    assert!(load(doc).unwrap_err().contains("unknown feature"));
}

/// Three actors on one parent row: a locker, a child insert and a delete.
fn three(iso: &str) -> Model {
    midwrite_model::scenario::testing::model_from_json(json!({
        "schemaVersion": 1,
        "name": "fk-three",
        "database": {
            "isolation": iso,
            "tables": fk_tables(),
            "initialState": { "users": [{"id": 1, "name": "a"}], "workspaces": [] }
        },
        "actors": [
            { "name": "A", "function": "a", "program": PARENT_FOR_UPDATE_TX, "arguments": {} },
            { "name": "B", "function": "b", "program": CHILD_TX, "arguments": {} },
            { "name": "C", "function": "c", "program": PARENT_DELETE, "arguments": {} }
        ],
        "invariants": [{ "name": "no-orphan", "assert": { "le": [
            { "count": { "table": "workspaces", "where": { "owner": 1 } } },
            1
        ] } }]
    }))
}

#[test]
fn reductions_preserve_foreign_key_lock_behavior() {
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
        for (label, a, b) in fk_workloads() {
            models.push((format!("{label}/{iso}"), fk_pair(iso, a, b)));
        }
    }
    for iso in ["read-committed", "repeatable-read", "serializable"] {
        models.push((format!("three/{iso}"), three(iso)));
        models.push((
            format!("claim/{iso}"),
            tasks_model(CLAIM, at_most_one_success(), iso),
        ));
        models.push((
            format!("claim-atomic/{iso}"),
            tasks_model(CLAIM_ATOMIC, at_most_one_success(), iso),
        ));
    }
    for iso in ["read-committed", "repeatable-read"] {
        for existing in [false, true] {
            for (label, a, b) in upsert_workloads() {
                models.push((
                    format!("upsert/{label}/{existing}/{iso}"),
                    upsert_pair(iso, a, b, existing),
                ));
            }
        }
    }
    models.push((
        "signup-response".into(),
        signup_model(RECORD_SIGNUP, true, false),
    ));
    models.push((
        "signup-idempotent-response".into(),
        signup_model(RECORD_SIGNUP_IDEMPOTENT, true, true),
    ));
    let mut compared = 0;
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
        let Some(f) = full(m) else { continue };
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
    assert_eq!(compared, models.len() * algs.len());
}

// ---------------------------------------------------------------------
// Track B: return values, the response step, crash after the last
// statement.
// ---------------------------------------------------------------------

use midwrite_model::explore::{Verdict, explore};

fn tasks_model(program: &str, invariant: serde_json::Value, iso: &str) -> Model {
    midwrite_model::scenario::testing::model_from_json(json!({
        "schemaVersion": 1,
        "name": "claims",
        "database": {
            "isolation": iso,
            "tables": [{ "name": "tasks", "columns": [
                {"name": "id", "notNull": true, "type": "int"},
                {"name": "assignee", "type": "text"}
              ], "primaryKey": ["id"] }],
            "initialState": { "tasks": [{"id": 1, "assignee": null}] }
        },
        "actors": [
            { "name": "A", "function": "claimTask", "program": program, "arguments": {"who": "a"} },
            { "name": "B", "function": "claimTask", "program": program, "arguments": {"who": "b"} }
        ],
        "invariants": [invariant]
    }))
}

/// The Phase 12 evaluation case `claimTask`: read, then assign
/// unconditionally; both callers can be told they claimed the task.
const CLAIM: &str = "\
let task = select tasks where @id == 1
if task == null {
  fail \"no task\"
}
if task.assignee != null {
  return false
}
update tasks where @id == 1 set @assignee = who
return true
";

/// `claimTaskAtomic`: the conditional UPDATE's row count decides.
const CLAIM_ATOMIC: &str = "\
update tasks where @id == 1 && @assignee == null set @assignee = who into n
return n > 0
";

fn at_most_one_success() -> serde_json::Value {
    json!({ "name": "at-most-one-claimant-told-success", "assert": { "le": [
        { "actors": { "function": "claimTask", "outcome": "ok", "returned": true } }, 1
    ] } })
}

#[test]
fn two_callers_can_both_be_told_they_claimed_the_task() {
    // Every statement autocommits on its own, so no isolation level helps.
    for iso in ["read-committed", "repeatable-read", "serializable"] {
        let m = tasks_model(CLAIM, at_most_one_success(), iso);
        assert!(m.tracks_results());
        let r = explore(&m);
        assert_eq!(r.verdict, Verdict::CounterexampleFound, "{iso}");
        let ce = r.counterexample.expect("a counterexample");
        let _ = ce;
    }
    // The database state alone is acceptable in every schedule: one
    // assignee. The violation is only in what the callers observe.
    let db_only = json!({ "name": "one-assignee", "assert": { "le": [
        { "count": { "table": "tasks", "where": { "id": 1 } } }, 1
    ] } });
    let m = tasks_model(CLAIM, db_only, "read-committed");
    assert!(!m.tracks_results());
    assert_eq!(explore(&m).verdict, Verdict::NoCounterexampleWithinBounds);
}

#[test]
fn the_atomic_claim_tells_at_most_one_caller_success() {
    for iso in ["read-committed", "repeatable-read", "serializable"] {
        let m = tasks_model(CLAIM_ATOMIC, at_most_one_success(), iso);
        let r = explore(&m);
        assert_eq!(r.verdict, Verdict::NoCounterexampleWithinBounds, "{iso}");
        assert!(r.unsupported.is_empty());
    }
}

#[test]
fn return_values_not_read_by_an_invariant_leave_the_state_space_unchanged() {
    let true_inv = json!({ "name": "true", "assert": { "eq": [0, 0] } });
    let with = tasks_model(CLAIM, true_inv.clone(), "read-committed");
    let without = tasks_model(
        &CLAIM
            .replace("return false", "return")
            .replace("return true", "return"),
        true_inv,
        "read-committed",
    );
    assert!(!with.tracks_results());
    let (a, b) = (explore(&with), explore(&without));
    assert_eq!(a.metrics.states, b.metrics.states);
    assert_eq!(a.metrics.transitions, b.metrics.transitions);
}

#[test]
fn an_unknown_return_value_read_by_an_invariant_is_refused() {
    use midwrite_model::ir::{Op, SourceRef, Step};
    use midwrite_model::value::Expr;
    let codes = vec![vec![
        Step {
            op: Op::ReturnValue {
                expr: Expr::Unknown("a call into an unanalyzed package".into()),
            },
            src: SourceRef::declared(1, "return verify(x)"),
        },
        Step {
            op: Op::Return,
            src: SourceRef::declared(1, "return verify(x)"),
        },
    ]];
    let doc = json!({
        "schemaVersion": 1, "name": "unknown-return",
        "database": { "tables": [{ "name": "tasks", "columns": [{"name": "id", "notNull": true}], "primaryKey": ["id"] }] },
        "actors": [{ "name": "A", "function": "claimTask", "codes": codes, "arguments": {} }],
        "invariants": [at_most_one_success()]
    });
    let e = load(doc.clone()).unwrap_err();
    assert!(e.contains("is not modeled"), "{e}");
    // Without an invariant over return values, the scenario loads.
    let mut ok = doc;
    ok["invariants"] = json!([{ "name": "true", "assert": { "eq": [0, 0] } }]);
    assert!(load(ok).is_ok());
}

fn signup_model(program: &str, response: bool, unique_request: bool) -> Model {
    let mut columns = json!([
        {"name": "id", "notNull": true, "type": "text",
         "default": { "generated": { "side": "database", "generator": "gen_random_uuid()" } }},
        {"name": "account", "notNull": true, "type": "int"},
        {"name": "request", "type": "text"}
    ]);
    let _ = &mut columns;
    midwrite_model::scenario::testing::model_from_json(json!({
        "schemaVersion": 1,
        "name": "signup",
        "database": {
            "tables": [{ "name": "events", "columns": columns, "primaryKey": ["id"],
                         "unique": if unique_request { json!([["request"]]) } else { json!([]) } }],
            "initialState": {}
        },
        "actors": [{
            "name": "client", "function": "recordSignup", "program": program,
            "arguments": {"account": 1, "rid": "r-1"},
            "retry": { "trigger": "onCrash" },
            "response": response
        }],
        "bounds": { "maxCrashes": 1, "maxRetries": 1 },
        "invariants": [{ "name": "one-signup-event", "assert": { "le": [
            { "count": { "table": "events", "where": { "account": 1 } } }, 1
        ] } }]
    }))
}

/// The Phase 12 evaluation case `recordSignup`: one INSERT, the response,
/// and a client that resubmits after a crash.
const RECORD_SIGNUP: &str = "\
insert events set @account = account, @request = rid
return true
";
const RECORD_SIGNUP_IDEMPOTENT: &str = "\
insert events set @account = account, @request = rid on conflict do nothing
return true
";

#[test]
fn a_crash_after_the_last_statement_and_before_the_response_duplicates_the_write() {
    // Without a response step the crash points are before the INSERT only:
    // the Phase 12 false negative.
    let m = signup_model(RECORD_SIGNUP, false, false);
    assert_eq!(explore(&m).verdict, Verdict::NoCounterexampleWithinBounds);
    // With the response as its own step, the write can commit, the process
    // crash before the response, and the resubmission insert again.
    let m = signup_model(RECORD_SIGNUP, true, false);
    let r = explore(&m);
    assert_eq!(r.verdict, Verdict::CounterexampleFound);
    let ce = r.counterexample.expect("a counterexample");
    let kinds: Vec<TKind> = ce.schedule.iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![
            TKind::Step,
            TKind::Crash,
            TKind::Retry,
            TKind::Step,
            TKind::Step
        ],
        "insert, crash before the response, retry, insert, respond"
    );
    // The request id as a unique key makes the resubmission harmless.
    let m = signup_model(RECORD_SIGNUP_IDEMPOTENT, true, true);
    assert_eq!(explore(&m).verdict, Verdict::NoCounterexampleWithinBounds);
}

#[test]
fn responses_are_inserted_before_every_return_and_keep_jumps() {
    let m = tasks_model(CLAIM, at_most_one_success(), "read-committed");
    let main = &m.actors[0].codes[0];
    assert!(
        !main
            .iter()
            .any(|s| matches!(s.op, midwrite_model::ir::Op::Respond))
    );
    let mut doc_actor = json!({ "name": "A", "function": "claimTask", "program": CLAIM, "arguments": {"who": "a"}, "response": true });
    let doc = json!({
        "schemaVersion": 1, "name": "claims",
        "database": { "tables": [{ "name": "tasks", "columns": [
            {"name": "id", "notNull": true, "type": "int"}, {"name": "assignee", "type": "text"}
          ], "primaryKey": ["id"] }], "initialState": { "tasks": [{"id": 1, "assignee": null}] } },
        "actors": [doc_actor.take()],
        "invariants": [at_most_one_success()]
    });
    let m = load(doc).unwrap();
    let main = &m.actors[0].codes[0];
    use midwrite_model::ir::Op;
    for (i, s) in main.iter().enumerate() {
        if matches!(s.op, Op::Return) {
            assert!(matches!(main[i - 1].op, Op::Respond), "{i}: {main:#?}");
        }
        if let Op::JumpIf { target, .. } | Op::Jump { target } = &s.op {
            assert!(
                !matches!(main[*target].op, Op::Return),
                "a jump skips its response"
            );
        }
    }
    // Both paths (claimed / already taken) respond: one counterexample run.
    let r = explore(&m);
    assert!(r.unsupported.is_empty());
}

// ---------------------------------------------------------------------
// Track C: a lagging read replica.
// ---------------------------------------------------------------------

fn replica_model(programs: &[(&str, &str)], features: serde_json::Value) -> Result<Model, String> {
    load(json!({
        "schemaVersion": 1,
        "name": "replicas",
        "features": features,
        "database": {
            "tables": [{ "name": "profiles", "columns": [
                {"name": "id", "notNull": true, "type": "int"},
                {"name": "name", "type": "text"}
              ], "primaryKey": ["id"] }],
            "initialState": { "profiles": [{"id": 1, "name": "old"}] }
        },
        "actors": programs.iter().map(|(name, program)| json!({
            "name": name, "function": "updateProfile", "program": program, "arguments": {}
        })).collect::<Vec<_>>(),
        "invariants": [{ "name": "the-caller-sees-its-write", "assert": { "eq": [
            { "actors": { "function": "updateProfile", "outcome": "ok", "returned": "new" } },
            { "actors": { "function": "updateProfile", "outcome": "ok" } }
        ] } }]
    }))
}

/// Write through the primary, then read the row back through the replica
/// (a `withReplicas` database's `select`).
const WRITE_THEN_REPLICA_READ: &str = "\
update profiles where @id == 1 set @name = \"new\"
let p = replica select profiles where @id == 1
return p.name
";
/// The same with the read on the primary (`db.$primary`).
const WRITE_THEN_PRIMARY_READ: &str = "\
update profiles where @id == 1 set @name = \"new\"
let p = select profiles where @id == 1
return p.name
";

#[test]
fn a_replica_read_after_a_primary_write_can_be_stale() {
    let m = replica_model(&[("A", WRITE_THEN_REPLICA_READ)], json!(["replicas"])).unwrap();
    let r = explore(&m);
    assert_eq!(r.verdict, Verdict::CounterexampleFound);
    let ce = r.counterexample.unwrap();
    // The read happens before the replica replays the commit.
    assert!(!ce.schedule.iter().any(|t| t.kind == TKind::Replicate));
    // Once the replica has replayed it, the read sees the write: a schedule
    // with the replay first exists and agrees.
    let m2 = m.clone();
    let mut s = initial_state(&m2);
    s = step(&m2, &s, 0); // UPDATE (commits)
    let t = enabled(&m2, &s)
        .into_iter()
        .find(|t| t.kind == TKind::Replicate)
        .expect("the replica is behind");
    s = apply(&m2, &s, t, &mut Recorder::off()).unwrap().state;
    s = step(&m2, &s, 0); // replica SELECT
    assert_eq!(
        s.actors[0].returned,
        vec![Ok(midwrite_model::value::Value::str("new"))]
    );
}

#[test]
fn a_primary_read_after_the_write_is_not_stale() {
    let m = replica_model(&[("A", WRITE_THEN_PRIMARY_READ)], json!(["replicas"])).unwrap();
    assert_eq!(explore(&m).verdict, Verdict::NoCounterexampleWithinBounds);
}

#[test]
fn replica_reads_need_the_feature_and_must_be_plain_reads() {
    let e = replica_model(&[("A", WRITE_THEN_REPLICA_READ)], json!([])).unwrap_err();
    assert!(e.contains("feature `replicas`"), "{e}");
    let locking = "let p = replica select profiles where @id == 1 for update\nreturn p.name\n";
    let e = replica_model(&[("A", locking)], json!(["replicas"])).unwrap_err();
    assert!(e.contains("only plain reads"), "{e}");
    // The lockstep oracle has no replica.
    let m = replica_model(&[("A", WRITE_THEN_REPLICA_READ)], json!(["replicas"])).unwrap();
    assert!(m.features.replicas);
}

#[test]
fn replica_progress_is_part_of_the_state_and_reductions_agree() {
    // Two writers and a replica reader: the replica replays commits in
    // commit order, so a reader may see the first write without the second.
    let reader = "\
let p = replica select profiles where @id == 1
return p.name
";
    let a = "update profiles where @id == 1 set @name = \"new\"\n";
    let b = "update profiles where @id == 1 set @name = \"newer\"\n";
    let m = replica_model(&[("W1", a), ("W2", b), ("R", reader)], json!(["replicas"])).unwrap();
    let violations = audit_independence(&m, 100_000);
    assert!(violations.is_empty(), "{}", violations.len());
    let f = full(&m).expect("small");
    for alg in [
        Alg::Sleep,
        Alg::Classic,
        Alg::Sdpor(SdporConfig {
            cache: true,
            sleep: true,
            no_enabling_edges: false,
            defects: 0,
        }),
    ] {
        let r = run(alg, &m);
        assert_eq!(discrepancy(&f, &r, &m), None, "{}", alg.name());
    }
    // The reader can observe each of: the initial name, "new", "newer".
    let mut seen = std::collections::BTreeSet::new();
    let mut stack = vec![initial_state(&m)];
    let mut visited = std::collections::BTreeSet::new();
    while let Some(s) = stack.pop() {
        if !visited.insert(s.canonical(&m)) {
            continue;
        }
        for r in &s.actors[2].returned {
            seen.insert(format!("{:?}", r));
        }
        for t in enabled(&m, &s) {
            if let Ok(x) = apply(&m, &s, t, &mut Recorder::off()) {
                stack.push(x.state);
            }
        }
    }
    assert_eq!(seen.len(), 3, "{seen:?}");
}

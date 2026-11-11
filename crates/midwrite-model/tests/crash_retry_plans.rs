//! Coordinator plans for crash/retry schedules and recovery probes
//! (no execution: the original-code runs are in midwrite-cli's crash_retry.rs).

use midwrite_model::probe;
use midwrite_model::scenario::testing::model_from_json;
use midwrite_model::source_verify::plan_steps;
use midwrite_model::state::TKind;
use midwrite_model::witness::replay;
use midwrite_replay::source::{PlanStep, SqlKind};
use serde_json::json;

fn payments(program: &str, dedup: &str, retry: serde_json::Value) -> midwrite_model::model::Model {
    model_from_json(json!({
        "schemaVersion": 1,
        "name": "pay",
        "channels": {"charge": {"dedup": dedup}},
        "database": {
            "tables": [{"name": "orders", "columns": [{"name": "id", "notNull": true}, "status"], "primaryKey": ["id"]}],
            "initialState": {"orders": [{"id": 1, "status": "unpaid"}]}
        },
        "actors": [{"name": "Client", "function": "pay", "program": program, "arguments": {"rid": "r1"}, "retry": retry}],
        "bounds": {"maxSteps": 30, "maxCrashes": 1, "maxRetries": 1},
        "invariants": [{"name": "once", "assert": {"le": [{"externals": {"channel": "charge"}}, 1]}}]
    }))
}

const PAY: &str = "\
let o = select orders where @id == 1
if o.status == \"paid\" {
  return
}
external charge key rid set amount = 5
update orders where @id == 1 set @status = \"paid\"
";

#[test]
fn a_probe_selects_the_shortest_complete_schedule_with_the_required_events() {
    let model = payments(PAY, "byKey", json!({"trigger": "onCrash"}));
    let schedule = probe::find(
        &model,
        &[
            "external:charge".into(),
            "crash:Client".into(),
            "retry:Client".into(),
        ],
    )
    .unwrap();
    let kinds: Vec<TKind> = schedule.iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        [
            TKind::Step,
            TKind::Step,
            TKind::Crash,
            TKind::Retry,
            TKind::Step,
            TKind::Step,
            TKind::Step
        ]
    );
    let end = replay(&model, &schedule, false).unwrap().state;
    assert!(end.is_finished());
    // byKey: the second charge with the same key is deduplicated.
    assert_eq!(end.externals.len(), 2);
    assert!(end.externals[1].deduplicated);
    // The plan kills the worker before the UPDATE it has announced, then
    // starts attempt 2.
    let steps = plan_steps(&model, &schedule).unwrap();
    let crash = steps
        .iter()
        .find_map(|s| match s {
            PlanStep::Crash(c) => Some(c),
            _ => None,
        })
        .unwrap();
    let next = crash.next.as_ref().unwrap();
    assert_eq!(next.kind, SqlKind::Update);
    assert_eq!(next.table.as_deref(), Some("orders"));
    assert!(!crash.waiting);
    assert!(
        steps
            .iter()
            .any(|s| matches!(s, PlanStep::Restart(r) if r.attempt == 2))
    );
    // The effects are planned with the key the model evaluated.
    let effects: Vec<_> = steps
        .iter()
        .filter_map(|s| match s {
            PlanStep::Release(e) if e.kind == SqlKind::Effect => Some(e.key.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(effects, [Some(json!("r1")), Some(json!("r1"))]);
}

#[test]
fn probes_with_impossible_or_unknown_events_are_refused() {
    let model = payments(PAY, "byKey", json!({"trigger": "onCrash"}));
    assert!(
        probe::find(&model, &["teleport:Client".into()])
            .unwrap_err()
            .contains("unknown probe event")
    );
    assert!(probe::find(&model, &[]).is_err());
    // Two retries do not fit maxRetries 1.
    let err = probe::find(
        &model,
        &[
            "crash:Client".into(),
            "retry:Client".into(),
            "retry:Client".into(),
        ],
    )
    .unwrap_err();
    assert!(err.contains("no complete schedule"), "{err}");
}

#[test]
fn a_crash_is_planned_at_the_effect_it_precedes() {
    // Crash before the effect: the worker must be killed with the effect
    // announced and held (never performed).
    let model = payments(PAY, "none", json!({"trigger": "onCrash"}));
    let schedule = probe::find(
        &model,
        &[
            "statement:Client".into(),
            "crash:Client".into(),
            "retry:Client".into(),
        ],
    )
    .unwrap();
    let steps = plan_steps(&model, &schedule).unwrap();
    let crash = steps
        .iter()
        .find_map(|s| match s {
            PlanStep::Crash(c) => Some(c),
            _ => None,
        })
        .unwrap();
    let next = crash.next.as_ref().unwrap();
    assert_eq!(next.kind, SqlKind::Effect);
    assert_eq!(next.table.as_deref(), Some("charge"));
    assert_eq!(next.key, Some(json!("r1")));
}

fn lock_model(b_program: &str) -> midwrite_model::model::Model {
    model_from_json(json!({
        "schemaVersion": 1,
        "name": "locks",
        "database": {
            "tables": [{"name": "t", "columns": [{"name": "id", "notNull": true}, "v"], "primaryKey": ["id"]}],
            "initialState": {"t": [{"id": 1, "v": 0}]}
        },
        "actors": [
            {"name": "A", "function": "a", "program": "begin\nupdate t where @id == 1 set @v = 1\ncommit\n"},
            {"name": "B", "function": "b", "program": b_program}
        ],
        "bounds": {"maxSteps": 30, "maxCrashes": 1},
        "invariants": [{"name": "trivial", "assert": {"le": [{"count": {"table": "t"}}, 1]}}]
    }))
}

#[test]
fn a_crash_while_waiting_is_planned_as_in_flight_and_checked_against_the_server() {
    let model = lock_model("begin\nupdate t where @id == 1 set @v = 2\ncommit\n");
    let schedule = probe::find(
        &model,
        &["blocked:B".into(), "crash:B".into(), "commit:A".into()],
    )
    .unwrap();
    let steps = plan_steps(&model, &schedule).unwrap();
    let crash = steps
        .iter()
        .find_map(|s| match s {
            PlanStep::Crash(c) => Some(c),
            _ => None,
        })
        .unwrap();
    assert!(crash.waiting && crash.next.is_none() && !crash.may_have_run);
}

#[test]
fn a_crash_after_the_lock_was_released_is_in_flight_or_refused() {
    // Inside a transaction: the statement may have run, but the transaction
    // never commits: plannable, marked as possibly executed.
    let model = lock_model("begin\nupdate t where @id == 1 set @v = 2\ncommit\n");
    let schedule = probe::find(
        &model,
        &["blocked:B".into(), "commit:A".into(), "crash:B".into()],
    )
    .unwrap();
    let steps = plan_steps(&model, &schedule).unwrap();
    assert!(
        steps
            .iter()
            .any(|s| matches!(s, PlanStep::Crash(c) if c.waiting && c.may_have_run))
    );
    // Autocommit: PostgreSQL may have executed AND committed the statement
    // after the lock was released, which the model rolls back: refused.
    let model = lock_model("update t where @id == 1 set @v = 2\n");
    let schedule = probe::find(
        &model,
        &["blocked:B".into(), "commit:A".into(), "crash:B".into()],
    )
    .unwrap();
    let err = plan_steps(&model, &schedule).unwrap_err();
    assert!(err.contains("autocommit"), "{err}");
}

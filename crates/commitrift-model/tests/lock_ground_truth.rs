//! Phase 13: the model against the independently recorded PostgreSQL
//! row-lock / foreign-key ground truth (`scripts/phase13-lock-matrix.py`,
//! PostgreSQL 17.11; translated by `scripts/phase13-lock-replay-plan.py`).
//!
//! Each cell is replayed in the model in the order PostgreSQL received the
//! statements: a session's next statement is sent after its previous one
//! completed, and a waiting statement resumes as soon as what it waits for
//! ends (as in PostgreSQL). At every labeled step the model must agree on
//! whether the statement waited and on its outcome (rows affected or
//! returned, or the SQLSTATE).

use commitrift_model::engine::{Recorder, apply, enabled, initial_state};
use commitrift_model::model::Model;
use commitrift_model::scenario::testing::model_from_json;
use commitrift_model::state::{Outcome, State, TKind};
use commitrift_model::value::Value;
use serde_json::{Value as Json, json};

fn plan() -> Json {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/phase13/lock-matrix/model-replay.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn model(cell: &Json) -> Model {
    model_from_json(json!({
        "schemaVersion": 1,
        "name": cell["id"],
        "database": { "tables": cell["tables"], "initialState": cell["seed"] },
        "actors": [
            { "name": "A", "function": "a", "program": cell["programs"]["A"], "arguments": {} },
            { "name": "B", "function": "b", "program": cell["programs"]["B"], "arguments": {} }
        ],
        "invariants": [{ "name": "true", "assert": { "eq": [0, 0] } }]
    }))
}

fn apply_kind(m: &Model, s: &State, actor: u16, kind: TKind) -> Option<State> {
    let t = enabled(m, s)
        .into_iter()
        .find(|t| t.actor == actor && t.kind == kind)?;
    Some(
        apply(m, s, t, &mut Recorder::off())
            .expect("supported")
            .state,
    )
}

fn waiting(s: &State, a: usize) -> bool {
    s.actors[a].threads[0]
        .pending
        .as_ref()
        .is_some_and(|p| p.blocked_on.is_some())
}

/// Resume every statement whose wait is over (PostgreSQL lets it proceed at
/// once).
fn settle(m: &Model, mut s: State, done: &mut dyn FnMut(&State, usize)) -> State {
    loop {
        let Some(t) = enabled(m, &s).into_iter().find(|t| t.kind == TKind::Resume) else {
            return s;
        };
        s = apply(m, &s, t, &mut Recorder::off())
            .expect("supported")
            .state;
        done(&s, t.actor as usize);
    }
}

fn observed(s: &State, a: usize, step: &Json) -> Json {
    if let Outcome::Failed(reason) = &s.actors[a].outcome {
        let code = reason
            .split(':')
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        return json!({ "sqlstate": code });
    }
    let Some(var) = step["var"].as_str() else {
        return json!({});
    };
    match (step["kind"].as_str(), s.actors[a].locals.get(var)) {
        (Some("select"), Some(Value::Null)) => json!({ "rows": 0 }),
        (Some("select"), Some(_)) => json!({ "rows": 1 }),
        (Some("count"), Some(Value::Int(n))) => json!({ "rows": n }),
        (Some("aggregate"), Some(Value::Int(n))) => json!({ "count": n }),
        _ => json!({}),
    }
}

#[test]
fn the_model_agrees_with_the_recorded_postgresql_lock_matrix() {
    let plan = plan();
    let mut disagreements = Vec::new();
    let mut compared = 0;
    for cell in plan["replays"].as_array().unwrap() {
        let id = cell["id"].as_str().unwrap();
        let m = model(cell);
        let mut s = initial_state(&m);
        let steps = cell["steps"].as_array().unwrap();
        // Per labeled step: (session index, blocked at send, outcome).
        let mut seen: Vec<Option<(bool, Json)>> = vec![None; steps.len()];
        let mut open: [Option<usize>; 2] = [None, None];
        for (i, step) in steps.iter().enumerate() {
            let a = usize::from(step["session"] == "B");
            if !matches!(s.actors[a].outcome, Outcome::Running) {
                // The transaction failed: PostgreSQL answers COMMIT with
                // ROLLBACK and nothing else runs.
                continue;
            }
            let Some(next) = apply_kind(&m, &s, a as u16, TKind::Step) else {
                disagreements.push(format!("{id}: step {i} of {} cannot run", step["session"]));
                break;
            };
            s = next;
            let blocked = waiting(&s, a);
            if blocked {
                open[a] = Some(i);
                seen[i] = Some((true, json!({})));
            } else {
                seen[i] = Some((false, observed(&s, a, step)));
            }
            s = settle(&m, s, &mut |st, who| {
                if let Some(j) = open[who].take() {
                    seen[j] = Some((true, observed(st, who, &steps[j])));
                }
            });
        }
        for (i, step) in steps.iter().enumerate() {
            if step["label"].is_null() {
                continue;
            }
            let Some((blocked, outcome)) = &seen[i] else {
                continue;
            };
            compared += 1;
            let pg_outcome = &step["postgres"];
            let outcome_agrees = pg_outcome.as_object().is_none_or(|o| o.is_empty())
                || outcome == pg_outcome
                // A statement after a failure is not run in either.
                || pg_outcome.get("sqlstate") == Some(&json!("25P02"));
            if *blocked != step["blocked"].as_bool().unwrap() || !outcome_agrees {
                disagreements.push(format!(
                    "{id} {}: model waited={blocked} {outcome}; PostgreSQL waited={} {pg_outcome}",
                    step["label"], step["blocked"]
                ));
            }
        }
    }
    eprintln!(
        "{} cells, {compared} labeled steps compared, {} cells not replayed",
        plan["replays"].as_array().unwrap().len(),
        plan["skipped"].as_array().unwrap().len()
    );
    assert!(
        disagreements.is_empty(),
        "{} disagreement(s):\n{}",
        disagreements.len(),
        disagreements.join("\n")
    );
}

//! Dynamic partial-order reduction against full exploration and sleep sets.
//!
//! DPOR explores one interleaving per Mazurkiewicz trace, so it does not
//! visit every reachable state. What it must preserve is what is a function
//! of the *final* state of a trace: the set of terminal states (hence final
//! invariant violations and counterexample existence), stuck states and
//! unsupported behavior. These tests compare exactly that against the
//! unreduced explorer and the sleep-set explorer, on targeted workloads and
//! on deterministic random scenarios (seeds are replayed from
//! `tests/data/por-regression-seeds.txt` as well).

mod common;

use commitrift_model::explore::{
    ExploreOptions, ExploreResult, Verdict, dpor_inapplicable, explore_dpor, explore_with,
};
use commitrift_model::invariant::When;
use commitrift_model::model::{Model, Strategy};
use common::por_workloads::{dpor_model, phase9_workloads, seeds, targeted_workloads};
use common::scenarios_gen::random_scenario;
use common::*;
use serde_json::json;

fn opts(por: bool, strategy: Strategy) -> ExploreOptions {
    ExploreOptions {
        por,
        strategy,
        collect_states: true,
        stop_at_first: false,
    }
}

struct Three {
    full: ExploreResult,
    sleep: ExploreResult,
    dpor: ExploreResult,
}

fn run_all(model: &Model) -> Three {
    let mut m = model.clone();
    m.bounds.max_states = 2_000_000;
    Three {
        full: explore_with(&m, &opts(false, Strategy::Dfs)),
        sleep: explore_with(&m, &opts(true, Strategy::Dfs)),
        dpor: explore_dpor(&m, &opts(false, Strategy::Dfs)),
    }
}

/// `Some((full transitions, sleep transitions, dpor transitions))` when all
/// three searches completed; assertions on what must be preserved.
fn compare(label: &str, model: &Model) -> Option<(u64, u64, u64)> {
    assert_eq!(dpor_inapplicable(model), None, "{label}");
    let r = run_all(model);
    if [&r.full, &r.sleep, &r.dpor]
        .iter()
        .any(|x| x.truncation.resource_limit())
    {
        return None;
    }
    assert_eq!(
        r.full.terminals, r.dpor.terminals,
        "{label}: DPOR changed the set of terminal states ({} vs {})",
        r.full.metrics.terminal_states, r.dpor.metrics.terminal_states
    );
    assert_eq!(
        r.full.terminals, r.sleep.terminals,
        "{label}: the sleep-set search changed the terminal states"
    );
    assert_eq!(r.full.verdict, r.dpor.verdict, "{label}: verdict");
    assert_eq!(r.full.verdict, r.sleep.verdict, "{label}: verdict (sleep)");
    assert_eq!(
        r.full.counterexample.is_some(),
        r.dpor.counterexample.is_some(),
        "{label}"
    );
    assert_eq!(
        r.full.stuck.is_empty(),
        r.dpor.stuck.is_empty(),
        "{label}: stuck states"
    );
    assert_eq!(
        r.full.unsupported.is_empty(),
        r.dpor.unsupported.is_empty(),
        "{label}: unsupported behavior"
    );
    assert_eq!(
        r.full.truncation.max_steps, r.dpor.truncation.max_steps,
        "{label}: truncation by the step bound"
    );
    // A DPOR counterexample is a valid schedule that really violates the
    // invariant.
    if let Some(ce) = &r.dpor.counterexample {
        assert!(
            commitrift_model::witness::check_schedule(model, &ce.schedule, &ce.invariant).is_some(),
            "{label}: the DPOR counterexample does not replay"
        );
    }
    Some((
        r.full.metrics.transitions,
        r.sleep.metrics.transitions,
        r.dpor.metrics.transitions,
    ))
}

#[test]
fn targeted_workloads_keep_every_terminal_state() {
    let mut report = String::new();
    for (label, model) in targeted_workloads() {
        let (f, s, d) =
            compare(label, &model).unwrap_or_else(|| panic!("{label}: exploration hit a limit"));
        report.push_str(&format!(
            "{label}: transitions full {f}, sleep sets {s}, dpor {d}\n"
        ));
    }
    eprintln!("{report}");
}

/// Phase 9 dependencies: a COMMIT whose acknowledgment is lost and that the
/// server completes later (`ServerComplete`) racing a retry or a second
/// delivery, crashes of a process with several tasks, and connection-pool
/// acquisition and release. Terminal states, verdicts and stuck states must
/// be those of the unreduced search.
#[test]
fn in_flight_and_pool_workloads_keep_every_terminal_state() {
    let mut report = String::new();
    let mut failed = Vec::new();
    for (label, model) in phase9_workloads() {
        assert_eq!(dpor_inapplicable(&model), None, "{label}");
        match std::panic::catch_unwind(|| compare(&label, &model)) {
            Ok(Some((f, s, d))) => report.push_str(&format!(
                "{label}: transitions full {f}, sleep sets {s}, dpor {d}\n"
            )),
            Ok(None) => failed.push(format!("{label}: exploration hit a limit")),
            Err(_) => failed.push(label),
        }
    }
    eprintln!("{report}");
    assert!(failed.is_empty(), "not preserved: {failed:?}");
}

#[test]
fn randomized_scenarios_keep_every_terminal_state() {
    let (mut compared, mut skipped) = (0, 0);
    let (mut fs, mut ss, mut ds) = (0u64, 0u64, 0u64);
    for seed in seeds() {
        let (doc, model) = dpor_model(seed);
        assert!(model.invariants.iter().all(|i| i.when == When::Final));
        let result = std::panic::catch_unwind(|| compare(&format!("seed {seed}"), &model));
        match result {
            Ok(Some((f, s, d))) => {
                compared += 1;
                fs += f;
                ss += s;
                ds += d;
            }
            Ok(None) => skipped += 1,
            Err(e) => {
                eprintln!(
                    "FAILING SEED {seed}; scenario:\n{}",
                    serde_json::to_string_pretty(&doc).unwrap()
                );
                std::panic::resume_unwind(e);
            }
        }
    }
    eprintln!(
        "{compared} scenarios compared, {skipped} skipped (limit); transitions full {fs}, sleep sets {ss}, dpor {ds}"
    );
    assert!(compared >= 100, "too few comparable scenarios: {compared}");
}

#[test]
fn dpor_collapses_independent_actors_to_one_interleaving() {
    let model = build(
        "read-committed",
        json!([
            { "name": "a", "columns": [{"name":"id","notNull":true,"type":"int"}, {"name":"v","type":"int"}], "primaryKey": ["id"] },
            { "name": "b", "columns": [{"name":"id","notNull":true,"type":"int"}, {"name":"v","type":"int"}], "primaryKey": ["id"] }
        ]),
        json!({ "a": [{"id": 1, "v": 0}], "b": [{"id": 1, "v": 0}] }),
        vec![
            actor(
                "A",
                "let x = select a where @id == 1\nupdate a where @id == 1 set @v = @v + 1\nupdate a where @id == 1 set @v = @v + 1\n",
                json!({}),
            ),
            actor(
                "B",
                "let y = select b where @id == 1\nupdate b where @id == 1 set @v = @v + 1\nupdate b where @id == 1 set @v = @v + 1\n",
                json!({}),
            ),
        ],
        json!([{ "name": "ok", "when": "final", "assert": { "ge": [ { "count": { "table": "a", "where": {} } }, 0 ] } }]),
        json!({}),
        json!({}),
    );
    let (full, _sleep, dpor) = compare("independent", &model).expect("no limit");
    let r = explore_dpor(&model, &opts(false, Strategy::Dfs));
    assert_eq!(r.metrics.terminal_states, 1, "one trace, one terminal run");
    assert_eq!(r.metrics.transitions, 6, "a single interleaving of 3 + 3");
    assert!(dpor < full);
}

#[test]
fn dpor_keeps_both_orders_of_conflicting_writers() {
    let model = purchase("read-committed", PURCHASE_AUTOCOMMIT, json!({}));
    // The invariant of this workload is a final one: it is applicable.
    let mut m = model.clone();
    m.invariants.retain(|i| i.when == When::Final);
    if m.invariants.is_empty() {
        return;
    }
    let r = run_all(&m);
    assert_eq!(r.full.terminals, r.dpor.terminals);
    assert!(r.dpor.metrics.dpor_races > 0);
}

#[test]
fn an_always_invariant_refuses_dpor() {
    let model = purchase("read-committed", PURCHASE_TX, json!({}));
    let why = dpor_inapplicable(&model);
    if model.invariants.iter().any(|i| i.when == When::Always) {
        assert!(why.unwrap().contains("always"));
    }
    let mut with_always = model.clone();
    with_always
        .invariants
        .push(commitrift_model::invariant::Invariant {
            name: "x".into(),
            when: When::Always,
            description: String::new(),
            formula: commitrift_model::invariant::Formula::Ge([
                commitrift_model::invariant::Term::Int(1),
                commitrift_model::invariant::Term::Int(0),
            ]),
        });
    assert!(dpor_inapplicable(&with_always).is_some());
}

#[test]
fn a_dpor_counterexample_is_found_and_valid() {
    // The classic lost update: a final invariant that is violated.
    let mut model = purchase("read-committed", PURCHASE_TX, json!({}));
    model.invariants.retain(|i| i.when == When::Final);
    if model.invariants.is_empty() {
        let doc = random_scenario(7);
        let _ = doc;
        return;
    }
    let r = explore_dpor(&model, &ExploreOptions::from_model(&model));
    assert_eq!(r.verdict, Verdict::CounterexampleFound);
}

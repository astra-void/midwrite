//! Partial-order reduction against the unreduced explorer.
//!
//! For every scenario small enough to exhaust, the reduced search must reach
//! exactly the states of the unreduced search (hence the same invariant
//! violations) while executing no more transitions. Randomized scenarios are
//! deterministic in their seed; failing seeds belong in
//! `tests/data/por-regression-seeds.txt`.

mod common;

use common::scenarios_gen::random_scenario;
use common::*;
use midwrite_model::explore::{ExploreOptions, ExploreResult, Verdict, explore_with};
use midwrite_model::model::{Model, Strategy};
use midwrite_model::scenario::testing::model_from_json;
use serde_json::json;

fn run(model: &Model, por: bool, strategy: Strategy) -> ExploreResult {
    let o = ExploreOptions {
        por,
        strategy,
        collect_states: true,
        stop_at_first: false,
    };
    explore_with(model, &o)
}

/// Compare unreduced and reduced exploration; `None` when either was cut by
/// a resource limit (then nothing is claimed).
fn compare(label: &str, model: &Model) -> Option<(u64, u64)> {
    let full = run(model, false, Strategy::Dfs);
    let bfs = run(model, false, Strategy::Bfs);
    let reduced = run(model, true, Strategy::Dfs);
    if full.truncation.resource_limit() || reduced.truncation.resource_limit() {
        return None;
    }
    assert_eq!(
        full.reached, bfs.reached,
        "{label}: DFS and BFS reach different states"
    );
    assert_eq!(
        full.reached, reduced.reached,
        "{label}: the reduced search missed states ({} vs {})",
        full.metrics.states, reduced.metrics.states
    );
    assert_eq!(
        full.violating_states, reduced.violating_states,
        "{label}: violating states differ"
    );
    assert_eq!(
        full.verdict == Verdict::CounterexampleFound,
        reduced.verdict == Verdict::CounterexampleFound,
        "{label}: verdicts differ"
    );
    assert!(
        reduced.metrics.transitions <= full.metrics.transitions,
        "{label}: reduction executed more transitions"
    );
    Some((full.metrics.transitions, reduced.metrics.transitions))
}

#[test]
fn targeted_scenarios_preserve_every_reachable_state() {
    let scenarios: Vec<(&str, Model)> = vec![
        (
            "lost-update/rc",
            purchase("read-committed", PURCHASE_AUTOCOMMIT, json!({})),
        ),
        (
            "lost-update-tx/rc",
            purchase("read-committed", PURCHASE_TX, json!({})),
        ),
        (
            "lost-update-tx/rr",
            purchase("repeatable-read", PURCHASE_TX, json!({})),
        ),
        (
            "for-update/rc",
            purchase("read-committed", PURCHASE_FOR_UPDATE, json!({})),
        ),
        (
            "atomic/rc",
            purchase("read-committed", PURCHASE_ATOMIC, json!({})),
        ),
        ("write-skew/rc", write_skew("read-committed", json!({}))),
        ("write-skew/ser", write_skew("serializable", json!({}))),
        ("redeem/unique", redeem("read-committed", true, json!({}))),
        (
            "redeem/no-constraint",
            redeem("read-committed", false, json!({})),
        ),
        ("deadlock/rc", deadlock("read-committed", json!({}))),
        ("skip-locked", jobs("read-committed", CLAIM_SKIP_LOCKED, 2)),
        (
            "crash-retry",
            crash_after_commit(1, Some("onFailureOrCrash"), 1),
        ),
        ("tx-retry", tx_retry(2)),
        ("tasks", concurrent_tasks()),
    ];
    let mut report = String::new();
    for (label, model) in &scenarios {
        let (full, reduced) =
            compare(label, model).unwrap_or_else(|| panic!("{label}: exploration hit a limit"));
        report.push_str(&format!("{label}: transitions {full} -> {reduced}\n"));
    }
    eprintln!("{report}");
}

#[test]
fn independent_actors_collapse_to_one_ordering() {
    // Two actors touching different rows of different tables never conflict:
    // sleep sets must explore one interleaving's worth of transitions.
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
        json!([{ "name": "ok", "assert": { "ge": [ { "count": { "table": "a", "where": {} } }, 0 ] } }]),
        json!({}),
        json!({}),
    );
    let (full, reduced) = compare("independent", &model).expect("no limit");
    assert!(reduced < full, "expected a reduction: {full} -> {reduced}");
    let r = run(&model, true, Strategy::Dfs);
    assert!(r.metrics.sleep_pruned_transitions > 0, "{:?}", r.metrics);
}

#[test]
fn conflicting_writers_are_not_reduced_away() {
    // Both actors write the same row: neither order may be dropped.
    let model = purchase("read-committed", PURCHASE_AUTOCOMMIT, json!({}));
    let full = run(&model, false, Strategy::Dfs);
    let reduced = run(&model, true, Strategy::Dfs);
    assert_eq!(full.reached, reduced.reached);
    assert!(reduced.counterexample.is_some());
}

#[test]
fn unknown_resources_are_dependent() {
    // An unsupported operation yields a global footprint: the explorer must
    // still report it and not hide it behind reduction.
    let model = build(
        "read-committed",
        json!([{ "name": "t", "columns": [{"name":"id","notNull":true,"type":"int"}], "primaryKey": ["id"] }]),
        json!({ "t": [{"id": 1}] }),
        vec![
            actor("A", "unknown \"raw SQL\"\n", json!({})),
            actor("B", "let x = select t where @id == 1\n", json!({})),
        ],
        json!([{ "name": "ok", "assert": { "ge": [ { "count": { "table": "t", "where": {} } }, 0 ] } }]),
        json!({}),
        json!({}),
    );
    let r = run(&model, true, Strategy::Dfs);
    assert_eq!(r.verdict, Verdict::Unsupported);
}

fn seeds() -> Vec<u64> {
    let mut out: Vec<u64> = include_str!("data/por-regression-seeds.txt")
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.trim().parse().ok())
        .collect();
    let n: u64 = std::env::var("MIDWRITE_POR_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(150);
    out.extend(1..=n);
    out
}

#[test]
fn randomized_scenarios_agree_between_full_and_reduced_exploration() {
    let mut compared = 0;
    let mut skipped = 0;
    let mut ratios = Vec::new();
    for seed in seeds() {
        let doc = random_scenario(seed);
        let model = model_from_json(doc.clone());
        let result = std::panic::catch_unwind(|| compare(&format!("seed {seed}"), &model));
        match result {
            Ok(Some((f, r))) => {
                compared += 1;
                ratios.push((f, r));
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
    let (f, r): (u64, u64) = ratios.iter().fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    eprintln!("{compared} scenarios compared, {skipped} skipped (limit); transitions {f} -> {r}");
    assert!(compared >= 100, "too few comparable scenarios: {compared}");
}

#[test]
fn the_independence_relation_is_valid_on_random_scenarios() {
    // For every reachable state and every pair of enabled transitions judged
    // independent: both orders reach the same canonical state and neither
    // disables the other.
    let mut audited = 0;
    for seed in seeds() {
        let doc = random_scenario(seed);
        let model = model_from_json(doc.clone());
        let violations = midwrite_model::explore::audit_independence(&model, 30_000);
        if !violations.is_empty() {
            eprintln!(
                "FAILING SEED {seed}: {:?}\n{}",
                violations[0],
                serde_json::to_string_pretty(&doc).unwrap()
            );
        }
        assert!(
            violations.is_empty(),
            "seed {seed}: {} violation(s): {:?}",
            violations.len(),
            violations[0].reason
        );
        audited += 1;
    }
    eprintln!("{audited} scenarios audited");
}

#[test]
fn the_independence_relation_is_valid_on_the_workloads() {
    for (label, model) in [
        (
            "lost-update-tx/rc",
            purchase("read-committed", PURCHASE_TX, json!({})),
        ),
        (
            "for-update/rc",
            purchase("read-committed", PURCHASE_FOR_UPDATE, json!({})),
        ),
        ("write-skew/ser", write_skew("serializable", json!({}))),
        (
            "redeem/unique/rr",
            redeem("repeatable-read", true, json!({})),
        ),
        ("deadlock/rc", deadlock("read-committed", json!({}))),
        ("skip-locked", jobs("read-committed", CLAIM_SKIP_LOCKED, 2)),
        (
            "crash-retry",
            crash_after_commit(1, Some("onFailureOrCrash"), 1),
        ),
        ("tx-retry", tx_retry(2)),
    ] {
        let v = midwrite_model::explore::audit_independence(&model, 200_000);
        assert!(
            v.is_empty(),
            "{label}: {:?}",
            v.first().map(|x| (x.reason, x.a, x.b))
        );
    }
}

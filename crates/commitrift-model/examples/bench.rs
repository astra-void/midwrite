//! Model-checker benchmarks: full vs reduced exploration, scaling with
//! actors, crashes and retries. Run with
//! `cargo run --release -p commitrift-model --example bench`.
//! Prints one TSV row per (scenario, mode); every number is measured.

#[path = "../tests/common/mod.rs"]
mod common;

use std::time::Instant;

use commitrift_model::explore::{ExploreOptions, explore_with};
use commitrift_model::model::{Model, Strategy};
use commitrift_model::scenario::testing::model_from_json;
use common::*;
use serde_json::json;

fn purchase_n(iso: &str, program: &'static str, n: usize, stock: i64) -> Model {
    let actors = (0..n)
        .map(|i| {
            let name: &'static str = Box::leak(format!("R{i}").into_boxed_str());
            actor(name, program, json!({"pid": "p1", "oid": format!("o{i}")}))
        })
        .collect();
    build(
        iso,
        products_tables(),
        json!({ "products": [{"id": "p1", "stock": stock}], "orders": [] }),
        actors,
        no_oversell(),
        json!({ "maxActors": 8, "maxStates": 5_000_000, "maxSteps": 200 }),
        json!({}),
    )
}

fn jobs_with(crashes: u32, retries: u32) -> Model {
    let program = "\
let job = select jobs where @id == 'j1'
if job == null || job.status != 'pending' {
  return
}
update jobs where @id == 'j1' set @status = 'processing'
external events key 'j1'
update jobs where @id == 'j1' set @status = 'done'
";
    let mut a = actor("W1", program, json!({}));
    a.retry = Some("atLeastOnce");
    let mut b = actor("W2", program, json!({}));
    b.retry = Some("atLeastOnce");
    build(
        "read-committed",
        json!([{ "name": "jobs", "columns": [{"name":"id","notNull":true}, "status"], "primaryKey": ["id"] }]),
        json!({ "jobs": [{"id": "j1", "status": "pending"}] }),
        vec![a, b],
        json!([{ "name": "once", "assert": { "le": [ { "externals": { "channel": "events" } }, 1 ] } }]),
        json!({ "maxCrashes": crashes, "maxRetries": retries, "maxStates": 5_000_000, "maxSteps": 200 }),
        json!({ "channels": { "events": { "dedup": "none" } } }),
    )
}

fn row(label: &str, model: &Model) {
    for (mode, por, strategy, stop) in [
        ("full-dfs", false, Strategy::Dfs, false),
        ("full-bfs", false, Strategy::Bfs, false),
        ("por-dfs", true, Strategy::Dfs, false),
        ("first-cex-bfs", false, Strategy::Bfs, true),
        ("first-cex-por", true, Strategy::Dfs, true),
    ] {
        let o = ExploreOptions {
            por,
            strategy,
            collect_states: false,
            stop_at_first: stop,
        };
        let t = Instant::now();
        let r = explore_with(model, &o);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let m = &r.metrics;
        println!(
            "{label}\t{mode}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.1}\t{}\t{:.2}\t{}",
            model.actors.len(),
            r.verdict.as_str(),
            m.states,
            m.transitions,
            m.schedules_in_explored_graph,
            m.sleep_pruned_transitions,
            m.visited_bytes as f64 / 1_048_576.0,
            m.max_depth,
            ms,
            m.first_violation_ms.map_or("-".into(), |v| v.to_string()),
        );
    }
}

fn main() {
    println!(
        "scenario\tmode\tactors\tverdict\tstates\ttransitions\tschedules\tsleep_pruned\tvisited_MiB\tmax_depth\twall_ms\tfirst_violation_ms"
    );
    for n in [2, 3, 4] {
        row(
            &format!("purchase-tx-rc-{n}"),
            &purchase_n("read-committed", PURCHASE_TX, n, 1),
        );
        row(
            &format!("purchase-atomic-{n}"),
            &purchase_n("read-committed", PURCHASE_ATOMIC_TX, n, 1),
        );
        row(
            &format!("purchase-for-update-{n}"),
            &purchase_n("read-committed", PURCHASE_FOR_UPDATE, n, 1),
        );
    }
    row("write-skew-ser-2", &write_skew("serializable", json!({})));
    row("write-skew-rr-2", &write_skew("repeatable-read", json!({})));
    row("deadlock-rc-2", &deadlock("read-committed", json!({})));
    for (c, r) in [(0, 0), (1, 0), (0, 1), (1, 1), (2, 2)] {
        row(&format!("jobs-crash{c}-retry{r}"), &jobs_with(c, r));
    }
    // Aggregate over the random differential scenarios.
    let mut agg = [(0u64, 0u64, 0f64); 2];
    for seed in 1..=300u64 {
        let m = model_from_json(common::scenarios_gen::random_scenario(seed));
        for (i, por) in [false, true].into_iter().enumerate() {
            let o = ExploreOptions {
                por,
                strategy: Strategy::Dfs,
                collect_states: false,
                stop_at_first: false,
            };
            let t = Instant::now();
            let r = explore_with(&m, &o);
            agg[i].0 += r.metrics.states;
            agg[i].1 += r.metrics.transitions;
            agg[i].2 += t.elapsed().as_secs_f64() * 1000.0;
        }
    }
    for (i, mode) in ["full-dfs", "por-dfs"].iter().enumerate() {
        println!(
            "random-300-seeds\t{mode}\t2-3\t-\t{}\t{}\t-\t-\t-\t-\t{:.1}\t-",
            agg[i].0, agg[i].1, agg[i].2
        );
    }
}

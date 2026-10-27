//! Workloads shared by the reduction differential tests (classic DPOR,
//! source-set DPOR): the targeted Phase 6-9 workloads, the Phase 9 scenario
//! files plus pool workloads, and the seeded random scenarios.
#![allow(dead_code)]

use super::scenarios_gen::random_scenario;
use super::*;
use commitrift_model::invariant::When;
use commitrift_model::model::Model;
use commitrift_model::scenario::testing::model_from_json;
use serde_json::{Value, json};

/// The targeted workloads whose invariants are all final (the others are
/// refused by every DPOR).
pub fn targeted_workloads() -> Vec<(&'static str, Model)> {
    let all: Vec<(&'static str, Model)> = vec![
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
    all.into_iter()
        .filter(|(_, m)| m.invariants.iter().all(|i| i.when == When::Final))
        .collect()
}

/// The Phase 9 model scenarios (`fixtures/phase9/model-scenarios`) and
/// three tasks of one process, pooled and unpooled, with crashes and an
/// in-flight COMMIT.
pub fn phase9_workloads() -> Vec<(String, Model)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/phase9/model-scenarios");
    let mut scenarios: Vec<(String, Value)> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| {
            let doc: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
            (p.file_name().unwrap().to_string_lossy().into_owned(), doc)
        })
        .collect();
    scenarios.sort_by(|a, b| a.0.cmp(&b.0));
    let pay = |pool: Option<u64>, crashes: u64, in_flight: u64| {
        let mut doc = json!({
            "schemaVersion": 1, "name": "pay",
            "database": {"isolation": "read-committed",
                "tables": [{"name": "payments", "columns": [
                    {"name": "id", "type": "int", "notNull": true, "default": {"sequence": {}}},
                    {"name": "req", "type": "text"}], "primaryKey": ["id"], "unique": [["req"]]}],
                "initialState": {"payments": []}},
            "actors": (0..3).map(|i| json!({
                "name": format!("C{i}"), "function": "pay",
                "program": "begin\ninsert payments set @req = rid on conflict do nothing\ncommit\n",
                "arguments": {"rid": format!("r{}", i % 2)}})).collect::<Vec<_>>(),
            "bounds": {"maxSteps": 60, "maxCrashes": crashes, "maxInFlight": in_flight, "maxStates": 2_000_000},
            "invariants": [{"name": "once", "when": "final",
                "assert": {"le": [{"count": {"table": "payments", "where": {}}}, 2]}}]
        });
        if let Some(n) = pool {
            doc["pools"] = json!({"P": n});
            for a in doc["actors"].as_array_mut().unwrap() {
                a["process"] = "P".into();
            }
        }
        doc
    };
    for (label, pool, crashes, in_flight) in [
        ("3 tasks, pool of 1", Some(1), 0, 0),
        ("3 tasks, pool of 2", Some(2), 0, 0),
        ("3 tasks, pool of 2, process crash", Some(2), 1, 0),
        (
            "3 tasks, pool of 1, crash with COMMIT in flight",
            Some(1),
            1,
            1,
        ),
        ("3 processes, crash with COMMIT in flight", None, 1, 1),
    ] {
        scenarios.push((label.to_owned(), pay(pool, crashes, in_flight)));
    }
    scenarios
        .into_iter()
        .map(|(label, doc)| (label, model_from_json(doc)))
        .collect()
}

/// A random scenario with only final invariants (DPOR is refused for
/// `always` ones) and, for some seeds, in-flight faults.
pub fn dpor_model(seed: u64) -> (Value, Model) {
    let mut doc = random_scenario(seed);
    let invariants: Vec<Value> = doc["invariants"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["when"] == "final")
        .cloned()
        .collect();
    doc["invariants"] = Value::Array(invariants);
    if seed.is_multiple_of(3) {
        doc["bounds"]["maxInFlight"] = 1.into();
        if doc["bounds"]["maxRetries"] == 0 && seed.is_multiple_of(2) {
            doc["bounds"]["maxRetries"] = 1.into();
        }
    }
    doc["bounds"]["maxStates"] = 2_000_000.into();
    let model = model_from_json(doc.clone());
    (doc, model)
}

pub fn seeds() -> Vec<u64> {
    let mut out: Vec<u64> = include_str!("../data/por-regression-seeds.txt")
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.trim().parse().ok())
        .collect();
    let n: u64 = std::env::var("COMMITRIFT_POR_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(150);
    out.extend(1..=n);
    out
}

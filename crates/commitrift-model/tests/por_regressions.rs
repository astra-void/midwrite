//! Regression fixtures for the defects that differential testing found in the
//! reductions, and the machinery that produced them (Phase 10).
//!
//! Each fixture is a minimized scenario with the seed it came from. Two things
//! are checked, and both matter:
//!
//! * every algorithm agrees with the unreduced search on it (the defect is
//!   fixed), and
//! * with the historical defect switched back on (`SdporConfig::defects`, or
//!   the claim-pool footprint mutation for the Phase 9 pool defect) the
//!   recorded algorithm still disagrees. A fixture that no longer exposes its
//!   defect would pass for the wrong reason.
//!
//! `regenerate_fixtures` (ignored) searches seeds, minimizes and rewrites the
//! files: `cargo test --release --test por_regressions regenerate -- --ignored`.

mod common;

use commitrift_model::explore::{
    DEFECT_ENABLER_ORDERS, DEFECT_LAST_RACE_ONLY, DEFECT_SLEEPING_CHOICE, SdporConfig,
};
use commitrift_model::footprint::mutation;
use commitrift_model::model::Model;
use commitrift_model::scenario::load_str;
use common::adversarial::generate;
use common::minimize::{minimize, size};
use common::por_diff::{Alg, discrepancy, full, run};
use common::por_workloads::dpor_model;
use serde_json::{Value, json};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/regressions");

fn load(doc: &Value) -> Option<Model> {
    load_str(
        &doc.to_string(),
        std::path::Path::new("."),
        &commitrift_model::provider::NoProvider,
    )
    .ok()
    .map(|(m, _)| m)
}

/// What a fixture is about.
#[derive(Clone, Copy, Debug)]
struct Kind {
    name: &'static str,
    /// The algorithm that exposed it, with the defect switched on.
    alg: Alg,
    /// Footprint mutation to apply while running the defective algorithm.
    claim_pool_mutation: bool,
}

const KINDS: [Kind; 4] = [
    Kind {
        name: "sleeping-choice-member",
        alg: Alg::Sdpor(SdporConfig {
            defects: DEFECT_SLEEPING_CHOICE,
            ..SdporConfig::STATELESS_SLEEP
        }),
        claim_pool_mutation: false,
    },
    Kind {
        name: "enabler-orders-the-enabled",
        alg: Alg::Sdpor(SdporConfig {
            defects: DEFECT_ENABLER_ORDERS,
            ..SdporConfig::STATEFUL
        }),
        claim_pool_mutation: false,
    },
    Kind {
        name: "last-race-only",
        alg: Alg::Sdpor(SdporConfig {
            defects: DEFECT_LAST_RACE_ONLY,
            ..SdporConfig::STATELESS
        }),
        claim_pool_mutation: false,
    },
    Kind {
        name: "pool-claim-without-pool-access",
        // The Phase 9 search: last race only, no enabling rule.
        alg: Alg::Sdpor(SdporConfig {
            no_enabling_edges: true,
            defects: DEFECT_LAST_RACE_ONLY,
            ..SdporConfig::STATELESS
        }),
        claim_pool_mutation: true,
    },
];

/// Whether `kind`'s defective algorithm disagrees with the unreduced search.
fn exposes(kind: Kind, doc: &Value) -> bool {
    let Some(model) = load(doc) else { return false };
    let Some(f) = full(&model) else { return false };
    mutation::mute_claim_pool(kind.claim_pool_mutation);
    let r = run(kind.alg, &model);
    mutation::mute_claim_pool(false);
    if r.truncation.max_states || r.truncation.max_transitions {
        return false;
    }
    discrepancy(&f, &r, &model).is_some()
}

fn every_algorithm_agrees(doc: &Value) -> Result<(), String> {
    let model = load(doc).ok_or("does not load")?;
    let f = full(&model).ok_or("the unreduced search hit a limit")?;
    for alg in [
        Alg::Sleep,
        Alg::Classic,
        Alg::Sdpor(SdporConfig::STATEFUL),
        Alg::Sdpor(SdporConfig::STATELESS),
        Alg::Sdpor(SdporConfig::STATELESS_SLEEP),
        Alg::Sdpor(SdporConfig::STATEFUL_SLEEP),
    ] {
        let r = run(alg, &model);
        if let Some(why) = discrepancy(&f, &r, &model) {
            return Err(format!("{}: {why}", alg.name()));
        }
    }
    Ok(())
}

fn fixtures() -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(DIR).expect("tests/data/regressions") {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "json") {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
            out.push((p.file_name().unwrap().to_string_lossy().into_owned(), v));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn every_fixture_is_handled_by_every_algorithm() {
    let fx = fixtures();
    assert!(fx.len() >= KINDS.len(), "fixtures missing: {}", fx.len());
    for (name, v) in &fx {
        if let Err(why) = every_algorithm_agrees(&v["scenario"]) {
            panic!("{name}: {why}");
        }
    }
}

#[test]
fn every_fixture_still_exposes_its_historical_defect() {
    for (name, v) in fixtures() {
        let kind = KINDS
            .iter()
            .find(|k| k.name == v["defect"].as_str().unwrap_or(""))
            .unwrap_or_else(|| panic!("{name}: unknown defect {}", v["defect"]));
        assert!(
            exposes(*kind, &v["scenario"]),
            "{name}: the fixture no longer exposes `{}`",
            kind.name
        );
    }
}

#[test]
fn every_defect_has_a_fixture() {
    let fx = fixtures();
    for k in KINDS {
        assert!(
            fx.iter().any(|(_, v)| v["defect"] == k.name),
            "no fixture for `{}`",
            k.name
        );
    }
}

/// Search seeds of both generators for scenarios that expose each defect,
/// minimize them and write the fixtures.
#[test]
#[ignore = "rewrites tests/data/regressions"]
fn regenerate_fixtures() {
    std::fs::create_dir_all(DIR).unwrap();
    let per_kind: usize = std::env::var("PER_KIND")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let limit: u64 = std::env::var("SEARCH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1500);
    // The Phase 9 pool defect is reproduced from the workload that exposed
    // it (three tasks of one process, a pool of two), not searched for.
    let pool_doc = json!({
        "schemaVersion": 1, "name": "pay",
        "database": {"isolation": "read-committed",
            "tables": [{"name": "payments", "columns": [
                {"name": "id", "type": "int", "notNull": true, "default": {"sequence": {}}},
                {"name": "req", "type": "text"}], "primaryKey": ["id"], "unique": [["req"]]}],
            "initialState": {"payments": []}},
        "actors": (0..3).map(|i| json!({
            "name": format!("C{i}"), "function": "pay", "process": "P",
            "program": "begin\ninsert payments set @req = rid on conflict do nothing\ncommit\n",
            "arguments": {"rid": format!("r{}", i % 2)}})).collect::<Vec<_>>(),
        "pools": {"P": 2},
        "bounds": {"maxSteps": 60, "maxStates": 2000000},
        "invariants": [{"name": "once", "when": "final",
            "assert": {"le": [{"count": {"table": "payments", "where": {}}}, 2]}}]
    });
    let only = std::env::var("ONLY_KIND").ok();
    for kind in KINDS {
        if only.as_deref().is_some_and(|o| o != kind.name) {
            continue;
        }
        if kind.claim_pool_mutation {
            assert!(
                exposes(kind, &pool_doc),
                "the pool workload no longer exposes the defect"
            );
            let small = minimize(&pool_doc, &mut |d| exposes(kind, d));
            let file = format!("{DIR}/{}-phase9-workload.json", kind.name);
            let fixture = json!({
                "defect": kind.name,
                "algorithm": kind.alg.name(),
                "origin": { "generator": "phase9 dpor.rs", "seed": null },
                "originalSize": size(&pool_doc),
                "minimizedSize": size(&small),
                "original": pool_doc,
                "scenario": small,
            });
            std::fs::write(
                &file,
                serde_json::to_string_pretty(&fixture).unwrap() + "\n",
            )
            .unwrap();
            eprintln!(
                "{}: size {} -> {}",
                kind.name,
                size(&pool_doc),
                size(&small)
            );
            continue;
        }
        let mut found = 0;
        'seeds: for seed in 1..=limit {
            for (generator, doc) in [
                ("adversarial", generate(seed).doc),
                ("random", dpor_model(seed).0),
            ] {
                if !exposes(kind, &doc) {
                    continue;
                }
                let small = minimize(&doc, &mut |d| exposes(kind, d));
                let file = format!("{DIR}/{}-{generator}-{seed}.json", kind.name);
                let fixture = json!({
                    "defect": kind.name,
                    "algorithm": kind.alg.name(),
                    "origin": { "generator": generator, "seed": seed },
                    "originalSize": size(&doc),
                    "minimizedSize": size(&small),
                    "original": doc,
                    "scenario": small,
                });
                std::fs::write(
                    &file,
                    serde_json::to_string_pretty(&fixture).unwrap() + "\n",
                )
                .unwrap();
                eprintln!(
                    "{}: {generator} seed {seed}: size {} -> {}",
                    kind.name,
                    size(&doc),
                    size(&small)
                );
                found += 1;
                if found >= per_kind {
                    break 'seeds;
                }
            }
        }
        assert!(found > 0, "no scenario exposes `{}`", kind.name);
    }
}

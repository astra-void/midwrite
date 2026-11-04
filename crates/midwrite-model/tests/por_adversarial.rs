//! The adversarial scenario campaign (Phase 10): generated scenarios with
//! pools, shared processes, tasks, locks, sequences, application state and
//! faults, compared across the unreduced search, the sleep-set search, the
//! classic DPOR and every configuration of the source-set DPOR.
//!
//! `MIDWRITE_ADV_SEEDS` sets how many seeds run (default 360, 40 per
//! profile; the differential campaign runs 120 in a debug build; the evidence
//! runs use thousands in release mode).

mod common;

use std::collections::BTreeMap;

use common::adversarial::{COMBINATIONS, Generated, Profile, combinations_of, generate, observe};
use common::por_diff::{Alg, discrepancy, full, run};
use midwrite_model::explore::SdporConfig;
use midwrite_model::model::Model;
use midwrite_model::scenario::load_str;

pub const ALGS: [Alg; 5] = [
    Alg::Sleep,
    Alg::Classic,
    Alg::Sdpor(SdporConfig::STATEFUL),
    Alg::Sdpor(SdporConfig::STATELESS_SLEEP),
    Alg::Sdpor(SdporConfig::STATEFUL_SLEEP),
];

/// Seeds for a campaign: `MIDWRITE_ADV_SEEDS`, else `release` in an
/// optimized build and `debug` in a debug one (the differential campaign runs
/// five searches per seed; the evidence runs use 3 000 seeds in release).
fn seeds_or(release: u64, debug: u64) -> u64 {
    std::env::var("MIDWRITE_ADV_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if cfg!(debug_assertions) {
            debug
        } else {
            release
        })
}

fn load(g: &Generated) -> Result<Model, String> {
    load_str(
        &g.doc.to_string(),
        std::path::Path::new("."),
        &midwrite_model::provider::NoProvider,
    )
    .map(|(m, _)| m)
    .map_err(|e| e.to_string())
}

#[derive(Default)]
struct Campaign {
    generated: u64,
    invalid: Vec<(u64, String)>,
    /// The unreduced search hit a limit: nothing was compared.
    unverified: u64,
    compared: u64,
    by_profile: BTreeMap<Profile, (u64, u64)>,
    features: BTreeMap<String, u64>,
    combos: Vec<u64>,
    discrepancies: Vec<(u64, String, String)>,
    transitions: BTreeMap<String, u64>,
    full_transitions: u64,
}

fn campaign(n: u64, algs: &[Alg], compare_classic_under: u64) -> Campaign {
    let mut c = Campaign {
        combos: vec![0; COMBINATIONS.len()],
        ..Campaign::default()
    };
    for seed in 1..=n {
        let g = generate(seed);
        c.generated += 1;
        let model = match load(&g) {
            Ok(m) => m,
            Err(e) => {
                c.invalid.push((seed, e));
                continue;
            }
        };
        let observed = observe(&model, 20_000);
        for f in &g.features {
            *c.features.entry(f.clone()).or_default() += 1;
        }
        for f in &observed {
            *c.features.entry((*f).to_owned()).or_default() += 1;
        }
        for i in combinations_of(&g.features, &observed) {
            c.combos[i] += 1;
        }
        let Some(f) = full(&model) else {
            c.unverified += 1;
            c.by_profile.entry(g.profile).or_default().0 += 1;
            continue;
        };
        c.compared += 1;
        c.by_profile.entry(g.profile).or_default().1 += 1;
        c.full_transitions += f.metrics.transitions;
        for alg in algs {
            // The classic search is cubic in what it re-explores: only on
            // small models.
            if *alg == Alg::Classic && f.metrics.transitions > compare_classic_under {
                continue;
            }
            let r = run(*alg, &model);
            if r.truncation.max_states || r.truncation.max_transitions {
                continue;
            }
            *c.transitions.entry(alg.name()).or_default() += r.metrics.transitions;
            if let Some(why) = discrepancy(&f, &r, &model) {
                c.discrepancies.push((seed, alg.name(), why));
            }
        }
    }
    c
}

fn report(c: &Campaign) -> String {
    let mut s = format!(
        "{} generated, {} invalid, {} unverified (unreduced search hit a limit), {} compared\n",
        c.generated,
        c.invalid.len(),
        c.unverified,
        c.compared
    );
    s.push_str("per profile (unverified, compared):\n");
    for (p, (u, k)) in &c.by_profile {
        s.push_str(&format!("  {p:?}: {u}, {k}\n"));
    }
    s.push_str("combinations covered (models):\n");
    for ((name, _), n) in COMBINATIONS.iter().zip(&c.combos) {
        s.push_str(&format!("  {name}: {n}\n"));
    }
    s.push_str(&format!(
        "transitions: unreduced {}, {}\n",
        c.full_transitions,
        c.transitions
            .iter()
            .map(|(a, t)| format!("{a} {t}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    s
}

#[test]
fn the_generator_produces_valid_models_and_reaches_every_combination() {
    let n = seeds_or(360, 360);
    let c = campaign(n, &[], 0);
    eprintln!("{}", report(&c));
    assert!(
        c.invalid.len() as f64 <= 0.02 * n as f64,
        "too many invalid scenarios: {:?}",
        &c.invalid[..c.invalid.len().min(3)]
    );
    assert!(
        c.unverified as f64 <= 0.25 * n as f64,
        "too many scenarios the unreduced search cannot finish: {}",
        c.unverified
    );
    let scale = (n as f64 / 360.0).max(1.0);
    for ((name, _), covered) in COMBINATIONS.iter().zip(&c.combos) {
        assert!(
            *covered as f64 >= 3.0 * scale.min(10.0),
            "combination `{name}` was covered by {covered} models only"
        );
    }
}

#[test]
fn every_reduction_agrees_with_the_unreduced_search_on_adversarial_scenarios() {
    let n = seeds_or(360, 120);
    let c = campaign(n, &ALGS, 3_000);
    eprintln!("{}", report(&c));
    assert!(
        c.discrepancies.is_empty(),
        "reductions disagree with the unreduced search: {:?}",
        &c.discrepancies[..c.discrepancies.len().min(10)]
    );
    assert!(c.compared as f64 >= 0.6 * n as f64);
}

/// `SEED=6 cargo test --release --test por_adversarial dump_seed -- --ignored
/// --nocapture`: print the scenario of a seed (for the `sdpor_check` example).
#[test]
#[ignore = "prints a scenario; for debugging"]
fn dump_seed() {
    let seed: u64 = std::env::var("SEED").unwrap().parse().unwrap();
    println!(
        "{}",
        serde_json::to_string_pretty(&generate(seed).doc).unwrap()
    );
}

// ---------------------------------------------------------------------------
// Metamorphic tests: transformations that must not change what a reduction
// reports. They do not assume that every reordering preserves fairness or
// bounded context switches (neither is in play: no context-switch bound, no
// liveness), and they leave pooled scenarios' declaration order alone (pool
// arrival at start-up follows it, `docs/phase9/LIMITATIONS.md`).
// ---------------------------------------------------------------------------

use common::adversarial::generate_profile;
use serde_json::Value;

/// What a transformation must preserve: verdict, number of distinct
/// terminal states, stuck-state existence, per algorithm.
fn signature(model: &Model) -> Option<Vec<(String, String, usize, bool)>> {
    let f = full(model)?;
    let mut out = vec![(
        "full".to_owned(),
        format!("{:?}", f.verdict),
        f.terminals.as_ref().map_or(0, |t| t.len()),
        f.stuck.is_empty(),
    )];
    for alg in [
        Alg::Sleep,
        Alg::Sdpor(SdporConfig::STATEFUL),
        Alg::Sdpor(SdporConfig::STATEFUL_SLEEP),
    ] {
        let r = run(alg, model);
        out.push((
            alg.name(),
            format!("{:?}", r.verdict),
            r.terminals.as_ref().map_or(0, |t| t.len()),
            r.stuck.is_empty(),
        ));
    }
    Some(out)
}

fn model_of(doc: &Value) -> Option<Model> {
    load_str(
        &doc.to_string(),
        std::path::Path::new("."),
        &midwrite_model::provider::NoProvider,
    )
    .ok()
    .map(|(m, _)| m)
}

/// Rename every actor (and process) and every table.
fn rename(doc: &Value) -> Value {
    let mut text = doc.to_string();
    // Tables are named t, p0.., log; programs mention them as words.
    for (from, to) in [("\"t\"", "\"tbl_main\""), ("\"log\"", "\"tbl_log\"")] {
        text = text.replace(from, to);
    }
    let mut d: Value = serde_json::from_str(&text).unwrap();
    for a in d["actors"].as_array_mut().unwrap() {
        let name = a["name"].as_str().unwrap().replace('A', "Renamed");
        a["name"] = name.into();
        if let Some(p) = a["program"].as_str() {
            let p = p
                .replace(" t where", " tbl_main where")
                .replace("update t ", "update tbl_main ")
                .replace("select t ", "select tbl_main ")
                .replace("count t ", "count tbl_main ")
                .replace("insert t ", "insert tbl_main ")
                .replace("delete t ", "delete tbl_main ")
                .replace("insert log ", "insert tbl_log ");
            a["program"] = p.into();
        }
    }
    d
}

/// Reverse the declaration order of the actors (not for pooled scenarios).
fn reverse_actors(doc: &Value) -> Value {
    let mut d = doc.clone();
    d["actors"].as_array_mut().unwrap().reverse();
    d
}

/// Add a pure local step to every program: it is not a visible operation.
fn add_pure_step(doc: &Value) -> Value {
    let mut d = doc.clone();
    for a in d["actors"].as_array_mut().unwrap() {
        let p = a["program"].as_str().unwrap().to_owned();
        let mut lines: Vec<&str> = p.lines().collect();
        let at = lines.len() / 2;
        lines.insert(at, "let pure_unused = 1");
        a["program"] = format!("{}\n", lines.join("\n")).into();
    }
    d
}

/// Add an independent copy of the whole workload under a separate namespace
/// would multiply the state space; instead add one extra independent actor on
/// a table of its own, which must not change the reduced verdicts.
fn add_independent_actor(doc: &Value) -> Value {
    let mut d = doc.clone();
    d["database"]["tables"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "name": "zz_private",
            "columns": [{"name":"id","notNull":true,"type":"int"}, {"name":"v","type":"int"}],
            "primaryKey": ["id"]
        }));
    d["database"]["initialState"]["zz_private"] = serde_json::json!([{"id": 1, "v": 0}]);
    d["actors"].as_array_mut().unwrap().push(serde_json::json!({
        "name": "Zextra", "function": "work", "arguments": {},
        "program": "update zz_private where @id == 1 set @v = @v + 1\n"
    }));
    d
}

#[test]
fn metamorphic_transformations_do_not_change_what_reductions_report() {
    let mut checked = [0u32; 4];
    for profile in [
        Profile::Rows,
        Profile::Locks,
        Profile::Serializable,
        Profile::Seq,
        Profile::Faults,
    ] {
        for seed in 0..12u64 {
            let g = generate_profile(1000 + seed * 9 + profile as u64, profile);
            let Some(base) = model_of(&g.doc) else {
                continue;
            };
            let Some(sig) = signature(&base) else {
                continue;
            };
            let pooled = g.doc.get("pools").is_some();
            let cases: [(&str, Value, bool); 4] = [
                ("rename", rename(&g.doc), true),
                ("reverse actors", reverse_actors(&g.doc), !pooled),
                ("pure step", add_pure_step(&g.doc), true),
                (
                    "independent actor",
                    add_independent_actor(&g.doc),
                    g.doc["actors"].as_array().unwrap().len() < 3,
                ),
            ];
            for (i, (name, doc, applies)) in cases.iter().enumerate() {
                if !applies {
                    continue;
                }
                let Some(m) = model_of(doc) else { continue };
                let Some(s2) = signature(&m) else { continue };
                // The independent actor changes the count of terminal states
                // only by doubling it at most (its one update commutes with
                // everything); verdicts and stuck-state existence must hold.
                if *name == "independent actor" {
                    for (a, b) in sig.iter().zip(&s2) {
                        assert_eq!(
                            (&a.0, &a.1, a.3),
                            (&b.0, &b.1, b.3),
                            "{name}: seed {} {profile:?}",
                            g.seed
                        );
                    }
                } else {
                    assert_eq!(sig, s2, "{name}: seed {} {profile:?}", g.seed);
                }
                checked[i] += 1;
            }
        }
    }
    eprintln!(
        "metamorphic cases checked: rename {}, reverse {}, pure {}, independent {}",
        checked[0], checked[1], checked[2], checked[3]
    );
    assert!(checked.iter().all(|c| *c >= 8), "{checked:?}");
}

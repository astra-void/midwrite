//! Compact state identity. The visited table stores interned
//! fragments instead of flat canonical encodings; it must identify exactly
//! the same states.

mod common;

use std::collections::{BTreeSet, VecDeque};

use common::scenarios_gen::random_scenario;
use common::*;
use midwrite_model::engine::{Recorder, apply, enabled, initial_state};
use midwrite_model::explore::{ExploreOptions, explore_with};
use midwrite_model::model::{Model, Strategy};
use midwrite_model::scenario::testing::model_from_json;
use midwrite_model::state::Interner;
use serde_json::json;

/// Every reachable state's flat canonical encoding, by an independent
/// breadth-first enumeration (no reduction, no interning).
fn flat_states(model: &Model) -> BTreeSet<Vec<u8>> {
    let init = initial_state(model);
    let mut seen = BTreeSet::new();
    seen.insert(init.canonical(model));
    let mut queue = VecDeque::from([init]);
    while let Some(s) = queue.pop_front() {
        if s.steps >= model.bounds.max_steps {
            continue;
        }
        for t in enabled(model, &s) {
            let Ok(a) = apply(model, &s, t, &mut Recorder::off()) else {
                continue;
            };
            if let Some(limit) = model.bounds.max_context_switches
                && a.state.switches > limit
            {
                continue;
            }
            if seen.insert(a.state.canonical(model)) {
                queue.push_back(a.state);
            }
            if seen.len() > 400_000 {
                return seen;
            }
        }
    }
    seen
}

fn check(label: &str, model: &Model) {
    let flat = flat_states(model);
    if flat.len() > 400_000 {
        return;
    }
    for (por, strategy) in [
        (false, Strategy::Bfs),
        (false, Strategy::Dfs),
        (true, Strategy::Dfs),
    ] {
        let r = explore_with(
            model,
            &ExploreOptions {
                por,
                strategy,
                collect_states: true,
                stop_at_first: false,
            },
        );
        if r.truncation.resource_limit() {
            continue;
        }
        assert_eq!(
            r.reached.as_ref().unwrap(),
            &flat,
            "{label}: the compact table identifies different states ({} vs {})",
            r.metrics.states,
            flat.len()
        );
        assert_eq!(r.metrics.states as usize, flat.len(), "{label}");
    }
}

#[test]
fn the_compact_table_identifies_exactly_the_states_of_the_flat_encoding() {
    for (label, model) in [
        (
            "lost-update-tx/rc",
            purchase("read-committed", PURCHASE_TX, json!({})),
        ),
        (
            "lost-update-tx/rr",
            purchase("repeatable-read", PURCHASE_TX, json!({})),
        ),
        ("write-skew/ser", write_skew("serializable", json!({}))),
        ("deadlock/rc", deadlock("read-committed", json!({}))),
        (
            "crash-retry",
            crash_after_commit(1, Some("onFailureOrCrash"), 1),
        ),
        ("tx-retry", tx_retry(2)),
        ("tasks", concurrent_tasks()),
    ] {
        check(label, &model);
    }
}

#[test]
fn random_scenarios_keep_their_states() {
    let n: u64 = std::env::var("MIDWRITE_POR_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    for seed in 1..=n {
        let mut doc = random_scenario(seed);
        if seed.is_multiple_of(3) {
            doc["bounds"]["maxInFlight"] = 1.into();
        }
        check(&format!("seed {seed}"), &model_from_json(doc));
    }
}

#[test]
fn fragments_are_shared_between_states() {
    // The point of interning: many states, few fragments.
    let models: Vec<Model> = (1..=30)
        .map(|seed| {
            let mut doc = random_scenario(seed);
            doc["bounds"]["maxStates"] = 50_000.into();
            model_from_json(doc)
        })
        .collect();
    let mut best = (0usize, 0usize, 0usize);
    for model in &models {
        if flat_states(model).len() > 20_000 {
            continue;
        }
        let mut interner = Interner::default();
        let mut key = Vec::new();
        let (mut keys, mut refs) = (0usize, 0usize);
        for s in flat_states_vec(model) {
            s.canonical_key(model, &mut interner, &mut key);
            keys += 1;
            refs += key.len();
        }
        if keys > best.0 {
            best = (keys, refs, interner.len());
        }
    }
    let (keys, refs, distinct) = best;
    assert!(keys > 50, "{keys} states");
    assert!(
        distinct * 3 < refs,
        "{distinct} distinct fragments for {refs} fragment references in {keys} states"
    );
}

fn flat_states_vec(model: &Model) -> Vec<midwrite_model::state::State> {
    let init = initial_state(model);
    let mut seen = BTreeSet::new();
    seen.insert(init.canonical(model));
    let mut out = vec![init.clone()];
    let mut queue = VecDeque::from([init]);
    while let Some(s) = queue.pop_front() {
        for t in enabled(model, &s) {
            let Ok(a) = apply(model, &s, t, &mut Recorder::off()) else {
                continue;
            };
            if seen.insert(a.state.canonical(model)) {
                out.push(a.state.clone());
                queue.push_back(a.state);
            }
        }
    }
    out
}

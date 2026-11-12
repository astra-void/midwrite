//! Source-set DPOR against the unreduced search, the sleep-set
//! search and the classic DPOR of Phase 9.
//!
//! What a DPOR must preserve is what is a function of the final state of a
//! trace: the set of terminal states (hence final-invariant violations and
//! counterexample existence), stuck states, unsupported behavior and
//! truncation by the step bound. The comparison is on exactly that, on the
//! targeted Phase 6-9 workloads, the Phase 9 model scenarios, pool workloads
//! and seeded random scenarios, for every configuration of the new search.
//! A scenario for which the unreduced search hits its limit is *skipped*, not
//! counted as agreeing.

mod common;

use common::por_workloads::{dpor_model, in_flight_workloads, seeds, targeted_workloads};
use midwrite_model::explore::{
    ExploreOptions, ExploreResult, SdporConfig, dpor_inapplicable, explore_dpor, explore_sdpor,
    explore_with,
};
use midwrite_model::model::{Model, Strategy};

fn opts(por: bool) -> ExploreOptions {
    ExploreOptions {
        por,
        strategy: Strategy::Dfs,
        collect_states: true,
        stop_at_first: false,
    }
}

pub const CONFIGS: [(&str, SdporConfig); 4] = [
    ("stateful", SdporConfig::STATEFUL),
    ("stateless+sleep", SdporConfig::STATELESS_SLEEP),
    ("stateful+sleep", SdporConfig::STATEFUL_SLEEP),
    ("stateless", SdporConfig::STATELESS),
];

pub struct Row {
    pub full: u64,
    pub sleep: u64,
    pub classic: Option<u64>,
    pub sdpor: Vec<u64>,
}

fn check_same(label: &str, what: &str, full: &ExploreResult, r: &ExploreResult, model: &Model) {
    assert_eq!(
        full.terminals, r.terminals,
        "{label}: {what} changed the set of terminal states ({} vs {})",
        full.metrics.terminal_states, r.metrics.terminal_states
    );
    assert_eq!(full.verdict, r.verdict, "{label}: {what}: verdict");
    assert_eq!(
        full.counterexample.is_some(),
        r.counterexample.is_some(),
        "{label}: {what}: counterexample existence"
    );
    assert_eq!(
        full.stuck.is_empty(),
        r.stuck.is_empty(),
        "{label}: {what}: stuck states"
    );
    assert_eq!(
        full.unsupported.is_empty(),
        r.unsupported.is_empty(),
        "{label}: {what}: unsupported behavior"
    );
    assert_eq!(
        full.truncation.max_steps, r.truncation.max_steps,
        "{label}: {what}: truncation by the step bound"
    );
    if let Some(ce) = &r.counterexample {
        assert!(
            midwrite_model::witness::check_schedule(model, &ce.schedule, &ce.invariant).is_some(),
            "{label}: {what}: the counterexample does not replay"
        );
    }
}

/// Compare every configuration; `None` when the unreduced search hit a limit
/// (nothing is claimed) or a reduced search did.
pub fn compare(label: &str, model: &Model, with_classic: bool) -> Option<Row> {
    assert_eq!(dpor_inapplicable(model), None, "{label}");
    let mut m = model.clone();
    m.bounds.max_states = 2_000_000;
    let full = explore_with(&m, &opts(false));
    if full.truncation.resource_limit() && !full.truncation.max_steps {
        return None;
    }
    let sleep = explore_with(&m, &opts(true));
    let mut sdpor = Vec::new();
    for (name, cfg) in CONFIGS {
        let r = explore_sdpor(&m, &opts(false), cfg);
        if r.truncation.max_states || r.truncation.max_transitions || r.truncation.max_wall_time {
            return None;
        }
        check_same(label, name, &full, &r, model);
        sdpor.push(r.metrics.transitions);
    }
    check_same(label, "sleep sets", &full, &sleep, model);
    let classic = with_classic.then(|| {
        let r = explore_dpor(&m, &opts(false));
        check_same(label, "classic DPOR", &full, &r, model);
        r.metrics.transitions
    });
    Some(Row {
        full: full.metrics.transitions,
        sleep: sleep.metrics.transitions,
        classic,
        sdpor,
    })
}

fn fmt(label: &str, r: &Row) -> String {
    let mut s = format!(
        "{label}: full {}, sleep {}, classic {}",
        r.full,
        r.sleep,
        r.classic.map_or("-".into(), |c| c.to_string())
    );
    for ((name, _), t) in CONFIGS.iter().zip(&r.sdpor) {
        s.push_str(&format!(", {name} {t}"));
    }
    s
}

#[test]
fn targeted_workloads_keep_every_terminal_state() {
    let mut report = String::new();
    for (label, model) in targeted_workloads() {
        let row = compare(label, &model, true)
            .unwrap_or_else(|| panic!("{label}: exploration hit a limit"));
        report.push_str(&fmt(label, &row));
        report.push('\n');
    }
    eprintln!("{report}");
}

#[test]
fn in_flight_and_pool_workloads_keep_every_terminal_state() {
    let mut report = String::new();
    let mut failed = Vec::new();
    for (label, model) in in_flight_workloads() {
        assert_eq!(dpor_inapplicable(&model), None, "{label}");
        match std::panic::catch_unwind(|| compare(&label, &model, true)) {
            Ok(Some(row)) => {
                report.push_str(&fmt(&label, &row));
                report.push('\n');
            }
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
    let mut sums = vec![0u64; CONFIGS.len()];
    let (mut fs, mut ss) = (0u64, 0u64);
    for seed in seeds() {
        let (doc, model) = dpor_model(seed);
        let result = std::panic::catch_unwind(|| compare(&format!("seed {seed}"), &model, false));
        match result {
            Ok(Some(row)) => {
                compared += 1;
                fs += row.full;
                ss += row.sleep;
                for (s, t) in sums.iter_mut().zip(&row.sdpor) {
                    *s += t;
                }
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
        "{compared} compared, {skipped} skipped (limit); transitions full {fs}, sleep {ss}, {}",
        CONFIGS
            .iter()
            .zip(&sums)
            .map(|((n, _), t)| format!("{n} {t}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    assert!(compared >= 100, "too few comparable scenarios: {compared}");
}

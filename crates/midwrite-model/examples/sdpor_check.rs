//! Compare the reductions on one scenario file (a model-checker scenario
//! without source extraction): `sdpor_check SCENARIO.json [key=value...]`
//! where `key=value` overrides `bounds.key` (numbers) before the run.

use midwrite_model::explore::{
    ExploreOptions, SdporConfig, explore_dpor, explore_sdpor, explore_with,
};
use midwrite_model::model::Strategy;
use midwrite_model::scenario::testing::model_from_json;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&args[1]).unwrap()).unwrap();
    for kv in &args[2..] {
        let (k, v) = kv.split_once('=').unwrap();
        doc["bounds"][k] = v.parse::<u64>().unwrap().into();
    }
    let model = model_from_json(doc);
    let o = |por| ExploreOptions {
        por,
        strategy: Strategy::Dfs,
        collect_states: true,
        stop_at_first: false,
    };
    let full = explore_with(&model, &o(false));
    if std::env::var_os("MUTE_CLAIM").is_some() {
        midwrite_model::footprint::mutation::mute_claim_pool(true);
    }
    println!(
        "full:      terminals {} transitions {}",
        full.metrics.terminal_states, full.metrics.transitions
    );
    let sleep = explore_with(&model, &o(true));
    println!(
        "sleep:     terminals {} transitions {}",
        sleep.metrics.terminal_states, sleep.metrics.transitions
    );
    let classic = explore_dpor(&model, &o(false));
    println!(
        "classic:   distinct terminals {} transitions {} same={}",
        classic.terminals.as_ref().map_or(0, |t| t.len()),
        classic.metrics.transitions,
        classic.terminals == full.terminals
    );
    let missing_for = |r: &midwrite_model::explore::ExploreResult| -> Vec<Vec<u8>> {
        full.terminals
            .as_ref()
            .unwrap()
            .iter()
            .filter(|k| !r.terminals.as_ref().unwrap().contains(*k))
            .cloned()
            .collect()
    };
    for (name, cfg) in [
        ("stateful", SdporConfig::STATEFUL),
        ("stateless+sleep", SdporConfig::STATELESS_SLEEP),
        ("stateful+sleep", SdporConfig::STATEFUL_SLEEP),
        ("stateless", SdporConfig::STATELESS),
    ] {
        if std::env::var("ONLY").is_ok_and(|v| v != name) {
            continue;
        }
        let mut cfg = cfg;
        if std::env::var_os("NOEDGES").is_some() {
            cfg.no_enabling_edges = true;
        }
        let r = explore_sdpor(&model, &o(false), cfg);
        println!(
            "{name:16} distinct terminals {} transitions {} same={}",
            r.terminals.as_ref().map_or(0, |t| t.len()),
            r.metrics.transitions,
            r.terminals == full.terminals
        );
        if r.terminals != full.terminals && std::env::var_os("SHOW_MISSING").is_some() {
            for key in missing_for(&r) {
                // Enumerate maximal paths of the unreduced graph to this key.
                use midwrite_model::engine::{Recorder, apply, enabled, initial_state};
                let mut stack = vec![(initial_state(&model), Vec::new())];
                let mut shown = 0;
                while let Some((s, path)) = stack.pop() {
                    let en = enabled(&model, &s);
                    if en.is_empty() && s.canonical(&model) == key && shown < 3 {
                        shown += 1;
                        println!("  missing terminal reached by: {path:?}");
                    }
                    for t in en {
                        if let Ok(a) = apply(&model, &s, t, &mut Recorder::off()) {
                            let mut p = path.clone();
                            p.push((t.kind, t.actor, t.thread));
                            stack.push((a.state, p));
                        }
                    }
                }
            }
        }
    }
}

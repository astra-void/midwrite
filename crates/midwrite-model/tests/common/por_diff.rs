//! Running the reductions side by side and describing how a reduced search
//! differs from the unreduced one.
#![allow(dead_code)]

use midwrite_model::explore::{
    ExploreOptions, ExploreResult, SdporConfig, explore_dpor, explore_sdpor, explore_with,
};
use midwrite_model::model::{Model, Strategy};

/// A search algorithm under test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alg {
    Sleep,
    Classic,
    Sdpor(SdporConfig),
}

impl Alg {
    pub fn name(self) -> String {
        match self {
            Alg::Sleep => "sleep".into(),
            Alg::Classic => "classic".into(),
            Alg::Sdpor(c) => format!(
                "sdpor[{}{}{}]",
                if c.cache { "cache" } else { "stateless" },
                if c.sleep { "+sleep" } else { "" },
                if c.no_enabling_edges { ",no-edges" } else { "" }
            ),
        }
    }
}

pub fn opts(por: bool) -> ExploreOptions {
    ExploreOptions {
        por,
        strategy: Strategy::Dfs,
        collect_states: true,
        stop_at_first: false,
    }
}

pub fn run(alg: Alg, model: &Model) -> ExploreResult {
    match alg {
        Alg::Sleep => explore_with(model, &opts(true)),
        Alg::Classic => explore_dpor(model, &opts(false)),
        Alg::Sdpor(c) => explore_sdpor(model, &opts(false), c),
    }
}

/// The unreduced search, or `None` when it hit a resource limit (then
/// nothing can be compared against it).
pub fn full(model: &Model) -> Option<ExploreResult> {
    let mut m = model.clone();
    m.bounds.max_states = m.bounds.max_states.max(2_000_000);
    let r = explore_with(&m, &opts(false));
    (!(r.truncation.max_states || r.truncation.max_transitions || r.truncation.max_wall_time))
        .then_some(r)
}

/// How `r` differs from the unreduced search `full` in what a reduction must
/// preserve: the set of terminal states, verdict, counterexample existence,
/// stuck states, unsupported behavior, truncation by the step bound, and the
/// validity of its counterexample. `None` when nothing differs.
pub fn discrepancy(full: &ExploreResult, r: &ExploreResult, model: &Model) -> Option<String> {
    if full.terminals != r.terminals {
        let (f, d) = (
            full.terminals.as_ref().map_or(0, |t| t.len()),
            r.terminals.as_ref().map_or(0, |t| t.len()),
        );
        return Some(format!("terminal states differ ({f} vs {d} distinct)"));
    }
    if full.verdict != r.verdict {
        return Some(format!("verdict {:?} vs {:?}", full.verdict, r.verdict));
    }
    if full.counterexample.is_some() != r.counterexample.is_some() {
        return Some("counterexample existence".into());
    }
    if full.stuck.is_empty() != r.stuck.is_empty() {
        return Some("stuck states".into());
    }
    if full.unsupported.is_empty() != r.unsupported.is_empty() {
        return Some("unsupported behavior".into());
    }
    if full.truncation.max_steps != r.truncation.max_steps {
        return Some("truncation by the step bound".into());
    }
    if let Some(ce) = &r.counterexample
        && midwrite_model::witness::check_schedule(model, &ce.schedule, &ce.invariant).is_none()
    {
        return Some("the counterexample does not replay".into());
    }
    None
}

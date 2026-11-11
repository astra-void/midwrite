//! Machine-checked audit of the dependence relation.
//!
//! Every reduction trusts the footprints of [`crate::footprint`]: transitions
//! whose footprints do not conflict are treated as independent. This module
//! walks the reachable states of a bounded model and tests that trust
//! directly, instead of inferring it from the verdicts of reduced searches.
//!
//! For every reachable state `s` and every pair `a`, `b` of transitions
//! enabled in `s` that the footprints judge independent:
//!
//! * **legal in both orders**: `b` is enabled after `a` and `a` after `b`
//!   (neither disables the other);
//! * **diamond**: both orders succeed and reach the same state under the
//!   equivalence the reductions preserve
//!   ([`crate::state::State::canonical_observable`]); equal states have equal
//!   futures, so equal enabled sets, pending server operations, connection
//!   ownership, task states and invariant values follow;
//! * **stable footprints** (reported separately, `footprint_drift`): `b`
//!   accesses the same resources after `a` as before it.
//!
//! And for every transition `t` enabled in `s` and every transition `x` that
//! `t` *enables* (enabled after `t`, not in `s`, and not the next step of
//! `t`'s own task):
//!
//! * **enabling pairs conflict**: `t` and `x` are footprint-dependent. The
//!   explorers also treat an enabling pair as dependent whatever the
//!   footprints say (`docs/architecture/partial-order-reduction.md`), so a violation
//!   here is a footprint that does not show a dependency the search had to
//!   supply itself; the Phase 9 pool hand-over defect was one.
//!
//! `co_enabling_pairs` counts independent pairs `a`, `b` that both enable the
//! same transition (`Crash` after the first visible operation of any task of
//! a process): the situation in which no single enabler is a necessary cause.

use std::collections::HashSet;

use crate::engine::{Recorder, apply, enabled, initial_state};
use crate::footprint::{Footprint, dependent};
use crate::model::Model;
use crate::state::Transition;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViolationKind {
    FirstDisablesSecond,
    SecondDisablesFirst,
    OrdersReachDifferentStates,
    OneOrderUnsupported,
    EnablingPairIndependent,
}

impl ViolationKind {
    pub fn describe(self) -> &'static str {
        match self {
            Self::FirstDisablesSecond => "the first transition disables the second",
            Self::SecondDisablesFirst => "the second transition disables the first",
            Self::OrdersReachDifferentStates => "the two orders reach different states",
            Self::OneOrderUnsupported => "one order reaches unsupported behavior",
            Self::EnablingPairIndependent => {
                "a transition enables another but their footprints do not conflict"
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Violation {
    /// A schedule reaching the state the pair was enabled in.
    pub schedule: Vec<Transition>,
    pub a: Transition,
    pub b: Transition,
    pub kind: ViolationKind,
}

#[derive(Clone, Debug, Default)]
pub struct AuditReport {
    pub states: u64,
    pub independent_pairs: u64,
    pub enabling_pairs: u64,
    pub co_enabling_pairs: u64,
    pub footprint_drift: u64,
    pub violations: Vec<Violation>,
    /// The walk stopped at the state limit.
    pub truncated: bool,
}

fn same_accesses(a: &Footprint, b: &Footprint) -> bool {
    a.acc.len() == b.acc.len() && a.acc.iter().all(|x| b.acc.contains(x))
}

/// Walk up to `max_states` states and audit every one; stop collecting after
/// `max_violations`.
pub fn audit(model: &Model, max_states: usize, max_violations: usize) -> AuditReport {
    let mut rep = AuditReport::default();
    let init = initial_state(model);
    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    seen.insert(init.canonical(model));
    let mut stack = vec![(init, Vec::<Transition>::new())];
    while let Some((s, path)) = stack.pop() {
        if seen.len() > max_states {
            rep.truncated = true;
            break;
        }
        rep.states += 1;
        let en = enabled(model, &s);
        let mut applied = Vec::new();
        for t in &en {
            if let Ok(a) = apply(model, &s, *t, &mut Recorder::off()) {
                applied.push((*t, a));
            }
        }
        let report = |rep: &mut AuditReport, a: Transition, b: Transition, kind| {
            if rep.violations.len() < max_violations {
                rep.violations.push(Violation {
                    schedule: path.clone(),
                    a,
                    b,
                    kind,
                });
            }
        };
        // Enabling pairs.
        for (t, at) in &applied {
            let after = enabled(model, &at.state);
            for x in after.iter().filter(|x| !en.contains(x)) {
                // The next step of the same task follows its program order.
                if x.kind == t.kind && x.actor == t.actor && x.thread == t.thread {
                    continue;
                }
                rep.enabling_pairs += 1;
                if let Ok(ax) = apply(model, &at.state, *x, &mut Recorder::off())
                    && !dependent(model, &at.footprint, &ax.footprint)
                {
                    report(&mut rep, *t, *x, ViolationKind::EnablingPairIndependent);
                }
            }
        }
        // Independent pairs.
        for i in 0..applied.len() {
            for j in (i + 1)..applied.len() {
                let (ta, aa) = &applied[i];
                let (tb, ab) = &applied[j];
                if dependent(model, &aa.footprint, &ab.footprint) {
                    continue;
                }
                rep.independent_pairs += 1;
                let after_a = enabled(model, &aa.state);
                let after_b = enabled(model, &ab.state);
                if !after_a.contains(tb) {
                    report(&mut rep, *ta, *tb, ViolationKind::FirstDisablesSecond);
                    continue;
                }
                if !after_b.contains(ta) {
                    report(&mut rep, *ta, *tb, ViolationKind::SecondDisablesFirst);
                    continue;
                }
                // Both enable the same transition: no single necessary cause.
                if after_a
                    .iter()
                    .any(|x| !en.contains(x) && after_b.contains(x) && x != ta && x != tb)
                {
                    rep.co_enabling_pairs += 1;
                }
                match (
                    apply(model, &aa.state, *tb, &mut Recorder::off()),
                    apply(model, &ab.state, *ta, &mut Recorder::off()),
                ) {
                    (Ok(x), Ok(y)) => {
                        if x.state.canonical_observable(model)
                            != y.state.canonical_observable(model)
                        {
                            report(
                                &mut rep,
                                *ta,
                                *tb,
                                ViolationKind::OrdersReachDifferentStates,
                            );
                        }
                        if !same_accesses(&ab.footprint, &x.footprint)
                            || !same_accesses(&aa.footprint, &y.footprint)
                        {
                            rep.footprint_drift += 1;
                        }
                    }
                    _ => report(&mut rep, *ta, *tb, ViolationKind::OneOrderUnsupported),
                }
            }
        }
        for (t, a) in applied {
            let key = a.state.canonical(model);
            if seen.insert(key) && a.state.steps < model.bounds.max_steps {
                let mut p = path.clone();
                p.push(t);
                stack.push((a.state, p));
            }
        }
    }
    rep
}

//! What the application could and could not know (Phase 9).
//!
//! For a schedule that contains a lost acknowledgment, [`analyze`] builds, for
//! each unresolved outcome, the *twin* schedule in which the server did the
//! other thing, replays it strictly, and compares
//!
//! * what the application of the affected actor observed in the two worlds
//!   (the first observation that differs is where the application could have
//!   learned which world it is in; if there is none, it could never know), and
//! * whether the declared invariants hold in the twin.
//!
//! This is the possible-worlds reading of the explored state graph: the two
//! worlds share the schedule up to the fault and are explored as separate
//! transitions; the application's *observation history* decides what it can
//! tell apart. It is exact for the bounded model: no observation is invented,
//! and a world is only a twin if the model's own replay accepts it.

use serde::{Deserialize, Serialize};

use crate::engine::{Event, EventKind, Recorder, apply, in_flight_ops, initial_state};
use crate::epistemic::{InFlightOp, OutcomeKnowledge, OutcomeSet, World};
use crate::invariant::When;
use crate::model::Model;
use crate::state::{TKind, Transition};
use crate::witness::{complete, replay};

/// One invariant evaluated on the final state of a world.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldVerdict {
    pub invariant: String,
    pub holds: bool,
}

/// The twin of one unresolved outcome.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Twin {
    /// 1-based index of the in-flight transition in the schedule.
    pub transition: usize,
    pub actor: String,
    /// `COMMIT` or the autocommit statement.
    pub operation: String,
    /// What happened in the schedule that was reported.
    pub actual: World,
    /// The other world.
    pub other: World,
    /// What the application can conclude at that moment.
    pub knowledge: String,
    /// `replayed` (the twin schedule is valid in the model) or `diverges`
    /// (the model rejects it at the step named in `detail`).
    pub twin: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// Invariants in the reported world and in the twin (final states).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actual_verdicts: Vec<WorldVerdict>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub twin_verdicts: Vec<WorldVerdict>,
    /// Observations of the actor after the fault, in the reported world.
    pub observations: usize,
    /// Number of observations the two worlds have in common before the first
    /// difference (`observations` when they never differ: the application
    /// can never tell the worlds apart).
    pub identical_observations: usize,
    /// The first observation that differs: (reported world, twin).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_difference: Option<(String, String)>,
    /// A one-sentence reading of the above.
    pub reading: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpistemicReport {
    pub twins: Vec<Twin>,
}

fn observed(e: &Event, actor: u16) -> bool {
    e.actor == actor
        && !matches!(
            e.kind,
            EventKind::AckLost
                | EventKind::ServerCompletion
                | EventKind::Crash
                | EventKind::Abort
                | EventKind::Resumed
        )
}

fn verdicts(model: &Model, state: &crate::state::State) -> Vec<WorldVerdict> {
    model
        .invariants
        .iter()
        .filter(|i| i.when == When::Final || i.when == When::Always)
        .map(|i| WorldVerdict {
            invariant: i.name.clone(),
            holds: i.holds(model, state),
        })
        .collect()
}

/// Replay `schedule` strictly and, if the final state is not quiescent,
/// complete it with the lowest enabled steps (so that `final` invariants can
/// be evaluated).
struct Run {
    events: Vec<Vec<Event>>,
    state: crate::state::State,
}

fn run(model: &Model, schedule: &[Transition]) -> Result<Run, String> {
    let r = replay(model, schedule, true).map_err(|e| e.to_string())?;
    let mut events: Vec<Vec<Event>> = r.steps.into_iter().map(|(_, e)| e).collect();
    let mut state = r.state;
    if let Some(rest) = complete(model, &state) {
        for t in rest {
            let mut rec = Recorder::on();
            let applied = apply(model, &state, t, &mut rec).map_err(|u| u.detail)?;
            state = applied.state;
            events.push(rec.events);
        }
    }
    Ok(Run { events, state })
}

fn observations_after(run: &Run, from: usize, actor: u16) -> Vec<String> {
    run.events
        .iter()
        .skip(from)
        .flatten()
        .filter(|e| observed(e, actor))
        .map(|e| e.text.clone())
        .collect()
}

type OrphanKey = (u16, u32);

fn orphan_keys(state: &crate::state::State) -> Vec<OrphanKey> {
    state.orphans.iter().map(|o| (o.actor, o.seq)).collect()
}

/// The twin analysis of every unresolved outcome in `schedule`.
pub fn analyze(model: &Model, schedule: &[Transition]) -> EpistemicReport {
    let mut report = EpistemicReport::default();
    let Ok(actual) = run(model, schedule) else {
        return report;
    };
    let actual_verdicts = verdicts(model, &actual.state);
    let mut state = initial_state(model);
    for (i, t) in schedule.iter().enumerate() {
        // The faults of this transition: (twin transition, actor, task, op,
        // world in the reported schedule).
        let mut faults: Vec<(Transition, usize, usize, InFlightOp, World)> = Vec::new();
        match t.kind {
            TKind::LostCommitted | TKind::LostNotCommitted => {
                let actual_world = if t.kind == TKind::LostCommitted {
                    World::Committed
                } else {
                    World::NotCommitted
                };
                let a = t.actor as usize;
                let th = t.thread as usize;
                let op = in_flight_ops(model, &state, model.process_of[a])
                    .into_iter()
                    .find(|(x, y, _)| (*x, *y) == (a, th))
                    .map(|(_, _, op)| op);
                if let Some(op) = op {
                    let twin = Transition {
                        kind: if t.kind == TKind::LostCommitted {
                            TKind::LostNotCommitted
                        } else {
                            TKind::LostCommitted
                        },
                        ..*t
                    };
                    faults.push((twin, a, th, op, actual_world));
                }
            }
            TKind::CrashInFlight => {
                let ops = in_flight_ops(model, &state, t.thread as usize);
                for (bit, (a, th, op)) in ops.iter().enumerate() {
                    let reaches = (t.actor >> bit) & 1 == 1;
                    let twin = Transition {
                        actor: t.actor ^ (1 << bit),
                        ..*t
                    };
                    faults.push((
                        twin,
                        *a,
                        *th,
                        *op,
                        if reaches {
                            World::Committed
                        } else {
                            World::NotCommitted
                        },
                    ));
                }
            }
            _ => {}
        }
        let before_orphans = orphan_keys(&state);
        let actual_after = apply(model, &state, *t, &mut Recorder::off()).ok();
        for (twin_t, a, th, op, actual_world) in faults {
            let other = match actual_world {
                World::Committed => World::NotCommitted,
                World::NotCommitted => World::Committed,
            };
            let name = model.actors[a].name.clone();
            let operation = match op {
                InFlightOp::Commit => "COMMIT".to_owned(),
                InFlightOp::Autocommit(table) => {
                    format!("autocommit write on {}", model.schema.table(table).name)
                }
            };
            let possible = possible_here(model, &state, a, th, op);
            let knowledge = OutcomeKnowledge::Ambiguous {
                possible_outcomes: possible,
            }
            .label();
            // Twin schedule: the server's other behavior. Operations the
            // twin sends to the server complete right after the fault (any
            // later completion is another schedule of the same model);
            // completions of operations the twin never delivers are dropped.
            let twin_after = apply(model, &state, twin_t, &mut Recorder::off()).ok();
            let new_of = |after: &Option<crate::engine::Applied>| -> Vec<OrphanKey> {
                after
                    .as_ref()
                    .map(|x| {
                        orphan_keys(&x.state)
                            .into_iter()
                            .filter(|k| !before_orphans.contains(k))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let actual_new = new_of(&actual_after);
            let twin_new = new_of(&twin_after);
            let mut twin_schedule: Vec<Transition> = Vec::new();
            for (j, x) in schedule.iter().enumerate() {
                if j == i {
                    twin_schedule.push(twin_t);
                    for k in twin_new.iter().filter(|k| !actual_new.contains(k)) {
                        twin_schedule.push(Transition {
                            kind: TKind::ServerComplete,
                            actor: k.0,
                            thread: k.1 as u16,
                        });
                    }
                    continue;
                }
                let dropped = j > i
                    && x.kind == TKind::ServerComplete
                    && actual_new
                        .iter()
                        .filter(|k| !twin_new.contains(k))
                        .any(|k| (k.0, k.1 as u16) == (x.actor, x.thread));
                if !dropped {
                    twin_schedule.push(*x);
                }
            }
            let before = observations_after(&actual, i, a as u16);
            let mut entry = Twin {
                transition: i + 1,
                actor: name,
                operation,
                actual: actual_world,
                other,
                knowledge,
                twin: "replayed".into(),
                detail: String::new(),
                actual_verdicts: actual_verdicts.clone(),
                twin_verdicts: Vec::new(),
                observations: before.len(),
                identical_observations: 0,
                first_difference: None,
                reading: String::new(),
            };
            match run(model, &twin_schedule) {
                Err(e) => {
                    entry.twin = "diverges".into();
                    entry.detail = e;
                    entry.identical_observations = before.len();
                }
                Ok(twin) => {
                    entry.twin_verdicts = verdicts(model, &twin.state);
                    let after = observations_after(&twin, i, a as u16);
                    let common = before
                        .iter()
                        .zip(&after)
                        .take_while(|(x, y)| x == y)
                        .count();
                    entry.identical_observations = common;
                    if common < before.len() || common < after.len() {
                        entry.first_difference = Some((
                            before
                                .get(common)
                                .cloned()
                                .unwrap_or_else(|| "(nothing further)".into()),
                            after
                                .get(common)
                                .cloned()
                                .unwrap_or_else(|| "(nothing further)".into()),
                        ));
                    }
                }
            }
            let differ = entry
                .actual_verdicts
                .iter()
                .zip(&entry.twin_verdicts)
                .any(|(x, y)| x.holds != y.holds);
            entry.reading = reading(&entry, differ);
            report.twins.push(entry);
        }
        match actual_after {
            Some(a) => state = a.state,
            None => break,
        }
    }
    report
}

fn possible_here(
    _model: &Model,
    _state: &crate::state::State,
    _a: usize,
    _th: usize,
    _op: InFlightOp,
) -> OutcomeSet {
    // Whether the server received the operation is what the application
    // cannot know: both outcomes are possible at the point of the fault.
    OutcomeSet::BOTH
}

fn reading(t: &Twin, verdicts_differ: bool) -> String {
    if t.twin == "diverges" {
        return format!(
            "the other world is not a valid schedule of the model ({})",
            t.detail
        );
    }
    let never = t.first_difference.is_none();
    let observation = if never {
        "the application's observations are identical in both worlds for the rest of the schedule: it can never learn which one it is in".to_owned()
    } else {
        format!(
            "the application's first observation that tells the worlds apart is its observation number {}",
            t.identical_observations + 1
        )
    };
    let decision = if verdicts_differ {
        "the invariant outcome differs between the worlds: the same decision is safe in one and unsafe in the other"
    } else {
        "the invariants have the same outcome in both worlds"
    };
    format!("{observation}; {decision}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_schedule_without_faults_has_no_twins() {
        let model = crate::scenario::testing::model_from_programs(
            "read-committed",
            &[(
                "A",
                "update products where @id == \"p1\" set @stock = 1\n",
                &[],
            )],
        );
        let (schedules, _) = crate::witness::enumerate_schedules(&model, 10);
        assert!(analyze(&model, &schedules[0]).twins.is_empty());
    }
}

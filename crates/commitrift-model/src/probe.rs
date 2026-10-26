//! Recovery probes (Phase 8).
//!
//! When the model finds NO counterexample (a safe or idempotent
//! alternative), there is no schedule to run against the original code.
//! A recovery probe names the events a schedule must contain (a crash
//! after an external effect, then a retry, …); the shortest COMPLETE
//! schedule of the model within the declared bounds that contains them in
//! order is selected here by breadth-first search. The schedule is the
//! model's — derived from the same transition relation as every
//! counterexample — never written by hand, and its final state is quiescent
//! (no task can move and no guaranteed retry is pending).

use std::collections::{HashSet, VecDeque};

use crate::engine::{Action, Expect, Recorder, apply, enabled, initial_state, is_quiescent};
use crate::model::Model;
use crate::state::{State, TKind, Transition};

/// The events a transition produces, in the probe vocabulary.
fn events(model: &Model, t: Transition, actions: &[Action]) -> Vec<String> {
    let name = |a: u16| model.actors[a as usize].name.clone();
    let mut out = Vec::new();
    match t.kind {
        TKind::Retry => out.push(format!("retry:{}", name(t.actor))),
        TKind::Deadlock => out.push("error:40P01".to_owned()),
        _ => {}
    }
    for a in actions {
        match a {
            Action::Crash { actor } => out.push(format!("crash:{}", name(*actor))),
            Action::External { channel, .. } => out.push(format!("external:{channel}")),
            Action::Commit { actor, expect, .. } => {
                if let Expect::Error(code) = expect {
                    out.push(format!("error:{code}"));
                } else {
                    out.push(format!("commit:{}", name(*actor)));
                }
            }
            Action::Stmt {
                actor,
                resumed,
                expect,
                ..
            } => {
                if !resumed {
                    out.push(format!("statement:{}", name(*actor)));
                }
                if let Expect::Error(code) = expect {
                    out.push(format!("error:{code}"));
                }
                if matches!(expect, Expect::Blocked(_)) {
                    out.push(format!("blocked:{}", name(*actor)));
                }
            }
            _ => {}
        }
    }
    out
}

/// The shortest complete schedule containing `require` in order, or why
/// there is none within the bounds.
pub fn find(model: &Model, require: &[String]) -> Result<Vec<Transition>, String> {
    if require.is_empty() {
        return Err("a recovery probe must require at least one event".to_owned());
    }
    for r in require {
        let known = [
            "external:",
            "crash:",
            "retry:",
            "commit:",
            "error:",
            "statement:",
            "blocked:",
        ];
        if !known.iter().any(|k| r.starts_with(k) && r.len() > k.len()) {
            return Err(format!(
                "unknown probe event `{r}` (external:, crash:, retry:, commit:, error:, statement:, blocked:)"
            ));
        }
    }
    struct Node {
        state: State,
        matched: usize,
        parent: Option<(usize, Transition)>,
    }
    let limit = usize::try_from(model.bounds.max_states).unwrap_or(usize::MAX);
    let mut nodes = vec![Node {
        state: initial_state(model),
        matched: 0,
        parent: None,
    }];
    let mut seen: HashSet<(Vec<u8>, usize)> = HashSet::new();
    seen.insert((nodes[0].state.canonical(model), 0));
    let mut queue = VecDeque::from([0usize]);
    while let Some(n) = queue.pop_front() {
        let en = enabled(model, &nodes[n].state);
        if nodes[n].matched == require.len()
            && is_quiescent(model, &en)
            && nodes[n].state.is_finished()
        {
            let mut path = Vec::new();
            let mut cur = n;
            while let Some((p, t)) = nodes[cur].parent {
                path.push(t);
                cur = p;
            }
            path.reverse();
            return Ok(path);
        }
        if nodes[n].state.steps >= model.bounds.max_steps {
            continue;
        }
        for t in en {
            let mut rec = Recorder::on();
            let Ok(applied) = apply(model, &nodes[n].state, t, &mut rec) else {
                continue;
            };
            let mut matched = nodes[n].matched;
            for e in events(model, t, &rec.actions) {
                if matched < require.len() && e == require[matched] {
                    matched += 1;
                }
            }
            if !seen.insert((applied.state.canonical(model), matched)) {
                continue;
            }
            if nodes.len() >= limit {
                return Err(format!(
                    "no schedule with the required events was found within maxStates {}",
                    model.bounds.max_states
                ));
            }
            nodes.push(Node {
                state: applied.state,
                matched,
                parent: Some((n, t)),
            });
            queue.push_back(nodes.len() - 1);
        }
    }
    Err(format!(
        "no complete schedule within the declared bounds contains {} in order",
        require.join(" → ")
    ))
}

/// A schedule as `kind actor[/thread]` lines (for the report).
pub fn render(model: &Model, schedule: &[Transition]) -> Vec<String> {
    schedule
        .iter()
        .map(|t| match t.kind {
            TKind::Crash => format!("crash process {}", model.process_names[t.thread as usize]),
            TKind::Retry => format!("retry {}", model.actors[t.actor as usize].name),
            k => format!(
                "{} {}/{}",
                format!("{k:?}").to_lowercase(),
                model.actors[t.actor as usize].name,
                t.thread
            ),
        })
        .collect()
}

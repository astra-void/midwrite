//! Effect summaries over a call graph: inertness and parameter use.
//!
//! See `docs/architecture/value-analysis.md`. A function is *inert*
//! when, on every path, it performs no database write, transaction,
//! configured external effect or unknown call, and every call it makes is
//! a modeled builtin, a call of an inert function, or an invocation of a
//! parameter (which makes inertness conditional on the argument passed).
//! Database reads are allowed.
//!
//! The frontend reports, per function, whether its own body already
//! refutes inertness and the calls whose targets it resolved
//! ([`FunctionFacts`]). [`solve`] computes, SCC by SCC in reverse
//! topological order (Tarjan), the *greatest* fixpoint of inertness (every
//! function starts inert and is demoted by a refuting fact; sound for a
//! safety property: an effect must occur at some finite depth, so
//! recursion needs no depth cut) together with the *least* fixpoint of
//! parameter uses (which parameters a function awaits, invokes or lets
//! escape, directly or by passing them on).

use serde::Serialize;

/// What a function does with one of its parameters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamUse {
    /// The value is awaited (`await p`, a promise combinator element,
    /// `p.then(...)`): a lazy builder passed for it executes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub awaited: bool,
    /// The value is called (`p()`): a callback.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub invoked: bool,
    /// The value reaches code that is not analyzed, which may call or await
    /// it at any time.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub escaped: bool,
}

impl ParamUse {
    pub fn join(&mut self, other: ParamUse) -> bool {
        let before = *self;
        self.awaited |= other.awaited;
        self.invoked |= other.invoked;
        self.escaped |= other.escaped;
        before != *self
    }

    /// Whether a function value passed for the parameter may run.
    pub fn may_run(self) -> bool {
        self.invoked || self.escaped
    }
}

/// An argument of a resolved call, as far as summaries care.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgFact {
    /// Anything that is neither a known function nor a parameter.
    Other,
    /// A function of the table (by index).
    Function(usize),
    /// Parameter `index` of the calling function, passed on.
    Param(u32),
}

/// A call whose target the frontend resolved to a function of the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallFact {
    pub callee: usize,
    pub args: Vec<ArgFact>,
    /// The call as written, for explanations.
    pub label: String,
}

/// What the frontend knows about one function's body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FunctionFacts {
    /// Why the body itself is not inert (a recognized write, an unknown
    /// call, ...), if it is not.
    pub refuted: Option<String>,
    pub calls: Vec<CallFact>,
    /// Direct uses of each parameter in the body.
    pub params: Vec<ParamUse>,
}

/// The solved summary of one function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Solved {
    /// `Ok` when inert (given inert arguments for invoked parameters),
    /// otherwise why not.
    pub inert: Result<(), String>,
    pub params: Vec<ParamUse>,
}

impl Solved {
    pub fn is_inert(&self) -> bool {
        self.inert.is_ok()
    }

    pub fn param(&self, index: usize) -> ParamUse {
        self.params.get(index).copied().unwrap_or_default()
    }
}

/// Counters of one [`solve`] run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SolveStats {
    pub functions: usize,
    pub inert: usize,
    pub sccs: usize,
    /// Size of the largest strongly connected component.
    pub largest_scc: usize,
    /// Recursive components (size > 1, or a self-call).
    pub recursive_sccs: usize,
    /// Fixpoint rounds over all components.
    pub iterations: usize,
    /// Components demoted because their iteration budget ran out.
    pub budget_exhausted: usize,
}

const MAX_REASON: usize = 240;

fn shorten(mut text: String) -> String {
    if text.chars().count() > MAX_REASON {
        text = text.chars().take(MAX_REASON - 1).collect();
        text.push('…');
    }
    text
}

/// Solve inertness and parameter uses for every function. `names[i]` names
/// function `i` in explanations.
pub fn solve(facts: &[FunctionFacts], names: &[String]) -> (Vec<Solved>, SolveStats) {
    let n = facts.len();
    let mut stats = SolveStats {
        functions: n,
        ..SolveStats::default()
    };
    let mut solved: Vec<Option<Solved>> = vec![None; n];
    for component in tarjan(facts) {
        stats.sccs += 1;
        stats.largest_scc = stats.largest_scc.max(component.len());
        let recursive = component.len() > 1
            || facts[component[0]]
                .calls
                .iter()
                .any(|call| call.callee == component[0]);
        if recursive {
            stats.recursive_sccs += 1;
        }
        // Optimistic start: inert unless the body refutes it.
        for &index in &component {
            solved[index] = Some(Solved {
                inert: match &facts[index].refuted {
                    Some(reason) => Err(reason.clone()),
                    None => Ok(()),
                },
                params: facts[index].params.clone(),
            });
        }
        let budget = 2 * component.len() + 2;
        let mut rounds = 0;
        loop {
            rounds += 1;
            stats.iterations += 1;
            let mut changed = false;
            for &index in &component {
                let (inert, params) = evaluate(index, facts, &solved, names);
                let current = solved[index].as_mut().expect("initialized");
                if current.inert.is_ok() && inert.is_err() {
                    current.inert = inert;
                    changed = true;
                }
                for (slot, used) in params.into_iter().enumerate() {
                    if slot >= current.params.len() {
                        current.params.resize(slot + 1, ParamUse::default());
                    }
                    changed |= current.params[slot].join(used);
                }
            }
            if !changed {
                break;
            }
            if rounds >= budget {
                stats.budget_exhausted += 1;
                for &index in &component {
                    let current = solved[index].as_mut().expect("initialized");
                    if current.inert.is_ok() {
                        current.inert = Err(
                            "the summary fixpoint of its recursive calls did not converge within its budget"
                                .to_owned(),
                        );
                    }
                }
                break;
            }
        }
    }
    let solved: Vec<Solved> = solved
        .into_iter()
        .map(|s| s.expect("every function is in a component"))
        .collect();
    stats.inert = solved.iter().filter(|s| s.is_inert()).count();
    (solved, stats)
}

/// One evaluation of function `index` against the current summaries.
fn evaluate(
    index: usize,
    facts: &[FunctionFacts],
    solved: &[Option<Solved>],
    names: &[String],
) -> (Result<(), String>, Vec<ParamUse>) {
    let own = &facts[index];
    let mut inert: Result<(), String> = match &own.refuted {
        Some(reason) => Err(reason.clone()),
        None => Ok(()),
    };
    let mut params = own.params.clone();
    let name = |i: usize| names.get(i).map_or("?", String::as_str);
    for call in &own.calls {
        let Some(callee) = solved.get(call.callee).and_then(Option::as_ref) else {
            continue;
        };
        if inert.is_ok()
            && let Err(reason) = &callee.inert
        {
            inert = Err(shorten(format!(
                "calls `{}` (`{}`), which is not inert: {reason}",
                call.label,
                name(call.callee)
            )));
        }
        for (slot, argument) in call.args.iter().enumerate() {
            let used = callee.param(slot);
            match *argument {
                ArgFact::Function(function) if used.may_run() => {
                    if inert.is_ok()
                        && let Some(Err(reason)) = solved
                            .get(function)
                            .and_then(Option::as_ref)
                            .map(|s| &s.inert)
                    {
                        inert = Err(shorten(format!(
                            "passes `{}` to `{}`, which may run it, and it is not inert: {reason}",
                            name(function),
                            call.label
                        )));
                    }
                }
                ArgFact::Param(parameter) => {
                    let parameter = parameter as usize;
                    if parameter >= params.len() {
                        params.resize(parameter + 1, ParamUse::default());
                    }
                    params[parameter].join(used);
                }
                _ => {}
            }
            // A value that is not a function, passed for a parameter the
            // callee invokes: what runs is unknown.
            if inert.is_ok() && matches!(argument, ArgFact::Other) && used.invoked {
                inert = Err(shorten(format!(
                    "`{}` calls its parameter {}, and `{}` passes a value that is not a known function for it",
                    name(call.callee),
                    slot + 1,
                    call.label
                )));
            }
        }
    }
    (inert, params)
}

/// Strongly connected components of the call graph, callees before
/// callers (reverse topological order). Iterative Tarjan: call graphs of
/// real projects are deep.
fn tarjan(facts: &[FunctionFacts]) -> Vec<Vec<usize>> {
    let n = facts.len();
    // Callees and function-valued arguments: both must be solved first.
    let edges: Vec<Vec<usize>> = facts
        .iter()
        .map(|fact| {
            let mut out = Vec::new();
            for call in &fact.calls {
                out.push(call.callee);
                for argument in &call.args {
                    if let ArgFact::Function(function) = argument {
                        out.push(*function);
                    }
                }
            }
            out
        })
        .collect();
    let mut index_of = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut components = Vec::new();
    let mut next = 0;
    for start in 0..n {
        if index_of[start] != usize::MAX {
            continue;
        }
        // (node, position in its call list)
        let mut work: Vec<(usize, usize)> = vec![(start, 0)];
        index_of[start] = next;
        low[start] = next;
        next += 1;
        stack.push(start);
        on_stack[start] = true;
        while let Some(top) = work.last_mut() {
            let (node, position) = *top;
            let calls = &edges[node];
            if position < calls.len() {
                top.1 += 1;
                let callee = calls[position];
                if callee >= n {
                    continue;
                }
                if index_of[callee] == usize::MAX {
                    index_of[callee] = next;
                    low[callee] = next;
                    next += 1;
                    stack.push(callee);
                    on_stack[callee] = true;
                    work.push((callee, 0));
                } else if on_stack[callee] {
                    low[node] = low[node].min(index_of[callee]);
                }
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[node]);
            }
            if low[node] == index_of[node] {
                let mut component = Vec::new();
                loop {
                    let member = stack.pop().expect("tarjan stack");
                    on_stack[member] = false;
                    component.push(member);
                    if member == node {
                        break;
                    }
                }
                component.sort_unstable();
                components.push(component);
            }
        }
    }
    components
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(callee: usize, args: Vec<ArgFact>) -> CallFact {
        CallFact {
            callee,
            args,
            label: format!("f{callee}()"),
        }
    }

    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("f{i}")).collect()
    }

    #[test]
    fn inertness_propagates_through_calls() {
        let facts = vec![
            FunctionFacts {
                calls: vec![call(1, vec![])],
                ..FunctionFacts::default()
            },
            FunctionFacts::default(),
            FunctionFacts {
                calls: vec![call(3, vec![])],
                ..FunctionFacts::default()
            },
            FunctionFacts {
                refuted: Some("publishes".into()),
                ..FunctionFacts::default()
            },
        ];
        let (solved, stats) = solve(&facts, &names(4));
        assert!(solved[0].is_inert());
        assert!(solved[1].is_inert());
        assert!(!solved[2].is_inert());
        assert!(solved[2].inert.as_ref().unwrap_err().contains("publishes"));
        assert_eq!(stats.inert, 2);
    }

    #[test]
    fn recursion_is_a_greatest_fixpoint() {
        // f0 <-> f1, both otherwise empty: inert. f2 <-> f3 with f3 writing.
        let facts = vec![
            FunctionFacts {
                calls: vec![call(1, vec![])],
                ..FunctionFacts::default()
            },
            FunctionFacts {
                calls: vec![call(0, vec![])],
                ..FunctionFacts::default()
            },
            FunctionFacts {
                calls: vec![call(3, vec![])],
                ..FunctionFacts::default()
            },
            FunctionFacts {
                refuted: Some("writes".into()),
                calls: vec![call(2, vec![])],
                ..FunctionFacts::default()
            },
        ];
        let (solved, stats) = solve(&facts, &names(4));
        assert!(solved[0].is_inert() && solved[1].is_inert());
        assert!(!solved[2].is_inert() && !solved[3].is_inert());
        assert_eq!(stats.recursive_sccs, 2);
        assert_eq!(stats.budget_exhausted, 0);
    }

    #[test]
    fn callbacks_make_inertness_conditional_and_param_uses_propagate() {
        // f0(action) invokes action; f1 passes its own param to f0; f2
        // passes an effectful f3 to f0; f4 passes an inert f5.
        let facts = vec![
            FunctionFacts {
                params: vec![ParamUse {
                    invoked: true,
                    ..ParamUse::default()
                }],
                ..FunctionFacts::default()
            },
            FunctionFacts {
                calls: vec![call(0, vec![ArgFact::Param(0)])],
                ..FunctionFacts::default()
            },
            FunctionFacts {
                calls: vec![call(0, vec![ArgFact::Function(3)])],
                ..FunctionFacts::default()
            },
            FunctionFacts {
                refuted: Some("writes".into()),
                ..FunctionFacts::default()
            },
            FunctionFacts {
                calls: vec![call(0, vec![ArgFact::Function(5)])],
                ..FunctionFacts::default()
            },
            FunctionFacts::default(),
        ];
        let (solved, _) = solve(&facts, &names(6));
        assert!(solved[0].is_inert());
        assert!(solved[1].param(0).invoked);
        assert!(!solved[2].is_inert());
        assert!(solved[4].is_inert());
    }

    #[test]
    fn invoking_an_unknown_value_is_not_inert() {
        let facts = vec![
            FunctionFacts {
                params: vec![ParamUse {
                    invoked: true,
                    ..ParamUse::default()
                }],
                ..FunctionFacts::default()
            },
            FunctionFacts {
                calls: vec![call(0, vec![ArgFact::Other])],
                ..FunctionFacts::default()
            },
        ];
        let (solved, _) = solve(&facts, &names(2));
        assert!(!solved[1].is_inert());
    }

    #[test]
    fn components_come_callees_first() {
        let facts = vec![
            FunctionFacts {
                calls: vec![call(1, vec![])],
                ..FunctionFacts::default()
            },
            FunctionFacts {
                calls: vec![call(2, vec![])],
                ..FunctionFacts::default()
            },
            FunctionFacts::default(),
        ];
        let order: Vec<Vec<usize>> = tarjan(&facts);
        assert_eq!(order, vec![vec![2], vec![1], vec![0]]);
    }
}

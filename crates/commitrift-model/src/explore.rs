//! Bounded explicit-state exploration.
//!
//! Three algorithms share one transition system ([`crate::engine`]):
//!
//! * breadth-first search (shortest schedules first, no reduction),
//! * depth-first search without reduction (the reference for differential
//!   tests), and
//! * depth-first search with *sleep sets* (partial-order reduction). Sleep
//!   sets prune transitions whose effect is reproduced by an already
//!   explored commuted ordering. Combined with canonical-state caching they
//!   visit every reachable state of the unreduced search (within the same
//!   bounds), which is what `docs/phase6/PARTIAL-ORDER-REDUCTION.md`
//!   documents and the differential tests check.
//!
//! Exploration order is deterministic: transitions are tried in
//! `(kind, actor, thread)` order.

use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use crate::engine::{Recorder, Unsupported, apply, enabled, initial_state, is_quiescent};
use crate::footprint::{Footprint, dependent};
use crate::invariant::When;
use crate::model::{Model, Strategy};
use crate::state::{State, Transition};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    CounterexampleFound,
    NoCounterexampleWithinBounds,
    Inconclusive,
    Unsupported,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CounterexampleFound => "COUNTEREXAMPLE_FOUND",
            Self::NoCounterexampleWithinBounds => "NO_COUNTEREXAMPLE_WITHIN_BOUNDS",
            Self::Inconclusive => "INCONCLUSIVE",
            Self::Unsupported => "UNSUPPORTED",
        }
    }
}

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pub states: u64,
    pub transitions: u64,
    pub terminal_states: u64,
    pub duplicate_state_hits: u64,
    pub sleep_pruned_transitions: u64,
    pub context_pruned_transitions: u64,
    pub max_depth: u32,
    /// Maximal schedules (root-to-terminal paths) in the explored graph;
    /// saturates at `u64::MAX`.
    pub schedules_in_explored_graph: u64,
    pub visited_bytes: u64,
    pub wall_ms: u64,
    pub first_violation_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Truncation {
    pub max_states: bool,
    pub max_transitions: bool,
    pub max_wall_time: bool,
    pub max_memory: bool,
    /// Some path reached `maxSteps` before finishing.
    pub max_steps: bool,
    /// Schedules needing more context switches than allowed were not
    /// explored (a declared bound, not a resource limit).
    pub max_context_switches: bool,
}

impl Truncation {
    /// A resource limit was hit: coverage is partial.
    pub fn resource_limit(&self) -> bool {
        self.max_states
            || self.max_transitions
            || self.max_wall_time
            || self.max_memory
            || self.max_steps
    }
}

#[derive(Clone, Debug)]
pub struct Counterexample {
    pub invariant: String,
    pub when: When,
    pub schedule: Vec<Transition>,
}

#[derive(Clone, Debug)]
pub struct StuckState {
    pub schedule: Vec<Transition>,
}

pub struct ExploreResult {
    pub verdict: Verdict,
    pub counterexample: Option<Counterexample>,
    pub metrics: Metrics,
    pub truncation: Truncation,
    pub unsupported: Vec<Unsupported>,
    pub stuck: Vec<StuckState>,
    /// Canonical keys of every state visited (only with `collect_states`).
    pub reached: Option<BTreeSet<Vec<u8>>>,
    /// Number of states violating some invariant (only meaningful with
    /// `stop_at_first == false`).
    pub violating_states: u64,
}

#[derive(Clone, Debug)]
pub struct ExploreOptions {
    pub por: bool,
    pub strategy: Strategy,
    pub collect_states: bool,
    pub stop_at_first: bool,
}

impl ExploreOptions {
    pub fn from_model(model: &Model) -> Self {
        Self {
            por: model.options.por,
            strategy: model.options.strategy,
            collect_states: false,
            stop_at_first: true,
        }
    }
}

struct Succ {
    t: Transition,
    foot: Footprint,
    state: Option<State>,
}

struct Frame {
    node: u32,
    succ: Vec<Succ>,
    next: usize,
    chosen: usize,
    sleep: Vec<Transition>,
    /// Transitions explored from this state before this visit.
    prior_done: Vec<Transition>,
    explored: Vec<usize>,
    depth: u32,
}

#[derive(Default)]
struct Node {
    /// Transitions already explored from this state (in earlier visits).
    done: Vec<Transition>,
    /// The smallest depth at which the state was expanded.
    depth: u32,
}

struct Ctx<'a> {
    model: &'a Model,
    started: Instant,
    metrics: Metrics,
    truncation: Truncation,
    unsupported: Vec<Unsupported>,
    stuck: Vec<StuckState>,
    visited: HashMap<Vec<u8>, u32>,
    nodes: Vec<Node>,
    children: Vec<Vec<u32>>,
    terminal: Vec<bool>,
    reached: Option<BTreeSet<Vec<u8>>>,
    counterexample: Option<Counterexample>,
    violating: u64,
}

impl Ctx<'_> {
    fn limits_hit(&mut self) -> bool {
        let b = &self.model.bounds;
        if self.metrics.states >= b.max_states {
            self.truncation.max_states = true;
        }
        if b.max_transitions
            .is_some_and(|m| self.metrics.transitions >= m)
        {
            self.truncation.max_transitions = true;
        }
        if b.max_wall_ms
            .is_some_and(|m| self.started.elapsed().as_millis() as u64 >= m)
        {
            self.truncation.max_wall_time = true;
        }
        if b.max_memory_bytes
            .is_some_and(|m| self.metrics.visited_bytes >= m)
        {
            self.truncation.max_memory = true;
        }
        self.truncation.max_states
            || self.truncation.max_transitions
            || self.truncation.max_wall_time
            || self.truncation.max_memory
    }

    fn intern(&mut self, key: Vec<u8>) -> (u32, bool) {
        if let Some(id) = self.visited.get(&key) {
            return (*id, false);
        }
        let id = self.nodes.len() as u32;
        self.metrics.visited_bytes += key.len() as u64 + 96;
        if let Some(r) = &mut self.reached {
            r.insert(key.clone());
        }
        self.visited.insert(key, id);
        self.nodes.push(Node::default());
        self.children.push(Vec::new());
        self.terminal.push(false);
        self.metrics.states += 1;
        (id, true)
    }

    fn check_invariants(&mut self, s: &State, terminal: bool, path: &dyn Fn() -> Vec<Transition>) {
        for inv in &self.model.invariants {
            let applicable = match inv.when {
                When::Always => true,
                When::Final => terminal,
            };
            if applicable && !inv.holds(self.model, s) {
                self.violating += 1;
                if self.counterexample.is_none() {
                    self.metrics.first_violation_ms =
                        Some(self.started.elapsed().as_millis() as u64);
                    self.counterexample = Some(Counterexample {
                        invariant: inv.name.clone(),
                        when: inv.when,
                        schedule: path(),
                    });
                }
                return;
            }
        }
    }

    fn note_unsupported(&mut self, u: Unsupported) {
        if !self
            .unsupported
            .iter()
            .any(|x| x.detail == u.detail && x.src == u.src && x.actor == u.actor)
        {
            self.unsupported.push(u);
        }
    }

    /// Expand a state into its successors (applying every enabled
    /// transition once).
    fn expand(&mut self, s: &State) -> Vec<Succ> {
        let model = self.model;
        let mut out = Vec::new();
        for t in enabled(model, s) {
            match apply(model, s, t, &mut Recorder::off()) {
                Ok(a) => {
                    if let Some(limit) = model.bounds.max_context_switches
                        && a.state.switches > limit
                    {
                        self.truncation.max_context_switches = true;
                        self.metrics.context_pruned_transitions += 1;
                        continue;
                    }
                    out.push(Succ {
                        t,
                        foot: a.footprint,
                        state: Some(a.state),
                    });
                }
                Err(u) => {
                    self.note_unsupported(u);
                    let mut foot = Footprint::default();
                    foot.push(crate::footprint::Access::Global);
                    out.push(Succ {
                        t,
                        foot,
                        state: None,
                    });
                }
            }
        }
        out
    }

    fn finish(mut self, strategy_ran: bool) -> ExploreResult {
        let _ = strategy_ran;
        self.metrics.wall_ms = self.started.elapsed().as_millis() as u64;
        self.metrics.schedules_in_explored_graph = count_schedules(&self.children, &self.terminal);
        let verdict = if self.counterexample.is_some() {
            Verdict::CounterexampleFound
        } else if !self.unsupported.is_empty() {
            Verdict::Unsupported
        } else if self.truncation.resource_limit() || !self.stuck.is_empty() {
            Verdict::Inconclusive
        } else {
            Verdict::NoCounterexampleWithinBounds
        };
        ExploreResult {
            verdict,
            counterexample: self.counterexample,
            metrics: self.metrics,
            truncation: self.truncation,
            unsupported: self.unsupported,
            stuck: self.stuck,
            reached: self.reached,
            violating_states: self.violating,
        }
    }
}

fn count_schedules(children: &[Vec<u32>], terminal: &[bool]) -> u64 {
    if children.is_empty() {
        return 0;
    }
    let n = children.len();
    let mut memo: Vec<Option<u64>> = vec![None; n];
    // Iterative post-order from node 0.
    let mut stack: Vec<(u32, usize)> = vec![(0, 0)];
    while let Some(&(node, idx)) = stack.last() {
        let kids = &children[node as usize];
        if idx < kids.len() {
            stack.last_mut().expect("non-empty").1 += 1;
            let k = kids[idx];
            if memo[k as usize].is_none() {
                stack.push((k, 0));
            }
        } else {
            let mut total: u64 = 0;
            if terminal[node as usize] || kids.is_empty() {
                total = 1;
            } else {
                for k in kids {
                    total = total.saturating_add(memo[*k as usize].unwrap_or(0));
                }
            }
            memo[node as usize] = Some(total);
            stack.pop();
        }
    }
    memo[0].unwrap_or(0)
}

pub fn explore(model: &Model) -> ExploreResult {
    explore_with(model, &ExploreOptions::from_model(model))
}

pub fn explore_with(model: &Model, opts: &ExploreOptions) -> ExploreResult {
    let mut ctx = Ctx {
        model,
        started: Instant::now(),
        metrics: Metrics::default(),
        truncation: Truncation::default(),
        unsupported: Vec::new(),
        stuck: Vec::new(),
        visited: HashMap::new(),
        nodes: Vec::new(),
        children: Vec::new(),
        terminal: Vec::new(),
        reached: opts.collect_states.then(BTreeSet::new),
        counterexample: None,
        violating: 0,
    };
    // Partial-order reduction is not sound together with a context-switch
    // bound (a commuted ordering can need more switches).
    let por = opts.por && model.bounds.max_context_switches.is_none();
    if por || opts.strategy == Strategy::Dfs {
        dfs(&mut ctx, opts, por);
    } else {
        bfs(&mut ctx, opts);
    }
    ctx.finish(true)
}

fn dfs(ctx: &mut Ctx<'_>, opts: &ExploreOptions, por: bool) {
    let model = ctx.model;
    let init = initial_state(model);
    let (root, _) = ctx.intern(init.canonical(model));
    ctx.check_invariants(&init, false, &Vec::new);
    let mut stack: Vec<Frame> = Vec::new();
    if let Some(f) = open_frame(ctx, root, init, Vec::new(), 0, &Vec::new) {
        stack.push(f);
    }
    let path_of = |stack: &[Frame]| -> Vec<Transition> {
        stack
            .iter()
            .filter_map(|f| f.succ.get(f.chosen).map(|s| s.t))
            .collect()
    };
    while !stack.is_empty() {
        if ctx.limits_hit() || (ctx.counterexample.is_some() && opts.stop_at_first) {
            break;
        }
        let top = stack.len() - 1;
        let frame = &mut stack[top];
        if frame.next >= frame.succ.len() {
            stack.pop();
            continue;
        }
        let i = frame.next;
        frame.next += 1;
        let t = frame.succ[i].t;
        if por && frame.sleep.contains(&t) {
            ctx.metrics.sleep_pruned_transitions += 1;
            continue;
        }
        if frame.prior_done.contains(&t) {
            continue; // explored in an earlier visit of this state
        }
        // Sleep set of the child: transitions that are sleeping, were
        // explored earlier from this state, or were explored before this one
        // in this visit, and that are independent of this transition.
        let child_sleep: Vec<Transition> = if por {
            let mut z: Vec<(Transition, &Footprint)> = Vec::new();
            for u in frame.sleep.iter().chain(frame.prior_done.iter()) {
                if let Some(s) = frame.succ.iter().find(|s| s.t == *u) {
                    z.push((*u, &s.foot));
                }
            }
            for &e in &frame.explored {
                z.push((frame.succ[e].t, &frame.succ[e].foot));
            }
            let mut out: Vec<Transition> = z
                .into_iter()
                .filter(|(_, f)| !dependent(model, f, &frame.succ[i].foot))
                .map(|(u, _)| u)
                .collect();
            out.sort();
            out.dedup();
            out
        } else {
            Vec::new()
        };
        frame.explored.push(i);
        frame.chosen = i;
        let depth = frame.depth + 1;
        let parent = frame.node;
        ctx.nodes[parent as usize].done.push(t);
        let Some(child) = frame.succ[i].state.take() else {
            continue; // unsupported transition: recorded, no successor
        };
        ctx.metrics.transitions += 1;
        ctx.metrics.max_depth = ctx.metrics.max_depth.max(depth);
        let (id, is_new) = ctx.intern(child.canonical(model));
        if !ctx.children[parent as usize].contains(&id) {
            ctx.children[parent as usize].push(id);
        }
        if is_new {
            ctx.check_invariants(&child, false, &|| path_of(&stack));
        } else {
            ctx.metrics.duplicate_state_hits += 1;
        }
        if ctx.counterexample.is_some() && opts.stop_at_first {
            break;
        }
        let path = path_of(&stack);
        if let Some(f) = open_frame(ctx, id, child, child_sleep, depth, &|| path.clone()) {
            stack.push(f);
        }
    }
}

/// Create the frame for a state about to be expanded, or `None` when there
/// is nothing left to explore from it: terminal states (whose final
/// invariants are checked here), states at the depth bound, and states
/// already explored from every transition that is not asleep.
fn open_frame(
    ctx: &mut Ctx<'_>,
    node: u32,
    state: State,
    sleep: Vec<Transition>,
    depth: u32,
    path: &dyn Fn() -> Vec<Transition>,
) -> Option<Frame> {
    let model = ctx.model;
    let en = enabled(model, &state);
    if is_quiescent(model, &en) {
        if !ctx.terminal[node as usize] {
            ctx.terminal[node as usize] = true;
            ctx.metrics.terminal_states += 1;
            if state.is_finished() {
                ctx.check_invariants(&state, true, path);
            } else {
                ctx.stuck.push(StuckState { schedule: path() });
            }
        }
        if en.is_empty() {
            return None;
        }
    }
    if state.steps >= model.bounds.max_steps {
        ctx.truncation.max_steps = true;
        return None;
    }
    // A shallower visit may reach transitions an earlier, deeper visit cut
    // at the step bound: start over.
    let n = &mut ctx.nodes[node as usize];
    if depth < n.depth || (n.done.is_empty() && n.depth == 0) {
        if depth < n.depth {
            n.done.clear();
        }
        n.depth = depth;
    }
    let prior_done = n.done.clone();
    if en
        .iter()
        .all(|t| sleep.contains(t) || prior_done.contains(t))
    {
        // Everything not asleep was explored before; sleeping transitions
        // are counted as pruned only once.
        return None;
    }
    let succ = ctx.expand(&state);
    Some(Frame {
        node,
        succ,
        next: 0,
        chosen: 0,
        sleep,
        prior_done,
        explored: Vec::new(),
        depth,
    })
}

fn bfs(ctx: &mut Ctx<'_>, opts: &ExploreOptions) {
    let model = ctx.model;
    let init = initial_state(model);
    let (root, _) = ctx.intern(init.canonical(model));
    ctx.check_invariants(&init, false, &Vec::new);
    // parents[node] = (parent node, transition) of the first discovery.
    let mut parents: Vec<Option<(u32, Transition)>> = vec![None];
    let mut queue: std::collections::VecDeque<(u32, State, u32)> =
        std::collections::VecDeque::new();
    queue.push_back((root, init, 0));
    let path_to = |parents: &[Option<(u32, Transition)>], mut node: u32| -> Vec<Transition> {
        let mut out = Vec::new();
        while let Some(Some((p, t))) = parents.get(node as usize) {
            out.push(*t);
            node = *p;
        }
        out.reverse();
        out
    };
    while let Some((node, state, depth)) = queue.pop_front() {
        if ctx.limits_hit() {
            break;
        }
        let en = enabled(model, &state);
        if is_quiescent(model, &en) {
            ctx.terminal[node as usize] = true;
            ctx.metrics.terminal_states += 1;
            if state.is_finished() {
                ctx.check_invariants(&state, true, &|| path_to(&parents, node));
                if ctx.counterexample.is_some() && opts.stop_at_first {
                    return;
                }
            } else {
                ctx.stuck.push(StuckState {
                    schedule: path_to(&parents, node),
                });
            }
            if en.is_empty() {
                continue;
            }
        }
        if state.steps >= model.bounds.max_steps {
            ctx.truncation.max_steps = true;
            continue;
        }
        let succs = ctx.expand(&state);
        for mut s in succs {
            let Some(child) = s.state.take() else {
                continue;
            };
            ctx.metrics.transitions += 1;
            ctx.metrics.max_depth = ctx.metrics.max_depth.max(depth + 1);
            let (id, is_new) = ctx.intern(child.canonical(model));
            if !ctx.children[node as usize].contains(&id) {
                ctx.children[node as usize].push(id);
            }
            if is_new {
                parents.push(Some((node, s.t)));
                ctx.check_invariants(&child, false, &|| path_to(&parents, id));
                if ctx.counterexample.is_some() && opts.stop_at_first {
                    return;
                }
                queue.push_back((id, child, depth + 1));
            } else {
                ctx.metrics.duplicate_state_hits += 1;
            }
        }
    }
}

/// A pair of transitions that the independence relation judged independent
/// but that do not commute (or one disables the other).
#[derive(Clone, Debug)]
pub struct IndependenceViolation {
    pub schedule: Vec<Transition>,
    pub a: Transition,
    pub b: Transition,
    pub reason: &'static str,
}

/// Audit the independence relation over every reachable state: for each
/// pair of transitions enabled in a state and judged independent, both
/// orders must reach the same canonical state, and neither may disable the
/// other. This is the property the sleep-set reduction relies on.
pub fn audit_independence(model: &Model, max_states: usize) -> Vec<IndependenceViolation> {
    let mut out = Vec::new();
    let init = initial_state(model);
    let mut seen: HashMap<Vec<u8>, ()> = HashMap::new();
    seen.insert(init.canonical(model), ());
    let mut stack: Vec<(State, Vec<Transition>)> = vec![(init, Vec::new())];
    while let Some((s, path)) = stack.pop() {
        if seen.len() > max_states || out.len() >= 5 {
            break;
        }
        let en = enabled(model, &s);
        let mut applied = Vec::new();
        for t in &en {
            if let Ok(a) = apply(model, &s, *t, &mut Recorder::off()) {
                applied.push((*t, a));
            }
        }
        for i in 0..applied.len() {
            for j in (i + 1)..applied.len() {
                let (ta, aa) = &applied[i];
                let (tb, ab) = &applied[j];
                if dependent(model, &aa.footprint, &ab.footprint) {
                    continue;
                }
                let mut report = |reason: &'static str| {
                    out.push(IndependenceViolation {
                        schedule: path.clone(),
                        a: *ta,
                        b: *tb,
                        reason,
                    });
                };
                // a then b
                let after_a = enabled(model, &aa.state);
                let after_b = enabled(model, &ab.state);
                if !after_a.contains(tb) {
                    report("the first transition disables the second");
                    continue;
                }
                if !after_b.contains(ta) {
                    report("the second transition disables the first");
                    continue;
                }
                let ab_state = apply(model, &aa.state, *tb, &mut Recorder::off());
                let ba_state = apply(model, &ab.state, *ta, &mut Recorder::off());
                match (ab_state, ba_state) {
                    (Ok(x), Ok(y)) => {
                        if x.state.canonical(model) != y.state.canonical(model) {
                            report("the two orders reach different states");
                        }
                    }
                    _ => report("one order reaches unsupported behavior"),
                }
            }
        }
        for (t, a) in applied {
            let key = a.state.canonical(model);
            if seen.insert(key, ()).is_none() && a.state.steps < model.bounds.max_steps {
                let mut p = path.clone();
                p.push(t);
                stack.push((a.state, p));
            }
        }
    }
    out
}

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

mod sdpor;
pub use sdpor::{
    DEFECT_ENABLER_ORDERS, DEFECT_LAST_RACE_ONLY, DEFECT_SLEEPING_CHOICE, SdporConfig,
    explore_sdpor,
};

use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use crate::engine::{Recorder, Unsupported, apply, enabled, initial_state, is_quiescent};
use crate::footprint::{Footprint, dependent};
use crate::invariant::When;
use crate::model::{Model, Strategy};
use crate::state::{State, TKind, Transition};

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

/// Where the memory of the visited-state structures goes (Phase 9).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageProfile {
    pub stored_states: u64,
    /// Distinct interned fragments, and fragment references held by the keys.
    pub fragments: u64,
    pub fragment_references: u64,
    pub key_arena_bytes: u64,
    pub table_bytes: u64,
    pub interner_bytes: u64,
    pub edge_bytes: u64,
    pub node_bytes: u64,
    pub bytes_per_state: f64,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pub storage: Option<StorageProfile>,
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
    /// DPOR only: reversible races found / backtrack points added.
    pub dpor_races: u64,
    pub dpor_backtracks: u64,
    /// DPOR only: deepest stack of frames held at once (the search keeps no
    /// visited-state table).
    pub dpor_max_frames: u64,
    /// Source-set DPOR only (Phase 10): summaries checked against the trace
    /// on a state-cache hit, distinct summaries and event descriptors held,
    /// and the memory of both.
    #[serde(skip_serializing_if = "is_zero")]
    pub dpor_summary_checks: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub dpor_summaries: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub dpor_descriptors: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub dpor_summary_bytes: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
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

/// Why nothing can proceed in a stuck state (Phase 9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StuckKind {
    /// Tasks wait for a connection of a bounded pool that no running task
    /// will return.
    PoolStarvation,
    /// A statement waits for a lock held by a transaction whose owner waits
    /// for that very statement (an application-level wait cycle; PostgreSQL
    /// sees one waiter and detects no deadlock).
    ApplicationWaitCycle,
    /// Another reason (not classified).
    Unexplained,
}

impl StuckKind {
    pub fn describe(self) -> &'static str {
        match self {
            Self::PoolStarvation => {
                "pool starvation: tasks wait for a connection no running task will return"
            }
            Self::ApplicationWaitCycle => {
                "application-level wait cycle: a statement waits for a lock whose holder waits for it (PostgreSQL detects no deadlock)"
            }
            Self::Unexplained => "stuck for a reason the model does not classify",
        }
    }
}

#[derive(Clone, Debug)]
pub struct StuckState {
    pub schedule: Vec<Transition>,
    pub kind: StuckKind,
}

/// Classify a stuck state.
pub fn classify_stuck(model: &Model, s: &State) -> StuckKind {
    let _ = model;
    let waiting_for_connection = s
        .actors
        .iter()
        .flat_map(|a| a.threads.iter())
        .any(|t| t.lease == crate::state::Lease::Waiting);
    if waiting_for_connection {
        return StuckKind::PoolStarvation;
    }
    let lock_wait = s.actors.iter().flat_map(|a| a.threads.iter()).any(|t| {
        t.pending
            .as_ref()
            .is_some_and(|p| p.blocked_on.is_some_and(|b| s.txn_active(b)))
    });
    if lock_wait {
        return StuckKind::ApplicationWaitCycle;
    }
    StuckKind::Unexplained
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
    /// Canonical keys of every terminal state (quiescent, finished) and of
    /// every stuck state (only with `collect_states`).
    pub terminals: Option<BTreeSet<Vec<u8>>>,
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

/// The explored graph's edges: per state a linked list of distinct children
/// (8 bytes per edge, 4 per state).
#[derive(Default)]
struct Edges {
    head: Vec<u32>,
    /// (child, next edge of the same parent), `u32::MAX` ends a list.
    edges: Vec<(u32, u32)>,
}

impl Edges {
    fn push_node(&mut self) {
        self.head.push(u32::MAX);
    }

    /// Add `parent → child` unless it is there.
    fn add(&mut self, parent: u32, child: u32) {
        let mut e = self.head[parent as usize];
        while e != u32::MAX {
            if self.edges[e as usize].0 == child {
                return;
            }
            e = self.edges[e as usize].1;
        }
        let at = self.edges.len() as u32;
        self.edges.push((child, self.head[parent as usize]));
        self.head[parent as usize] = at;
    }

    fn children(&self, parent: u32) -> impl Iterator<Item = u32> + '_ {
        let mut e = self.head[parent as usize];
        std::iter::from_fn(move || {
            if e == u32::MAX {
                return None;
            }
            let (child, next) = self.edges[e as usize];
            e = next;
            Some(child)
        })
    }

    fn len(&self) -> usize {
        self.head.len()
    }

    fn memory_bytes(&self) -> u64 {
        (self.head.capacity() * 4 + self.edges.capacity() * 8) as u64
    }
}

struct Ctx<'a> {
    model: &'a Model,
    started: Instant,
    metrics: Metrics,
    truncation: Truncation,
    unsupported: Vec<Unsupported>,
    stuck: Vec<StuckState>,
    /// Visited states: compact keys of interned fragments (Phase 9).
    table: crate::keytable::KeyTable,
    interner: crate::state::Interner,
    key_buf: Vec<u32>,
    /// Fragment references held by the stored keys.
    key_refs: u64,
    nodes: Vec<Node>,
    children: Edges,
    terminal: Vec<bool>,
    reached: Option<BTreeSet<Vec<u8>>>,
    terminals: Option<BTreeSet<Vec<u8>>>,
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

    fn intern(&mut self, state: &State) -> (u32, bool) {
        let model = self.model;
        state.canonical_key(model, &mut self.interner, &mut self.key_buf);
        let (id, new) = self.table.intern(&self.key_buf);
        if !new {
            return (id, false);
        }
        self.key_refs += self.key_buf.len() as u64;
        if let Some(r) = &mut self.reached {
            r.insert(state.canonical(model));
        }
        self.nodes.push(Node::default());
        self.children.push_node();
        self.terminal.push(false);
        self.metrics.states += 1;
        self.metrics.visited_bytes = self.table.memory_bytes()
            + self.interner.memory_bytes()
            + self.children.memory_bytes()
            + (self.nodes.capacity() * std::mem::size_of::<Node>() + self.terminal.capacity())
                as u64;
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

    /// A quiescent state in which tasks still run: the implicit
    /// `termination` invariant (when the model has one) is checked.
    fn check_stuck(&mut self, s: &State, path: &dyn Fn() -> Vec<Transition>) {
        for inv in &self.model.invariants {
            if inv.name == crate::model::TERMINATION && !inv.holds(self.model, s) {
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
        if !self.table.is_empty() {
            let n = self.table.len() as u64;
            let nodes = (self.nodes.capacity() * std::mem::size_of::<Node>()
                + self.terminal.capacity()) as u64;
            self.metrics.storage = Some(StorageProfile {
                stored_states: n,
                fragments: self.interner.len() as u64,
                fragment_references: self.key_refs,
                key_arena_bytes: self.table.arena_len() as u64,
                table_bytes: self.table.memory_bytes(),
                interner_bytes: self.interner.memory_bytes(),
                edge_bytes: self.children.memory_bytes(),
                node_bytes: nodes,
                bytes_per_state: self.metrics.visited_bytes as f64 / n as f64,
            });
        }
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
            terminals: self.terminals,
            violating_states: self.violating,
        }
    }
}

fn count_schedules(children: &Edges, terminal: &[bool]) -> u64 {
    if children.len() == 0 {
        return 0;
    }
    let n = children.len();
    let mut memo: Vec<Option<u64>> = vec![None; n];
    // Iterative post-order from node 0.
    let mut stack: Vec<(u32, usize)> = vec![(0, 0)];
    while let Some(&(node, idx)) = stack.last() {
        let kids: Vec<u32> = children.children(node).collect();
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
                for k in &kids {
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
        table: crate::keytable::KeyTable::new(),
        interner: crate::state::Interner::default(),
        key_buf: Vec::new(),
        key_refs: 0,
        nodes: Vec::new(),
        children: Edges::default(),
        terminal: Vec::new(),
        reached: opts.collect_states.then(BTreeSet::new),
        terminals: opts.collect_states.then(BTreeSet::new),
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
    let (root, _) = ctx.intern(&init);
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
        let (id, is_new) = ctx.intern(&child);
        ctx.children.add(parent, id);
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
            if let Some(t) = &mut ctx.terminals {
                t.insert(state.canonical_observable(model));
            }
            if state.is_finished() {
                ctx.check_invariants(&state, true, path);
            } else {
                ctx.check_stuck(&state, path);
                ctx.stuck.push(StuckState {
                    schedule: path(),
                    kind: classify_stuck(model, &state),
                });
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
    let (root, _) = ctx.intern(&init);
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
            if let Some(t) = &mut ctx.terminals {
                t.insert(state.canonical_observable(model));
            }
            if state.is_finished() {
                ctx.check_invariants(&state, true, &|| path_to(&parents, node));
                if ctx.counterexample.is_some() && opts.stop_at_first {
                    return;
                }
            } else {
                ctx.check_stuck(&state, &|| path_to(&parents, node));
                ctx.stuck.push(StuckState {
                    schedule: path_to(&parents, node),
                    kind: classify_stuck(model, &state),
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
            let (id, is_new) = ctx.intern(&child);
            ctx.children.add(node, id);
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
                        if x.state.canonical_observable(model)
                            != y.state.canonical_observable(model)
                        {
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

// ---------------------------------------------------------------------------
// Dynamic partial-order reduction (Phase 9)
// ---------------------------------------------------------------------------

/// Why DPOR cannot be used for `model`, if it cannot: the search explores one
/// interleaving per Mazurkiewicz trace, which preserves what is a function of
/// the final state of a trace (final invariants, deadlocks and stuck states,
/// unsupported behavior that is enabled) but not the intermediate states of
/// the other linearizations of the trace.
pub fn dpor_inapplicable(model: &Model) -> Option<String> {
    if model.invariants.iter().any(|i| i.when == When::Always) {
        return Some(
            "an `always` invariant is checked in every state, and DPOR does not visit the intermediate states of the interleavings it skips".to_owned(),
        );
    }
    if model.bounds.max_context_switches.is_some() {
        return Some(
            "maxContextSwitches is set (a commuted ordering can need more context switches)"
                .to_owned(),
        );
    }
    None
}

/// A bit set over the events of the current trace.
#[derive(Clone, Default)]
struct Bits(Vec<u64>);

impl Bits {
    fn set(&mut self, i: usize) {
        let w = i / 64;
        if self.0.len() <= w {
            self.0.resize(w + 1, 0);
        }
        self.0[w] |= 1 << (i % 64);
    }
    fn get(&self, i: usize) -> bool {
        self.0.get(i / 64).is_some_and(|w| w & (1 << (i % 64)) != 0)
    }
    fn or(&mut self, other: &Bits) {
        if self.0.len() < other.0.len() {
            self.0.resize(other.0.len(), 0);
        }
        for (a, b) in self.0.iter_mut().zip(&other.0) {
            *a |= *b;
        }
    }
}

/// The task whose program order a transition belongs to: the transitions of
/// one task are sequenced by its program, so two of them are never a
/// reversible race. Different tasks of one actor are concurrent.
fn program_actor(t: &Transition) -> Option<(u16, u16)> {
    match t.kind {
        TKind::Step
        | TKind::Resume
        | TKind::Deadlock
        | TKind::LostCommitted
        | TKind::LostNotCommitted => Some((t.actor, t.thread)),
        TKind::Crash
        | TKind::Retry
        | TKind::CrashInFlight
        | TKind::ServerComplete
        | TKind::Replicate => None,
    }
}

/// Transitions of one choice: the server's alternative behaviors for the
/// same operation of a task (it answers, or the answer is lost and the
/// operation reaches the server or not), or the outcomes of the operations a
/// crashing process has in flight. They are alternatives of one process, not
/// concurrent processes, so exploring one of them means exploring all.
fn choice_class(t: &Transition) -> Option<(u8, u16, u16)> {
    match t.kind {
        TKind::Step | TKind::LostCommitted | TKind::LostNotCommitted => {
            Some((0, t.actor, t.thread))
        }
        TKind::CrashInFlight => Some((1, 0, t.thread)),
        _ => None,
    }
}

struct TraceEvent {
    t: Transition,
    /// The task (actor, thread) whose program order the event belongs to.
    program: Option<(u16, u16)>,
    foot: Footprint,
    /// Events that happen before this one (transitive closure of the
    /// dependence relation along the trace).
    before: Bits,
}

struct DFrame {
    /// Every enabled transition applied once: footprint and successor.
    succ: Vec<Succ>,
    /// Transitions that must still be explored from this state.
    backtrack: BTreeSet<Transition>,
    done: BTreeSet<Transition>,
    fresh: bool,
}

/// Depth-first search with dynamic partial-order reduction
/// (Flanagan and Godefroid, POPL 2005), over transitions instead of
/// processes, with *no* state cache and *no* sleep sets:
///
/// * after reaching a state, every enabled transition `u` is compared with
///   the executed trace: the last event `e_i` that is dependent with `u` and
///   not already ordered before it through another dependent event is a
///   reversible race; the transition `u` (or, when `u` was not enabled before
///   `e_i`, every transition enabled there) is added to the backtrack set of
///   the state before `e_i`;
/// * each state explores only its backtrack set, seeded with its first
///   enabled transition.
///
/// The dependence relation is the footprint relation of the sleep-set
/// explorer (and audited by `audit_independence`); crashes, retries, server
/// completions and unknown resources are dependent with everything. Memory
/// is the stack: nothing is stored per distinct state.
///
/// Not used when [`dpor_inapplicable`] says so; the caller falls back to the
/// sleep-set search and reports why.
pub fn explore_dpor(model: &Model, opts: &ExploreOptions) -> ExploreResult {
    dpor_impl(model, opts, None)
}

/// What the classic DPOR search spends its transitions on (Phase 10
/// profiling): collected only by [`explore_dpor_profiled`], which keeps a
/// side table of every state and trace signature it meets (so it needs more
/// memory than [`explore_dpor`]).
#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DporProfile {
    /// Distinct canonical states among the nodes the search visited.
    pub distinct_states: u64,
    /// Frames opened at a state some earlier frame already opened.
    pub revisited_frames: u64,
    /// Transitions executed while at least one frame on the stack is a
    /// revisit of an earlier state: work a visited-state table would skip.
    pub transitions_below_revisit: u64,
    /// Terminal (quiescent) nodes, and the distinct Mazurkiewicz traces
    /// among them (identical happens-before graphs over identical events).
    pub terminal_nodes: u64,
    pub distinct_traces: u64,
    /// Distinct canonical terminal states.
    pub distinct_terminal_states: u64,
    /// Backtrack points added, by origin.
    pub adds_precise: u64,
    pub adds_fallback_all: u64,
    pub adds_choice_siblings: u64,
    pub adds_disabled_alternatives: u64,
    /// Races whose backtrack point was already present.
    pub redundant_race_requests: u64,
    /// Race requests that fell back to "every enabled transition".
    pub fallback_requests: u64,
}

pub fn explore_dpor_profiled(model: &Model, opts: &ExploreOptions) -> (ExploreResult, DporProfile) {
    let mut prof = DporProfile::default();
    let r = dpor_impl(model, opts, Some(&mut prof));
    (r, prof)
}

struct ProfileTables {
    states: crate::keytable::KeyTable,
    terminals: crate::keytable::KeyTable,
    interner: crate::state::Interner,
    key_buf: Vec<u32>,
    traces: std::collections::HashSet<u64>,
    /// Number of open frames that are revisits.
    revisit_depth: Vec<bool>,
    below_revisit: u32,
}

impl ProfileTables {
    fn new() -> Self {
        Self {
            states: crate::keytable::KeyTable::new(),
            terminals: crate::keytable::KeyTable::new(),
            interner: crate::state::Interner::default(),
            key_buf: Vec::new(),
            traces: std::collections::HashSet::new(),
            revisit_depth: Vec::new(),
            below_revisit: 0,
        }
    }
}

fn dpor_impl(
    model: &Model,
    opts: &ExploreOptions,
    mut prof: Option<&mut DporProfile>,
) -> ExploreResult {
    let mut pt = prof.is_some().then(ProfileTables::new);
    let mut ctx = Ctx {
        model,
        started: Instant::now(),
        metrics: Metrics::default(),
        truncation: Truncation::default(),
        unsupported: Vec::new(),
        stuck: Vec::new(),
        table: crate::keytable::KeyTable::new(),
        interner: crate::state::Interner::default(),
        key_buf: Vec::new(),
        key_refs: 0,
        nodes: Vec::new(),
        children: Edges::default(),
        terminal: Vec::new(),
        reached: None,
        terminals: opts.collect_states.then(BTreeSet::new),
        counterexample: None,
        violating: 0,
    };
    let init = initial_state(model);
    let mut stack: Vec<DFrame> = Vec::new();
    let mut trace: Vec<TraceEvent> = Vec::new();
    let mut path: Vec<Transition> = Vec::new();
    let init_enabled = enabled(model, &init);
    if let Some(f) = dpor_open(&mut ctx, &init, &init_enabled, &path) {
        stack.push(f);
    }
    ctx.metrics.dpor_max_frames = ctx.metrics.dpor_max_frames.max(stack.len() as u64);
    if let Some(pt) = &mut pt {
        pt.revisit_depth.push(false);
        profile_state(pt, model, &init);
    }
    while !stack.is_empty() {
        if ctx.limits_hit() || (ctx.counterexample.is_some() && opts.stop_at_first) {
            break;
        }
        let depth = stack.len() - 1;
        if stack[depth].fresh {
            stack[depth].fresh = false;
            dpor_races(&mut ctx, &mut stack, &trace, prof.as_deref_mut());
            let f = &mut stack[depth];
            if f.backtrack.is_empty()
                && let Some(first) = f.succ.first()
            {
                f.backtrack.insert(first.t);
            }
        }
        let f = &mut stack[depth];
        // Alternatives of one choice are explored together.
        let classes: Vec<(u8, u16, u16)> = f.backtrack.iter().filter_map(choice_class).collect();
        for s in &f.succ {
            if choice_class(&s.t).is_some_and(|c| classes.contains(&c))
                && !f.done.contains(&s.t)
                && f.backtrack.insert(s.t)
            {
                ctx.metrics.dpor_backtracks += 1;
                if let Some(p) = prof.as_deref_mut() {
                    p.adds_choice_siblings += 1;
                }
            }
        }
        let Some(t) = f.backtrack.iter().find(|t| !f.done.contains(t)).copied() else {
            stack.pop();
            if let Some(pt) = &mut pt
                && pt.revisit_depth.pop() == Some(true)
            {
                pt.below_revisit -= 1;
            }
            if depth > 0 {
                trace.pop();
                path.pop();
            }
            continue;
        };
        f.done.insert(t);
        let Some(i) = f.succ.iter().position(|s| s.t == t) else {
            continue;
        };
        let foot = f.succ[i].foot.clone();
        let Some(child) = f.succ[i].state.take() else {
            continue; // unsupported transition: recorded, no successor
        };
        ctx.metrics.transitions += 1;
        ctx.metrics.max_depth = ctx.metrics.max_depth.max(depth as u32 + 1);
        if let (Some(pt), Some(p)) = (&pt, prof.as_deref_mut())
            && pt.below_revisit > 0
        {
            p.transitions_below_revisit += 1;
        }
        // Happens-before of the new event.
        let mut before = Bits::default();
        for (k, e) in trace.iter().enumerate() {
            if dependent(model, &e.foot, &foot) {
                before.set(k);
                before.or(&e.before);
            }
        }
        trace.push(TraceEvent {
            t,
            program: program_actor(&t),
            foot,
            before,
        });
        path.push(t);
        let child_enabled = enabled(model, &child);
        // A transition that `t` disables is lost for good from the later
        // states of this branch, so no later race can ask for it: it is an
        // alternative at this state (the disabling pairs are dependent).
        let frame = &mut stack[depth];
        let disabled: Vec<Transition> = frame
            .succ
            .iter()
            .map(|s| s.t)
            .filter(|u| *u != t && !child_enabled.contains(u))
            .collect();
        for u in disabled {
            if !frame.done.contains(&u) && frame.backtrack.insert(u) {
                ctx.metrics.dpor_backtracks += 1;
                if let Some(p) = prof.as_deref_mut() {
                    p.adds_disabled_alternatives += 1;
                }
            }
        }
        let revisit = match &mut pt {
            Some(pt) => profile_state(pt, model, &child),
            None => false,
        };
        if revisit && let Some(p) = prof.as_deref_mut() {
            p.revisited_frames += 1;
        }
        if let (Some(pt), Some(p)) = (&mut pt, prof.as_deref_mut())
            && is_quiescent(model, &child_enabled)
        {
            profile_terminal(pt, p, model, &child, &trace);
        }
        match dpor_open(&mut ctx, &child, &child_enabled, &path) {
            Some(frame) => {
                stack.push(frame);
                ctx.metrics.dpor_max_frames = ctx.metrics.dpor_max_frames.max(stack.len() as u64);
                if let Some(pt) = &mut pt {
                    pt.revisit_depth.push(revisit);
                    if revisit {
                        pt.below_revisit += 1;
                    }
                }
            }
            None => {
                trace.pop();
                path.pop();
            }
        }
    }
    if let (Some(pt), Some(p)) = (&pt, prof) {
        p.distinct_states = pt.states.len() as u64;
        p.distinct_terminal_states = pt.terminals.len() as u64;
        p.distinct_traces = pt.traces.len() as u64;
    }
    ctx.finish(true)
}

/// Record `state` in the profile tables; `true` when it was seen before.
fn profile_state(pt: &mut ProfileTables, model: &Model, state: &State) -> bool {
    state.canonical_key(model, &mut pt.interner, &mut pt.key_buf);
    let (_, new) = pt.states.intern(&pt.key_buf);
    !new
}

/// A terminal node: count it, its canonical state and its trace signature
/// (the happens-before closure of every event, labelled by transition and
/// occurrence, combined commutatively).
fn profile_terminal(
    pt: &mut ProfileTables,
    p: &mut DporProfile,
    model: &Model,
    state: &State,
    trace: &[TraceEvent],
) {
    use std::hash::{Hash, Hasher};
    p.terminal_nodes += 1;
    state.canonical_key(model, &mut pt.interner, &mut pt.key_buf);
    pt.terminals.intern(&pt.key_buf);
    let mut occurrence: HashMap<Transition, u32> = HashMap::new();
    let ids: Vec<(Transition, u32)> = trace
        .iter()
        .map(|e| {
            let o = occurrence.entry(e.t).or_insert(0);
            let id = (e.t, *o);
            *o += 1;
            id
        })
        .collect();
    let mut sig: u64 = 0;
    for (k, e) in trace.iter().enumerate() {
        let mut preds: Vec<(Transition, u32)> = (0..k)
            .filter(|&j| e.before.get(j))
            .map(|j| ids[j])
            .collect();
        preds.sort();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        ids[k].hash(&mut h);
        preds.hash(&mut h);
        sig = sig.wrapping_add(h.finish());
    }
    pt.traces.insert(sig);
}

/// Create the frame for `state`, or `None` for a leaf. Terminal states have
/// their final invariants checked here.
fn dpor_open(
    ctx: &mut Ctx<'_>,
    state: &State,
    en: &[Transition],
    path: &[Transition],
) -> Option<DFrame> {
    let model = ctx.model;
    ctx.metrics.states += 1;
    if is_quiescent(model, en) {
        ctx.metrics.terminal_states += 1;
        if let Some(t) = &mut ctx.terminals {
            t.insert(state.canonical_observable(model));
        }
        if state.is_finished() {
            ctx.check_invariants(state, true, &|| path.to_vec());
        } else {
            ctx.check_stuck(state, &|| path.to_vec());
            ctx.stuck.push(StuckState {
                schedule: path.to_vec(),
                kind: classify_stuck(model, state),
            });
        }
        if en.is_empty() {
            return None;
        }
    }
    if state.steps >= model.bounds.max_steps {
        ctx.truncation.max_steps = true;
        return None;
    }
    let mut succ = ctx.expand(state);
    if succ.is_empty() {
        return None;
    }
    // A statement that starts waiting for a lock has not done its work yet:
    // the footprint of the step does not show what the completed statement
    // would conflict with in another order. Treat it as dependent with
    // everything (it is completed by a separate `Resume`).
    for s in &mut succ {
        let blocked = s.state.as_ref().is_some_and(|c| {
            c.actors
                .get(s.t.actor as usize)
                .and_then(|a| a.threads.get(s.t.thread as usize))
                .is_some_and(|th| th.pending.as_ref().is_some_and(|p| p.blocked_on.is_some()))
        });
        if blocked && s.t.kind == TKind::Step {
            s.foot.push(crate::footprint::Access::Global);
        }
    }
    Some(DFrame {
        succ,
        backtrack: BTreeSet::new(),
        done: BTreeSet::new(),
        fresh: true,
    })
}

/// Race detection at the top frame: for every enabled transition, find the
/// last reversible race with the trace and add backtrack points.
fn dpor_races(
    ctx: &mut Ctx<'_>,
    stack: &mut [DFrame],
    trace: &[TraceEvent],
    mut prof: Option<&mut DporProfile>,
) {
    let model = ctx.model;
    let top = stack.len() - 1;
    debug_assert_eq!(trace.len(), top);
    let mut adds: Vec<(usize, Option<Transition>)> = Vec::new();
    for u in &stack[top].succ {
        // Events dependent with u.
        let deps: Vec<usize> = (0..trace.len())
            .filter(|&k| dependent(model, &trace[k].foot, &u.foot))
            .collect();
        if deps.is_empty() {
            continue;
        }
        let mut ordered = Bits::default();
        let program = program_actor(&u.t);
        for &k in &deps {
            ordered.or(&trace[k].before);
            // The earlier steps of the same actor precede `u` by program
            // order: they are not races.
            if program.is_some() && trace[k].program == program {
                ordered.set(k);
            }
        }
        // Every dependent event not ordered before another dependent one is
        // a race. The Phase 9 search handled only the last one, which loses
        // terminal states when a transition is enabled conditionally (found
        // in Phase 10 on a shared process with one crash and no pool: `Crash`
        // is enabled only once a task has started, so the earlier race is the
        // one whose reversal runs the other task first).
        for &i in deps.iter().filter(|&&k| !ordered.get(k)) {
            ctx.metrics.dpor_races += 1;
            // Frame `i` holds the state before event `i`.
            if stack[i].succ.iter().any(|s| s.t == u.t) {
                adds.push((i, Some(u.t)));
            } else {
                adds.push((i, None));
            }
        }
    }
    for (i, t) in adds {
        let frame = &mut stack[i];
        let candidates: Vec<Transition> = match t {
            Some(t) => vec![t],
            None => frame.succ.iter().map(|s| s.t).collect(),
        };
        let fallback = t.is_none();
        let mut any_new = false;
        for c in candidates {
            if !frame.done.contains(&c) && frame.backtrack.insert(c) {
                ctx.metrics.dpor_backtracks += 1;
                any_new = true;
                if let Some(p) = prof.as_deref_mut() {
                    if fallback {
                        p.adds_fallback_all += 1;
                    } else {
                        p.adds_precise += 1;
                    }
                }
            }
        }
        if let Some(p) = prof.as_deref_mut() {
            if fallback {
                p.fallback_requests += 1;
            }
            if !any_new {
                p.redundant_race_requests += 1;
            }
        }
    }
}

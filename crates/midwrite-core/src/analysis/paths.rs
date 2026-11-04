//! Path enumeration over the structured Effect IR.
//!
//! A *trace* is the ordered list of events along one path through a function:
//! executed effects, transaction begin/end, and the branch decisions taken.
//! Traces are enumerated forward; each node transforms the set of traces that
//! reach it into the traces that leave it normally, plus traces that left via
//! `return`, `throw`, `break` or `continue`. Branches that contain neither
//! effects nor exits do not fork paths.
//!
//! Each branch arm may carry [`Fact`]s about variables that never change. A
//! trace that takes an arm whose facts contradict the facts of an earlier
//! decision on the same trace (`if (a) ...; if (!a) ...`) cannot execute and
//! is discarded. Conditions without facts are treated as unknown: they never
//! discard a trace.

use crate::fasthash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::borrow::Cow;

use crate::ir::{
    Branch, CallScope, Effect, EffectKind, Exit, ExitKind, Fact, FunctionIr, Loop, Node, Switch,
    TransactionId, Try,
};

/// Context of an effect event that weakens conclusions about it.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct EffectContext<'ir> {
    /// Innermost loop the effect executes in.
    pub in_loop: Option<&'ir Loop>,
    /// Innermost `try` with a `catch` handler protecting the effect.
    pub guarded_by: Option<&'ir Try>,
    /// The effect runs in a `catch` handler entered by approximation.
    pub in_handler: Option<&'ir Try>,
}

/// A branch decision recorded on a trace.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Decision<'ir> {
    Arm {
        branch: &'ir Branch,
        arm: usize,
    },
    Case {
        switch: &'ir Switch,
        case: usize,
    },
    NoCase {
        switch: &'ir Switch,
    },
    LoopEntered(&'ir Loop),
    LoopSkipped(&'ir Loop),
    /// An exception escaped the `try` block before any of its effects ran
    /// (an approximation of "some operation in the try block threw").
    CatchImplicit(&'ir Try),
    /// An explicit `throw` in the `try` block was caught.
    CatchExplicit(&'ir Try),
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Event<'ir> {
    Effect {
        effect: &'ir Effect,
        ctx: EffectContext<'ir>,
    },
    TxBegin(TransactionId),
    TxEnd {
        transaction: TransactionId,
        committed: bool,
    },
    Decision(Decision<'ir>),
    /// A `return` whose facts hold from here on (the returned value of a
    /// transaction callback, bound to a variable).
    Returned(&'ir Exit),
    /// The path reached control flow the frontend could not model.
    Incomplete(&'ir Exit),
}

pub(crate) type Trace<'ir> = Vec<Event<'ir>>;

/// How inlined calls are walked (Phase 7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathMode {
    /// Every trace that reaches an inlined call is walked through the
    /// callee's body (Phases 2–6).
    Legacy,
    /// A call whose facts share no variable with facts outside it is walked
    /// once; the resulting *compact summary* (its trace fragments, grouped
    /// by exit) is spliced onto every incoming trace. Produces exactly the
    /// legacy traces, in the same order, with the same statistics
    /// (`docs/phase7/COMPACT-SUMMARIES.md`).
    Compact,
    /// Phase 8: [`Self::Compact`], and a call whose body corresponds
    /// node for node to an already summarized call (same callee, same
    /// structure, fact variables renamed bijectively — two instantiations
    /// of one callee expansion) reuses that summary through the
    /// correspondence instead of being walked
    /// (`docs/phase8/PARAMETERIZED-SUMMARIES.md`).
    Shared,
}

static PATH_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2);

/// Select how inlined calls are walked for this process (the CLI reads
/// `MIDWRITE_PATHS=legacy|compact`).
pub fn set_path_mode(mode: PathMode) {
    PATH_MODE.store(
        match mode {
            PathMode::Legacy => 0,
            PathMode::Compact => 1,
            PathMode::Shared => 2,
        },
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// Apply `MIDWRITE_PATHS` (`legacy`, `compact` or `shared`) if set.
pub fn path_mode_from_env() -> Result<(), String> {
    match std::env::var("MIDWRITE_PATHS").as_deref() {
        Err(_) => Ok(()),
        Ok("legacy") => {
            set_path_mode(PathMode::Legacy);
            Ok(())
        }
        Ok("compact") => {
            set_path_mode(PathMode::Compact);
            Ok(())
        }
        Ok("shared") => {
            set_path_mode(PathMode::Shared);
            Ok(())
        }
        Ok(other) => Err(format!(
            "MIDWRITE_PATHS=`{other}`: expected `legacy`, `compact` or `shared`"
        )),
    }
}

pub fn path_mode() -> PathMode {
    match PATH_MODE.load(std::sync::atomic::Ordering::Relaxed) {
        0 => PathMode::Legacy,
        1 => PathMode::Compact,
        _ => PathMode::Shared,
    }
}

/// Counters of compact-summary use (for measurements; never in reports).
#[derive(Clone, Copy, Debug, Default)]
pub struct CompactStats {
    /// Call scopes summarized (walked once).
    pub built: usize,
    /// Incoming traces served from a summary instead of a walk.
    pub reused: usize,
    /// Calls walked the legacy way because their facts are shared.
    pub shared_facts: usize,
    /// Phase 8: call scopes whose summary was instantiated from another
    /// instance through a verified correspondence (not walked).
    pub instantiated: usize,
    /// Phase 8: candidates with an equal shape whose correspondence failed.
    pub shape_rejected: usize,
}

/// A summarized call (Phase 8): the call, the context key and the effect
/// context its summary was built in.
type ShapeEntry<'ir> = (&'ir CallScope, [usize; 3], EffectContext<'ir>);

/// The traces a call scope produces from an empty trace, grouped like the
/// legacy walk groups them after leaving the call.
#[derive(Clone)]
struct Compact<'ir> {
    normal: Vec<Trace<'ir>>,
    returned: Vec<Trace<'ir>>,
    thrown: Vec<Trace<'ir>>,
    pruned: usize,
    merged: usize,
    truncated: bool,
}

#[derive(Debug)]
pub(crate) struct Paths<'ir> {
    pub traces: Vec<Trace<'ir>>,
    /// More paths existed than `max_paths`; only a prefix was explored.
    pub truncated: bool,
    /// Number of traces discarded because their branch decisions contradict
    /// each other.
    pub pruned: usize,
    /// Number of traces merged into an equivalent trace when leaving an
    /// inlined call.
    pub merged: usize,
}

/// Enumerate the traces of `function`, exploring at most `max_paths` paths.
pub(crate) fn enumerate(function: &FunctionIr, max_paths: usize) -> Paths<'_> {
    enumerate_with(function, max_paths, false)
}

/// Like [`enumerate`]; with `include_reads`, database reads on the explored
/// paths are recorded as events too (for failure traces). Reads never fork
/// paths and are ignored when traces are merged, so the set and order of
/// traces, and every finding, are the same either way.
pub(crate) fn enumerate_with(
    function: &FunctionIr,
    max_paths: usize,
    include_reads: bool,
) -> Paths<'_> {
    enumerate_in(function, max_paths, include_reads, path_mode()).0
}

/// [`enumerate_with`] in an explicit [`PathMode`], with compact-summary
/// counters.
pub(crate) fn enumerate_in(
    function: &FunctionIr,
    max_paths: usize,
    include_reads: bool,
    mode: PathMode,
) -> (Paths<'_>, CompactStats) {
    let mut walker = Walker {
        include_reads,
        max_paths: max_paths.max(1),
        truncated: false,
        pruned: 0,
        merged: 0,
        ctx: EffectContext::default(),
        fact_uses: HashMap::default(),
        live: HashMap::default(),
        compact: mode != PathMode::Legacy,
        summaries: HashMap::default(),
        stats: CompactStats::default(),
        shared: mode == PathMode::Shared,
        shapes: HashMap::default(),
        repeated: HashSet::default(),
        shape_cache: HashMap::default(),
    };
    count_fact_variables(&function.body, &mut walker.fact_uses);
    if walker.shared {
        let mut seen: HashMap<&str, usize> = HashMap::default();
        count_callees(&function.body, &mut seen);
        walker.repeated = seen
            .into_iter()
            .filter(|(_, n)| *n > 1)
            .map(|(callee, _)| callee)
            .collect();
    }
    let flow = walker.block(&function.body, vec![Vec::new()]);
    let mut traces = flow.normal;
    traces.extend(flow.returned);
    traces.extend(flow.thrown);
    // `break`/`continue` cannot leave a function body; keep them anyway.
    traces.extend(flow.broke);
    traces.extend(flow.continued);
    (
        Paths {
            traces,
            truncated: walker.truncated,
            pruned: walker.pruned,
            merged: walker.merged,
        },
        walker.stats,
    )
}

/// Traces leaving a node, grouped by how they leave it.
#[derive(Debug, Default)]
struct Flow<'ir> {
    normal: Vec<Trace<'ir>>,
    returned: Vec<Trace<'ir>>,
    thrown: Vec<Trace<'ir>>,
    broke: Vec<Trace<'ir>>,
    continued: Vec<Trace<'ir>>,
}

impl<'ir> Flow<'ir> {
    fn normal(traces: Vec<Trace<'ir>>) -> Self {
        Self {
            normal: traces,
            ..Self::default()
        }
    }

    fn len(&self) -> usize {
        self.normal.len()
            + self.returned.len()
            + self.thrown.len()
            + self.broke.len()
            + self.continued.len()
    }

    /// Merge all buckets of `other` into `self`.
    fn merge(&mut self, mut other: Self) {
        self.normal.append(&mut other.normal);
        self.absorb_exits(other);
    }

    /// Merge the non-normal buckets of `other` into `self`.
    fn absorb_exits(&mut self, other: Self) {
        self.returned.extend(other.returned);
        self.thrown.extend(other.thrown);
        self.broke.extend(other.broke);
        self.continued.extend(other.continued);
    }
}

struct Walker<'ir> {
    include_reads: bool,
    max_paths: usize,
    truncated: bool,
    pruned: usize,
    merged: usize,
    ctx: EffectContext<'ir>,
    /// How many facts of the whole function mention each variable.
    fact_uses: HashMap<String, usize>,
    /// Per inlined call (by address): the fact variables of its body that
    /// facts outside the call also mention (live after the call).
    live: HashMap<usize, HashSet<String>>,
    compact: bool,
    /// Compact summaries by (call address, effect context).
    summaries: HashMap<(usize, [usize; 3]), Compact<'ir>>,
    stats: CompactStats,
    /// Phase 8 ([`PathMode::Shared`]): summarized calls by shape, with the
    /// context key and effect context their summary was built in.
    shared: bool,
    shapes: HashMap<u64, Vec<ShapeEntry<'ir>>>,
    /// Callees with at least two call scopes in the function: only their
    /// calls can share a summary, so only they are hashed.
    repeated: HashSet<&'ir str>,
    /// Shape of each hashed call scope (by address).
    shape_cache: HashMap<usize, u64>,
}

impl<'ir> Walker<'ir> {
    fn block(&mut self, nodes: &'ir [Node], input: Vec<Trace<'ir>>) -> Flow<'ir> {
        let mut flow = Flow::default();
        let mut current = input;
        for node in nodes {
            if current.is_empty() {
                // Every path has left this block; the rest is unreachable.
                break;
            }
            let mut step = self.node(node, current);
            current = std::mem::take(&mut step.normal);
            flow.absorb_exits(step);
            self.limit(&mut current, &mut flow);
        }
        flow.normal = current;
        flow
    }

    fn node(&mut self, node: &'ir Node, mut input: Vec<Trace<'ir>>) -> Flow<'ir> {
        match node {
            // Reads neither commit nor become visible: they cannot be part of
            // a finding, so they neither appear in traces nor fork paths.
            Node::Effect(effect) if is_read(effect) && !self.include_reads => Flow::normal(input),
            Node::Effect(effect) => {
                let event = Event::Effect {
                    effect,
                    ctx: self.ctx,
                };
                for trace in &mut input {
                    trace.push(event);
                }
                Flow::normal(input)
            }
            Node::Transaction(scope) => {
                let transaction = scope.transaction;
                for trace in &mut input {
                    trace.push(Event::TxBegin(transaction));
                }
                let inner = self.block(&scope.body, input);
                // The callback finishing (normally or via `return`) commits;
                // an exception rolls back and propagates out of the call.
                let mut committed = inner.normal;
                committed.extend(inner.returned);
                for trace in &mut committed {
                    trace.push(Event::TxEnd {
                        transaction,
                        committed: true,
                    });
                }
                let mut aborted = inner.thrown;
                for trace in &mut aborted {
                    trace.push(Event::TxEnd {
                        transaction,
                        committed: false,
                    });
                }
                Flow {
                    normal: committed,
                    thrown: aborted,
                    ..Flow::default()
                }
            }
            Node::Branch(branch) => {
                if !branch.arms.iter().any(|arm| is_relevant(&arm.body)) {
                    return Flow::normal(input);
                }
                let mut out = Flow::default();
                for (arm_index, arm) in branch.arms.iter().enumerate() {
                    let traces = self.decide(
                        &input,
                        Decision::Arm {
                            branch,
                            arm: arm_index,
                        },
                    );
                    if traces.is_empty() {
                        continue;
                    }
                    out.merge(self.block(&arm.body, traces));
                }
                out
            }
            Node::Switch(switch) => self.switch(switch, input),
            Node::Loop(lp) => self.loop_node(lp, input),
            Node::Call(call) => self.call(call, input),
            Node::Try(try_node) => self.try_node(try_node, input),
            Node::Exit(exit) => match exit.kind {
                ExitKind::Return if !exit.facts.is_empty() => {
                    let mut returned = Vec::with_capacity(input.len());
                    for mut trace in input {
                        if feasible(&trace, &exit.facts) {
                            trace.push(Event::Returned(exit));
                            returned.push(trace);
                        } else {
                            self.pruned += 1;
                        }
                    }
                    Flow {
                        returned,
                        ..Flow::default()
                    }
                }
                ExitKind::Return => Flow {
                    returned: input,
                    ..Flow::default()
                },
                ExitKind::Throw | ExitKind::Rollback => Flow {
                    thrown: input,
                    ..Flow::default()
                },
                ExitKind::Break => Flow {
                    broke: input,
                    ..Flow::default()
                },
                ExitKind::Continue => Flow {
                    continued: input,
                    ..Flow::default()
                },
                ExitKind::Unsupported => {
                    for trace in &mut input {
                        trace.push(Event::Incomplete(exit));
                    }
                    Flow {
                        returned: input,
                        ..Flow::default()
                    }
                }
            },
        }
    }

    /// An inlined callee: its `return` resumes the caller, its exceptions
    /// propagate. Paths that stopped at unsupported control flow stay stopped.
    ///
    /// Each incoming trace is walked through the callee separately, and the
    /// traces it produces are merged when they are equivalent (see
    /// [`merge_equivalent`]): the callee's branch facts concern only its own
    /// variables, which nothing after the call refers to, so paths that
    /// differ only in which arms the callee took behave identically from
    /// here on. This keeps a callee's early returns from multiplying the
    /// caller's paths.
    fn call(&mut self, call: &'ir CallScope, input: Vec<Trace<'ir>>) -> Flow<'ir> {
        let address = std::ptr::from_ref(call) as usize;
        if !self.live.contains_key(&address) {
            let mut inside = HashMap::default();
            count_fact_variables(&call.body, &mut inside);
            let live: HashSet<String> = inside
                .into_iter()
                .filter(|(variable, count)| {
                    self.fact_uses.get(variable).copied().unwrap_or(0) > *count
                })
                .map(|(variable, _)| variable)
                .collect();
            self.live.insert(address, live);
        }
        // A callee whose facts share no variable with facts outside it is
        // walked identically whatever the incoming trace is: its branch
        // feasibility only consults its own facts (contradiction needs a
        // common variable), and everything else depends only on the effect
        // context. Walk it once and splice.
        if self.compact && self.live.get(&address).is_some_and(HashSet::is_empty) {
            return self.call_compact(call, address, input);
        }
        if self.compact {
            self.stats.shared_facts += 1;
        }
        let mut out = Flow::default();
        for trace in input {
            if out.len() >= self.max_paths {
                // The caller's block would drop the rest anyway.
                self.truncated = true;
                break;
            }
            let entry = trace.len();
            let inner = self.block(&call.body, vec![trace]);
            let mut normal = inner.normal;
            for trace in inner.returned {
                if matches!(trace.last(), Some(Event::Incomplete(_))) {
                    out.returned.push(trace);
                } else {
                    normal.push(trace);
                }
            }
            // `break`/`continue` cannot leave a function body; keep them anyway.
            normal.extend(inner.broke);
            normal.extend(inner.continued);
            out.normal
                .extend(self.merge_equivalent(normal, entry, address));
            out.thrown
                .extend(self.merge_equivalent(inner.thrown, entry, address));
        }
        out
    }

    /// [`Self::call`] through a compact summary: the callee is walked from an
    /// empty trace (exactly the per-trace work of the legacy walk, with the
    /// same budget, pruning and merging) once per effect context, and its
    /// fragments are appended to each incoming trace. Statistics are
    /// accounted per incoming trace, as the legacy walk accounts them.
    fn call_compact(
        &mut self,
        call: &'ir CallScope,
        address: usize,
        input: Vec<Trace<'ir>>,
    ) -> Flow<'ir> {
        let ctx = [
            self.ctx
                .in_loop
                .map_or(0, |l| std::ptr::from_ref(l) as usize),
            self.ctx
                .guarded_by
                .map_or(0, |t| std::ptr::from_ref(t) as usize),
            self.ctx
                .in_handler
                .map_or(0, |t| std::ptr::from_ref(t) as usize),
        ];
        let key = (address, ctx);
        let shape = (self.shared
            && self.repeated.contains(&*call.callee)
            && !self.summaries.contains_key(&key))
        .then(|| call_shape(call, &mut self.shape_cache));
        if let Some(shape) = shape {
            for &(other, other_key, other_ctx) in self.shapes.get(&shape).into_iter().flatten() {
                if other.callee != call.callee {
                    continue;
                }
                let Some(summary) = self
                    .summaries
                    .get(&(std::ptr::from_ref(other) as usize, other_key))
                else {
                    continue;
                };
                let mut corr = Corr::default();
                let instance = if corr.nodes(&other.body, &call.body) {
                    corr.compact(summary, &other_ctx, &self.ctx)
                } else {
                    None
                };
                match instance {
                    Some(instance) => {
                        self.summaries.insert(key, instance);
                        self.stats.instantiated += 1;
                        break;
                    }
                    None => self.stats.shape_rejected += 1,
                }
            }
        }
        if !self.summaries.contains_key(&key) {
            if let Some(shape) = shape {
                self.shapes
                    .entry(shape)
                    .or_default()
                    .push((call, ctx, self.ctx));
            }
            let (pruned, merged, truncated) = (self.pruned, self.merged, self.truncated);
            self.truncated = false;
            let inner = self.block(&call.body, vec![Vec::new()]);
            let mut normal = inner.normal;
            let mut returned = Vec::new();
            for trace in inner.returned {
                if matches!(trace.last(), Some(Event::Incomplete(_))) {
                    returned.push(trace);
                } else {
                    normal.push(trace);
                }
            }
            normal.extend(inner.broke);
            normal.extend(inner.continued);
            let normal = self.merge_equivalent(normal, 0, address);
            let thrown = self.merge_equivalent(inner.thrown, 0, address);
            let summary = Compact {
                normal,
                returned,
                thrown,
                pruned: self.pruned - pruned,
                merged: self.merged - merged,
                truncated: self.truncated,
            };
            self.pruned = pruned;
            self.merged = merged;
            self.truncated = truncated;
            self.summaries.insert(key, summary);
            self.stats.built += 1;
        }
        let summary = &self.summaries[&key];
        let mut out = Flow::default();
        let (mut pruned, mut merged, mut truncated) = (0, 0, false);
        for trace in input {
            if out.len() >= self.max_paths {
                truncated = true;
                break;
            }
            pruned += summary.pruned;
            merged += summary.merged;
            truncated |= summary.truncated;
            let splice = |fragments: &[Trace<'ir>], into: &mut Vec<Trace<'ir>>| {
                for fragment in fragments {
                    let mut t = Vec::with_capacity(trace.len() + fragment.len());
                    t.extend_from_slice(&trace);
                    t.extend_from_slice(fragment);
                    into.push(t);
                }
            };
            // The legacy walk emits, per incoming trace, returned
            // (incomplete) traces, then the merged normal ones, then the
            // merged thrown ones, each in walk order.
            splice(&summary.returned, &mut out.returned);
            splice(&summary.normal, &mut out.normal);
            splice(&summary.thrown, &mut out.thrown);
            self.stats.reused += 1;
        }
        self.pruned += pruned;
        self.merged += merged;
        self.truncated |= truncated;
        out
    }

    /// Drop the traces that are dominated by another trace leaving the same
    /// call from the same incoming trace: one with the same events after
    /// `entry` (effects, transaction boundaries, catch-handler entries), and
    /// no more branch decisions between any two consecutive events. A
    /// dominated trace produces the same findings with at most the same
    /// confidence, and the facts that tell it apart concern only the
    /// variables of the call being left, which nothing after the call
    /// refers to.
    fn merge_equivalent(
        &mut self,
        traces: Vec<Trace<'ir>>,
        entry: usize,
        call: usize,
    ) -> Vec<Trace<'ir>> {
        if traces.len() < 2 {
            return traces;
        }
        let live = self.live.get(&call).cloned().unwrap_or_default();
        // Kept traces by event key: (index, decisions between events).
        type Kept = Vec<(usize, Vec<usize>)>;
        let mut groups: HashMap<Vec<[usize; 5]>, Kept> = HashMap::default();
        let mut keep = vec![true; traces.len()];
        for (index, trace) in traces.iter().enumerate() {
            let mut key = Vec::new();
            let mut counts = vec![0usize];
            for event in &trace[entry.min(trace.len())..] {
                match event {
                    // Facts about the callee's own variables are dead after
                    // the call. Facts a callee's return states about a
                    // variable that facts outside the call also mention (a
                    // binding of the call's result, Phase 5) are not:
                    // traces that differ in them stay apart.
                    Event::Returned(exit) => {
                        let facts: Vec<String> = exit
                            .facts
                            .iter()
                            .filter(|fact| fact.variable().is_some_and(|v| live.contains(v)))
                            .map(|fact| format!("{fact:?}"))
                            .collect();
                        if !facts.is_empty() {
                            key.push([11, fact_key(&facts), 0, 0, 0]);
                            counts.push(0);
                        }
                    }
                    // Reads (recorded only for failure traces) do not
                    // distinguish traces.
                    Event::Effect { effect, .. } if is_read(effect) => {}
                    Event::Decision(
                        Decision::Arm { .. }
                        | Decision::Case { .. }
                        | Decision::NoCase { .. }
                        | Decision::LoopEntered(_)
                        | Decision::LoopSkipped(_),
                    ) => *counts.last_mut().expect("never empty") += 1,
                    other => {
                        key.push(event_key(other));
                        counts.push(0);
                    }
                }
            }
            let kept = groups.entry(key).or_default();
            let dominates = |a: &[usize], b: &[usize]| a.iter().zip(b).all(|(x, y)| x <= y);
            if kept.iter().any(|(_, other)| dominates(other, &counts)) {
                keep[index] = false;
                continue;
            }
            kept.retain(|(other_index, other)| {
                let dominated = dominates(&counts, other);
                if dominated {
                    keep[*other_index] = false;
                }
                !dominated
            });
            kept.push((index, counts));
        }
        let before = traces.len();
        let out: Vec<Trace<'ir>> = traces
            .into_iter()
            .zip(keep)
            .filter_map(|(trace, keep)| keep.then_some(trace))
            .collect();
        self.merged += before - out.len();
        out
    }

    fn switch(&mut self, switch: &'ir Switch, input: Vec<Trace<'ir>>) -> Flow<'ir> {
        if !switch.cases.iter().any(|case| is_relevant(&case.body)) {
            return Flow::normal(input);
        }
        let mut out = Flow::default();
        for entry in 0..switch.cases.len() {
            let mut current = self.decide(
                &input,
                Decision::Case {
                    switch,
                    case: entry,
                },
            );
            // Fall through subsequent cases until every path has left.
            for case in &switch.cases[entry..] {
                if current.is_empty() {
                    break;
                }
                let mut step = self.block(&case.body, current);
                current = std::mem::take(&mut step.normal);
                out.normal.append(&mut step.broke);
                out.absorb_exits(step);
            }
            out.normal.extend(current);
        }
        if !switch.has_default {
            let unmatched = self.decide(&input, Decision::NoCase { switch });
            out.normal.extend(unmatched);
        }
        out
    }

    fn loop_node(&mut self, lp: &'ir Loop, input: Vec<Trace<'ir>>) -> Flow<'ir> {
        if !is_relevant(&lp.body) {
            return Flow::normal(input);
        }
        let mut out = Flow::default();
        let entered = if lp.runs_at_least_once {
            input
        } else {
            out.normal = with_decision(&input, Decision::LoopSkipped(lp));
            with_decision(&input, Decision::LoopEntered(lp))
        };
        let saved = self.ctx;
        self.ctx.in_loop = Some(lp);
        let body = self.block(&lp.body, entered);
        self.ctx = saved;
        out.normal.extend(body.normal);
        out.normal.extend(body.broke);
        out.normal.extend(body.continued);
        out.returned.extend(body.returned);
        out.thrown.extend(body.thrown);
        out
    }

    fn try_node(&mut self, try_node: &'ir Try, input: Vec<Trace<'ir>>) -> Flow<'ir> {
        let handler_relevant = try_node
            .handler
            .as_ref()
            .is_some_and(|handler| is_relevant(&handler.body));
        let finalizer_relevant = try_node.finalizer.as_deref().is_some_and(is_relevant);
        if !is_relevant(&try_node.block) && !handler_relevant && !finalizer_relevant {
            return Flow::normal(input);
        }

        let saved = self.ctx;
        if try_node.handler.is_some() {
            self.ctx.guarded_by = Some(try_node);
        }
        let handler_entry = if handler_relevant {
            with_decision(&input, Decision::CatchImplicit(try_node))
        } else {
            Vec::new()
        };
        let mut block = self.block(&try_node.block, input);
        self.ctx = saved;

        let mut out = Flow::default();
        if let Some(handler) = &try_node.handler {
            let mut entry = handler_entry;
            entry.extend(with_decision(
                &block.thrown,
                Decision::CatchExplicit(try_node),
            ));
            block.thrown.clear();
            self.ctx.in_handler = Some(try_node);
            let handled = self.block(&handler.body, entry);
            self.ctx = saved;
            out.merge(handled);
        }
        out.merge(block);

        match &try_node.finalizer {
            Some(finalizer) if finalizer_relevant => self.finalize(finalizer, out),
            _ => out,
        }
    }

    /// Run a `finally` block on every way of leaving the `try` statement.
    /// A normally completing finalizer resumes the original completion.
    fn finalize(&mut self, finalizer: &'ir [Node], flow: Flow<'ir>) -> Flow<'ir> {
        let mut out = Flow::default();
        let normal = self.through_finalizer(finalizer, flow.normal, &mut out);
        let returned = self.through_finalizer(finalizer, flow.returned, &mut out);
        let thrown = self.through_finalizer(finalizer, flow.thrown, &mut out);
        let broke = self.through_finalizer(finalizer, flow.broke, &mut out);
        let continued = self.through_finalizer(finalizer, flow.continued, &mut out);
        out.normal.extend(normal);
        out.returned.extend(returned);
        out.thrown.extend(thrown);
        out.broke.extend(broke);
        out.continued.extend(continued);
        out
    }

    /// Walk `finalizer` for `traces`; returns the traces that complete it
    /// normally and moves the finalizer's own exits into `out`.
    fn through_finalizer(
        &mut self,
        finalizer: &'ir [Node],
        traces: Vec<Trace<'ir>>,
        out: &mut Flow<'ir>,
    ) -> Vec<Trace<'ir>> {
        if traces.is_empty() {
            return traces;
        }
        let mut step = self.block(finalizer, traces);
        let normal = std::mem::take(&mut step.normal);
        out.absorb_exits(step);
        normal
    }

    /// Copies of `traces` that take `decision`, without the copies for which
    /// the decision contradicts an earlier one.
    fn decide(&mut self, traces: &[Trace<'ir>], decision: Decision<'ir>) -> Vec<Trace<'ir>> {
        let facts = decision_facts(&decision);
        let mut out = Vec::with_capacity(traces.len());
        for trace in traces {
            if feasible(trace, &facts) {
                let mut trace = trace.clone();
                trace.push(Event::Decision(decision));
                out.push(trace);
            } else {
                self.pruned += 1;
            }
        }
        out
    }

    /// Enforce the path budget on the traces currently alive in a block.
    fn limit(&mut self, current: &mut Vec<Trace<'ir>>, flow: &mut Flow<'ir>) {
        let total = current.len() + flow.len();
        if total > self.max_paths {
            self.truncated = true;
            let keep = self.max_paths.saturating_sub(flow.len()).max(1);
            current.truncate(keep);
        }
    }
}

/// Whether two traces consist of the same events (IR identity, arms,
/// contexts).
pub(crate) fn same_trace(a: &Trace<'_>, b: &Trace<'_>) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            event_key(x) == event_key(y)
                && match (x, y) {
                    (
                        Event::Decision(Decision::Arm { arm: p, .. }),
                        Event::Decision(Decision::Arm { arm: q, .. }),
                    ) => p == q,
                    (
                        Event::Decision(Decision::Case { case: p, .. }),
                        Event::Decision(Decision::Case { case: q, .. }),
                    ) => p == q,
                    (Event::Decision(d), Event::Decision(e)) => {
                        std::mem::discriminant(d) == std::mem::discriminant(e)
                    }
                    _ => true,
                }
        })
}

fn with_decision<'ir>(traces: &[Trace<'ir>], decision: Decision<'ir>) -> Vec<Trace<'ir>> {
    traces
        .iter()
        .map(|trace| {
            let mut trace = trace.clone();
            trace.push(Event::Decision(decision));
            trace
        })
        .collect()
}

fn is_read(effect: &Effect) -> bool {
    matches!(effect.kind, EffectKind::DatabaseRead { .. })
}

/// An identity for an event; for a branch decision, which arm it takes is ignored.
fn event_key(event: &Event<'_>) -> [usize; 5] {
    fn ptr<T>(value: &T) -> usize {
        std::ptr::from_ref(value) as usize
    }
    fn opt<T>(value: Option<&T>) -> usize {
        value.map_or(0, ptr)
    }
    match event {
        Event::Effect { effect, ctx } => [
            1,
            ptr(*effect),
            opt(ctx.in_loop),
            opt(ctx.guarded_by),
            opt(ctx.in_handler),
        ],
        Event::TxBegin(id) => [2, id.0 as usize, 0, 0, 0],
        Event::TxEnd {
            transaction,
            committed,
        } => [3, transaction.0 as usize, usize::from(*committed), 0, 0],
        Event::Incomplete(exit) => [4, ptr(*exit), 0, 0, 0],
        Event::Returned(exit) => [10, ptr(*exit), 0, 0, 0],
        Event::Decision(decision) => match *decision {
            Decision::Arm { branch, .. } => [5, ptr(branch), 0, 0, 0],
            Decision::Case { switch, .. } | Decision::NoCase { switch } => {
                [6, ptr(switch), 0, 0, 0]
            }
            Decision::LoopEntered(lp) | Decision::LoopSkipped(lp) => [7, ptr(lp), 0, 0, 0],
            Decision::CatchImplicit(try_node) => [8, ptr(try_node), 0, 0, 0],
            Decision::CatchExplicit(try_node) => [9, ptr(try_node), 0, 0, 0],
        },
    }
}

/// The facts taking `decision` implies.
pub(crate) fn decision_facts<'ir>(decision: &Decision<'ir>) -> Cow<'ir, [Fact]> {
    match *decision {
        Decision::Arm { branch, arm } => Cow::Borrowed(&branch.arms[arm].facts),
        Decision::Case { switch, case } => Cow::Borrowed(&switch.cases[case].facts),
        // No case matched: the discriminant differs from every literal case.
        Decision::NoCase { switch } => Cow::Owned(
            switch
                .cases
                .iter()
                .flat_map(|case| &case.facts)
                .filter_map(|fact| match fact {
                    Fact::Equals { variable, literal } => Some(Fact::NotEquals {
                        variable: variable.clone(),
                        literal: literal.clone(),
                    }),
                    _ => None,
                })
                .collect(),
        ),
        Decision::LoopEntered(_)
        | Decision::LoopSkipped(_)
        | Decision::CatchImplicit(_)
        | Decision::CatchExplicit(_) => Cow::Borrowed(&[]),
    }
}

/// Whether `facts` are consistent with each other and with the facts of
/// every decision already on `trace`.
fn feasible(trace: &Trace<'_>, facts: &[Fact]) -> bool {
    if facts.is_empty() {
        return true;
    }
    for (index, fact) in facts.iter().enumerate() {
        if matches!(fact, Fact::Never)
            || facts[index + 1..]
                .iter()
                .any(|other| fact.contradicts(other))
        {
            return false;
        }
    }
    trace.iter().all(|event| match event {
        Event::Decision(decision) => decision_facts(decision)
            .iter()
            .all(|prior| facts.iter().all(|fact| !fact.contradicts(prior))),
        Event::Returned(exit) => exit
            .facts
            .iter()
            .all(|prior| facts.iter().all(|fact| !fact.contradicts(prior))),
        _ => true,
    })
}

/// Whether a block contains anything that affects traces: effects,
/// transactions or exits.
pub(crate) fn is_relevant(nodes: &[Node]) -> bool {
    nodes.iter().any(|node| match node {
        Node::Effect(effect) => !is_read(effect),
        Node::Transaction(_) | Node::Exit(_) => true,
        Node::Branch(branch) => branch.arms.iter().any(|arm| is_relevant(&arm.body)),
        Node::Switch(switch) => switch.cases.iter().any(|case| is_relevant(&case.body)),
        Node::Loop(lp) => is_relevant(&lp.body),
        Node::Call(call) => is_relevant(&call.body),
        Node::Try(try_node) => {
            is_relevant(&try_node.block)
                || try_node
                    .handler
                    .as_ref()
                    .is_some_and(|handler| is_relevant(&handler.body))
                || try_node.finalizer.as_deref().is_some_and(is_relevant)
        }
    })
}

/// Count the facts (arm and exit facts) that mention each variable.
fn count_fact_variables(nodes: &[Node], out: &mut HashMap<String, usize>) {
    fn walk(nodes: &[Node], out: &mut HashMap<String, usize>) {
        let add = |facts: &[Fact], out: &mut HashMap<String, usize>| {
            for fact in facts {
                if let Some(variable) = fact.variable() {
                    *out.entry(variable.to_owned()).or_default() += 1;
                }
            }
        };
        for node in nodes {
            match node {
                Node::Branch(branch) => {
                    for arm in &branch.arms {
                        add(&arm.facts, out);
                        walk(&arm.body, out);
                    }
                }
                Node::Switch(switch) => {
                    for case in &switch.cases {
                        add(&case.facts, out);
                        walk(&case.body, out);
                    }
                }
                Node::Exit(exit) => add(&exit.facts, out),
                Node::Transaction(scope) => walk(&scope.body, out),
                Node::Loop(lp) => walk(&lp.body, out),
                Node::Call(call) => walk(&call.body, out),
                Node::Try(t) => {
                    walk(&t.block, out);
                    if let Some(h) = &t.handler {
                        walk(&h.body, out);
                    }
                    if let Some(f) = &t.finalizer {
                        walk(f, out);
                    }
                }
                Node::Effect(_) => {}
            }
        }
    }
    walk(nodes, out);
}

/// A merge key for a set of facts (order-independent).
fn fact_key(facts: &[String]) -> usize {
    let mut sorted: Vec<&String> = facts.iter().collect();
    sorted.sort();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in sorted
        .iter()
        .flat_map(|f| f.bytes().chain(std::iter::once(0)))
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash as usize
}

// ---------------------------------------------------------------------------
// Phase 8: summaries shared between call instances
// ---------------------------------------------------------------------------

/// A canonical hash of everything the walker reads from a call body: node
/// kinds and nesting, whether effects are reads, facts (variables numbered
/// by first occurrence, so instantiation prefixes do not matter), exit
/// kinds, loop and `try` shapes, transaction nesting. Equal shapes are only
/// candidates: [`Corr::nodes`] decides. Cached by call address.
fn call_shape(call: &CallScope, cache: &mut HashMap<usize, u64>) -> u64 {
    struct H<'ir, 'c> {
        hash: u64,
        vars: HashMap<&'ir str, u64>,
        cache: &'c mut HashMap<usize, u64>,
    }
    impl<'ir> H<'ir, '_> {
        fn word(&mut self, w: u64) {
            for byte in w.to_le_bytes() {
                self.hash ^= u64::from(byte);
                self.hash = self.hash.wrapping_mul(0x0100_0000_01b3);
            }
        }
        fn text(&mut self, t: &str) {
            for byte in t.bytes() {
                self.hash ^= u64::from(byte);
                self.hash = self.hash.wrapping_mul(0x0100_0000_01b3);
            }
            self.word(0xff);
        }
        fn literal(&mut self, literal: &crate::ir::Literal) {
            use crate::ir::Literal;
            match literal {
                Literal::Boolean(b) => self.word(100 + u64::from(*b)),
                Literal::String(v) => {
                    self.word(102);
                    self.text(v);
                }
                Literal::Number(v) => {
                    self.word(103);
                    self.text(v);
                }
                Literal::Null => self.word(104),
                Literal::Undefined => self.word(105),
            }
        }
        fn facts(&mut self, facts: &'ir [Fact]) {
            self.word(facts.len() as u64);
            for fact in facts {
                let (tag, variable) = match fact {
                    Fact::Never => (1, None),
                    Fact::Truthy { variable, value } => (2 + u64::from(*value), Some(variable)),
                    Fact::Equals { variable, literal } => {
                        self.literal(literal);
                        (4, Some(variable))
                    }
                    Fact::NotEquals { variable, literal } => {
                        self.literal(literal);
                        (5, Some(variable))
                    }
                    Fact::Object { variable } => (6, Some(variable)),
                };
                self.word(tag);
                if let Some(v) = variable {
                    let next = self.vars.len() as u64;
                    let n = *self.vars.entry(v.as_str()).or_insert(next);
                    self.word(n);
                }
            }
        }
        fn nodes(&mut self, nodes: &'ir [Node]) {
            self.word(nodes.len() as u64);
            for node in nodes {
                match node {
                    Node::Effect(e) => self.word(10 + u64::from(is_read(e))),
                    Node::Transaction(t) => {
                        self.word(12);
                        self.nodes(&t.body);
                    }
                    Node::Branch(b) => {
                        self.word(13);
                        self.word(b.arms.len() as u64);
                        for arm in &b.arms {
                            self.facts(&arm.facts);
                            self.nodes(&arm.body);
                        }
                    }
                    Node::Switch(sw) => {
                        self.word(14 + u64::from(sw.has_default));
                        self.word(sw.cases.len() as u64);
                        for case in &sw.cases {
                            self.facts(&case.facts);
                            self.nodes(&case.body);
                        }
                    }
                    Node::Loop(lp) => {
                        self.word(16 + u64::from(lp.runs_at_least_once));
                        self.nodes(&lp.body);
                    }
                    Node::Try(t) => {
                        self.word(18);
                        self.nodes(&t.block);
                        match &t.handler {
                            Some(h) => {
                                self.word(1);
                                self.nodes(&h.body);
                            }
                            None => self.word(0),
                        }
                        match &t.finalizer {
                            Some(f) => {
                                self.word(1);
                                self.nodes(f);
                            }
                            None => self.word(0),
                        }
                    }
                    Node::Exit(x) => {
                        self.word(20 + x.kind as u64);
                        self.facts(&x.facts);
                    }
                    // A nested call is hashed on its own (variables numbered
                    // within it), once: the hash stays invariant under a
                    // bijective renaming, and nested bodies are not hashed
                    // again for every enclosing call.
                    Node::Call(c) => {
                        self.word(40);
                        let nested = call_shape(c, self.cache);
                        self.word(nested);
                    }
                }
            }
        }
    }
    let address = std::ptr::from_ref(call) as usize;
    if let Some(hash) = cache.get(&address) {
        return *hash;
    }
    let mut h = H {
        hash: 0xcbf2_9ce4_8422_2325,
        vars: HashMap::default(),
        cache,
    };
    h.text(&call.callee);
    h.nodes(&call.body);
    let hash = h.hash;
    cache.insert(address, hash);
    hash
}

/// Count the call scopes of each callee in `nodes`.
fn count_callees<'ir>(nodes: &'ir [Node], out: &mut HashMap<&'ir str, usize>) {
    for node in nodes {
        match node {
            Node::Call(call) => {
                *out.entry(&*call.callee).or_default() += 1;
                count_callees(&call.body, out);
            }
            Node::Transaction(t) => count_callees(&t.body, out),
            Node::Branch(b) => b.arms.iter().for_each(|a| count_callees(&a.body, out)),
            Node::Switch(sw) => sw.cases.iter().for_each(|c| count_callees(&c.body, out)),
            Node::Loop(lp) => count_callees(&lp.body, out),
            Node::Try(t) => {
                count_callees(&t.block, out);
                if let Some(h) = &t.handler {
                    count_callees(&h.body, out);
                }
                if let Some(f) = &t.finalizer {
                    count_callees(f, out);
                }
            }
            Node::Effect(_) | Node::Exit(_) => {}
        }
    }
}

/// A structure-preserving correspondence from one call body (`a`) to
/// another (`b`): every node the walker can put into a trace, transaction
/// ids, and a BIJECTIVE renaming of fact variables. When it exists, walking
/// `b` makes exactly the decisions walking `a` makes (feasibility depends
/// only on facts, which correspond; merging only on event identities, which
/// map one to one), so `a`'s summary mapped through it is `b`'s summary.
#[derive(Default)]
struct Corr<'ir> {
    effects: HashMap<usize, &'ir Effect>,
    branches: HashMap<usize, &'ir Branch>,
    switches: HashMap<usize, &'ir Switch>,
    loops: HashMap<usize, &'ir Loop>,
    tries: HashMap<usize, &'ir Try>,
    exits: HashMap<usize, &'ir Exit>,
    transactions: HashMap<TransactionId, TransactionId>,
    transactions_back: HashMap<TransactionId, TransactionId>,
    vars: HashMap<String, String>,
    vars_back: HashMap<String, String>,
}

fn addr<T>(value: &T) -> usize {
    std::ptr::from_ref(value) as usize
}

impl<'ir> Corr<'ir> {
    fn var(&mut self, a: &str, b: &str) -> bool {
        match (self.vars.get(a), self.vars_back.get(b)) {
            (None, None) => {
                self.vars.insert(a.to_owned(), b.to_owned());
                self.vars_back.insert(b.to_owned(), a.to_owned());
                true
            }
            (Some(x), Some(y)) => x == b && y == a,
            _ => false,
        }
    }

    fn transaction(&mut self, a: TransactionId, b: TransactionId) -> bool {
        match (self.transactions.get(&a), self.transactions_back.get(&b)) {
            (None, None) => {
                self.transactions.insert(a, b);
                self.transactions_back.insert(b, a);
                true
            }
            (Some(x), Some(y)) => *x == b && *y == a,
            _ => false,
        }
    }

    fn facts(&mut self, a: &[Fact], b: &[Fact]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|pair| match pair {
                (Fact::Never, Fact::Never) => true,
                (
                    Fact::Truthy {
                        variable: x,
                        value: p,
                    },
                    Fact::Truthy {
                        variable: y,
                        value: q,
                    },
                ) => p == q && self.var(x, y),
                (
                    Fact::Equals {
                        variable: x,
                        literal: p,
                    },
                    Fact::Equals {
                        variable: y,
                        literal: q,
                    },
                )
                | (
                    Fact::NotEquals {
                        variable: x,
                        literal: p,
                    },
                    Fact::NotEquals {
                        variable: y,
                        literal: q,
                    },
                ) => p == q && self.var(x, y),
                (Fact::Object { variable: x }, Fact::Object { variable: y }) => self.var(x, y),
                _ => false,
            })
    }

    fn nodes(&mut self, a: &'ir [Node], b: &'ir [Node]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| self.node(x, y))
    }

    fn node(&mut self, a: &'ir Node, b: &'ir Node) -> bool {
        match (a, b) {
            (Node::Effect(x), Node::Effect(y)) => {
                self.effects.insert(addr(x), y);
                is_read(x) == is_read(y)
            }
            (Node::Transaction(x), Node::Transaction(y)) => {
                self.transaction(x.transaction, y.transaction) && self.nodes(&x.body, &y.body)
            }
            (Node::Branch(x), Node::Branch(y)) => {
                self.branches.insert(addr(x), y);
                x.arms.len() == y.arms.len()
                    && x.arms.iter().zip(&y.arms).all(|(p, q)| {
                        self.facts(&p.facts, &q.facts) && self.nodes(&p.body, &q.body)
                    })
            }
            (Node::Switch(x), Node::Switch(y)) => {
                self.switches.insert(addr(x), y);
                x.has_default == y.has_default
                    && x.cases.len() == y.cases.len()
                    && x.cases.iter().zip(&y.cases).all(|(p, q)| {
                        self.facts(&p.facts, &q.facts) && self.nodes(&p.body, &q.body)
                    })
            }
            (Node::Loop(x), Node::Loop(y)) => {
                self.loops.insert(addr(x), y);
                x.runs_at_least_once == y.runs_at_least_once && self.nodes(&x.body, &y.body)
            }
            (Node::Try(x), Node::Try(y)) => {
                self.tries.insert(addr(x), y);
                self.nodes(&x.block, &y.block)
                    && match (&x.handler, &y.handler) {
                        (Some(p), Some(q)) => self.nodes(&p.body, &q.body),
                        (None, None) => true,
                        _ => false,
                    }
                    && match (&x.finalizer, &y.finalizer) {
                        (Some(p), Some(q)) => self.nodes(p, q),
                        (None, None) => true,
                        _ => false,
                    }
            }
            (Node::Exit(x), Node::Exit(y)) => {
                self.exits.insert(addr(x), y);
                x.kind == y.kind && self.facts(&x.facts, &y.facts)
            }
            (Node::Call(x), Node::Call(y)) => self.nodes(&x.body, &y.body),
            _ => false,
        }
    }

    /// One effect-context slot: a node inside the call maps through the
    /// correspondence; the context the summary was built in maps to the
    /// current one.
    fn slot<T>(
        map: &HashMap<usize, &'ir T>,
        value: Option<&'ir T>,
        built: Option<&'ir T>,
        now: Option<&'ir T>,
    ) -> Option<Option<&'ir T>> {
        match value {
            None if built.is_none() => Some(now),
            None => None,
            Some(v) => match map.get(&addr(v)) {
                Some(mapped) => Some(Some(*mapped)),
                None if built.is_some_and(|b| addr(b) == addr(v)) => Some(now),
                None => None,
            },
        }
    }

    fn event(
        &self,
        event: &Event<'ir>,
        built: &EffectContext<'ir>,
        now: &EffectContext<'ir>,
    ) -> Option<Event<'ir>> {
        Some(match *event {
            Event::Effect { effect, ctx } => Event::Effect {
                effect: self.effects.get(&addr(effect))?,
                ctx: EffectContext {
                    in_loop: Self::slot(&self.loops, ctx.in_loop, built.in_loop, now.in_loop)?,
                    guarded_by: Self::slot(
                        &self.tries,
                        ctx.guarded_by,
                        built.guarded_by,
                        now.guarded_by,
                    )?,
                    in_handler: Self::slot(
                        &self.tries,
                        ctx.in_handler,
                        built.in_handler,
                        now.in_handler,
                    )?,
                },
            },
            Event::TxBegin(id) => Event::TxBegin(*self.transactions.get(&id)?),
            Event::TxEnd {
                transaction,
                committed,
            } => Event::TxEnd {
                transaction: *self.transactions.get(&transaction)?,
                committed,
            },
            Event::Returned(exit) => Event::Returned(self.exits.get(&addr(exit))?),
            Event::Incomplete(exit) => Event::Incomplete(self.exits.get(&addr(exit))?),
            Event::Decision(decision) => Event::Decision(match decision {
                Decision::Arm { branch, arm } => Decision::Arm {
                    branch: self.branches.get(&addr(branch))?,
                    arm,
                },
                Decision::Case { switch, case } => Decision::Case {
                    switch: self.switches.get(&addr(switch))?,
                    case,
                },
                Decision::NoCase { switch } => Decision::NoCase {
                    switch: self.switches.get(&addr(switch))?,
                },
                Decision::LoopEntered(lp) => Decision::LoopEntered(self.loops.get(&addr(lp))?),
                Decision::LoopSkipped(lp) => Decision::LoopSkipped(self.loops.get(&addr(lp))?),
                Decision::CatchImplicit(t) => Decision::CatchImplicit(self.tries.get(&addr(t))?),
                Decision::CatchExplicit(t) => Decision::CatchExplicit(self.tries.get(&addr(t))?),
            }),
        })
    }

    fn traces(
        &self,
        traces: &[Trace<'ir>],
        built: &EffectContext<'ir>,
        now: &EffectContext<'ir>,
    ) -> Option<Vec<Trace<'ir>>> {
        traces
            .iter()
            .map(|t| t.iter().map(|e| self.event(e, built, now)).collect())
            .collect()
    }

    /// `summary` (built in context `built`) as the summary of the
    /// corresponding call in context `now`; `None` if some event does not
    /// map (the call is then walked).
    fn compact(
        &self,
        summary: &Compact<'ir>,
        built: &EffectContext<'ir>,
        now: &EffectContext<'ir>,
    ) -> Option<Compact<'ir>> {
        Some(Compact {
            normal: self.traces(&summary.normal, built, now)?,
            returned: self.traces(&summary.returned, built, now)?,
            thrown: self.traces(&summary.thrown, built, now)?,
            pruned: summary.pruned,
            merged: summary.merged,
            truncated: summary.truncated,
        })
    }
}

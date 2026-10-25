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

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

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
    let mut walker = Walker {
        include_reads,
        max_paths: max_paths.max(1),
        truncated: false,
        pruned: 0,
        merged: 0,
        ctx: EffectContext::default(),
        fact_uses: HashMap::new(),
        live: HashMap::new(),
    };
    count_fact_variables(&function.body, &mut walker.fact_uses);
    let flow = walker.block(&function.body, vec![Vec::new()]);
    let mut traces = flow.normal;
    traces.extend(flow.returned);
    traces.extend(flow.thrown);
    // `break`/`continue` cannot leave a function body; keep them anyway.
    traces.extend(flow.broke);
    traces.extend(flow.continued);
    Paths {
        traces,
        truncated: walker.truncated,
        pruned: walker.pruned,
        merged: walker.merged,
    }
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
            let mut inside = HashMap::new();
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
        let mut groups: HashMap<Vec<[usize; 5]>, Kept> = HashMap::new();
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

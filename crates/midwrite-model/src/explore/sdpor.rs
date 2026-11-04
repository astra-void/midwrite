//! Phase 10: source-set dynamic partial-order reduction with sleep sets and a
//! state cache whose entries carry a summary of the events below them.
//!
//! The classic search of Phase 9 ([`super::explore_dpor`]) is stateless and
//! has no sleep sets. Profiled on the four-actor team-app workload it ran
//! 984 511 maximal executions that contain 148 distinct Mazurkiewicz traces:
//! every trace was explored about 6 600 times, and 99.96 % of its transitions
//! ran below a state it had already visited (`docs/phase10/BASELINE.md`). Two
//! independent mechanisms remove that work, and this module offers both, one
//! at a time or together ([`SdporConfig`]):
//!
//! * **Sleep sets** (`sleep`): a transition that was explored from a state
//!   and is independent of the next one stays asleep in the successor, so a
//!   commuted ordering of the same trace is not explored again.
//! * **A state cache** (`cache`): a state whose subtree was explored to the
//!   end is not explored again. Plain stateful DFS would be unsound for DPOR,
//!   because the races between the events *above* the revisited state and the
//!   events *below* it were detected on the first visit against a different
//!   prefix. Every cached state therefore stores a *summary*: the set of
//!   `(transition, footprint)` pairs executed anywhere below it. When the
//!   search reaches a cached state again it runs the race detection of each
//!   summarized event against the current trace, exactly as if the event were
//!   enabled there (Yang et al., "Efficient stateful dynamic partial order
//!   reduction", SPIN 2008). The detection ignores the happens-before chains
//!   that pass through the events below the state, so it can only find more
//!   races than the full search would, never fewer.
//!
//! Both modes use the same race detection as the classic search
//! (Flanagan–Godefroid): the last event `e_i` of the trace that is dependent
//! with an enabled transition `u`, and not already ordered before it, is a
//! reversible race, and `u` (or, when `u` is not enabled before `e_i`, every
//! transition enabled there) becomes a backtrack point of the state before
//! `e_i`. On top of it:
//!
//! * **Enabling edges.** The footprint relation was designed to say which
//!   transitions *commute*; the Phase 9 pool-handover bug showed that it can
//!   miss which transitions *enable* each other. Here the happens-before
//!   relation also contains an edge from the last event after which `u`
//!   became enabled (`u` was not enabled in the state before it, and stayed
//!   enabled until now) to `u`, computed from the enabled sets the search
//!   holds anyway. A missing footprint access can no longer hide an enabling
//!   dependency.
//! * The five refinements of `docs/phase9/DYNAMIC-POR.md` (program order per
//!   task, disabled alternatives, a blocked step is dependent with everything,
//!   alternatives of one choice explored together, pool claims).
//!
//! What it preserves is what the classic search preserved, and only that: the
//! set of terminal states (so every final invariant, counterexample
//! existence), stuck states, deadlocks and enabled unsupported behavior. It
//! does not visit every intermediate state, so it is refused with `always`
//! invariants and a context-switch bound, like the classic search
//! ([`super::dpor_inapplicable`]).

use super::*;

/// Which mechanisms are on. At least one should be: with neither this is the
/// classic search plus the enabling edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SdporConfig {
    /// Do not re-explore a state whose subtree was completed.
    pub cache: bool,
    /// Sleep sets.
    pub sleep: bool,
    /// Do not treat the event that enabled a transition as dependent with it
    /// (off by default; on only to measure what the rule contributes).
    pub no_enabling_edges: bool,
    /// Reintroduce defects this search had during development, to test that
    /// the regression fixtures still catch them (`DEFECT_*`; 0 in every real
    /// use).
    pub defects: u8,
}

/// A sleeping member of a choice class did not make its siblings obligations
/// (found on random seed 87: the sibling outcomes of a lost acknowledgment
/// were never explored below a state where the answered outcome slept).
pub const DEFECT_SLEEPING_CHOICE: u8 = 1;
/// The event that enabled a transition was an *ordered* predecessor of it
/// rather than a dependent one (found on shared processes without a pool:
/// `Crash` is enabled by the first visible operation of any task of the
/// process, so no single enabler is a necessary cause).
pub const DEFECT_ENABLER_ORDERS: u8 = 2;
/// Only the last racing event of each enabled transition was handled, as in
/// the classic search (found on a shared process with one crash and no pool:
/// `Crash` is enabled only after a task started, so the earlier race is the
/// one whose reversal runs the other task first).
pub const DEFECT_LAST_RACE_ONLY: u8 = 4;

impl SdporConfig {
    pub const STATEFUL: Self = Self {
        cache: true,
        sleep: false,
        no_enabling_edges: false,
        defects: 0,
    };
    pub const STATELESS_SLEEP: Self = Self {
        cache: false,
        sleep: true,
        no_enabling_edges: false,
        defects: 0,
    };
    pub const STATEFUL_SLEEP: Self = Self {
        cache: true,
        sleep: true,
        no_enabling_edges: false,
        defects: 0,
    };
    pub const STATELESS: Self = Self {
        cache: false,
        sleep: false,
        no_enabling_edges: false,
        defects: 0,
    };
}

/// A transition together with the footprint it had when it was executed.
struct Descriptors {
    by_transition: HashMap<Transition, Vec<(Footprint, u32)>>,
    all: Vec<(Transition, Footprint)>,
}

impl Descriptors {
    fn id(&mut self, t: Transition, foot: &Footprint) -> u32 {
        let bucket = self.by_transition.entry(t).or_default();
        if let Some((_, id)) = bucket.iter().find(|(f, _)| f.acc == foot.acc) {
            return *id;
        }
        let id = self.all.len() as u32;
        self.all.push((t, foot.clone()));
        bucket.push((foot.clone(), id));
        id
    }
}

/// Interned summaries: sorted, duplicate-free lists of descriptor ids.
#[derive(Default)]
struct SummaryPool {
    sets: Vec<Box<[u32]>>,
    index: HashMap<Box<[u32]>, u32>,
}

impl SummaryPool {
    fn intern(&mut self, sorted: &[u32]) -> u32 {
        if let Some(id) = self.index.get(sorted) {
            return *id;
        }
        let id = self.sets.len() as u32;
        let boxed: Box<[u32]> = sorted.into();
        self.sets.push(boxed.clone());
        self.index.insert(boxed, id);
        id
    }

    /// An estimate: each set is held twice (the list and the index key),
    /// plus a fixed overhead per allocation.
    fn memory_bytes(&self) -> u64 {
        self.sets
            .iter()
            .map(|s| s.len() as u64 * 4 + 32)
            .sum::<u64>()
            * 2
    }
}

const INCOMPLETE: u32 = u32::MAX;

/// Per visited state (cache mode).
#[derive(Clone)]
struct NodeInfo {
    /// The interned summary of a completed exploration, or [`INCOMPLETE`].
    summary: u32,
    /// `State::steps` of the visit that was explored: a later visit that
    /// reaches the state with fewer steps has more room below the step bound
    /// and explores again.
    steps: u32,
    /// The transitions the completed exploration left asleep (sorted): it is
    /// complete for every transition not in this set, so a later visit may
    /// reuse it only if its own sleep set contains all of them.
    sleep: Box<[Transition]>,
}

impl NodeInfo {
    fn incomplete() -> Self {
        Self {
            summary: INCOMPLETE,
            steps: u32::MAX,
            sleep: Box::new([]),
        }
    }
}

struct SFrame {
    node: u32,
    steps: u32,
    /// Every enabled transition applied once.
    succ: Vec<Succ>,
    backtrack: BTreeSet<Transition>,
    done: BTreeSet<Transition>,
    sleep: Vec<Transition>,
    fresh: bool,
    /// Descriptors executed below this frame so far (cache mode).
    summary: BTreeSet<u32>,
}

struct Sdpor {
    cfg: SdporConfig,
    descs: Descriptors,
    pool: SummaryPool,
    info: Vec<NodeInfo>,
    /// `SDPOR_TRACE` set: print each executed transition with the frame's
    /// sets on stderr (a debugging aid; read once per search).
    trace: bool,
}

pub fn explore_sdpor(model: &Model, opts: &ExploreOptions, cfg: SdporConfig) -> ExploreResult {
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
    let mut st = Sdpor {
        cfg,
        descs: Descriptors {
            by_transition: HashMap::new(),
            all: Vec::new(),
        },
        pool: SummaryPool::default(),
        info: Vec::new(),
        trace: std::env::var_os("SDPOR_TRACE").is_some(),
    };
    let init = initial_state(model);
    let mut stack: Vec<SFrame> = Vec::new();
    let mut trace: Vec<TraceEvent> = Vec::new();
    let mut path: Vec<Transition> = Vec::new();
    let root = st.node_of(&mut ctx, &init).0;
    let init_enabled = enabled(model, &init);
    match st.open(&mut ctx, root, &init, &init_enabled, Vec::new(), &path) {
        Some(f) => stack.push(f),
        None => st.complete_leaf(root, &init),
    }
    ctx.metrics.dpor_max_frames = ctx.metrics.dpor_max_frames.max(stack.len() as u64);
    while !stack.is_empty() {
        if ctx.limits_hit() || (ctx.counterexample.is_some() && opts.stop_at_first) {
            break;
        }
        let depth = stack.len() - 1;
        if stack[depth].fresh {
            stack[depth].fresh = false;
            st.races(&mut ctx, &mut stack, &trace);
            let f = &mut stack[depth];
            if f.backtrack.is_empty()
                && let Some(first) = f
                    .succ
                    .iter()
                    .map(|s| s.t)
                    .find(|t| !f.sleep.contains(t) && !f.done.contains(t))
            {
                f.backtrack.insert(first);
            }
        }
        let f = &mut stack[depth];
        // Alternatives of one choice are explored together. A member that
        // sleeps counts too: sleeping says the outcome it stands for is
        // covered elsewhere, not that its sibling outcomes are.
        let sleeping: &[Transition] = if st.cfg.defects & DEFECT_SLEEPING_CHOICE != 0 {
            &[]
        } else {
            &f.sleep
        };
        let classes: Vec<(u8, u16, u16)> = f
            .backtrack
            .iter()
            .chain(sleeping.iter())
            .filter_map(choice_class)
            .collect();
        for s in &f.succ {
            if choice_class(&s.t).is_some_and(|c| classes.contains(&c))
                && !f.done.contains(&s.t)
                && !f.sleep.contains(&s.t)
                && f.backtrack.insert(s.t)
            {
                ctx.metrics.dpor_backtracks += 1;
            }
        }
        let next = f
            .backtrack
            .iter()
            .find(|t| !f.done.contains(t) && !f.sleep.contains(t))
            .copied();
        let Some(t) = next else {
            let done = stack.pop().expect("non-empty");
            if st.cfg.sleep {
                // Pending backtrack points that sleep: covered elsewhere.
                ctx.metrics.sleep_pruned_transitions += done
                    .backtrack
                    .iter()
                    .filter(|t| done.sleep.contains(t) && !done.done.contains(t))
                    .count() as u64;
            }
            st.finish_frame(&mut stack, done);
            if depth > 0 {
                trace.pop();
                path.pop();
            }
            continue;
        };
        let Some(i) = f.succ.iter().position(|s| s.t == t) else {
            f.done.insert(t);
            continue;
        };
        // Sleep set of the child: what sleeps here or was explored before
        // `t` from this state, and is independent of `t`.
        let child_sleep: Vec<Transition> = if st.cfg.sleep {
            let mut out: Vec<Transition> = Vec::new();
            for u in f.sleep.iter().chain(f.done.iter()) {
                if *u == t {
                    continue;
                }
                if let Some(s) = f.succ.iter().find(|s| s.t == *u)
                    && !dependent(model, &s.foot, &f.succ[i].foot)
                {
                    out.push(*u);
                }
            }
            out.sort();
            out.dedup();
            out
        } else {
            Vec::new()
        };
        f.done.insert(t);
        if st.trace {
            let short = |t: &Transition| {
                let k = match t.kind {
                    TKind::Step => "S",
                    TKind::Resume => "R",
                    TKind::Crash => "X",
                    TKind::Retry => "T",
                    TKind::Deadlock => "D",
                    TKind::LostCommitted => "LC",
                    TKind::LostNotCommitted => "LN",
                    TKind::CrashInFlight => "XF",
                    TKind::ServerComplete => "SC",
                    TKind::Replicate => "RP",
                };
                format!("{k}{}.{}", t.actor, t.thread)
            };
            let list = |v: &mut dyn Iterator<Item = &Transition>| {
                v.map(short).collect::<Vec<_>>().join(",")
            };
            eprintln!(
                "{}{}  enabled=[{}] sleep=[{}] backtrack=[{}] done=[{}] child_sleep=[{}]",
                "  ".repeat(depth),
                short(&t),
                f.succ
                    .iter()
                    .map(|s| short(&s.t))
                    .collect::<Vec<_>>()
                    .join(","),
                list(&mut f.sleep.iter()),
                list(&mut f.backtrack.iter()),
                list(&mut f.done.iter()),
                list(&mut child_sleep.iter())
            );
        }
        let foot = f.succ[i].foot.clone();
        let Some(child) = f.succ[i].state.take() else {
            continue; // unsupported transition: recorded, no successor
        };
        ctx.metrics.transitions += 1;
        ctx.metrics.max_depth = ctx.metrics.max_depth.max(depth as u32 + 1);
        let frame_node = f.node;
        if st.cfg.cache {
            let d = st.descs.id(t, &foot);
            stack[depth].summary.insert(d);
        }
        // Happens-before of the new event: dependent events, the earlier
        // events of its task and the event that enabled it, closed
        // transitively.
        let enabler = if st.cfg.no_enabling_edges {
            None
        } else {
            enabling_event(&stack, depth, &t)
        };
        let program = program_actor(&t);
        let before = predecessors(
            model,
            &trace,
            &foot,
            program,
            enabler,
            st.cfg.defects & DEFECT_ENABLER_ORDERS != 0,
        )
        .full;
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
        // alternative at this state.
        let frame = &mut stack[depth];
        let disabled: Vec<Transition> = frame
            .succ
            .iter()
            .map(|s| s.t)
            .filter(|u| *u != t && !child_enabled.contains(u))
            .collect();
        for u in disabled {
            if !frame.done.contains(&u) && !frame.sleep.contains(&u) && frame.backtrack.insert(u) {
                ctx.metrics.dpor_backtracks += 1;
            }
        }
        // The state cache.
        let (id, is_new) = st.node_of(&mut ctx, &child);
        if st.cfg.cache {
            ctx.children.add(frame_node, id);
        }
        if !is_new {
            ctx.metrics.duplicate_state_hits += 1;
            let info = &st.info[id as usize];
            if st.cfg.cache
                && info.summary != INCOMPLETE
                && info.steps <= child.steps
                && info.sleep.iter().all(|u| child_sleep.contains(u))
            {
                let sid = info.summary;
                st.summary_races(&mut ctx, &mut stack, &trace, sid);
                let members: Vec<u32> = st.pool.sets[sid as usize].to_vec();
                stack[depth].summary.extend(members);
                trace.pop();
                path.pop();
                continue;
            }
        }
        match st.open(&mut ctx, id, &child, &child_enabled, child_sleep, &path) {
            Some(frame) => {
                stack.push(frame);
                ctx.metrics.dpor_max_frames = ctx.metrics.dpor_max_frames.max(stack.len() as u64);
            }
            None => {
                st.complete_leaf(id, &child);
                trace.pop();
                path.pop();
            }
        }
    }
    if st.cfg.cache {
        ctx.metrics.dpor_summary_bytes = st.pool.memory_bytes()
            + st.descs
                .all
                .iter()
                .map(|(_, f)| 64 + f.acc.len() as u64 * 96)
                .sum::<u64>();
        ctx.metrics.dpor_summaries = st.pool.sets.len() as u64;
        ctx.metrics.dpor_descriptors = st.descs.all.len() as u64;
    }
    ctx.finish(true)
}

impl Sdpor {
    /// The table id of `state` and whether it is new. Without the cache
    /// every visit is "new" and ids are meaningless.
    fn node_of(&mut self, ctx: &mut Ctx<'_>, state: &State) -> (u32, bool) {
        if !self.cfg.cache {
            ctx.metrics.states += 1;
            self.info.clear();
            self.info.push(NodeInfo::incomplete());
            return (0, true);
        }
        let (id, new) = ctx.intern(state);
        if new {
            self.info.push(NodeInfo::incomplete());
        }
        (id, new)
    }

    fn complete_leaf(&mut self, node: u32, state: &State) {
        if !self.cfg.cache {
            return;
        }
        let empty = self.pool.intern(&[]);
        let info = &mut self.info[node as usize];
        if info.summary == INCOMPLETE || state.steps < info.steps {
            *info = NodeInfo {
                summary: empty,
                steps: state.steps,
                sleep: Box::new([]),
            };
        }
    }

    /// A frame ran out of transitions: its subtree is complete. Store the
    /// summary and pass it to the parent.
    fn finish_frame(&mut self, stack: &mut [SFrame], done: SFrame) {
        if !self.cfg.cache {
            return;
        }
        let mut members: BTreeSet<u32> = done.summary;
        let old = &self.info[done.node as usize];
        // The union of two explorations covers what either covers: it
        // leaves asleep only what both left asleep.
        let sleep: Box<[Transition]> = if old.summary == INCOMPLETE {
            done.sleep.clone().into_boxed_slice()
        } else {
            members.extend(self.pool.sets[old.summary as usize].iter().copied());
            old.sleep
                .iter()
                .filter(|u| done.sleep.contains(u))
                .copied()
                .collect()
        };
        let steps = done.steps.min(old.steps);
        let sorted: Vec<u32> = members.iter().copied().collect();
        let sid = self.pool.intern(&sorted);
        self.info[done.node as usize] = NodeInfo {
            summary: sid,
            steps,
            sleep,
        };
        if let Some(parent) = stack.last_mut() {
            parent.summary.extend(members);
        }
    }

    /// Create the frame for a state about to be expanded, or `None` for a
    /// leaf. Terminal states have their final invariants checked here (once
    /// per distinct state in cache mode).
    fn open(
        &mut self,
        ctx: &mut Ctx<'_>,
        node: u32,
        state: &State,
        en: &[Transition],
        sleep: Vec<Transition>,
        path: &[Transition],
    ) -> Option<SFrame> {
        let model = ctx.model;
        if is_quiescent(model, en) {
            let first = !self.cfg.cache || !ctx.terminal[node as usize];
            if first {
                if self.cfg.cache {
                    ctx.terminal[node as usize] = true;
                }
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
        // A statement that starts waiting for a lock has not done its work
        // yet: the footprint of the step does not show what the completed
        // statement conflicts with in another order. It is dependent with
        // everything (a separate `Resume` completes it).
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
        Some(SFrame {
            node,
            steps: state.steps,
            succ,
            backtrack: BTreeSet::new(),
            done: BTreeSet::new(),
            sleep,
            fresh: true,
            summary: BTreeSet::new(),
        })
    }

    /// Race detection at the top frame: every enabled transition against the
    /// trace.
    fn races(&mut self, ctx: &mut Ctx<'_>, stack: &mut [SFrame], trace: &[TraceEvent]) {
        let model = ctx.model;
        let top = stack.len() - 1;
        debug_assert_eq!(trace.len(), top);
        let mut adds: Vec<(usize, Vec<Transition>)> = Vec::new();
        for u in &stack[top].succ {
            let enabler = if self.cfg.no_enabling_edges {
                None
            } else {
                enabling_event(stack, top, &u.t)
            };
            let preds = predecessors(
                model,
                trace,
                &u.foot,
                program_actor(&u.t),
                enabler,
                self.cfg.defects & DEFECT_ENABLER_ORDERS != 0,
            );
            // Every racing event, not just the last (Abdulla et al.): a
            // transition that is enabled conditionally (`Crash` needs a
            // running task that has done something) can race with an earlier
            // event whose reversal is not the reversal of the last one.
            let racing: Vec<usize> = if self.cfg.defects & DEFECT_LAST_RACE_ONLY != 0 {
                preds.races.last().copied().into_iter().collect()
            } else {
                preds.races.clone()
            };
            for i in racing {
                ctx.metrics.dpor_races += 1;
                // Source set: the initials of `notdep(e_i).u` -- the events
                // after `e_i` that do not happen after it, then `u` -- are
                // the transitions that can start a reordering with `u`
                // before `e_i`.
                let mut candidates: Vec<Transition> = Vec::new();
                let after_i: Vec<usize> = (i + 1..trace.len())
                    .filter(|&k| !trace[k].before.get(i))
                    .collect();
                for &k in &after_i {
                    if !after_i.iter().any(|&j| j < k && trace[k].before.get(j))
                        && !candidates.contains(&trace[k].t)
                    {
                        candidates.push(trace[k].t);
                    }
                }
                if !after_i.iter().any(|&j| preds.full.get(j)) && !candidates.contains(&u.t) {
                    candidates.push(u.t);
                }
                adds.push((i, candidates));
            }
        }
        self.add_backtracks(ctx, stack, adds);
    }

    /// Race detection of the events below a cached state against the trace
    /// that now leads to it.
    fn summary_races(
        &mut self,
        ctx: &mut Ctx<'_>,
        stack: &mut [SFrame],
        trace: &[TraceEvent],
        summary: u32,
    ) {
        let model = ctx.model;
        let mut adds: Vec<(usize, Vec<Transition>)> = Vec::new();
        for &d in self.pool.sets[summary as usize].iter() {
            let (t, foot) = &self.descs.all[d as usize];
            ctx.metrics.dpor_summary_checks += 1;
            let preds = predecessors(model, trace, foot, program_actor(t), None, false);
            for &i in &preds.races {
                ctx.metrics.dpor_races += 1;
                // The events below the cached state are not in the trace, so
                // the initials of the reordered sequence are not known. The
                // classic choice is sound: the racing transition itself and
                // the tasks of the events that happen before it after `e_i`
                // (the set `E` of Flanagan and Godefroid).
                let mut candidates: Vec<Transition> = vec![*t];
                for (k, e) in trace.iter().enumerate().skip(i + 1) {
                    if preds.full.get(k) && !candidates.contains(&e.t) {
                        candidates.push(e.t);
                    }
                }
                adds.push((i, candidates));
            }
        }
        self.add_backtracks(ctx, stack, adds);
    }

    /// Add, for each race, one transition of its candidate set to the
    /// backtrack set of the state before the earlier event, unless the set
    /// already contains a candidate. Candidates that sleep there are covered
    /// by the exploration that put them to sleep; candidates that are not
    /// enabled there cannot start the reordering. When none is enabled, every
    /// transition enabled there that does not sleep is an alternative.
    fn add_backtracks(
        &mut self,
        ctx: &mut Ctx<'_>,
        stack: &mut [SFrame],
        adds: Vec<(usize, Vec<Transition>)>,
    ) {
        for (i, candidates) in adds {
            let frame = &mut stack[i];
            let enabled_here: Vec<Transition> = candidates
                .into_iter()
                .filter(|c| frame.succ.iter().any(|s| s.t == *c))
                .collect();
            let chosen: Vec<Transition> = if enabled_here.is_empty() {
                frame.succ.iter().map(|s| s.t).collect()
            } else if enabled_here.iter().any(|c| frame.backtrack.contains(c)) {
                continue;
            } else {
                vec![enabled_here[0]]
            };
            let awake: Vec<Transition> = chosen
                .into_iter()
                .filter(|c| !frame.sleep.contains(c))
                .collect();
            if enabled_here.len() > 1
                && awake.len() == 1
                && enabled_here.iter().any(|c| !frame.sleep.contains(c))
            {
                // Prefer a candidate that is not asleep.
                if let Some(c) = enabled_here.iter().find(|c| !frame.sleep.contains(c)) {
                    if frame.backtrack.insert(*c) {
                        ctx.metrics.dpor_backtracks += 1;
                    }
                    continue;
                }
            }
            for c in awake {
                if !frame.done.contains(&c) && frame.backtrack.insert(c) {
                    ctx.metrics.dpor_backtracks += 1;
                }
            }
        }
    }
}

/// The event after which `t` became enabled and stayed enabled up to the
/// state of frame `at`: the last frame `k < at` whose state does not enable
/// `t`. `None` when `t` was enabled in every state of the trace.
fn enabling_event(stack: &[SFrame], at: usize, t: &Transition) -> Option<usize> {
    (0..at)
        .rev()
        .find(|&k| !stack[k].succ.iter().any(|s| s.t == *t))
}

/// The happens-before predecessors of a transition about to extend `trace`.
struct Predecessors {
    /// Every event that happens before it: the events dependent with it, the
    /// earlier events of its task and the event that enabled it, and
    /// everything that happens before those.
    full: Bits,
    /// The events that race with it, in trace order: dependent with it, and
    /// ordered before it by nothing but that dependence (not through another
    /// predecessor, not by program order).
    races: Vec<usize>,
}

fn predecessors(
    model: &Model,
    trace: &[TraceEvent],
    foot: &Footprint,
    program: Option<(u16, u16)>,
    enabler: Option<usize>,
    enabler_orders: bool,
) -> Predecessors {
    let mut deps: Vec<usize> = Vec::new();
    let mut direct = Bits::default();
    let mut ordered = Bits::default();
    for (k, e) in trace.iter().enumerate() {
        let same_task = program.is_some() && e.program == program;
        // The event that enabled the transition is dependent with it, whatever
        // the footprints say: an enabling pair does not commute. It is a race
        // like any other dependent pair (the transition was not enabled
        // before it, so a reordering needs another way to get there), never
        // an ordered one: a transition can have several independent ways to
        // become enabled (`Crash` after the first visible operation of any
        // task of the process).
        let dep = dependent(model, &e.foot, foot) || (!enabler_orders && enabler == Some(k));
        if dep {
            deps.push(k);
        }
        if dep || same_task || (enabler_orders && enabler == Some(k)) {
            direct.set(k);
            ordered.or(&e.before);
        }
    }
    if enabler_orders && let Some(k) = enabler {
        ordered.set(k);
    }
    let mut full = ordered.clone();
    full.or(&direct);
    // A dependent event is a race unless something else orders it before the
    // transition (another dependent event, or the program order of its task).
    let races: Vec<usize> = deps
        .iter()
        .copied()
        .filter(|&k| {
            !ordered.get(k)
                && !(program.is_some() && trace[k].program == program)
                && !(enabler_orders && enabler == Some(k))
        })
        .collect();
    Predecessors { full, races }
}

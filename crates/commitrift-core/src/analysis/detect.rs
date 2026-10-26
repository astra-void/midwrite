//! Hazard detection on enumerated traces.
//!
//! Each trace is replayed with a model of transaction scopes. The replay
//! yields the trace's *points*: the moments at which something becomes
//! externally durable or visible. These are
//!
//! * a database write outside any transaction (it commits on its own),
//! * the successful end of a root transaction that staged at least one write,
//! * an external effect.
//!
//! A failure between two adjacent points leaves the first done and the second
//! not, so every adjacent pair is a candidate hazard. External effects that
//! happen while a root transaction with writes is open are candidates for
//! CP1003 instead.
//!
//! Operations inlined from the summary of a same-file function are treated
//! like the caller's own, except that a hazard entirely inside one inlined
//! call is left to the callee's own analysis, and an operation the callee
//! performs only on some of its paths lowers confidence.

use std::collections::BTreeMap;

use crate::diagnostic::{Confidence, DiagnosticCode, PathCertainty, Policy};
use crate::ir::{
    CallId, DbHandle, Effect, EffectId, EffectKind, Execution, Exit, FunctionIr, SummaryOrigin,
    TransactionId,
};
use crate::location::{Location, Sources};

use super::AnalysisOptions;
use super::paths::{Decision, EffectContext, Event, Trace};

/// An effect together with its position on a trace.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Occurrence<'ir> {
    pub at: usize,
    pub effect: &'ir Effect,
    pub ctx: EffectContext<'ir>,
}

#[derive(Clone, Debug)]
pub(crate) enum Point<'ir> {
    /// Database state becomes durable.
    Commit {
        /// Index of the event at which the commit happens.
        at: usize,
        /// The root transaction committing, or `None` for an auto-commit write.
        transaction: Option<TransactionId>,
        writes: Vec<Occurrence<'ir>>,
        /// For an auto-commit write issued while this transaction was open
        /// (it bypasses the transaction).
        bypassed: Option<TransactionId>,
        /// A transaction handle used after its transaction ended.
        escaped: bool,
    },
    /// An external effect is performed.
    External {
        occurrence: Occurrence<'ir>,
        /// Root transactions open while it runs (outermost first).
        inside: Vec<TransactionId>,
    },
}

impl<'ir> Point<'ir> {
    pub fn at(&self) -> usize {
        match self {
            Self::Commit { at, .. } => *at,
            Self::External { occurrence, .. } => occurrence.at,
        }
    }

    fn key(&self) -> PointKey {
        match self {
            Self::Commit {
                transaction: Some(id),
                ..
            } => PointKey::Transaction(*id),
            Self::Commit { writes, .. } => PointKey::Effect(writes[0].effect.id),
            Self::External { occurrence, .. } => PointKey::Effect(occurrence.effect.id),
        }
    }

    /// The outermost inlined call the point comes from, if any.
    pub fn call(&self, function: &FunctionIr) -> Option<CallId> {
        match self {
            Self::Commit {
                transaction: Some(id),
                ..
            } => transaction_call(function, *id),
            Self::Commit { writes, .. } => effect_call(writes[0].effect),
            Self::External { occurrence, .. } => effect_call(occurrence.effect),
        }
    }

    /// Effects whose execution properties bear on the point.
    pub fn occurrences(&self) -> &[Occurrence<'ir>] {
        match self {
            // A transaction commits when its callback completes; the
            // properties of individual staged writes do not move the commit.
            Self::Commit {
                transaction: Some(_),
                ..
            } => &[],
            Self::Commit { writes, .. } => writes,
            Self::External { occurrence, .. } => std::slice::from_ref(occurrence),
        }
    }
}

/// The outermost inlined call an effect comes from.
fn effect_call(effect: &Effect) -> Option<CallId> {
    effect.call().map(|site| site.call)
}

/// The outermost inlined call a transaction is started in.
fn transaction_call(function: &FunctionIr, id: TransactionId) -> Option<CallId> {
    function
        .transaction(id)
        .summary
        .as_ref()
        .and_then(SummaryOrigin::outermost)
        .map(|site| site.call)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PointKey {
    Effect(EffectId),
    Transaction(TransactionId),
}

#[derive(Clone, Debug)]
pub(crate) enum Shape<'ir> {
    /// A failure between two adjacent points.
    Sequence {
        first: Point<'ir>,
        second: Point<'ir>,
    },
    /// An external effect while a root transaction with writes is open.
    InsideTransaction {
        external: Occurrence<'ir>,
        transaction: TransactionId,
        writes: Vec<Occurrence<'ir>>,
        /// `Some(false)` when the transaction rolls back on this trace.
        committed: Option<bool>,
    },
    /// An operation whose semantics are unknown, near recognized effects.
    Unknown {
        occurrence: Occurrence<'ir>,
        preceding: Option<Point<'ir>>,
        following: Option<Point<'ir>>,
        inside: Option<TransactionId>,
        /// Other unknown operations of the same inlined call that are
        /// reported together with this one (one diagnostic per call).
        siblings: Vec<&'ir Effect>,
    },
    /// The path reached unsupported control flow.
    Incomplete(&'ir Exit),
}

/// A candidate diagnostic found on one trace.
#[derive(Clone, Debug)]
pub(crate) struct Finding<'ir> {
    pub code: DiagnosticCode,
    pub shape: Shape<'ir>,
    pub confidence: Confidence,
    pub certainty: PathCertainty,
    pub conditions: Vec<Decision<'ir>>,
    pub notes: Vec<String>,
    /// For a sequence: the other operations that take the place of the
    /// non-primary point on other paths (the same hazard reached
    /// differently). Reported as one diagnostic.
    pub alternatives: Vec<Point<'ir>>,
    /// Index of the trace (path) the finding was found on.
    pub trace: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Sequence(DiagnosticCode, PointKey, PointKey),
    Inside(EffectId, TransactionId),
    Unknown(EffectId),
    /// Unknown operations inside one inlined call, reported once per call.
    UnknownInCall(CallId),
    /// Under the recommended policy, every unrecognized call of the function
    /// that is reported only because of its context, reported once.
    UnknownCalls,
    Incomplete(Location),
}

/// Find the best finding for every distinct hazard across all traces.
pub(crate) fn detect<'ir>(
    function: &'ir FunctionIr,
    traces: &[Trace<'ir>],
    options: &AnalysisOptions,
    src: &Sources<'_>,
) -> Vec<Finding<'ir>> {
    let has_recognized = function.has_recognized_effects();
    let executions = transaction_executions(function);
    let mut best: BTreeMap<Key, (Rank, Finding<'ir>)> = BTreeMap::new();
    // Every relevant unknown operation of each group, in order.
    let mut grouped_unknowns: BTreeMap<Key, Vec<&'ir Effect>> = BTreeMap::new();
    let mut offer = |key: Key, rank: Rank, finding: Finding<'ir>| match best.get(&key) {
        Some((existing, _)) if *existing <= rank => {}
        _ => {
            best.insert(key, (rank, finding));
        }
    };

    for (trace_index, trace) in traces.iter().enumerate() {
        let facts = replay(function, trace);

        for pair in facts.points.windows(2) {
            let [first, second] = pair else { continue };
            let code = match (first, second) {
                (
                    Point::External { inside, .. },
                    Point::Commit {
                        transaction: Some(tx),
                        ..
                    },
                ) if inside.contains(tx) => continue, // reported as CP1003
                (Point::Commit { .. }, Point::External { .. }) => {
                    DiagnosticCode::DbCommitThenExternal
                }
                (Point::External { .. }, Point::Commit { .. }) => {
                    DiagnosticCode::ExternalThenDbCommit
                }
                (Point::Commit { .. }, Point::Commit { .. }) => {
                    DiagnosticCode::IndependentDbCommits
                }
                (Point::External { .. }, Point::External { .. }) => {
                    DiagnosticCode::IndependentExternalEffects
                }
            };
            let call = first.call(function);
            if call.is_some() && call == second.call(function) {
                // Both happen inside one inlined call: the callee's own
                // analysis reports this pair.
                continue;
            }
            let mut occurrences = first.occurrences().to_vec();
            occurrences.extend_from_slice(second.occurrences());
            let (mut confidence, mut certainty, conditions, mut notes) =
                assess(trace, first.at(), second.at(), &occurrences, src);
            for point in [first, second] {
                if let Some(note) = partial_commit(function, point, src) {
                    confidence = confidence.min(Confidence::Medium);
                    certainty = certainty.max(PathCertainty::Conditional);
                    notes.push(note);
                }
                // A transaction that is not awaited commits at an
                // undetermined time relative to later operations.
                if let Point::Commit {
                    transaction: Some(id),
                    ..
                } = point
                    && let Some(execution) = executions.get(id).copied()
                    && !execution.is_ordered()
                {
                    let info = function.transaction(*id);
                    let (cap, kind) = if execution == Execution::Concurrent {
                        (Confidence::Low, PathCertainty::Approximated)
                    } else {
                        (Confidence::Medium, PathCertainty::Conditional)
                    };
                    confidence = confidence.min(cap);
                    certainty = certainty.max(kind);
                    notes.push(format!(
                        "The transaction `{}` ({}) is not awaited: it may commit after later operations, and its failure may go unobserved.",
                        info.label,
                        src.at(info.location)
                    ));
                }
            }
            let key = Key::Sequence(code, first.key(), second.key());
            offer(
                key,
                Rank::new(confidence, &conditions, trace_index),
                Finding {
                    code,
                    shape: Shape::Sequence {
                        first: first.clone(),
                        second: second.clone(),
                    },
                    confidence,
                    certainty,
                    conditions,
                    notes,
                    alternatives: Vec::new(),
                    trace: trace_index,
                },
            );
        }

        for point in &facts.points {
            let Point::External { occurrence, inside } = point else {
                continue;
            };
            for &transaction in inside {
                let call = effect_call(occurrence.effect);
                if call.is_some() && call == transaction_call(function, transaction) {
                    continue; // reported by the callee's own analysis
                }
                let Some(writes) = facts.all_staged.get(&transaction) else {
                    continue;
                };
                if writes.is_empty() {
                    continue;
                }
                // The window spans the external effect and the nearest write.
                let window = match writes.iter().rev().find(|w| w.at < occurrence.at) {
                    Some(before) => (before.at, occurrence.at),
                    None => (occurrence.at, writes[0].at),
                };
                let (confidence, certainty, conditions, notes) = assess(
                    trace,
                    window.0,
                    window.1,
                    std::slice::from_ref(occurrence),
                    src,
                );
                offer(
                    Key::Inside(occurrence.effect.id, transaction),
                    Rank::new(confidence, &conditions, trace_index),
                    Finding {
                        code: DiagnosticCode::ExternalInsideTransaction,
                        shape: Shape::InsideTransaction {
                            external: *occurrence,
                            transaction,
                            writes: writes.clone(),
                            committed: facts.ended.get(&transaction).copied(),
                        },
                        confidence,
                        certainty,
                        conditions,
                        notes,
                        alternatives: Vec::new(),
                        trace: trace_index,
                    },
                );
            }
        }

        for &(occurrence, inside) in &facts.unknowns {
            let EffectKind::Unknown { reason, .. } = &occurrence.effect.kind else {
                continue;
            };
            // Inside an inlined call, only the caller's context counts: when
            // the callee itself did something durable or visible first, the
            // callee's own analysis reports the unknown call.
            let call = effect_call(occurrence.effect);
            let outside = |point: &Point<'ir>| call.is_none() || point.call(function) != call;
            let preceding = facts
                .points
                .iter()
                .rev()
                .find(|p| p.at() < occurrence.at)
                .filter(|p| outside(p))
                .cloned();
            let following = facts
                .points
                .iter()
                .find(|p| p.at() > occurrence.at && outside(p))
                .cloned();
            let inside_with_writes =
                inside.is_some_and(|tx| facts.all_staged.get(&tx).is_some_and(|w| !w.is_empty()));
            // An unknown call matters when something durable or visible has
            // already happened (it may be the second half of a partial
            // completion) or when it runs inside a transaction with writes.
            // Unknown calls before the first recognized effect are not
            // reported: they are overwhelmingly reads and validation.
            // The audit policy also reports unknown calls before the first
            // recognized operation: they may be unconfigured effects.
            let leading = options.policy == Policy::Audit && call.is_none() && following.is_some();
            let relevant = (call.is_none() && reason.is_reported_alone())
                || preceding.is_some()
                || inside_with_writes
                || leading;
            if !relevant {
                continue;
            }
            // Prefer the most informative context: something committed before it.
            let informativeness = u8::from(preceding.is_some()) * 2
                + u8::from(following.is_some() || inside.is_some());
            let generic = !reason.is_reported_alone();
            let key = match call {
                _ if generic && options.policy == Policy::Recommended => Key::UnknownCalls,
                Some(call) => Key::UnknownInCall(call),
                None => Key::Unknown(occurrence.effect.id),
            };
            if matches!(key, Key::UnknownCalls | Key::UnknownInCall(_)) {
                let effects = grouped_unknowns.entry(key).or_default();
                if !effects.iter().any(|e| e.id == occurrence.effect.id) {
                    effects.push(occurrence.effect);
                }
            }
            offer(
                key,
                Rank {
                    confidence: Confidence::Low,
                    conditions: usize::from(3 - informativeness),
                    trace: trace_index,
                },
                Finding {
                    code: DiagnosticCode::AnalysisIncomplete,
                    shape: Shape::Unknown {
                        occurrence,
                        preceding,
                        following,
                        inside,
                        siblings: Vec::new(),
                    },
                    confidence: Confidence::Low,
                    certainty: PathCertainty::Unknown,
                    conditions: Vec::new(),
                    notes: Vec::new(),
                    alternatives: Vec::new(),
                    trace: trace_index,
                },
            );
        }

        if has_recognized {
            for &exit in &facts.incomplete {
                offer(
                    Key::Incomplete(exit.location),
                    Rank {
                        confidence: Confidence::Low,
                        conditions: 0,
                        trace: trace_index,
                    },
                    Finding {
                        code: DiagnosticCode::AnalysisIncomplete,
                        shape: Shape::Incomplete(exit),
                        confidence: Confidence::Low,
                        certainty: PathCertainty::Unknown,
                        conditions: Vec::new(),
                        notes: Vec::new(),
                        alternatives: Vec::new(),
                        trace: trace_index,
                    },
                );
            }
        }
    }

    for (key, (_, finding)) in &mut best {
        if let Shape::Unknown {
            occurrence,
            siblings,
            ..
        } = &mut finding.shape
            && let Some(effects) = grouped_unknowns.get(key)
        {
            siblings.extend(
                effects
                    .iter()
                    .filter(|effect| effect.id != occurrence.effect.id)
                    .copied(),
            );
        }
    }
    merge_alternatives(function, best.into_values().collect())
}

/// How each transaction scope of the function is executed.
fn transaction_executions(function: &FunctionIr) -> BTreeMap<TransactionId, Execution> {
    fn visit(nodes: &[crate::ir::Node], out: &mut BTreeMap<TransactionId, Execution>) {
        use crate::ir::Node;
        for node in nodes {
            match node {
                Node::Transaction(scope) => {
                    out.insert(scope.transaction, scope.execution);
                    visit(&scope.body, out);
                }
                Node::Branch(branch) => branch.arms.iter().for_each(|arm| visit(&arm.body, out)),
                Node::Switch(switch) => switch.cases.iter().for_each(|case| visit(&case.body, out)),
                Node::Loop(lp) => visit(&lp.body, out),
                Node::Call(call) => visit(&call.body, out),
                Node::Try(try_node) => {
                    visit(&try_node.block, out);
                    if let Some(handler) = &try_node.handler {
                        visit(&handler.body, out);
                    }
                    if let Some(finalizer) = &try_node.finalizer {
                        visit(finalizer, out);
                    }
                }
                Node::Effect(_) | Node::Exit(_) => {}
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(&function.body, &mut out);
    out
}

/// Where a point is reported in the analyzed function (the operation, or
/// the outermost call it was inlined through) and its label.
fn point_site(function: &FunctionIr, point: &Point<'_>) -> (Location, String) {
    match point {
        Point::Commit {
            transaction: Some(id),
            ..
        } => {
            let info = function.transaction(*id);
            let location = info
                .summary
                .as_ref()
                .and_then(SummaryOrigin::outermost)
                .map_or(info.location, |site| site.location);
            (location, info.label.clone())
        }
        Point::Commit { writes, .. } => site_of(writes[0].effect),
        Point::External { occurrence, .. } => site_of(occurrence.effect),
    }
}

fn site_of(effect: &Effect) -> (Location, String) {
    (
        effect.call().map_or(effect.location, |site| site.location),
        effect.label.to_string(),
    )
}

/// One finding per sequence hazard and primary operation: when the same
/// operation (for CP1001 the external effect that may not happen, for
/// CP1002 the external effect performed first, ...) pairs with different
/// operations on different paths, the best-ranked pair is reported and the
/// others are listed as alternatives.
///
/// Primary operations are grouped by where they are reported and their
/// label, so that two identical operations inside one inlined call (two
/// paths of the callee publishing the same event) give one diagnostic at the
/// call.
fn merge_alternatives<'ir>(
    function: &FunctionIr,
    findings: Vec<(Rank, Finding<'ir>)>,
) -> Vec<Finding<'ir>> {
    let mut groups: BTreeMap<(DiagnosticCode, Location, String), usize> = BTreeMap::new();
    let mut out: Vec<(Rank, Finding<'ir>)> = Vec::with_capacity(findings.len());
    for (rank, finding) in findings {
        let Shape::Sequence { first, second } = &finding.shape else {
            out.push((rank, finding));
            continue;
        };
        let (primary, other) = if primary_is_first(finding.code, first) {
            (first, second)
        } else {
            (second, first)
        };
        let (location, label) = point_site(function, primary);
        let key = (finding.code, location, label);
        match groups.get(&key) {
            None => {
                groups.insert(key, out.len());
                out.push((rank, finding));
            }
            Some(&index) => {
                let other = other.clone();
                let (kept_rank, kept) = &mut out[index];
                let site = |finding: &Finding<'ir>| match &finding.shape {
                    Shape::Sequence { first, second } => {
                        let other = if primary_is_first(finding.code, first) {
                            second
                        } else {
                            first
                        };
                        Some(point_site(function, other))
                    }
                    _ => None,
                };
                if site(kept) == site(&finding) {
                    // The same pair as reported, through another identical
                    // operation: nothing to add.
                    if rank < *kept_rank {
                        *kept_rank = rank;
                        let alternatives = std::mem::take(&mut kept.alternatives);
                        *kept = finding;
                        kept.alternatives = alternatives;
                    }
                    continue;
                }
                if rank < *kept_rank {
                    // The new pair is better: it becomes the reported one.
                    let previous = std::mem::replace(kept, finding);
                    *kept_rank = rank;
                    if let Shape::Sequence { first, second } = &previous.shape {
                        let previous_other = if primary_is_first(previous.code, first) {
                            second
                        } else {
                            first
                        };
                        kept.alternatives.push(previous_other.clone());
                    }
                    kept.alternatives.extend(previous.alternatives);
                } else {
                    kept.alternatives.push(other);
                }
            }
        }
    }
    out.into_iter().map(|(_, finding)| finding).collect()
}

/// Whether the diagnostic about a sequence is located at (and is about) its
/// first point rather than its second.
pub(crate) fn primary_is_first(code: DiagnosticCode, first: &Point<'_>) -> bool {
    match code {
        DiagnosticCode::ExternalThenDbCommit => true,
        DiagnosticCode::IndependentDbCommits => matches!(
            first,
            Point::Commit {
                bypassed: Some(_),
                ..
            }
        ),
        _ => false,
    }
}

/// Ordering used to pick the best trace for a hazard: higher confidence,
/// then fewer conditions, then the earliest trace (deterministic).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rank {
    confidence: Confidence,
    conditions: usize,
    trace: usize,
}

impl Rank {
    fn new(confidence: Confidence, conditions: &[Decision<'_>], trace: usize) -> Self {
        Self {
            confidence,
            conditions: conditions.len(),
            trace,
        }
    }
}

impl PartialOrd for Rank {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Rank {
    /// `Less` means better.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .confidence
            .cmp(&self.confidence)
            .then(self.conditions.cmp(&other.conditions))
            .then(self.trace.cmp(&other.trace))
    }
}

/// A note when `point` is the commit of a transaction inlined from a
/// summary whose callee commits it only on some paths.
fn partial_commit(function: &FunctionIr, point: &Point<'_>, src: &Sources<'_>) -> Option<String> {
    let Point::Commit {
        transaction: Some(id),
        ..
    } = point
    else {
        return None;
    };
    let info = function.transaction(*id);
    let summary = info.summary.as_ref()?;
    let site = summary.outermost()?;
    (!summary.on_every_path).then(|| {
        format!(
            "The transaction `{}` comes from the summary of `{}` (called at {}), which commits it only on some of its paths.",
            info.label,
            site.callee,
            src.at(site.location)
        )
    })
}

/// Confidence, path certainty, path conditions and notes for a window
/// `from..=to` of a trace involving `occurrences`.
fn assess<'ir>(
    trace: &Trace<'ir>,
    from: usize,
    to: usize,
    occurrences: &[Occurrence<'ir>],
    src: &Sources<'_>,
) -> (Confidence, PathCertainty, Vec<Decision<'ir>>, Vec<String>) {
    let mut confidence = Confidence::High;
    let mut certainty = PathCertainty::StraightLine;
    let mut notes = Vec::new();
    let (from, to) = (from.min(to), from.max(to));

    let conditions: Vec<Decision<'ir>> = trace[from..=to]
        .iter()
        .filter_map(|event| match event {
            Event::Decision(decision) => Some(*decision),
            _ => None,
        })
        .collect();
    if !conditions.is_empty() {
        confidence = confidence.min(Confidence::Medium);
        certainty = certainty.max(PathCertainty::Conditional);
        notes.push(format!(
            "The sequence depends on {} branch decision(s) between the two operations (see pathConditions). CommitRift only rules out paths whose conditions are simple tests of the same unchanging variable that contradict each other; other conditions are assumed to be satisfiable together.",
            conditions.len()
        ));
    }

    if let Some(catch) = trace[..=to].iter().find_map(|event| match event {
        Event::Decision(Decision::CatchImplicit(try_node)) => Some(try_node),
        _ => None,
    }) {
        confidence = confidence.min(Confidence::Low);
        certainty = certainty.max(PathCertainty::Approximated);
        let handler = src.at(catch
            .handler
            .as_ref()
            .map_or(catch.location, |h| h.location));
        notes.push(format!(
            "This path enters the catch handler at {handler} as if the try block failed before any of its effects; the actual failure point is approximated."
        ));
    }

    for occurrence in occurrences {
        let effect = occurrence.effect;
        match effect.execution {
            Execution::FireAndForget => {
                confidence = confidence.min(Confidence::Medium);
                certainty = certainty.max(PathCertainty::Conditional);
                notes.push(format!(
                    "`{}` is not awaited: it may complete after later operations, and its failure may go unobserved.",
                    effect.label
                ));
            }
            Execution::Concurrent => {
                confidence = confidence.min(Confidence::Low);
                certainty = certainty.max(PathCertainty::Approximated);
                notes.push(format!(
                    "`{}` is started concurrently with other operations; their completion order is not determined.",
                    effect.label
                ));
            }
            Execution::Awaited | Execution::Returned => {}
        }
        if let Some(summary) = &effect.summary
            && !summary.on_every_path
            && let Some(site) = summary.outermost()
        {
            confidence = confidence.min(Confidence::Medium);
            certainty = certainty.max(PathCertainty::Conditional);
            notes.push(format!(
                "`{}` comes from the summary of `{}` (called at {}), which performs it only on some of its paths.",
                effect.label,
                site.callee,
                src.at(site.location)
            ));
        }
        if let Some(summary) = &effect.summary
            && let Some((site, assumption)) = summary.calls.iter().find_map(|site| {
                site.resolution
                    .as_deref()
                    .and_then(|r| r.assumption())
                    .map(|a| (site, a))
            })
        {
            confidence = confidence.min(Confidence::Medium);
            certainty = certainty.max(PathCertainty::Conditional);
            let note = format!(
                "`{}` is reached through `{}` (at {}), whose method target was resolved by assuming {assumption}.",
                effect.label,
                site.label,
                src.at(site.location)
            );
            if !notes.contains(&note) {
                notes.push(note);
            }
        }
        if let Some(lp) = occurrence.ctx.in_loop {
            confidence = confidence.min(Confidence::Medium);
            certainty = certainty.max(PathCertainty::Conditional);
            notes.push(format!(
                "`{}` runs inside a {} loop at {}; only zero or one iteration is modeled.",
                effect.label,
                lp.kind,
                src.at(lp.location)
            ));
        }
        if let Some(guard) = occurrence.ctx.guarded_by {
            let handler = src.at(guard
                .handler
                .as_ref()
                .map_or(guard.location, |h| h.location));
            let note = format!(
                "The operations run inside a try block whose catch handler (at {handler}) may compensate for some failures; CommitRift does not verify compensation, and a process crash bypasses catch handlers."
            );
            if !notes.contains(&note) {
                notes.push(note);
            }
        }
    }
    (confidence, certainty, conditions, notes)
}

/// Facts derived from replaying one trace.
#[derive(Debug, Default)]
pub(super) struct Facts<'ir> {
    pub(super) points: Vec<Point<'ir>>,
    /// Every write staged in each root transaction on this trace, including
    /// writes later discarded by a nested rollback.
    all_staged: BTreeMap<TransactionId, Vec<Occurrence<'ir>>>,
    /// End event index and outcome of each root transaction.
    ended: BTreeMap<TransactionId, bool>,
    /// Unknown effects, with the innermost open root transaction that does
    /// not come from the same inlined call.
    unknowns: Vec<(Occurrence<'ir>, Option<TransactionId>)>,
    incomplete: Vec<&'ir Exit>,
}

pub(super) fn replay<'ir>(function: &'ir FunctionIr, trace: &Trace<'ir>) -> Facts<'ir> {
    let mut facts = Facts::default();
    let mut open: Vec<TransactionId> = Vec::new();
    // Writes that will commit with each open root transaction, with the
    // (possibly nested) transaction that issued them.
    let mut staged: BTreeMap<TransactionId, Vec<(TransactionId, Occurrence<'ir>)>> =
        BTreeMap::new();
    let root_of = |mut id: TransactionId| {
        while let Some(parent) = function.transaction(id).parent {
            id = parent;
        }
        id
    };

    for (at, event) in trace.iter().enumerate() {
        match *event {
            Event::TxBegin(id) => open.push(id),
            Event::TxEnd {
                transaction,
                committed,
            } => {
                if let Some(position) = open.iter().rposition(|&id| id == transaction) {
                    open.truncate(position);
                }
                let root = root_of(transaction);
                if root == transaction {
                    facts.ended.insert(transaction, committed);
                    let writes: Vec<Occurrence<'ir>> = staged
                        .remove(&transaction)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(_, occurrence)| occurrence)
                        .collect();
                    if committed && !writes.is_empty() {
                        facts.points.push(Point::Commit {
                            at,
                            transaction: Some(transaction),
                            writes,
                            bypassed: None,
                            escaped: false,
                        });
                    }
                } else if !committed {
                    // A nested transaction (savepoint) rolled back: discard
                    // the writes it and its descendants issued.
                    if let Some(writes) = staged.get_mut(&root) {
                        writes.retain(|(issuer, _)| !is_within(function, *issuer, transaction));
                    }
                }
            }
            Event::Effect { effect, ctx } => {
                let occurrence = Occurrence { at, effect, ctx };
                let open_roots: Vec<TransactionId> = open
                    .iter()
                    .copied()
                    .filter(|&id| function.transaction(id).parent.is_none())
                    .collect();
                match &effect.kind {
                    EffectKind::DatabaseWrite { handle, .. } => match handle {
                        DbHandle::Transaction { id } if open.contains(id) => {
                            let root = root_of(*id);
                            staged.entry(root).or_default().push((*id, occurrence));
                            facts.all_staged.entry(root).or_default().push(occurrence);
                        }
                        DbHandle::Transaction { .. } => facts.points.push(Point::Commit {
                            at,
                            transaction: None,
                            writes: vec![occurrence],
                            bypassed: None,
                            escaped: true,
                        }),
                        DbHandle::Root { .. } | DbHandle::Parameter { .. } => {
                            facts.points.push(Point::Commit {
                                at,
                                transaction: None,
                                writes: vec![occurrence],
                                bypassed: open_roots.last().copied(),
                                escaped: false,
                            })
                        }
                    },
                    EffectKind::External { .. } => facts.points.push(Point::External {
                        occurrence,
                        inside: open_roots,
                    }),
                    EffectKind::Unknown { .. } => {
                        let call = effect_call(effect);
                        let inside =
                            open_roots.iter().rev().copied().find(|&id| {
                                call.is_none() || transaction_call(function, id) != call
                            });
                        facts.unknowns.push((occurrence, inside));
                    }
                    EffectKind::DatabaseRead { .. } => {}
                }
            }
            Event::Decision(_) | Event::Returned(_) => {}
            Event::Incomplete(exit) => facts.incomplete.push(exit),
        }
    }
    facts
}

/// Whether `id` is `ancestor` or nested (transitively) inside it.
fn is_within(function: &FunctionIr, mut id: TransactionId, ancestor: TransactionId) -> bool {
    loop {
        if id == ancestor {
            return true;
        }
        match function.transaction(id).parent {
            Some(parent) => id = parent,
            None => return false,
        }
    }
}

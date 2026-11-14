//! Builds a [`FailureTrace`] from a finding and the path it was found on.
//!
//! Every event of the path becomes one step, in order; nothing is added
//! that the path does not contain, and no argument value is invented (steps
//! carry the source labels, tables and handles the frontend recorded).

use crate::diagnostic::{Diagnostic, DiagnosticCode, PathCertainty};
use crate::ir::{DbHandle, Effect, EffectKind, ExternalCategory, SummaryOrigin};
use crate::trace::{
    BoundaryKind, CallFrame, CheckpointHint, Completeness, FailureTrace, InjectionPoint, StepKind,
    StepRole, StepState, TraceStep, simulate_crash,
};
use crate::verification::VerificationLevel;

use super::detect::{Finding, Occurrence, Point, Shape};
use super::paths::{Event, Trace};
use super::render::Renderer;

/// A trace before it is attached to its rendered diagnostic.
pub(crate) struct Draft {
    steps: Vec<TraceStep>,
    injection: InjectionPoint,
    completeness: Completeness,
    assumptions: Vec<String>,
    limitations: Vec<String>,
}

/// Why no trace is produced for a diagnostic.
pub(crate) fn skip_reason(code: DiagnosticCode) -> String {
    match code {
        DiagnosticCode::AnalysisIncomplete => {
            "MW9001 reports an analysis limitation, not a failure path".to_owned()
        }
        _ => "the finding has no failure boundary on a single path".to_owned(),
    }
}

impl Renderer<'_> {
    pub(crate) fn draft_trace(
        &self,
        path: &Trace<'_>,
        finding: &Finding<'_>,
        truncated: bool,
    ) -> Result<Draft, String> {
        let mut steps: Vec<TraceStep> = path
            .iter()
            .enumerate()
            .map(|(index, event)| self.step(index, event))
            .collect();

        let (boundary, after, before, after_hint, before_hint) = match &finding.shape {
            Shape::Sequence { first, second } => {
                let boundary = match finding.code {
                    DiagnosticCode::DbCommitThenExternal => BoundaryKind::AfterCommitBeforeExternal,
                    DiagnosticCode::ExternalThenDbCommit => BoundaryKind::AfterExternalBeforeCommit,
                    DiagnosticCode::IndependentDbCommits => BoundaryKind::BetweenCommits,
                    DiagnosticCode::IndependentExternalEffects => {
                        BoundaryKind::BetweenExternalEffects
                    }
                    code => return Err(skip_reason(code)),
                };
                mark_point(&mut steps, first, StepRole::CompletedFirst);
                mark_point(&mut steps, second, StepRole::NotYetPerformed);
                (
                    boundary,
                    first.at(),
                    Some(second.at()),
                    hint_after(first),
                    hint_before(second),
                )
            }
            Shape::InsideTransaction {
                external,
                transaction,
                writes,
                ..
            } => {
                let end = path
                    .iter()
                    .enumerate()
                    .skip(external.at)
                    .find_map(|(i, e)| {
                        matches!(e, Event::TxEnd { transaction: t, .. } if t == transaction)
                            .then_some(i)
                    });
                for write in writes {
                    set_role(&mut steps, write.at, StepRole::StagedWrite);
                }
                if let Some(begin) = path
                    .iter()
                    .position(|e| matches!(e, Event::TxBegin(t) if t == transaction))
                {
                    set_role(&mut steps, begin, StepRole::Transaction);
                }
                if let Some(end) = end {
                    set_role(&mut steps, end, StepRole::NotYetPerformed);
                }
                set_role(&mut steps, external.at, StepRole::CompletedFirst);
                let table = writes.first().and_then(|w| table_of(w.effect));
                (
                    BoundaryKind::AfterExternalInsideTransaction,
                    external.at,
                    end,
                    external_hint(external, "after"),
                    end.map(|_| ("before-tx-commit".to_owned(), table)),
                )
            }
            Shape::Unknown { .. } | Shape::Incomplete(_) => return Err(skip_reason(finding.code)),
        };

        let window: Vec<usize> = match before {
            Some(before) => (after + 1..before).collect(),
            None => Vec::new(),
        };
        for &index in &window {
            if steps[index].role.is_none() {
                steps[index].role = Some(StepRole::InWindow);
            }
        }

        let (states, _) = simulate_crash(&steps, after);
        for (step, state) in steps.iter_mut().zip(states) {
            step.at_failure = state;
        }

        let mut limitations = Vec::new();
        let mut completeness = match finding.certainty {
            PathCertainty::StraightLine | PathCertainty::Conditional => Completeness::Complete,
            PathCertainty::Approximated => Completeness::Approximated,
            PathCertainty::Truncated | PathCertainty::Unknown => Completeness::Incomplete,
        };
        let unknown_in_window: Vec<String> = window
            .iter()
            .filter(|&&i| steps[i].kind == StepKind::Unknown)
            .map(|&i| format!("`{}` (step {i})", steps[i].label))
            .collect();
        if !unknown_in_window.is_empty() {
            completeness = Completeness::Incomplete;
            limitations.push(format!(
                "The failure window contains operations whose semantics are unknown: {}. They may themselves commit or publish, so the window may be split differently at runtime.",
                unknown_in_window.join(", ")
            ));
        }
        if steps.iter().any(|s| s.kind == StepKind::Unsupported) {
            completeness = Completeness::Incomplete;
            limitations.push(
                "The path reaches control flow the analysis does not model; steps after it are unknown.".to_owned(),
            );
        }
        if truncated {
            limitations.push(
                "Not every path of the function was explored (path budget exhausted); this trace is one explored path.".to_owned(),
            );
        }

        let mut assumptions = Vec::new();
        for step in &steps {
            if let Some(assumption) = self.assumption(step)
                && !assumptions.contains(&assumption)
            {
                assumptions.push(assumption);
            }
        }
        let branches: Vec<String> = steps
            .iter()
            .filter(|s| s.kind == StepKind::Branch)
            .map(|s| s.index.to_string())
            .collect();
        if !branches.is_empty() {
            assumptions.push(format!(
                "The branch decisions at step(s) {} are taken together. Only contradictions between simple tests of the same unchanging variable are ruled out; other combinations are not proven feasible.",
                branches.join(", ")
            ));
        }
        for effect in [after]
            .into_iter()
            .chain(before)
            .filter_map(|i| self.effect_of(path, i))
        {
            if let Some(recognition) = &effect.recognition {
                let text = format!(
                    "`{}` is recognized by {} (`{}`); the analysis does not check types.",
                    effect.label,
                    recognition.basis.describe(),
                    recognition.receiver
                );
                if !assumptions.contains(&text) {
                    assumptions.push(text);
                }
            }
        }
        assumptions.push(
            "The application's invariants are unknown: the trace shows that the partial state can arise, not that it violates a business rule.".to_owned(),
        );

        let describe = |i: usize| format!("`{}` (step {i})", steps[i].label);
        let description = match (boundary, before) {
            (BoundaryKind::AfterCommitBeforeExternal, Some(b)) => format!(
                "After {} commits and before {} is performed.",
                describe(after),
                describe(b)
            ),
            (BoundaryKind::AfterExternalBeforeCommit, Some(b)) => format!(
                "After {} is performed and before {} commits.",
                describe(after),
                describe(b)
            ),
            (BoundaryKind::BetweenCommits, Some(b)) => format!(
                "After {} commits and before {} commits.",
                describe(after),
                describe(b)
            ),
            (BoundaryKind::BetweenExternalEffects, Some(b)) => format!(
                "After {} is performed and before {} is performed.",
                describe(after),
                describe(b)
            ),
            (BoundaryKind::AfterExternalInsideTransaction, Some(b)) => format!(
                "After {} is performed and before the transaction commits ({}), while its writes are staged.",
                describe(after),
                describe(b)
            ),
            (_, None) => format!(
                "After {} is performed, while the transaction's writes are staged.",
                describe(after)
            ),
        };
        let mut checkpoints = vec![CheckpointHint {
            name: after_hint.0,
            step: after,
            resource: after_hint.1,
        }];
        if let (Some(b), Some((name, resource))) = (before, before_hint) {
            checkpoints.push(CheckpointHint {
                name,
                step: b,
                resource,
            });
        }
        let open_transactions = open_at(&steps, after);

        Ok(Draft {
            steps,
            injection: InjectionPoint {
                boundary,
                after_step: after,
                before_step: before,
                window,
                description,
                checkpoints,
                open_transactions,
            },
            completeness,
            assumptions,
            limitations,
        })
    }

    fn effect_of<'p>(&self, path: &Trace<'p>, index: usize) -> Option<&'p Effect> {
        match path.get(index)? {
            Event::Effect { effect, .. } => Some(*effect),
            _ => None,
        }
    }

    fn step(&self, index: usize, event: &Event<'_>) -> TraceStep {
        let mut step = TraceStep {
            index,
            kind: StepKind::Branch,
            label: String::new(),
            description: String::new(),
            location: None,
            call_chain: Vec::new(),
            transaction: None,
            parent_transaction: None,
            handle: None,
            table: None,
            operation: None,
            category: None,
            best_effort: false,
            execution: None,
            role: None,
            at_failure: StepState::NotReached,
        };
        match *event {
            Event::Effect { effect, .. } => {
                step.label = effect.label.to_string();
                step.description = effect.kind.describe();
                step.location = Some(self.src.resolve(effect.location));
                step.call_chain = self.chain(effect.summary.as_ref());
                step.execution = Some(execution_name(effect).to_owned());
                match &effect.kind {
                    EffectKind::DatabaseRead { handle, table } => {
                        step.kind = StepKind::DbRead;
                        step.table = table.clone();
                        self.handle(&mut step, handle);
                    }
                    EffectKind::DatabaseWrite {
                        operation,
                        table,
                        handle,
                    } => {
                        step.kind = StepKind::DbWrite;
                        step.table = table.clone();
                        step.operation = Some(operation.to_string());
                        self.handle(&mut step, handle);
                    }
                    EffectKind::External {
                        category,
                        best_effort,
                        ..
                    } => {
                        step.kind = StepKind::External;
                        step.category = Some(category.to_string());
                        step.best_effort = *best_effort;
                    }
                    EffectKind::Unknown { detail, .. } => {
                        step.kind = StepKind::Unknown;
                        step.description = format!("{} ({detail})", effect.kind.describe());
                    }
                }
            }
            Event::TxBegin(id) => {
                let info = self.function.transaction(id);
                step.kind = StepKind::TxBegin;
                step.label = info.label.clone();
                step.description = format!(
                    "transaction {id} begins; handle `{}`",
                    info.handle.as_deref().unwrap_or("<none>")
                );
                step.location = Some(self.src.resolve(info.location));
                step.call_chain = self.chain(info.summary.as_ref());
                step.transaction = Some(id.to_string());
                step.parent_transaction = info.parent.map(|p| p.to_string());
            }
            Event::TxEnd {
                transaction,
                committed,
            } => {
                let info = self.function.transaction(transaction);
                step.kind = if committed {
                    StepKind::TxCommit
                } else {
                    StepKind::TxRollback
                };
                step.label = info.label.clone();
                step.description = if committed {
                    format!("transaction {transaction} commits: its callback completes normally")
                } else {
                    format!(
                        "transaction {transaction} rolls back: its callback throws or rolls back"
                    )
                };
                step.location = Some(self.src.resolve(info.location));
                step.call_chain = self.chain(info.summary.as_ref());
                step.transaction = Some(transaction.to_string());
                step.parent_transaction = info.parent.map(|p| p.to_string());
            }
            Event::Decision(decision) => {
                let condition = self.condition(&decision);
                step.kind = StepKind::Branch;
                step.label = condition.description.clone();
                step.description = condition.description;
                step.location = Some(condition.location);
            }
            Event::Returned(exit) => {
                step.kind = StepKind::Return;
                step.label = "return".to_owned();
                step.description =
                    "a transaction callback returns a value tested later in the function"
                        .to_owned();
                step.location = Some(self.src.resolve(exit.location));
            }
            Event::Incomplete(exit) => {
                step.kind = StepKind::Unsupported;
                step.label = exit
                    .detail
                    .clone()
                    .unwrap_or_else(|| "unsupported control flow".to_owned());
                step.description = "control flow the analysis does not model".to_owned();
                step.location = Some(self.src.resolve(exit.location));
            }
        }
        step
    }

    fn handle(&self, step: &mut TraceStep, handle: &DbHandle) {
        match handle {
            DbHandle::Root { name } => step.handle = Some(name.clone()),
            DbHandle::Transaction { id } => {
                let info = self.function.transaction(*id);
                step.handle = Some(info.handle.clone().unwrap_or_else(|| id.to_string()));
                step.transaction = Some(id.to_string());
            }
            DbHandle::Parameter { name, .. } | DbHandle::Field { name, .. } => {
                step.handle = Some(name.clone())
            }
        }
    }

    fn chain(&self, summary: Option<&SummaryOrigin>) -> Vec<CallFrame> {
        summary
            .map(|summary| {
                summary
                    .calls
                    .iter()
                    .map(|site| CallFrame {
                        callee: site.callee.to_string(),
                        label: site.label.to_string(),
                        location: self.src.resolve(site.location),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn assumption(&self, step: &TraceStep) -> Option<String> {
        match step.kind {
            StepKind::DbWrite if step.transaction.is_none() => Some(format!(
                "`{}` runs outside any transaction: it commits on its own when its awaited promise resolves (auto-commit).",
                step.label
            )),
            StepKind::TxBegin if step.parent_transaction.is_none() => Some(format!(
                "The transaction `{}` commits when its callback completes normally and rolls back when the callback throws or the connection is lost.",
                step.label
            )),
            StepKind::External => Some(format!(
                "`{}` becomes visible to other systems when its call completes{}.",
                step.label,
                if step.best_effort {
                    "; the configuration declares it best-effort"
                } else {
                    ""
                }
            )),
            _ => None,
        }
    }
}

impl Draft {
    /// Attach the draft to its rendered diagnostic.
    pub(crate) fn finish(self, diagnostic: &Diagnostic) -> FailureTrace {
        let (_, simulation) = simulate_crash(&self.steps, self.injection.after_step);
        let mut limitations = self.limitations;
        limitations.extend(diagnostic.limitations.iter().cloned());
        limitations.push(
            "This trace is a static candidate: no application code was executed to produce it."
                .to_owned(),
        );
        let mut trace = FailureTrace {
            id: String::new(),
            finding_id: String::new(),
            code: diagnostic.code.as_str().to_owned(),
            function: diagnostic.function.clone(),
            location: diagnostic.location.clone(),
            verification: VerificationLevel::StaticCandidate,
            completeness: self.completeness,
            steps: self.steps,
            injection: self.injection,
            simulation,
            assumptions: self.assumptions,
            limitations,
            value_relation: diagnostic.value_relation.clone(),
        };
        trace.assign_finding(&diagnostic.fingerprint);
        let problems = trace.validate();
        if !problems.is_empty() {
            trace.completeness = Completeness::Incomplete;
            trace.limitations.insert(
                0,
                format!(
                    "The trace failed validation against the analysis model ({}); it must not be used as a reproduction plan.",
                    problems.join("; ")
                ),
            );
        }
        trace
    }
}

fn execution_name(effect: &Effect) -> &'static str {
    match effect.execution {
        crate::ir::Execution::Awaited => "awaited",
        crate::ir::Execution::Returned | crate::ir::Execution::ReturnedLazy => "returned",
        crate::ir::Execution::FireAndForget => "fireAndForget",
        crate::ir::Execution::Concurrent => "concurrent",
    }
}

fn set_role(steps: &mut [TraceStep], index: usize, role: StepRole) {
    if let Some(step) = steps.get_mut(index) {
        step.role = Some(role);
    }
}

fn mark_point(steps: &mut [TraceStep], point: &Point<'_>, role: StepRole) {
    match point {
        Point::Commit {
            at,
            transaction,
            writes,
            ..
        } => {
            set_role(steps, *at, role);
            if transaction.is_some() {
                for write in writes {
                    if steps.get(write.at).is_some_and(|s| s.role.is_none()) {
                        set_role(steps, write.at, role);
                    }
                }
            }
        }
        Point::External { occurrence, .. } => set_role(steps, occurrence.at, role),
    }
}

fn table_of(effect: &Effect) -> Option<String> {
    match &effect.kind {
        EffectKind::DatabaseWrite { table, .. } | EffectKind::DatabaseRead { table, .. } => {
            table.clone()
        }
        _ => None,
    }
}

fn is_publish(effect: &Effect) -> bool {
    matches!(
        effect.kind,
        EffectKind::External {
            category: ExternalCategory::MessagePublish,
            ..
        }
    )
}

fn external_hint(occurrence: &Occurrence<'_>, position: &str) -> (String, Option<String>) {
    let what = if is_publish(occurrence.effect) {
        "event-publish"
    } else {
        "external-effect"
    };
    (
        format!("{position}-{what}"),
        Some(occurrence.effect.label.to_string()),
    )
}

fn hint_after(point: &Point<'_>) -> (String, Option<String>) {
    match point {
        Point::Commit { writes, .. } => (
            "after-db-commit".to_owned(),
            writes.first().and_then(|w| table_of(w.effect)),
        ),
        Point::External { occurrence, .. } => external_hint(occurrence, "after"),
    }
}

fn hint_before(point: &Point<'_>) -> Option<(String, Option<String>)> {
    Some(match point {
        Point::Commit {
            transaction: Some(_),
            writes,
            ..
        } => (
            "before-tx-commit".to_owned(),
            writes.first().and_then(|w| table_of(w.effect)),
        ),
        Point::Commit { writes, .. } => (
            "before-db-write".to_owned(),
            writes.first().and_then(|w| table_of(w.effect)),
        ),
        Point::External { occurrence, .. } => external_hint(occurrence, "before"),
    })
}

/// Root transactions open right after step `after`.
fn open_at(steps: &[TraceStep], after: usize) -> Vec<String> {
    let mut open: Vec<String> = Vec::new();
    for step in steps.iter().take(after + 1) {
        match step.kind {
            StepKind::TxBegin => open.extend(step.transaction.clone()),
            StepKind::TxCommit | StepKind::TxRollback => {
                if let Some(tx) = &step.transaction
                    && let Some(position) = open.iter().rposition(|o| o == tx)
                {
                    open.truncate(position);
                }
            }
            _ => {}
        }
    }
    open
}

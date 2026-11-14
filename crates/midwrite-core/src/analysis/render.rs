//! Turns findings into self-explanatory [`Diagnostic`]s.

use crate::diagnostic::{
    Confidence, Diagnostic, DiagnosticCode, EffectRole, PathCertainty, PathCondition, Policy,
    RecoveryNote, RelatedEffect, Relationship, Severity,
};
use crate::ir::{
    CallSite, DbHandle, Effect, EffectKind, EvidenceStrength, ExternalCategory, FunctionIr,
    Recovery, SummaryOrigin, TransactionId, UnknownReason,
};
use crate::location::{Location, Sources};

use super::detect::{Finding, Occurrence, Point, Shape, primary_is_first};
use super::paths::Decision;

/// Builds diagnostics for one function.
pub(crate) struct Renderer<'a> {
    pub function: &'a FunctionIr,
    pub src: Sources<'a>,
    pub policy: Policy,
}

/// The external effect a sequence claims may not happen (MW1001 and MW1005
/// only), with the recovery declared for it, when the declaration covers a
/// lost effect (`bestEffort`, `reconciled`).
fn lost_effect_recovery<'ir>(
    code: DiagnosticCode,
    shape: &Shape<'ir>,
) -> Option<(&'ir Effect, Recovery)> {
    match (code, shape) {
        (
            DiagnosticCode::DbCommitThenExternal | DiagnosticCode::IndependentExternalEffects,
            Shape::Sequence {
                second: Point::External { occurrence, .. },
                ..
            },
        ) => match occurrence.effect.kind {
            EffectKind::External {
                best_effort: true, ..
            } => Some((occurrence.effect, Recovery::BestEffort)),
            EffectKind::External {
                recovery: Some(recovery),
                ..
            } if recovery.covers_lost_effect() => Some((occurrence.effect, recovery)),
            _ => None,
        },
        _ => None,
    }
}

/// Whether the two sides of a finding visibly concern the same values.
fn relationship(shape: &Shape<'_>) -> Option<Relationship> {
    fn side<'ir>(point: &Point<'ir>) -> Vec<&'ir Effect> {
        match point {
            Point::Commit { writes, .. } => writes.iter().map(|w| w.effect).collect(),
            Point::External { occurrence, .. } => vec![occurrence.effect],
        }
    }
    let (first, second): (Vec<&Effect>, Vec<&Effect>) = match shape {
        Shape::Sequence { first, second } => (side(first), side(second)),
        Shape::InsideTransaction {
            external, writes, ..
        } => (
            writes.iter().map(|w| w.effect).collect(),
            vec![external.effect],
        ),
        Shape::Unknown { .. } | Shape::Incomplete(_) => return None,
    };
    let names = |effects: &[&Effect]| -> Option<std::collections::BTreeSet<String>> {
        if effects.iter().any(|e| e.summary.is_some()) {
            return None;
        }
        let names: std::collections::BTreeSet<String> = effects
            .iter()
            .flat_map(|e| e.references.iter().cloned())
            .collect();
        (!names.is_empty()).then_some(names)
    };
    let not_comparable = Relationship {
        status: "notComparable",
        shared: Vec::new(),
    };
    let (Some(a), Some(b)) = (names(&first), names(&second)) else {
        return Some(not_comparable);
    };
    // `a` and `a.b` concern the same value; `a.b` and `a.c` may not.
    let related = |x: &str, y: &str| {
        x == y || y.starts_with(&format!("{x}.")) || x.starts_with(&format!("{y}."))
    };
    let mut shared: Vec<String> = Vec::new();
    for x in &a {
        for y in &b {
            if related(x, y) {
                let shorter = if x.len() <= y.len() { x } else { y };
                if !shared.contains(shorter) {
                    shared.push(shorter.clone());
                }
            }
        }
    }
    shared.sort();
    Some(Relationship {
        status: if shared.is_empty() {
            "noSharedArgument"
        } else {
            "sharedArguments"
        },
        shared,
    })
}

/// How the two sides of a finding are related by value provenance.
fn value_relation(
    function: &FunctionIr,
    shape: &Shape<'_>,
) -> Option<crate::absint::ValueRelation> {
    use crate::absint::relation::{Member, relate};
    fn members<'ir>(effects: impl Iterator<Item = &'ir Effect>) -> Vec<Member<'ir>> {
        effects
            .map(|effect| Member {
                effect: effect.id.0,
                provenance: effect.provenance.as_deref(),
            })
            .collect()
    }
    fn side<'ir>(point: &Point<'ir>) -> Vec<Member<'ir>> {
        match point {
            Point::Commit { writes, .. } => members(writes.iter().map(|w| w.effect)),
            Point::External { occurrence, .. } => members(std::iter::once(occurrence.effect)),
        }
    }
    let (first, second) = match shape {
        Shape::Sequence { first, second } => (side(first), side(second)),
        Shape::InsideTransaction {
            external, writes, ..
        } => {
            // Writes may come before or after the external effect; relate
            // the effect performed first to the other.
            let before: Vec<&Effect> = writes
                .iter()
                .filter(|w| w.at < external.at)
                .map(|w| w.effect)
                .collect();
            if before.is_empty() {
                (
                    members(std::iter::once(external.effect)),
                    members(writes.iter().map(|w| w.effect)),
                )
            } else {
                (
                    members(before.into_iter()),
                    members(std::iter::once(external.effect)),
                )
            }
        }
        Shape::Unknown { .. } | Shape::Incomplete(_) => return None,
    };
    let mut relation = relate(&first, &second);
    // `effect#3.result.id` → the result of `db.insert(orders)`.id
    let labels: std::collections::HashMap<u32, String> = function
        .effects()
        .iter()
        .map(|effect| (effect.id.0, effect.label.to_string()))
        .collect();
    let readable = |text: &str| -> String {
        let Some(rest) = text.strip_prefix("effect#") else {
            return text.to_owned();
        };
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        match (
            digits.parse::<u32>().ok().and_then(|id| labels.get(&id)),
            rest[digits.len()..].strip_prefix(".result"),
        ) {
            (Some(label), Some(tail)) => format!("result of `{label}`{tail}"),
            _ => text.to_owned(),
        }
    };
    for shared in &mut relation.shared {
        shared.first = readable(&shared.first);
        shared.second = readable(&shared.second);
    }
    Some(relation)
}

/// External effects of the finding that are already performed when the
/// failure happens and whose receivers are declared idempotent.
fn idempotent_effects<'ir>(shape: &Shape<'ir>) -> Vec<&'ir Effect> {
    let performed: Vec<&'ir Effect> = match shape {
        Shape::Sequence {
            first: Point::External { occurrence, .. },
            ..
        } => vec![occurrence.effect],
        Shape::InsideTransaction { external, .. } => vec![external.effect],
        _ => Vec::new(),
    };
    performed
        .into_iter()
        .filter(|effect| {
            matches!(
                effect.kind,
                EffectKind::External {
                    recovery: Some(Recovery::IdempotentConsumer),
                    ..
                }
            )
        })
        .collect()
}

/// The severity of a finding under `policy`.
fn severity(policy: Policy, code: DiagnosticCode, shape: &Shape<'_>) -> Severity {
    match (policy, code) {
        (Policy::Strict | Policy::Audit, _) => code.severity(),
        (Policy::Recommended, _) if lost_effect_recovery(code, shape).is_some() => Severity::Info,
        (Policy::Recommended, DiagnosticCode::IndependentDbCommits) => {
            // A write that bypasses its enclosing transaction (or uses a
            // transaction handle after the transaction ended) is a specific
            // mistake; two writes that are merely sequential are a review
            // candidate.
            let specific = matches!(shape, Shape::Sequence { first, second }
                if [first, second].iter().any(|point| matches!(point,
                    Point::Commit { bypassed: Some(_), .. } | Point::Commit { escaped: true, .. })));
            if specific {
                Severity::Warning
            } else {
                Severity::Info
            }
        }
        (Policy::Recommended, DiagnosticCode::IndependentExternalEffects) => Severity::Info,
        (Policy::Recommended, _) => code.severity(),
    }
}

/// The weakest recognition evidence among `effects`.
fn weakest<'e>(effects: impl IntoIterator<Item = &'e Effect>) -> EvidenceStrength {
    effects
        .into_iter()
        .map(|effect| {
            effect
                .recognition
                .as_ref()
                .map_or(EvidenceStrength::Unknown, |r| r.basis.strength())
        })
        .min()
        .unwrap_or(EvidenceStrength::Unknown)
}

/// The highest confidence the evidence supports: a match on the names along
/// a member path is never `high`.
fn evidence_cap(strength: EvidenceStrength) -> Confidence {
    match strength {
        EvidenceStrength::Unknown => Confidence::Low,
        EvidenceStrength::MemberPath => Confidence::Medium,
        _ => Confidence::High,
    }
}

impl Renderer<'_> {
    pub fn render(&self, finding: Finding<'_>) -> Diagnostic {
        let Finding {
            code,
            shape,
            mut confidence,
            certainty,
            conditions,
            notes,
            alternatives,
            trace: _,
        } = finding;
        let path_conditions = conditions
            .iter()
            .map(|decision| self.condition(decision))
            .collect();
        let info = code.info();
        let mut limitations = notes;
        let strength = self.evidence_strength(&shape);
        if code != DiagnosticCode::AnalysisIncomplete {
            let cap = evidence_cap(strength);
            if cap < confidence {
                confidence = cap;
                limitations.push(
                    "Confidence is capped at medium: an operation was matched only by the names along a configured member path, not by provenance or import.".to_owned(),
                );
            }
        }
        let mut recovery = Vec::new();
        match lost_effect_recovery(code, &shape) {
            Some((effect, Recovery::BestEffort)) => {
                limitations.push(format!(
                    "The configuration marks `{}` as best-effort (`bestEffort`): the application accepts that it is sometimes not performed. Under the `recommended` policy this finding is informational.",
                    effect.label
                ));
                recovery.push(RecoveryNote {
                    effect: effect.label.to_string(),
                    declaration: Recovery::BestEffort.into(),
                    source: "configuration",
                    verified: false,
                    effect_on_finding: "informational",
                    scope: "effect",
                    evidence: None,
                    limits: None,
                });
            }
            Some((effect, declaration)) => {
                limitations.push(format!(
                    "The configuration declares `{}` as reconciled (`\"recovery\": \"reconciled\"`): a process outside the analyzed code is asserted to derive it from committed database state when it is missing. Midwrite does not verify that this process exists or covers this window; the window is still reported. Under the `recommended` policy this finding is informational.",
                    effect.label
                ));
                recovery.push(RecoveryNote {
                    effect: effect.label.to_string(),
                    declaration: declaration.into(),
                    source: "configuration",
                    verified: false,
                    effect_on_finding: "informational",
                    scope: "effect",
                    evidence: None,
                    limits: None,
                });
            }
            None => {}
        }
        for effect in idempotent_effects(&shape) {
            limitations.push(format!(
                "The configuration declares the receivers of `{}` idempotent (`\"recovery\": \"idempotentConsumer\"`, not verified): performing it again on a retry is harmless, but that does not undo it when the rest of the operation never happens.",
                effect.label
            ));
            recovery.push(RecoveryNote {
                effect: effect.label.to_string(),
                declaration: Recovery::IdempotentConsumer.into(),
                source: "configuration",
                verified: false,
                effect_on_finding: "noteOnly",
                scope: "effect",
                evidence: None,
                limits: None,
            });
        }
        if let Shape::Sequence { first, .. } = &shape
            && !alternatives.is_empty()
        {
            let mut names: Vec<String> = Vec::new();
            for point in &alternatives {
                let name = format!(
                    "{} ({})",
                    self.point_name(point),
                    self.src.at(self.point_location(point))
                );
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            let role = if primary_is_first(code, first) {
                "follows it"
            } else {
                "precedes it"
            };
            limitations.push(format!(
                "On other paths a different operation {role}: {}. Those paths are not reported separately.",
                names.join(", ")
            ));
        }
        limitations.push(info.limitation.to_owned());

        let parts = match &shape {
            Shape::Sequence { first, second } => self.sequence(code, first, second),
            Shape::InsideTransaction {
                external,
                transaction,
                writes,
                committed,
            } => self.inside_transaction(*external, *transaction, writes, *committed),
            Shape::Unknown {
                occurrence,
                preceding,
                following,
                inside,
                siblings,
            } => {
                let continuations = matches!(
                    occurrence.effect.kind,
                    EffectKind::Unknown {
                        reason: UnknownReason::UnmodeledContinuation,
                        ..
                    }
                );
                if !siblings.is_empty() {
                    const SHOWN: usize = 5;
                    let mut names: Vec<String> = siblings
                        .iter()
                        .take(SHOWN)
                        .map(|e| format!("`{}` ({})", e.label, self.src.at(e.location)))
                        .collect();
                    if siblings.len() > SHOWN {
                        names.push(format!("and {} more", siblings.len() - SHOWN));
                    }
                    limitations.insert(
                        0,
                        if continuations {
                            format!(
                                "These other promise continuations of the same function are not analyzed in sequence either and are reported once, here: {}.",
                                names.join(", ")
                            )
                        } else {
                            format!(
                                "These other calls in the same function (or in the same inlined call) are also not analyzed and are reported once, here: {}.",
                                names.join(", ")
                            )
                        },
                    );
                }
                let mut parts =
                    self.unknown(*occurrence, preceding.as_ref(), following.as_ref(), *inside);
                if continuations {
                    // Every grouped continuation keeps its own location in
                    // the structured output.
                    for (offset, sibling) in siblings.iter().enumerate() {
                        parts.effects.insert(
                            1 + offset,
                            self.related(
                                EffectRole::Unanalyzed,
                                &sibling.label,
                                sibling.kind.describe(),
                                sibling.location,
                            ),
                        );
                    }
                }
                parts
            }
            Shape::Incomplete(exit) => Parts {
                message: format!(
                    "unsupported control flow ({}); paths through this point are not analyzed further",
                    exit.detail.as_deref().unwrap_or("unknown construct")
                ),
                location: exit.location,
                effects: Vec::new(),
                failure_sequence: Vec::new(),
                evidence: vec![
                    "The function contains recognized effects, so the unanalyzed paths may hide hazards."
                        .to_owned(),
                ],
                mitigation: None,
            },
        };

        let mut diagnostic = Diagnostic {
            code,
            severity: severity(self.policy, code, &shape),
            kind: code.kind(),
            title: info.title,
            message: parts.message,
            location: self.src.resolve(parts.location),
            function: self.function.name.clone(),
            confidence,
            reproduced: false,
            effects: parts.effects,
            failure_sequence: parts.failure_sequence,
            path_conditions,
            evidence: parts.evidence,
            mitigation: parts
                .mitigation
                .unwrap_or_else(|| info.mitigation.to_owned()),
            limitations,
            path_certainty: certainty,
            evidence_strength: strength,
            fingerprint: String::new(),
            baseline: None,
            relationship: relationship(&shape),
            value_relation: value_relation(self.function, &shape),
            recovery,
        };
        diagnostic.fingerprint = diagnostic.compute_fingerprint(Some(self.src.file));
        diagnostic
    }

    /// The weakest recognition evidence among the operations of a finding.
    fn evidence_strength<'ir>(&self, shape: &Shape<'ir>) -> EvidenceStrength {
        fn point<'ir>(
            point: &Point<'ir>,
            effects: &mut Vec<&'ir Effect>,
            transactions: &mut Vec<TransactionId>,
        ) {
            match point {
                Point::Commit {
                    transaction,
                    writes,
                    ..
                } => {
                    effects.extend(writes.iter().map(|w| w.effect));
                    transactions.extend(*transaction);
                }
                Point::External { occurrence, .. } => effects.push(occurrence.effect),
            }
        }
        let mut effects: Vec<&'ir Effect> = Vec::new();
        let mut transactions: Vec<TransactionId> = Vec::new();
        match shape {
            Shape::Sequence { first, second } => {
                point(first, &mut effects, &mut transactions);
                point(second, &mut effects, &mut transactions);
            }
            Shape::InsideTransaction {
                external,
                transaction,
                writes,
                ..
            } => {
                effects.push(external.effect);
                effects.extend(writes.iter().map(|w| w.effect));
                transactions.push(*transaction);
            }
            Shape::Unknown { .. } | Shape::Incomplete(_) => return EvidenceStrength::Unknown,
        }
        let strength = weakest(effects);
        transactions
            .iter()
            .map(|tx| self.function.transaction(*tx).recognition.basis.strength())
            .fold(strength, EvidenceStrength::min)
    }

    /// Diagnostic for a function whose paths were not all explored.
    pub fn truncated(&self, max_paths: usize) -> Diagnostic {
        let code = DiagnosticCode::AnalysisIncomplete;
        let info = code.info();
        let mut diagnostic = Diagnostic {
            code,
            severity: code.severity(),
            kind: code.kind(),
            title: info.title,
            message: format!(
                "`{}` has more than {max_paths} execution paths; only the first {max_paths} were analyzed",
                self.function.name
            ),
            location: self.src.resolve(self.function.location),
            function: self.function.name.clone(),
            confidence: Confidence::Low,
            reproduced: false,
            effects: Vec::new(),
            failure_sequence: Vec::new(),
            path_conditions: Vec::new(),
            evidence: vec![
                "The function contains recognized effects, so the unexplored paths may hide hazards."
                    .to_owned(),
            ],
            mitigation: "Split the function into smaller functions, or raise the path limit."
                .to_owned(),
            limitations: vec![info.limitation.to_owned()],
            path_certainty: PathCertainty::Truncated,
            evidence_strength: EvidenceStrength::Unknown,
            fingerprint: String::new(),
            baseline: None,
            recovery: Vec::new(),
            relationship: None,
            value_relation: None,
        };
        diagnostic.fingerprint = diagnostic.compute_fingerprint(Some(self.src.file));
        diagnostic
    }

    fn sequence(&self, code: DiagnosticCode, first: &Point<'_>, second: &Point<'_>) -> Parts {
        let a = self.point_name(first);
        let b = self.point_name(second);
        let message = match code {
            DiagnosticCode::DbCommitThenExternal => format!(
                "{a} commits before {b} ({}); a failure in between leaves the database changed without the external effect",
                category(second)
            ),
            DiagnosticCode::ExternalThenDbCommit => format!(
                "{a} ({}) is performed before {b} commits; a failure in between leaves an external effect without the database change",
                category(first)
            ),
            DiagnosticCode::IndependentDbCommits => format!(
                "{a} and {b} commit independently; a failure in between persists only the first change"
            ),
            _ => format!(
                "{a} and {b} are independent external effects; a failure in between performs only the first"
            ),
        };
        let location = match (code, first) {
            (DiagnosticCode::ExternalThenDbCommit, _) => self.point_location(first),
            // A write that bypasses its enclosing transaction is the one to fix.
            (
                DiagnosticCode::IndependentDbCommits,
                Point::Commit {
                    bypassed: Some(_), ..
                },
            ) => self.point_location(first),
            _ => self.point_location(second),
        };

        let mut effects = self.point_effects(first, EffectRole::CompletedFirst);
        effects.extend(self.point_effects(second, EffectRole::NotYetPerformed));

        let failure = match code {
            DiagnosticCode::DbCommitThenExternal => format!(
                "The process crashes or is restarted, or the call fails, after the commit and before {b} completes."
            ),
            DiagnosticCode::ExternalThenDbCommit => format!(
                "The process crashes, or the database operation fails, after {a} and before {b} commits."
            ),
            _ => format!("A failure occurs after {a} and before {b} completes."),
        };
        let never = capitalize(&match second {
            Point::Commit { .. } => format!("{b} never commits."),
            Point::External { .. } => format!("{b} is never performed."),
        });
        let failure_sequence = numbered([
            self.point_step(first),
            failure,
            never,
            format!("Result: {}.", code.info().outcome),
        ]);

        let mut evidence = self.point_evidence(first);
        evidence.extend(self.point_evidence(second));

        let mitigation = [first, second].iter().find_map(|point| match point {
            Point::Commit {
                bypassed: Some(tx),
                writes,
                ..
            } => {
                let info = self.function.transaction(*tx);
                let handle = info.handle.as_deref().unwrap_or("tx");
                let write = writes[0].effect;
                Some(match write.summary.as_ref().and_then(|s| s.calls.last()) {
                    Some(callee) => format!(
                        "`{}` issues `{}` on `{}`, so it does not join the transaction at {}. Pass the transaction handle `{handle}` to `{}` and issue the write on it, so that it commits atomically with the transaction. {}",
                        callee.callee,
                        write.label,
                        receiver_of(write),
                        self.src.at(info.location),
                        callee.callee,
                        code.info().mitigation
                    ),
                    None => format!(
                        "Issue `{}` on the transaction handle `{handle}` of the transaction at {} instead of on `{}`, so that it commits atomically with the transaction. {}",
                        write.label,
                        self.src.at(info.location),
                        receiver_of(write),
                        code.info().mitigation
                    ),
                })
            }
            _ => None,
        });

        Parts {
            message,
            location,
            effects,
            failure_sequence,
            evidence,
            mitigation,
        }
    }

    fn inside_transaction(
        &self,
        external: Occurrence<'_>,
        transaction: TransactionId,
        writes: &[Occurrence<'_>],
        committed: Option<bool>,
    ) -> Parts {
        let info = self.function.transaction(transaction);
        let effect = external.effect;
        let summary = effect.summary.as_ref();
        let message = format!(
            "`{}`{} ({}) runs inside the transaction started at {} before it commits; if the transaction rolls back, the external effect remains",
            effect.label,
            via(summary),
            effect_category(effect),
            self.src.at(info.location)
        );

        let mut effects = vec![self.related(
            EffectRole::Transaction,
            &info.label,
            "transaction; its writes become durable only when it commits".to_owned(),
            info.location,
        )];
        effects.extend(writes.iter().map(|w| {
            self.related(
                EffectRole::StagedWrite,
                &w.effect.label,
                format!("{} (staged in the transaction)", w.effect.kind.describe()),
                w.effect.location,
            )
        }));
        effects.push(self.related(
            EffectRole::CompletedFirst,
            &effect.label,
            format!(
                "{}{}; visible immediately",
                effect.kind.describe(),
                inside_callee(summary)
            ),
            effect.location,
        ));
        effects.extend(self.call_site(summary));

        let outcome = if committed == Some(false) {
            "On this path the transaction does not commit (an exception or rollback ends the callback), so its staged writes are discarded.".to_owned()
        } else {
            "The transaction then fails to commit: a later statement throws, the commit is rejected (constraint, serialization or connection failure), or the process dies. Its staged writes roll back.".to_owned()
        };
        let failure_sequence = numbered([
            format!(
                "The transaction started by `{}` ({}) stages its writes without committing them: {}.",
                info.label,
                self.src.at(info.location),
                list_effects(writes, &self.src)
            ),
            format!(
                "`{}` ({}{}) performs an external effect ({}) while the transaction is still open.",
                effect.label,
                self.src.at(effect.location),
                reached(summary, &self.src),
                effect_category(effect)
            ),
            outcome,
            format!(
                "Result: {}.",
                DiagnosticCode::ExternalInsideTransaction.info().outcome
            ),
        ]);

        let mut evidence = vec![self.transaction_evidence(transaction)];
        evidence.extend(writes.iter().map(|w| recognition_evidence(w.effect)));
        evidence.push(recognition_evidence(effect));
        evidence.extend(effect_summary_evidence(effect, &self.src));

        Parts {
            message,
            location: effect_site(effect),
            effects,
            failure_sequence,
            evidence,
            mitigation: None,
        }
    }

    fn unknown(
        &self,
        occurrence: Occurrence<'_>,
        preceding: Option<&Point<'_>>,
        following: Option<&Point<'_>>,
        inside: Option<TransactionId>,
    ) -> Parts {
        let effect = occurrence.effect;
        let EffectKind::Unknown { reason, detail, .. } = &effect.kind else {
            unreachable!("unknown findings are built from unknown effects")
        };
        let label = &effect.label;
        let summary = effect.summary.as_ref();
        let via = via(summary);
        let (message, mut evidence) = match reason {
            UnknownReason::RawSql => (
                format!(
                    "raw SQL `{label}`{via} is not analyzed; any writes it performs are not considered"
                ),
                vec![format!(
                    "`{label}` executes raw SQL; Midwrite does not parse SQL."
                )],
            ),
            UnknownReason::OpaqueTransaction => (
                format!("the transaction `{label}`{via} cannot be analyzed: {detail}"),
                vec![format!("`{label}` starts a transaction but {detail}.")],
            ),
            UnknownReason::UnsupportedReceiver => (
                format!("`{label}`{via} is not analyzed: {detail}"),
                vec![format!(
                    "Member receivers are matched only when `databaseReceivers` or `externalEffects` names their path; `{label}` is skipped rather than guessed."
                )],
            ),
            UnknownReason::UnresolvedHandle => (
                format!("`{label}`{via} is not analyzed: {detail}"),
                vec![
                    "Operations on a callee's parameter are attributed to the handle the caller passes; this argument is not a recognized database or transaction handle, so the operation is not guessed to be a write on some other handle.".to_owned(),
                ],
            ),
            UnknownReason::UnrecognizedDatabaseMethod => (
                format!("`{label}`{via} calls a database method that Midwrite does not model"),
                vec![detail.clone()],
            ),
            UnknownReason::UnmodeledContinuation => (
                format!(
                    "the promise continuation `{label}`{via} is not analyzed in sequence: {detail}"
                ),
                vec![
                    "The handler's operations are analyzed as a function of their own, but they are not ordered with the operations of this function: no finding pairs them with the code around the chain.".to_owned(),
                    "This is a coverage gap, not a clean result: a partial-completion hazard through this handler would not be reported.".to_owned(),
                ],
            ),
            UnknownReason::UnrecognizedCall => {
                let context = if let Some(point) = preceding {
                    format!(
                        "runs after {} has {}",
                        self.point_name(point),
                        match point {
                            Point::Commit { .. } => "committed",
                            Point::External { .. } => "been performed",
                        }
                    )
                } else if let Some(tx) = inside {
                    format!(
                        "runs inside the transaction started at {}, before it commits",
                        self.src.at(self.function.transaction(tx).location)
                    )
                } else if let Some(point) = following {
                    format!("runs before {}", self.point_name(point))
                } else {
                    "runs near recognized effects".to_owned()
                };
                (
                    format!(
                        "`{label}`{via} {context}; Midwrite does not know whether it has side effects of its own"
                    ),
                    vec![format!(
                        "`{label}` matches no `databaseReceivers` or `externalEffects` entry ({detail})."
                    )],
                )
            }
        };

        evidence.extend(effect_summary_evidence(effect, &self.src));
        let mut effects = vec![self.related(
            EffectRole::Unanalyzed,
            label,
            format!("{}{}", effect.kind.describe(), inside_callee(summary)),
            effect.location,
        )];
        effects.extend(self.call_site(summary));
        let reached = reached(summary, &self.src);
        let mut failure_sequence = Vec::new();
        if let Some(point) = preceding {
            effects.extend(self.point_effects(point, EffectRole::Context));
            evidence.extend(self.point_evidence(point));
            if *reason == UnknownReason::UnrecognizedCall {
                failure_sequence = numbered([
                    self.point_step(point),
                    format!(
                        "`{label}` ({}{reached}) runs; its effects are unknown.",
                        self.src.at(effect.location)
                    ),
                    format!(
                        "If `{label}` performs an independent side effect, a failure before it completes leaves the earlier effect without it."
                    ),
                ]);
            }
        } else if let Some(point) = following {
            effects.extend(self.point_effects(point, EffectRole::Context));
            evidence.extend(self.point_evidence(point));
            if *reason == UnknownReason::UnrecognizedCall {
                failure_sequence = numbered([
                    format!(
                        "`{label}` ({}{reached}) runs; its effects are unknown.",
                        self.src.at(effect.location)
                    ),
                    format!(
                        "If it performs an independent side effect, a failure before {} completes leaves that effect without it.",
                        self.point_name(point)
                    ),
                ]);
            }
        }

        let mitigation = (*reason == UnknownReason::UnmodeledContinuation).then(|| {
            "Rewrite the chain with `await` (and `try`/`catch`/`finally`) so that the handler's operations are ordered with the rest of the function, or inspect the handler manually. If the handler's effect is already described elsewhere (for example a configured `externalEffects` entry on the function the chain calls), suppress this notice with a reason.".to_owned()
        });
        Parts {
            message,
            location: effect_site(effect),
            effects,
            failure_sequence,
            evidence,
            mitigation,
        }
    }

    /// Short name of a point for messages.
    fn point_name(&self, point: &Point<'_>) -> String {
        match point {
            Point::Commit {
                transaction: Some(tx),
                ..
            } => {
                let info = self.function.transaction(*tx);
                format!(
                    "the transaction started at {}{}",
                    self.src.at(info.location),
                    via(info.summary.as_ref())
                )
            }
            Point::Commit { writes, .. } => effect_name(writes[0].effect),
            Point::External { occurrence, .. } => effect_name(occurrence.effect),
        }
    }

    /// Where the point is reported in this function: the operation itself,
    /// or the call through which it was inlined from a summary.
    fn point_location(&self, point: &Point<'_>) -> Location {
        match point {
            Point::Commit {
                transaction: Some(tx),
                ..
            } => {
                let info = self.function.transaction(*tx);
                outermost(info.summary.as_ref()).map_or(info.location, |site| site.location)
            }
            Point::Commit { writes, .. } => effect_site(writes[0].effect),
            Point::External { occurrence, .. } => effect_site(occurrence.effect),
        }
    }

    /// First step of a failure sequence: the point taking effect.
    fn point_step(&self, point: &Point<'_>) -> String {
        match point {
            Point::Commit {
                transaction: Some(tx),
                writes,
                ..
            } => {
                let info = self.function.transaction(*tx);
                format!(
                    "The transaction started by `{}` ({}{}) commits, making its writes durable: {}.",
                    info.label,
                    self.src.at(info.location),
                    reached(info.summary.as_ref(), &self.src),
                    list_effects(writes, &self.src)
                )
            }
            Point::Commit {
                writes,
                bypassed,
                escaped,
                ..
            } => {
                let effect = writes[0].effect;
                let why = if let Some(tx) = bypassed {
                    let info = self.function.transaction(*tx);
                    format!(
                        "it is issued on `{}`, not on the handle `{}` of the enclosing transaction started at {}, so it is not part of that transaction",
                        receiver_of(effect),
                        info.handle.as_deref().unwrap_or("tx"),
                        self.src.at(info.location)
                    )
                } else if *escaped {
                    "it uses a transaction handle after its transaction ended".to_owned()
                } else {
                    "it is not inside a recognized transaction".to_owned()
                };
                format!(
                    "`{}` ({}{}) commits immediately: {why}.",
                    effect.label,
                    self.src.at(effect.location),
                    reached(effect.summary.as_ref(), &self.src)
                )
            }
            Point::External { occurrence, .. } => {
                let effect = occurrence.effect;
                format!(
                    "`{}` ({}{}) performs an external effect ({}); other systems can observe it as soon as it succeeds.",
                    effect.label,
                    self.src.at(effect.location),
                    reached(effect.summary.as_ref(), &self.src),
                    effect_category(effect)
                )
            }
        }
    }

    fn point_effects(&self, point: &Point<'_>, role: EffectRole) -> Vec<RelatedEffect> {
        match point {
            Point::Commit {
                transaction: Some(tx),
                writes,
                ..
            } => {
                let info = self.function.transaction(*tx);
                let mut effects = vec![self.related(
                    EffectRole::Transaction,
                    &info.label,
                    format!(
                        "transaction committing {} write(s){}",
                        writes.len(),
                        inside_callee(info.summary.as_ref())
                    ),
                    info.location,
                )];
                effects.extend(writes.iter().map(|w| {
                    self.related(
                        role,
                        &w.effect.label,
                        format!(
                            "{} (committed with the transaction){}",
                            w.effect.kind.describe(),
                            inside_callee(w.effect.summary.as_ref())
                        ),
                        w.effect.location,
                    )
                }));
                effects.extend(self.call_site(info.summary.as_ref()));
                effects
            }
            Point::Commit { writes, .. } => {
                let mut effects: Vec<RelatedEffect> = writes
                    .iter()
                    .map(|w| {
                        self.related(
                            role,
                            &w.effect.label,
                            format!(
                                "{} (commits on its own){}",
                                w.effect.kind.describe(),
                                inside_callee(w.effect.summary.as_ref())
                            ),
                            w.effect.location,
                        )
                    })
                    .collect();
                effects.extend(self.call_site(writes[0].effect.summary.as_ref()));
                effects
            }
            Point::External { occurrence, .. } => {
                let effect = occurrence.effect;
                let mut effects = vec![self.related(
                    role,
                    &effect.label,
                    format!(
                        "{}{}",
                        effect.kind.describe(),
                        inside_callee(effect.summary.as_ref())
                    ),
                    effect.location,
                )];
                effects.extend(self.call_site(effect.summary.as_ref()));
                effects
            }
        }
    }

    /// The call through which an operation was inlined, as a related effect.
    fn call_site(&self, summary: Option<&SummaryOrigin>) -> Option<RelatedEffect> {
        let site = outermost(summary)?;
        Some(self.related(
            EffectRole::CallSite,
            &site.label,
            format!(
                "call of `{}`, {}; the operations inside it are taken from its summary",
                site.callee,
                declared(site, &self.src)
            ),
            site.location,
        ))
    }

    fn point_evidence(&self, point: &Point<'_>) -> Vec<String> {
        match point {
            Point::Commit {
                transaction: Some(tx),
                writes,
                ..
            } => {
                let info = self.function.transaction(*tx);
                let mut evidence = vec![self.transaction_evidence(*tx)];
                evidence.extend(info.summary.as_ref().and_then(|summary| {
                    summary_evidence(
                        &format!(
                            "The transaction `{}` ({})",
                            info.label,
                            self.src.at(info.location)
                        ),
                        summary,
                        "commits it",
                        &self.src,
                    )
                }));
                evidence.extend(writes.iter().map(|w| recognition_evidence(w.effect)));
                evidence
            }
            Point::Commit { writes, .. } => writes
                .iter()
                .flat_map(|w| {
                    std::iter::once(recognition_evidence(w.effect))
                        .chain(effect_summary_evidence(w.effect, &self.src))
                })
                .collect(),
            Point::External { occurrence, .. } => {
                std::iter::once(recognition_evidence(occurrence.effect))
                    .chain(effect_summary_evidence(occurrence.effect, &self.src))
                    .collect()
            }
        }
    }

    fn transaction_evidence(&self, tx: TransactionId) -> String {
        let info = self.function.transaction(tx);
        format!(
            "`{}`: `{}` is a database handle ({}; {}); the transaction commits when its callback completes.",
            info.label,
            info.recognition.receiver,
            info.recognition.basis.describe(),
            info.recognition.detail
        )
    }

    pub(crate) fn condition(&self, decision: &Decision<'_>) -> PathCondition {
        let (description, location) = match *decision {
            Decision::Arm { branch, arm } => (
                format!(
                    "{} `{}` takes the `{}` path",
                    branch.kind, branch.condition, branch.arms[arm].label
                ),
                branch.location,
            ),
            Decision::Case { switch, case } => (
                format!(
                    "switch on `{}` enters `{}`",
                    switch.discriminant, switch.cases[case].label
                ),
                switch.location,
            ),
            Decision::NoCase { switch } => (
                format!("switch on `{}` matches no case", switch.discriminant),
                switch.location,
            ),
            Decision::LoopEntered(lp) => (
                format!("{} loop `{}` executes its body", lp.kind, lp.header),
                lp.location,
            ),
            Decision::LoopSkipped(lp) => (
                format!("{} loop `{}` does not execute its body", lp.kind, lp.header),
                lp.location,
            ),
            Decision::CatchImplicit(try_node) => (
                "an exception in the try block is caught (approximated as occurring before the block's effects)".to_owned(),
                try_node.handler.as_ref().map_or(try_node.location, |h| h.location),
            ),
            Decision::CatchExplicit(try_node) => (
                "an explicit `throw` in the try block is caught".to_owned(),
                try_node.handler.as_ref().map_or(try_node.location, |h| h.location),
            ),
        };
        PathCondition {
            description,
            location: self.src.resolve(location),
        }
    }

    fn related(
        &self,
        role: EffectRole,
        label: &str,
        description: String,
        location: Location,
    ) -> RelatedEffect {
        RelatedEffect {
            role,
            label: label.to_owned(),
            description,
            location: self.src.resolve(location),
        }
    }
}

struct Parts {
    message: String,
    location: Location,
    effects: Vec<RelatedEffect>,
    failure_sequence: Vec<String>,
    evidence: Vec<String>,
    mitigation: Option<String>,
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn numbered<const N: usize>(steps: [String; N]) -> Vec<String> {
    steps
        .into_iter()
        .enumerate()
        .map(|(index, step)| format!("{}. {step}", index + 1))
        .collect()
}

fn list_effects(occurrences: &[Occurrence<'_>], src: &Sources<'_>) -> String {
    const SHOWN: usize = 3;
    let mut parts: Vec<String> = occurrences
        .iter()
        .take(SHOWN)
        .map(|o| format!("`{}` ({})", o.effect.label, src.at(o.effect.location)))
        .collect();
    if occurrences.len() > SHOWN {
        parts.push(format!("and {} more", occurrences.len() - SHOWN));
    }
    parts.join(", ")
}

fn category(point: &Point<'_>) -> String {
    match point {
        Point::External { occurrence, .. } => effect_category(occurrence.effect),
        Point::Commit { .. } => "database commit".to_owned(),
    }
}

fn effect_category(effect: &Effect) -> String {
    match effect.kind {
        EffectKind::External { category, .. } => category.to_string(),
        _ => ExternalCategory::Other.to_string(),
    }
}

fn receiver_of(effect: &Effect) -> &str {
    match &effect.kind {
        EffectKind::DatabaseRead {
            handle: DbHandle::Root { name },
            ..
        }
        | EffectKind::DatabaseWrite {
            handle: DbHandle::Root { name },
            ..
        } => name,
        _ => effect
            .recognition
            .as_ref()
            .map_or("db", |recognition| recognition.receiver.as_str()),
    }
}

fn outermost(summary: Option<&SummaryOrigin>) -> Option<&crate::ir::CallSite> {
    summary.and_then(SummaryOrigin::outermost)
}

/// Where an effect is reported in this function: its own location, or the
/// outermost call through which it was inlined from a summary.
fn effect_site(effect: &Effect) -> Location {
    effect.call().map_or(effect.location, |site| site.location)
}

/// The effect's label for messages, with the call it was inlined through.
fn effect_name(effect: &Effect) -> String {
    format!("`{}`{}", effect.label, via(effect.summary.as_ref()))
}

/// ` (via `notify(id)`)` for an operation inlined from a summary.
fn via(summary: Option<&SummaryOrigin>) -> String {
    outermost(summary)
        .map(|site| format!(" (via `{}`)", site.label))
        .unwrap_or_default()
}

/// `, inside `notify`` for an operation inlined from a summary.
fn inside_callee(summary: Option<&SummaryOrigin>) -> String {
    summary
        .and_then(|summary| summary.calls.last())
        .map(|site| format!(", inside `{}`", site.callee))
        .unwrap_or_default()
}

/// `, in `notify`, reached through `notify(id)` at 15:3` for an operation
/// inlined from a summary.
fn reached(summary: Option<&SummaryOrigin>, src: &Sources<'_>) -> String {
    match summary.and_then(|summary| summary.calls.last()) {
        Some(innermost) => format!(
            ", in `{}`, reached through {}",
            innermost.callee,
            call_chain(summary.expect("checked above"), src)
        ),
        None => String::new(),
    }
}

fn call_chain(summary: &SummaryOrigin, src: &Sources<'_>) -> String {
    summary
        .calls
        .iter()
        .map(|site| format!("`{}` at {}", site.label, src.at(site.location)))
        .collect::<Vec<_>>()
        .join(" → ")
}

/// `a function declared in this file`, or `declared in other.ts at 3:1`.
fn declared(site: &CallSite, src: &Sources<'_>) -> String {
    if src.is_other_file(site.callee_location) {
        format!("declared in {}", src.at(site.callee_location))
    } else {
        "a function declared in this file".to_owned()
    }
}

/// Evidence that `subject` comes from a summary rather than from the
/// function's own code.
fn summary_evidence(
    subject: &str,
    summary: &SummaryOrigin,
    verb: &str,
    src: &Sources<'_>,
) -> Option<String> {
    let outermost = summary.outermost()?;
    let innermost = summary.calls.last()?;
    let paths = if summary.on_every_path {
        "on every path through it"
    } else {
        "only on some paths through it"
    };
    Some(format!(
        "{subject} is taken from the summary of `{}`, {}, reached through {}; `{}` {verb} {paths}.",
        innermost.callee,
        declared(innermost, src),
        call_chain(summary, src),
        outermost.callee
    ))
}

fn effect_summary_evidence(effect: &Effect, src: &Sources<'_>) -> Option<String> {
    summary_evidence(
        &format!("`{}` ({})", effect.label, src.at(effect.location)),
        effect.summary.as_ref()?,
        "performs it",
        src,
    )
}

fn recognition_evidence(effect: &Effect) -> String {
    match &effect.recognition {
        Some(recognition) => format!(
            "`{}` ({}): `{}` — {}; {}.",
            effect.label,
            effect.kind.describe(),
            recognition.receiver,
            recognition.basis.describe(),
            recognition.detail
        ),
        None => format!("`{}`: not recognized.", effect.label),
    }
}

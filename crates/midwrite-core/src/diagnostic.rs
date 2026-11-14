//! Diagnostics and the diagnostic code catalog.

use std::fmt;
use std::str::FromStr;

use serde::{Serialize, Serializer};

use crate::ir::EvidenceStrength;
use crate::location::SourceLocation;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How strongly the analysis supports a diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// The path involves an approximation (catch-handler entry, concurrency),
    /// or the diagnostic reports that something could not be analyzed.
    Low,
    /// The sequence depends on branch conditions, loop iterations or an
    /// unawaited operation.
    Medium,
    /// Both operations were recognized and occur in order on a straight-line
    /// path segment.
    High,
}

impl fmt::Display for Confidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        })
    }
}

/// How certain the reported execution path is, independently of how the
/// operations on it were recognized. Ordered from most to least certain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PathCertainty {
    /// The two operations follow each other on a straight-line segment.
    StraightLine,
    /// The sequence depends on branch decisions, a loop iteration, an
    /// unawaited operation, or an operation a callee performs only on some
    /// paths.
    Conditional,
    /// The path relies on an approximation: concurrent operations, or a catch
    /// handler entered as if the try block failed before its effects.
    Approximated,
    /// Not every path of the function was explored.
    Truncated,
    /// The semantics of an operation on the path are unknown (MW9001).
    Unknown,
}

/// Which diagnostics are reported, and at which severity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Policy {
    /// Actionable cross-resource hazards (MW1001–MW1003) and writes that
    /// bypass an enclosing transaction are warnings. Broad atomicity checks
    /// (other MW1004, and MW1005) are reported as `info`: review candidates,
    /// not likely bugs.
    #[default]
    Recommended,
    /// Every hazard at its catalog severity (the Phase 1 behavior).
    Strict,
    /// `strict`, plus speculative notes: unknown calls that run before the
    /// first recognized operation on a path (possible unconfigured effects).
    Audit,
}

impl Policy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recommended => "recommended",
            Self::Strict => "strict",
            Self::Audit => "audit",
        }
    }
}

impl FromStr for Policy {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "recommended" => Ok(Self::Recommended),
            "strict" => Ok(Self::Strict),
            "audit" => Ok(Self::Audit),
            other => Err(format!(
                "unknown policy `{other}` (expected recommended, strict or audit)"
            )),
        }
    }
}

/// Whether a diagnostic was already present in a baseline file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BaselineState {
    /// Not in the baseline: counts toward `--fail-on`.
    New,
    /// In the baseline: still reported, but does not fail the run.
    Existing,
}

/// What a diagnostic claims.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticKind {
    /// A failure at the reported point *can* leave partial state. It is not a
    /// proven bug: Midwrite does not know the application's invariants.
    PotentialHazard,
    /// The analysis could not reach a conclusion for part of the code.
    AnalysisLimitation,
}

/// Stable diagnostic codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiagnosticCode {
    /// MW1001
    DbCommitThenExternal,
    /// MW1002
    ExternalThenDbCommit,
    /// MW1003
    ExternalInsideTransaction,
    /// MW1004
    IndependentDbCommits,
    /// MW1005
    IndependentExternalEffects,
    /// MW9001
    AnalysisIncomplete,
}

impl DiagnosticCode {
    pub const ALL: [Self; 6] = [
        Self::DbCommitThenExternal,
        Self::ExternalThenDbCommit,
        Self::ExternalInsideTransaction,
        Self::IndependentDbCommits,
        Self::IndependentExternalEffects,
        Self::AnalysisIncomplete,
    ];

    pub fn as_str(self) -> &'static str {
        self.info().code
    }

    /// The fixed text fingerprints are computed from. It never changes, so
    /// that baselines keep matching across releases.
    pub fn fingerprint_code(self) -> &'static str {
        match self {
            Self::DbCommitThenExternal => "CP1001",
            Self::ExternalThenDbCommit => "CP1002",
            Self::ExternalInsideTransaction => "CP1003",
            Self::IndependentDbCommits => "CP1004",
            Self::IndependentExternalEffects => "CP1005",
            Self::AnalysisIncomplete => "CP9001",
        }
    }

    pub fn severity(self) -> Severity {
        self.info().severity
    }

    pub fn kind(self) -> DiagnosticKind {
        match self {
            Self::AnalysisIncomplete => DiagnosticKind::AnalysisLimitation,
            _ => DiagnosticKind::PotentialHazard,
        }
    }

    pub fn info(self) -> &'static CodeInfo {
        match self {
            Self::DbCommitThenExternal => &MW1001,
            Self::ExternalThenDbCommit => &MW1002,
            Self::ExternalInsideTransaction => &MW1003,
            Self::IndependentDbCommits => &MW1004,
            Self::IndependentExternalEffects => &MW1005,
            Self::AnalysisIncomplete => &MW9001,
        }
    }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for DiagnosticCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown diagnostic code `{0}`")]
pub struct UnknownCode(pub String);

impl FromStr for DiagnosticCode {
    type Err = UnknownCode;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text_trimmed = text.trim();
        Self::ALL
            .into_iter()
            .find(|code| code.as_str().eq_ignore_ascii_case(text_trimmed))
            .ok_or_else(|| UnknownCode(text.to_owned()))
    }
}

/// Static documentation for a diagnostic code.
#[derive(Debug)]
pub struct CodeInfo {
    pub code: &'static str,
    pub title: &'static str,
    pub severity: Severity,
    /// What the failure leaves behind.
    pub outcome: &'static str,
    pub mitigation: &'static str,
    /// A limitation that applies to every instance of the code.
    pub limitation: &'static str,
    /// Long-form documentation shown by `midwrite explain`.
    pub explanation: &'static str,
}

const GENERAL_LIMITATION: &str = "Midwrite does not know the application's invariants or what code outside this function does; a retry, reconciliation job, or idempotent consumer elsewhere may already repair this state.";

static MW1001: CodeInfo = CodeInfo {
    code: "MW1001",
    title: "database commit followed by an independent external effect",
    severity: Severity::Warning,
    outcome: "the database change is durable but the external effect never happens, and nothing records that it is still owed",
    mitigation: "Record the intent in the same database transaction as the state change (transactional outbox) and perform the external effect from a relay that retries until it succeeds, or make a reconciliation job derive the missing effect from database state.",
    limitation: GENERAL_LIMITATION,
    explanation: r#"MW1001: database commit followed by an independent external effect

A database write commits (on its own, or as part of a transaction that has
completed), and afterwards the function performs an external effect such as
publishing a message. The two are not atomic. If the process crashes, is
redeployed, times out, or the external call fails after the commit, the
database reflects the change but the external effect never happens.

    await db.update(orders).set({ status: "paid" }).where(eq(orders.id, id));
    // <- a crash here leaves a paid order without an event
    await queue.publish("order.completed", { id });

Mitigations
  * Transactional outbox: insert the event into an outbox table inside the
    same transaction as the state change, and publish it from a separate
    relay that retries until it succeeds.
  * Reconciliation: periodically derive missing effects from database state.
  * Make consumers tolerate duplicates so that a relay can safely retry.

What Midwrite checks
  Both operations must be recognized (a configured database receiver or a
  transaction handle, and a configured external effect), and they must occur
  in that order on one path through the function. Branch conditions on that
  path are reported as `pathConditions`; paths whose simple conditions
  contradict each other are discarded, other conditions are not proven
  feasible.

What it does not check
  Whether something outside the function already compensates (retries,
  reconciliation jobs, idempotent consumers)."#,
};

static MW1002: CodeInfo = CodeInfo {
    code: "MW1002",
    title: "external effect followed by an independent database commit",
    severity: Severity::Warning,
    outcome: "other systems observe the external effect, but the database change it implies never commits",
    mitigation: "Commit the database change first and drive the external effect from it (transactional outbox), or make the external effect safe to observe without the database change (idempotent, reversible, or reconciled).",
    limitation: GENERAL_LIMITATION,
    explanation: r#"MW1002: external effect followed by an independent database commit

The function performs an external effect (for example publishes a message)
and only afterwards commits a database write. If the process fails between
the two, or the database write fails, other systems have already observed an
effect that describes state which never became durable.

    await queue.publish("order.completed", { id });
    // <- a crash or a failed write here: consumers act on an order that is not paid
    await db.update(orders).set({ status: "paid" }).where(eq(orders.id, id));

Mitigations
  * Commit first, then publish through a transactional outbox.
  * If the external effect must come first (e.g. charging a card), persist a
    pending record before it and reconcile its outcome afterwards.

What Midwrite checks
  Both operations are recognized and occur in this order on one path.

What it does not check
  Whether consumers can tolerate events for state that does not exist."#,
};

static MW1003: CodeInfo = CodeInfo {
    code: "MW1003",
    title: "external effect executed inside a database transaction before commit",
    severity: Severity::Warning,
    outcome: "the external effect is visible while the transaction's writes may still roll back; consumers may also act before the data is committed",
    mitigation: "Do not perform external effects inside the transaction callback. Insert an outbox row with the transaction handle instead, and publish after commit from a relay.",
    limitation: GENERAL_LIMITATION,
    explanation: r#"MW1003: external effect executed inside a database transaction before commit

An external effect runs inside a transaction callback that also writes to the
database. The writes are not durable until the transaction commits when the
callback returns, but the external effect is visible immediately. If a later
statement throws, the commit fails (constraint, serialization or connection
failure), or the process dies, the writes roll back while the effect remains.
Consumers can also react before the commit and read stale data.

    await db.transaction(async (tx) => {
      await tx.update(orders).set({ status: "paid" }).where(eq(orders.id, id));
      await queue.publish("order.completed", { id }); // visible before commit
    });

Mitigations
  * Replace the external call with an outbox insert on the transaction handle:
        await tx.insert(outbox).values({ topic: "order.completed", payload: { id } });
    and publish from a relay after commit.
  * Moving the call after the transaction instead turns this into MW1001.

What Midwrite checks
  The external effect is recognized, it executes while a recognized
  transaction is open, and that transaction performs at least one recognized
  write on the same path."#,
};

static MW1004: CodeInfo = CodeInfo {
    code: "MW1004",
    title: "independent database commits",
    severity: Severity::Warning,
    outcome: "only some of the database changes are persisted",
    mitigation: "Perform the writes inside one `db.transaction(async (tx) => ...)` and issue every write on the transaction handle `tx`, not on `db`.",
    limitation: GENERAL_LIMITATION,
    explanation: r#"MW1004: independent database commits

Two database writes commit independently: neither runs inside a recognized
transaction, they run in different transactions, or a write inside a
transaction callback uses the root database object (`db`) instead of the
transaction handle (`tx`) and therefore commits on its own. A failure between
the commits leaves only part of the change persisted.

    await db.update(orders).set({ status: "paid" }).where(eq(orders.id, id));
    // <- a crash here leaves a paid order without a payment row
    await db.insert(payments).values({ orderId: id, amount });

    await db.transaction(async (tx) => {
      await tx.update(orders).set({ status: "paid" });
      await db.insert(audit).values({ id });   // commits outside the transaction
    });

Mitigations
  * Wrap the writes in one transaction and use the transaction handle for all
    of them.

What Midwrite checks
  Both writes are recognized and executed (awaited, returned from an async
  function, or explicitly executed) on one path.

What it does not check
  Whether the application considers the two writes related; independent
  writes are sometimes intentional. Under the default `recommended` policy
  this code is a warning only when a write bypasses its enclosing
  transaction; otherwise it is an `info` review candidate."#,
};

static MW1005: CodeInfo = CodeInfo {
    code: "MW1005",
    title: "independent external effects",
    severity: Severity::Warning,
    outcome: "only some of the external effects happen",
    mitigation: "Make each external effect idempotent and drive them from durable state (an outbox or a durable workflow) so that a retry can complete the remaining ones.",
    limitation: GENERAL_LIMITATION,
    explanation: r#"MW1005: independent external effects

Two configured external effects run one after the other. They cannot be made
atomic with a database transaction; a failure between them leaves only the
first one performed.

    await queue.publish("order.completed", { id });
    await mailer.send({ to, subject: "Receipt" });  // never sent if the process dies

Mitigations
  * Record the pending effects durably (outbox, durable workflow engine) and
    perform each one idempotently with retries.

What Midwrite checks
  Both calls match configured external effects and occur in this order on one
  path.

What it does not check
  Whether the second effect is owed because of the first; two effects are
  often independent on purpose. Under the default `recommended` policy this
  code is an `info` review candidate."#,
};

static MW9001: CodeInfo = CodeInfo {
    code: "MW9001",
    title: "analysis incomplete: unsupported or unknown effect semantics",
    severity: Severity::Info,
    outcome: "Midwrite cannot tell whether a partial-completion hazard exists here",
    mitigation: "If the operation performs a side effect, describe it in `midwrite.json` (`databaseReceivers` or `externalEffects`) so that it can be analyzed; otherwise no action is needed.",
    limitation: "This is not a finding. It marks code the analysis did not understand, so that silence is not mistaken for safety.",
    explanation: r#"MW9001: analysis incomplete due to unsupported or unknown effect semantics

Midwrite found code next to recognized side effects that it cannot
interpret, so it makes no claim about it. Typical causes:

  * An awaited call that matches no configuration entry, positioned on a
    path with recognized database or external effects. It might be pure, or
    it might publish, charge, or write somewhere.
  * Raw SQL (`db.execute(sql`...`)`), whose reads or writes are not parsed.
  * A transaction whose callback is not an inline function, or whose handle
    parameter is destructured, so its body cannot be attributed.
  * A member receiver that resembles a configured one but whose path is not
    configured (`this.repo.db` when only `db` or `this.db` is configured).
  * A call of a function (declared in the same file or imported from an
    analyzed module) that is not summarized: it performs no recognized
    operation, the call is recursive, it lies deeper than `--max-call-depth`,
    it writes only through parameters for which the call passes no
    recognized database handle, or the import cannot be linked to a function
    (package, missing module, missing or ambiguous export).
  * Unsupported control flow (for example labeled `break`/`continue`) or a
    function with more paths than the analysis explores.

What to do
  If the call is a side effect, add it to `midwrite.json` so it is analyzed
  (member receivers by their path, for example `"this.db"`).
  If it is not, this note can be ignored; it does not fail the build at the
  default `--fail-on warning` threshold."#,
};

/// A role an effect plays in a diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EffectRole {
    /// The effect that completes before the failure point.
    CompletedFirst,
    /// The effect that has not happened yet at the failure point.
    NotYetPerformed,
    /// A write staged inside a transaction that has not committed.
    StagedWrite,
    /// The transaction whose commit (or rollback) matters.
    Transaction,
    /// The operation Midwrite could not interpret.
    Unanalyzed,
    /// A nearby recognized effect providing context.
    Context,
    /// A call of a function declared in the same file; the operations
    /// listed with it are taken from that function's summary.
    CallSite,
}

/// An operation referenced by a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedEffect {
    pub role: EffectRole,
    pub label: String,
    pub description: String,
    pub location: SourceLocation,
}

/// A branch decision a reported path depends on. Not proven feasible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathCondition {
    pub description: String,
    pub location: SourceLocation,
}

/// Whether the two operations of a finding visibly concern the same values.
/// Descriptive only: it never removes or demotes a finding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Relationship {
    /// `sharedArguments`: both operations' arguments refer to at least one
    /// common variable (by name, in the same function). `noSharedArgument`:
    /// both have argument references and none is common, so the operations
    /// may concern different entities (a review hint). `notComparable`: an
    /// operation comes from a summarized call (its names belong to another
    /// scope) or has no variable in its arguments.
    pub status: &'static str,
    pub shared: Vec<String>,
}

/// The recovery property a [`RecoveryNote`] records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecoveryDeclaration {
    BestEffort,
    Reconciled,
    IdempotentConsumer,
    /// A function-level contract: the function is executed again after a
    /// failure (`recoveryContracts[].strategy: "retried"`).
    Retried,
}

impl From<crate::ir::Recovery> for RecoveryDeclaration {
    fn from(value: crate::ir::Recovery) -> Self {
        match value {
            crate::ir::Recovery::BestEffort => Self::BestEffort,
            crate::ir::Recovery::Reconciled => Self::Reconciled,
            crate::ir::Recovery::IdempotentConsumer => Self::IdempotentConsumer,
        }
    }
}

impl PartialEq<crate::ir::Recovery> for RecoveryDeclaration {
    fn eq(&self, other: &crate::ir::Recovery) -> bool {
        *self == Self::from(*other)
    }
}

/// A recovery property that bears on a finding, with where it comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryNote {
    /// Label of the operation (scope `effect`) or name of the function
    /// (scope `function`) the property is declared for.
    pub effect: String,
    pub declaration: RecoveryDeclaration,
    /// The provenance of the property: `sourceCode` (established by the
    /// analysis), `configuration` (asserted by the user), `runtimeObservation`
    /// (observed by a replay harness; only in verification reports) or
    /// `unknown`. Static analysis attaches `configuration` only.
    pub source: &'static str,
    /// Whether Midwrite established the property. Always `false` for a
    /// configuration assertion.
    pub verified: bool,
    /// What the declaration changed: `informational` (severity lowered
    /// under the `recommended` policy) or `noteOnly`.
    pub effect_on_finding: &'static str,
    /// `effect` (an `externalEffects[]` declaration) or `function` (a
    /// `recoveryContracts[]` contract).
    pub scope: &'static str,
    /// For a contract: where the mechanism is, as the configuration cites it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    /// For a contract: what the mechanism does not do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub severity: Severity,
    pub kind: DiagnosticKind,
    pub title: &'static str,
    pub message: String,
    pub location: SourceLocation,
    pub function: String,
    pub confidence: Confidence,
    /// Always `false` in Phase 1: no runtime reproduction is attempted.
    pub reproduced: bool,
    pub effects: Vec<RelatedEffect>,
    pub failure_sequence: Vec<String>,
    pub path_conditions: Vec<PathCondition>,
    pub evidence: Vec<String>,
    pub mitigation: String,
    pub limitations: Vec<String>,
    /// How certain the reported path is (one input of `confidence`).
    pub path_certainty: PathCertainty,
    /// The weakest recognition evidence among the operations the finding
    /// relies on (the other input of `confidence`).
    pub evidence_strength: EvidenceStrength,
    /// Stable identity of the finding, independent of line numbers: a hash
    /// of the code, the file, the normalized function name and the operations'
    /// labels. Generated anonymous-function positions are excluded. Used by
    /// baselines, SARIF and replay traces.
    pub fingerprint: String,
    /// Set when the run compares against a baseline file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<BaselineState>,
    /// Recovery properties declared in the configuration for the
    /// operations of the finding. Asserted, not verified.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recovery: Vec<RecoveryNote>,
    /// For a finding about two operations: whether their arguments share a
    /// variable. Descriptive only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relationship: Option<Relationship>,
    /// For a finding about two operations: how they are related by the
    /// values their arguments carry (abstract value provenance, across
    /// summarized calls). Descriptive only: never removes or demotes a
    /// finding. Added in Phase 5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_relation: Option<crate::absint::ValueRelation>,
    /// Hazard findings of one report with the same `group` are the same
    /// effect pair on the same failure path, reported in several functions
    /// (a helper and the callers that reach it, several exported wrappers),
    /// with the same severity, confidence and recovery context
    /// ([`Self::group_key`]). A presentation may show them together; each
    /// keeps its own location, function and fingerprint. Meaningful within
    /// one report only (it includes source positions). Assigned when the
    /// report is assembled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// 64-bit FNV-1a, a hash whose output does not depend on the Rust version.
pub fn stable_hash(parts: &[&str]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in part.bytes().chain(std::iter::once(0x1f)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

impl Diagnostic {
    /// The fingerprint of this diagnostic in `file` (normally its own file):
    /// code, file, function, and the role and label of each related effect,
    /// without any line or column, including positions embedded in generated
    /// anonymous-function names. Named functions retain their existing IDs.
    pub fn compute_fingerprint(&self, file: Option<&str>) -> String {
        let mut parts: Vec<String> = vec![
            self.code.fingerprint_code().to_owned(),
            file.unwrap_or("").to_owned(),
            fingerprint_function(&self.function),
        ];
        for effect in &self.effects {
            parts.push(format!("{:?}={}", effect.role, effect.label));
        }
        if self.effects.is_empty() {
            // MW9001 without related effects (unsupported control flow,
            // truncation): the message names the construct.
            parts.push(strip_positions(&self.message));
        }
        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        format!("{:016x}", stable_hash(&parts))
    }
}

impl Diagnostic {
    /// What must agree for two hazard findings to be one presentation
    /// ([`Self::group`]): the code, the operations of the pair (role,
    /// label and location; call sites and context are the callers' own),
    /// the conditions of the path, how certain the path is, severity and
    /// confidence, and every recovery declaration. `None` for analysis
    /// limitations, which are never grouped this way.
    pub fn group_key(&self) -> Option<String> {
        if self.kind != DiagnosticKind::PotentialHazard {
            return None;
        }
        let mut parts: Vec<String> = vec![
            self.code.as_str().to_owned(),
            format!("{:?}", self.severity),
            format!("{:?}", self.confidence),
            format!("{:?}", self.path_certainty),
        ];
        let at = |location: &SourceLocation| {
            format!("{}:{}:{}", location.file, location.line, location.column)
        };
        for effect in &self.effects {
            if matches!(
                effect.role,
                EffectRole::CompletedFirst
                    | EffectRole::NotYetPerformed
                    | EffectRole::StagedWrite
                    | EffectRole::Transaction
            ) {
                parts.push(format!(
                    "{:?}={}@{}",
                    effect.role,
                    effect.label,
                    at(&effect.location)
                ));
            }
        }
        for condition in &self.path_conditions {
            parts.push(format!(
                "if {}@{}",
                condition.description,
                at(&condition.location)
            ));
        }
        for note in &self.recovery {
            parts.push(format!(
                "recovery {:?} {} {}",
                note.declaration, note.scope, note.effect
            ));
        }
        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        Some(format!("{:016x}", stable_hash(&parts)))
    }
}

/// Anonymous functions have display names such as
/// `handler::<anonymous@12:5>`. Keep the enclosing name but exclude the
/// generated position from identity. Only this exact generated syntax is
/// normalized: numbers in user-supplied names remain meaningful.
fn fingerprint_function(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(start) = rest.find("<anonymous@") {
        out.push_str(&rest[..start]);
        rest = &rest[start + "<anonymous@".len()..];
        let position = rest.find('>').and_then(|end| {
            let (line, column) = rest[..end].split_once(':')?;
            let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
            (digits(line) && digits(column)).then_some(end)
        });
        if let Some(end) = position {
            out.push_str("<anonymous>");
            rest = &rest[end + 1..];
        } else {
            out.push_str("<anonymous@");
        }
    }
    out.push_str(rest);
    out
}

/// `text` with `12:5`-style positions removed, so that moving code does not
/// change it.
fn strip_positions(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            let mut run = String::from(c);
            while let Some(&next) = chars.peek() {
                if next.is_ascii_digit() || next == ':' {
                    run.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            if run.contains(':') {
                out.push('#');
            } else {
                out.push_str(&run);
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl Diagnostic {
    /// Ordering key: file, position, code, then related locations.
    pub fn sort_key(&self) -> impl Ord + '_ {
        (
            &self.location.file,
            self.location.line,
            self.location.column,
            self.code,
            self.effects
                .iter()
                .map(|effect| (effect.location.line, effect.location.column))
                .collect::<Vec<_>>(),
            &self.message,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_function_excludes_only_generated_positions() {
        assert_eq!(fingerprint_function("checkout"), "checkout");
        assert_eq!(
            fingerprint_function("checkout::<anonymous@12:5>::<anonymous@99:3>"),
            "checkout::<anonymous>::<anonymous>"
        );
        for name in [
            "Service.v2",
            "12:5",
            "<anonymous@x:5>",
            "<anonymous@12:>",
            "<anonymous@12:5",
        ] {
            assert_eq!(fingerprint_function(name), name);
        }
    }

    #[test]
    fn codes_round_trip_through_strings() {
        for code in DiagnosticCode::ALL {
            assert_eq!(code.as_str().parse::<DiagnosticCode>(), Ok(code));
            assert!(code.info().explanation.starts_with(code.as_str()));
        }
        assert_eq!(
            "mw1001".parse::<DiagnosticCode>(),
            Ok(DiagnosticCode::DbCommitThenExternal)
        );
        assert!("MW0000".parse::<DiagnosticCode>().is_err());
        for code in DiagnosticCode::ALL {
            assert_eq!(code.as_str()[2..], code.fingerprint_code()[2..]);
        }
    }

    #[test]
    fn only_cp9001_is_a_limitation() {
        for code in DiagnosticCode::ALL {
            let is_limitation = code.kind() == DiagnosticKind::AnalysisLimitation;
            assert_eq!(is_limitation, code == DiagnosticCode::AnalysisIncomplete);
        }
    }

    #[test]
    fn severity_ordering() {
        assert!(Severity::Info < Severity::Warning);
        assert!(Severity::Warning < Severity::Error);
    }
}

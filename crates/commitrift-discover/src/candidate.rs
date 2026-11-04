//! Candidate, property and assumption types.
//!
//! The vocabulary is deliberately small and every item carries where it
//! comes from. A property's [`Origin`] is never upgraded by any later step:
//! a counterexample against a [`Origin::HeuristicHypothesis`] stays a
//! counterexample against a hypothesis.

use std::collections::BTreeMap;

use serde::Serialize;

use commitrift_model::invariant::Formula;
use commitrift_model::value::Value;

use crate::effects::Val;

/// The hazard family of a candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    /// A read finds no row with a key, then a row with that key is inserted.
    CheckThenAct,
    /// A read checks a row's state, then the row is moved to another state
    /// by an unconditional update; effects follow the check.
    StatusTransition,
    /// A value is read, then written back as a function of what was read.
    ReadModifyWrite,
    /// A predicate over several rows is read, then a different row is
    /// written so that the predicate changes.
    WriteSkew,
    /// A declared at-least-once delivery runs non-idempotent effects.
    DuplicateProcessing,
    /// An external effect and database writes are not atomic, and the caller
    /// retries after a crash.
    ExternalEffectGap,
    /// A write is repeated after its outcome is unknown.
    AmbiguousCommit,
    /// Rows are locked in an order that depends on the arguments.
    LockOrder,
    /// Phase 12: an insert committed before a crash is repeated by the
    /// caller's declared retry (with a generated key it cannot collide).
    ResubmittedWrite,
    /// Phase 14: a read finds a parent row, then a child row referencing it
    /// is inserted, while another entry deletes the parent.
    ParentChild,
    /// Phase 14: a DECLARED property over what callers are told (return
    /// values); never inferred.
    ReturnValue,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CheckThenAct => "check-then-act",
            Self::StatusTransition => "status-transition",
            Self::ReadModifyWrite => "read-modify-write",
            Self::WriteSkew => "write-skew",
            Self::DuplicateProcessing => "duplicate-processing",
            Self::ExternalEffectGap => "external-effect-gap",
            Self::AmbiguousCommit => "ambiguous-commit",
            Self::LockOrder => "lock-order",
            Self::ResubmittedWrite => "resubmitted-write",
            Self::ParentChild => "parent-child",
            Self::ReturnValue => "return-value",
        }
    }
}

/// Where a property comes from (`docs/phase11/INVARIANT-SYNTHESIS.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Origin {
    /// Derived from a database constraint in the analyzed schema.
    SchemaEnforced,
    /// An explicit assertion in source with a stated translation.
    SourceAsserted,
    /// Written in `commitrift-contracts.json`.
    UserDeclared,
    /// Follows from a declared integration contract (at-least-once
    /// delivery: a consumer must tolerate redelivery).
    FrameworkContract,
    /// Plausible, not established as a requirement.
    HeuristicHypothesis,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SchemaEnforced => "SCHEMA_ENFORCED",
            Self::SourceAsserted => "SOURCE_ASSERTED",
            Self::UserDeclared => "USER_DECLARED",
            Self::FrameworkContract => "FRAMEWORK_CONTRACT",
            Self::HeuristicHypothesis => "HEURISTIC_HYPOTHESIS",
        }
    }
    /// Whether a violation of this property may be called a defect of the
    /// application (given the stated bounds and assumptions).
    pub fn established(self) -> bool {
        !matches!(self, Self::HeuristicHypothesis)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Strength {
    DatabaseIntegrity,
    ApplicationSafety,
    ConcurrencyRelational,
    RecoverySafety,
    BoundedLiveness,
}

/// A property before its values are known: instantiated with the
/// synthesized arguments and rows of each scenario.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "template")]
pub enum Spec {
    /// At most one committed row of `table` with the key values a scenario's
    /// actors use (`key[i]` is the column and how the first actor computes
    /// it), among rows matching `filter`.
    AtMostOne {
        table: String,
        key: Vec<(String, Val)>,
        filter: BTreeMap<String, Value>,
    },
    /// At least `min` rows matching `filter` per value of `groupBy` (the
    /// values a scenario uses).
    AtLeast {
        table: String,
        group_by: Vec<String>,
        filter: BTreeMap<String, Value>,
        min: i64,
    },
    NonNegative {
        table: String,
        column: String,
    },
    /// At most `max` performed effects on `channel` — per actor when
    /// `perRequest` (each request may cause `max`).
    MaxExternal {
        channel: String,
        max: i64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        per_request: bool,
    },
    /// The cell `table.column` of the row with `key` ends with a value some
    /// serial execution of the same actors produces (computed by running
    /// the actors one after the other in the model).
    SerialCell {
        table: String,
        column: String,
        key: Vec<(String, Val)>,
    },
    /// No actor fails.
    NoFailedActors,
    /// Phase 14: every successful call of `function` returned the value of
    /// its parameter path `argument` (how the first actor computes it).
    ReturnsArgument {
        function: Option<String>,
        argument: Val,
    },
    /// Phase 14: no committed `child` row whose `column` is the key (how
    /// the first actor computes it) without a `parent` row whose
    /// `parent_column` is that key.
    NoOrphan {
        child: String,
        column: String,
        parent: String,
        parent_column: String,
        key: Val,
    },
    Formula {
        formula: Formula,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Property {
    pub name: String,
    pub description: String,
    pub origin: Origin,
    pub strength: Strength,
    pub spec: Spec,
    /// Why this property, from which evidence.
    pub rationale: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

/// An assumption a candidate or scenario needs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Assumption {
    /// `invocation`, `concurrency`, `delivery`, `retry`, `fault`,
    /// `external`, `precondition`, `bounds`; Phase 16: `loop`,
    /// `caught-error`, `request`.
    pub kind: String,
    /// `source-derived`, `declared`, `framework`, `discovery-default`.
    pub origin: String,
    pub text: String,
}

impl Assumption {
    pub fn new(kind: &str, origin: &str, text: impl Into<String>) -> Self {
        Self {
            kind: kind.to_owned(),
            origin: origin.to_owned(),
            text: text.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryRef {
    pub selector: String,
    pub kind: String,
    pub name: String,
    /// `file@line:column` of the function node.
    pub anchor: String,
    /// How the entry is invoked: `direct`, `wrapper-awaited`,
    /// `wrapper-awaited-when-guards-pass`, `declared`, `unknown`.
    pub invocation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrapper: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub guards: Vec<String>,
}

/// One operation of a candidate's conflict.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictOp {
    /// Index into the candidate's `entries`.
    pub entry: usize,
    /// `check`, `act`, `read`, `write`, `transition`, `consequence`,
    /// `external`, `write-after`, `first-write`, `retried-write`,
    /// `first-lock`, `second-lock`.
    pub role: String,
    pub operation: String,
    pub location: String,
    pub function: String,
    /// Index of the db effect (or external, for role `external`) in the
    /// entry's summary.
    #[serde(skip)]
    pub effect: usize,
    #[serde(skip)]
    pub is_external: bool,
}

/// A logical resource the candidate's operations share.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    /// `unique-key`, `row`, `row-predicate`, `column`, `external-channel`.
    pub kind: String,
    pub table: String,
    pub columns: Vec<String>,
    pub text: String,
}

/// How the actors' arguments relate in a scenario.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Relation {
    /// Every actor gets the same arguments.
    Same,
    /// The actors differ in the candidate's logical key (a control).
    DistinctKey,
    /// The actors write different rows of one group (write skew).
    DistinctRow,
    /// Two key arguments are swapped between the actors (lock order).
    Swapped,
    /// Everything equal except the idempotency key an external effect
    /// carries (a client that makes a new key per attempt).
    DistinctRequestKey,
}

/// The environment a scenario of the candidate explores.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    pub actors: usize,
    pub relation: Relation,
    /// `on-crash`, `on-failure-or-crash`, `at-least-once`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<String>,
    pub max_crashes: u32,
    pub max_retries: u32,
    pub max_in_flight: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    /// `CR-SYNTH-NNN`, assigned after ranking.
    pub id: String,
    pub fingerprint: String,
    pub class: Class,
    pub title: String,
    pub entries: Vec<EntryRef>,
    /// Index into `entries` of the entry of each actor.
    pub actor_entries: Vec<usize>,
    pub resource: Resource,
    pub conflict: Vec<ConflictOp>,
    pub properties: Vec<Property>,
    /// Protections recognized in source or schema (a unique constraint, a
    /// row lock, a conditional update, a deduplicating channel).
    pub protections: Vec<String>,
    pub assumptions: Vec<Assumption>,
    /// Operations the extractor did not understand, in these entries.
    pub unsupported: Vec<String>,
    /// Whether an unsupported operation lies on a path through the
    /// candidate's operations (it then blocks a strong claim).
    pub unsupported_on_path: bool,
    /// Why no scenario is built (a missing contract or invariant).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
    pub environment: Environment,
    /// What would make a result about this candidate inconclusive.
    pub inconclusive_if: Vec<String>,
    /// Parameter paths the conflict's external effects send as idempotency
    /// keys.
    #[serde(skip)]
    pub request_keys: Vec<String>,
}

impl Candidate {
    pub fn strongest_origin(&self) -> Option<Origin> {
        self.properties.iter().map(|p| p.origin).min()
    }
}

/// A combination considered and rejected, with the reason.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rejected {
    pub class: Class,
    pub entries: Vec<String>,
    pub reason: String,
    pub location: String,
}

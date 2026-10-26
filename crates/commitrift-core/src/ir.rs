//! The Effect IR.
//!
//! A [`FunctionIr`] is a structured, ordered description of the side effects a
//! single function body may perform. It deliberately keeps the *shape* of the
//! source control flow (sequences, branches, loops, `try`, transaction scopes,
//! early exits) instead of flattening it into a CFG: the analysis enumerates
//! paths through this tree, and the tree is easy to print and inspect.
//!
//! Everything a frontend could not understand is kept explicit
//! ([`EffectKind::Unknown`], [`ExitKind::Unsupported`]) so that later stages
//! never mistake "not understood" for "no effect".

use std::fmt::{self, Write as _};

use serde::{Deserialize, Serialize};

use crate::absint::{AbsValue, Provenance};
use crate::location::{FileId, Location};

/// Identifies an effect within one [`FunctionIr`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct EffectId(pub u32);

/// Identifies a recognized transaction scope within one [`FunctionIr`].
/// It is also the index into [`FunctionIr::transactions`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct TransactionId(pub u32);

impl fmt::Display for TransactionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "tx#{}", self.0)
    }
}

/// Identifies a call site within one [`FunctionIr`] at which the body of a
/// function declared in the same file was inlined ([`Node::Call`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct CallId(pub u32);

impl fmt::Display for CallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "call#{}", self.0)
    }
}

/// The lowered effects of one source file.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileIr {
    pub path: String,
    pub functions: Vec<FunctionIr>,
    /// Other files that locations in `functions` refer to
    /// ([`Location::file`] indexes this list). Empty unless operations were
    /// inlined from functions declared in other modules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
}

/// The lowered effects of one function body.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionIr {
    pub name: String,
    pub location: Location,
    pub is_async: bool,
    /// Metadata for every transaction scope in `body`, indexed by [`TransactionId`].
    pub transactions: Vec<TransactionInfo>,
    pub body: Vec<Node>,
    /// What the function returns, as an abstract value over its parameters
    /// (`param#i`), its effects' results and its calls' results. `None`
    /// when the frontend did not compute values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returns: Option<Box<AbsValue>>,
}

impl FunctionIr {
    pub fn transaction(&self, id: TransactionId) -> &TransactionInfo {
        &self.transactions[id.0 as usize]
    }

    /// All effects in the body, in source order.
    pub fn effects(&self) -> Vec<&Effect> {
        let mut out = Vec::new();
        collect_effects(&self.body, &mut out);
        out
    }

    /// Whether the function contains at least one effect recognized as a
    /// database operation or a configured external effect.
    pub fn has_recognized_effects(&self) -> bool {
        !self.transactions.is_empty()
            || self
                .effects()
                .iter()
                .any(|effect| effect.recognition.is_some())
    }
}

impl FunctionIr {
    /// The effect with identifier `id`, for in-place updates.
    pub fn effect_mut(&mut self, id: EffectId) -> Option<&mut Effect> {
        fn find(nodes: &mut [Node], id: EffectId) -> Option<&mut Effect> {
            for node in nodes {
                let found = match node {
                    Node::Effect(effect) if effect.id == id => return Some(effect),
                    Node::Effect(_) | Node::Exit(_) => None,
                    Node::Transaction(scope) => find(&mut scope.body, id),
                    Node::Branch(branch) => branch
                        .arms
                        .iter_mut()
                        .find_map(|arm| find(&mut arm.body, id)),
                    Node::Switch(switch) => switch
                        .cases
                        .iter_mut()
                        .find_map(|case| find(&mut case.body, id)),
                    Node::Loop(lp) => find(&mut lp.body, id),
                    Node::Call(call) => find(&mut call.body, id),
                    Node::Try(try_node) => find(&mut try_node.block, id)
                        .or_else(|| {
                            try_node
                                .handler
                                .as_mut()
                                .and_then(|handler| find(&mut handler.body, id))
                        })
                        .or_else(|| {
                            try_node
                                .finalizer
                                .as_mut()
                                .and_then(|finalizer| find(finalizer, id))
                        }),
                };
                if found.is_some() {
                    return found;
                }
            }
            None
        }
        find(&mut self.body, id)
    }

    /// Apply `f` to every location in the function: its own, its nodes',
    /// its transactions' and the call sites recorded in summaries.
    pub fn map_locations(&mut self, f: &mut dyn FnMut(&mut Location)) {
        f(&mut self.location);
        for info in &mut self.transactions {
            info.map_locations(f);
        }
        map_node_locations(&mut self.body, f);
    }
}

impl TransactionInfo {
    pub fn map_locations(&mut self, f: &mut dyn FnMut(&mut Location)) {
        f(&mut self.location);
        if let Some(summary) = &mut self.summary {
            summary.map_locations(f);
        }
    }
}

impl SummaryOrigin {
    pub fn map_locations(&mut self, f: &mut dyn FnMut(&mut Location)) {
        for site in &mut self.calls {
            f(&mut site.location);
            f(&mut site.callee_location);
        }
    }
}

/// Apply `f` to every location in `nodes`, recursively.
pub fn map_node_locations(nodes: &mut [Node], f: &mut dyn FnMut(&mut Location)) {
    for node in nodes {
        match node {
            Node::Effect(effect) => {
                f(&mut effect.location);
                if let Some(summary) = &mut effect.summary {
                    summary.map_locations(f);
                }
            }
            Node::Transaction(scope) => map_node_locations(&mut scope.body, f),
            Node::Branch(branch) => {
                f(&mut branch.location);
                for arm in &mut branch.arms {
                    map_node_locations(&mut arm.body, f);
                }
            }
            Node::Switch(switch) => {
                f(&mut switch.location);
                for case in &mut switch.cases {
                    map_node_locations(&mut case.body, f);
                }
            }
            Node::Loop(lp) => {
                f(&mut lp.location);
                map_node_locations(&mut lp.body, f);
            }
            Node::Try(try_node) => {
                f(&mut try_node.location);
                map_node_locations(&mut try_node.block, f);
                if let Some(handler) = &mut try_node.handler {
                    f(&mut handler.location);
                    map_node_locations(&mut handler.body, f);
                }
                if let Some(finalizer) = &mut try_node.finalizer {
                    map_node_locations(finalizer, f);
                }
            }
            Node::Exit(exit) => f(&mut exit.location),
            Node::Call(call) => {
                f(&mut call.location);
                f(&mut call.callee_location);
                map_node_locations(&mut call.body, f);
            }
        }
    }
}

fn collect_effects<'ir>(nodes: &'ir [Node], out: &mut Vec<&'ir Effect>) {
    for node in nodes {
        match node {
            Node::Effect(effect) => out.push(effect),
            Node::Transaction(scope) => collect_effects(&scope.body, out),
            Node::Branch(branch) => {
                for arm in &branch.arms {
                    collect_effects(&arm.body, out);
                }
            }
            Node::Switch(switch) => {
                for case in &switch.cases {
                    collect_effects(&case.body, out);
                }
            }
            Node::Loop(lp) => collect_effects(&lp.body, out),
            Node::Call(call) => collect_effects(&call.body, out),
            Node::Try(try_node) => {
                collect_effects(&try_node.block, out);
                if let Some(handler) = &try_node.handler {
                    collect_effects(&handler.body, out);
                }
                if let Some(finalizer) = &try_node.finalizer {
                    collect_effects(finalizer, out);
                }
            }
            Node::Exit(_) => {}
        }
    }
}

/// Static facts about a recognized transaction scope.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionInfo {
    pub id: TransactionId,
    /// The enclosing transaction when this is a nested transaction (savepoint)
    /// started from a parent transaction handle.
    pub parent: Option<TransactionId>,
    pub label: String,
    pub location: Location,
    /// Name of the callback parameter that holds the transaction handle.
    pub handle: Option<String>,
    pub recognition: Recognition,
    /// Set when the transaction is started inside another function and was
    /// inlined from that function's summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<SummaryOrigin>,
    /// Set when the transaction is started on a parameter of the function
    /// (`db.transaction(...)` where `db` is a parameter): when the function
    /// is inlined at a call that passes a transaction handle for it, the
    /// transaction is nested in (a savepoint of) the caller's transaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_parameter: Option<u32>,
}

/// One step of a function body.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "node", rename_all = "camelCase")]
pub enum Node {
    Effect(Effect),
    Transaction(TransactionScope),
    Branch(Branch),
    Switch(Switch),
    Loop(Loop),
    Try(Try),
    Exit(Exit),
    /// An awaited call of a function declared in the same file, with the
    /// callee's summarized body inlined.
    Call(CallScope),
}

/// An operation that may have an observable effect outside the process.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Effect {
    pub id: EffectId,
    pub kind: EffectKind,
    /// Short source-derived label such as `db.update(orders)`. Shared: an
    /// inlined operation is copied into every caller (Phase 8).
    pub label: std::sync::Arc<str>,
    pub location: Location,
    pub execution: Execution,
    /// Why the frontend believes this is the effect it claims to be.
    /// `None` for unknown effects. Boxed: most IR nodes are not recognized
    /// effects, and every node is as large as the largest variant.
    pub recognition: Option<Box<Recognition>>,
    /// Set when the effect is performed inside a function declared in the
    /// same file and was inlined from that function's summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<SummaryOrigin>,
    /// Variables the operation's arguments refer to (`id`, `input.orderId`),
    /// by name within the function that contains the call, sorted. Table
    /// names are excluded. Used only to describe whether two operations
    /// visibly concern the same values; never to suppress a finding.
    #[serde(default, skip_serializing_if = "<[String]>::is_empty")]
    pub references: Box<[String]>,
    /// Where the values of the operation's arguments come from (abstract
    /// value provenance), in the frame of the function this IR belongs to.
    /// `None` for unknown effects and when no values were computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Box<Provenance>>,
}

impl Effect {
    /// The outermost inlined call through which this function reaches the
    /// effect, if it comes from a summary.
    pub fn call(&self) -> Option<&CallSite> {
        self.summary.as_ref().and_then(SummaryOrigin::outermost)
    }
}

/// Where an inlined operation comes from: the chain of same-file calls
/// through which the function reaches it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryOrigin {
    /// The calls, outermost (in the function being analyzed) first.
    pub calls: Vec<CallSite>,
    /// Whether the outermost callee performs the operation on every path
    /// through it. Operations on only some paths lower confidence.
    pub on_every_path: bool,
}

impl SummaryOrigin {
    pub fn outermost(&self) -> Option<&CallSite> {
        self.calls.first()
    }
}

/// A call of a function declared in the same file, as recorded in a summary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallSite {
    pub call: CallId,
    /// Name of the called function. Shared (`Arc`): every operation inlined
    /// through a call carries the whole chain of its call sites (Phase 8).
    pub callee: std::sync::Arc<str>,
    /// The call as written, such as `notify(id)`.
    pub label: std::sync::Arc<str>,
    /// Location of the call expression.
    pub location: Location,
    /// Location of the callee's declaration.
    pub callee_location: Location,
    /// How the call target was determined, when not by a plain function
    /// name or import (method dispatch, callback, immediate invocation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<std::sync::Arc<CallResolution>>,
}

/// How a call target was determined beyond a plain function name or import.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CallResolution {
    /// A method found by dispatch on the receiver's class.
    Method {
        /// The class the method is declared in.
        class: String,
        /// `true` when the receiver's class is exactly known (`new C()`);
        /// `false` for `this`, resolved because no analyzed subclass
        /// overrides the method.
        exact: bool,
        /// Set when the resolution relies on an assumption the analysis
        /// cannot check (an unanalyzed superclass, an escaped receiver).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        assumption: Option<String>,
    },
    /// The function passed by the caller for parameter `parameter` of an
    /// enclosing callee (`await action()`).
    Callback { parameter: u32 },
    /// A function expression called where it is defined (`(async () => …)()`).
    Immediate,
}

impl CallResolution {
    pub fn assumption(&self) -> Option<&str> {
        match self {
            Self::Method { assumption, .. } => assumption.as_deref(),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum EffectKind {
    DatabaseRead {
        handle: DbHandle,
        table: Option<String>,
    },
    /// An executed database write. Whether and when it commits depends on
    /// `handle` and on the transaction scopes enclosing it on a given path.
    DatabaseWrite {
        operation: WriteOperation,
        table: Option<String>,
        handle: DbHandle,
    },
    /// A configured effect outside the database (message publish, HTTP, ...).
    External {
        category: ExternalCategory,
        /// The configuration declares that the application accepts this
        /// effect sometimes not being performed (`bestEffort`).
        #[serde(
            rename = "bestEffort",
            default,
            skip_serializing_if = "std::ops::Not::not"
        )]
        best_effort: bool,
        /// A recovery property the configuration asserts for this effect
        /// (never inferred, never verified).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recovery: Option<Recovery>,
    },
    /// A call whose semantics are not known to CommitRift.
    Unknown {
        reason: UnknownReason,
        detail: String,
    },
}

impl EffectKind {
    pub fn describe(&self) -> String {
        match self {
            Self::DatabaseRead { table, .. } => match table {
                Some(table) => format!("database read from `{table}`"),
                None => "database read".to_owned(),
            },
            Self::DatabaseWrite {
                operation, table, ..
            } => match table {
                Some(table) => format!("database {operation} on `{table}`"),
                None => format!("database {operation}"),
            },
            Self::External {
                category,
                best_effort: false,
                ..
            } => format!("external effect ({category})"),
            Self::External { category, .. } => format!("best-effort external effect ({category})"),
            Self::Unknown { reason, .. } => format!("unknown effect ({reason})"),
        }
    }
}

/// A recovery property that the configuration *asserts* for an external
/// effect. CommitRift does not verify it; it changes how a finding is
/// presented, never whether the failure window is reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Recovery {
    /// The application accepts that the effect is sometimes not performed
    /// (same as `"bestEffort": true`).
    BestEffort,
    /// A process outside the analyzed code derives this effect from
    /// committed database state and performs it when it is missing, so a
    /// lost effect is eventually repaired.
    Reconciled,
    /// Receivers deduplicate repeated deliveries by a stable key, so
    /// performing the effect again (a retry) is harmless. Says nothing about
    /// an effect that is never performed.
    IdempotentConsumer,
}

impl Recovery {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BestEffort => "bestEffort",
            Self::Reconciled => "reconciled",
            Self::IdempotentConsumer => "idempotentConsumer",
        }
    }

    /// Whether the declaration covers an effect that is never performed
    /// (the claim of CP1001 and CP1005 about their second effect).
    pub fn covers_lost_effect(self) -> bool {
        matches!(self, Self::BestEffort | Self::Reconciled)
    }
}

/// The database handle an operation is issued on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DbHandle {
    /// The root database object (auto-commit outside a transaction).
    Root { name: String },
    /// The handle passed to a recognized transaction callback.
    Transaction { id: TransactionId },
    /// A parameter of the function (`index` counts from 0). Which handle it
    /// holds is decided by each caller: when the function is inlined at a
    /// call, the argument replaces it (see
    /// [`crate::analysis::CallArgument`]). Analyzed on its own, the function
    /// treats a `configured` parameter (its name matches a configured
    /// database receiver) as the root handle of that name, and an operation
    /// on any other parameter as an unrecognized call.
    Parameter {
        index: u32,
        name: String,
        configured: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WriteOperation {
    Insert,
    Update,
    Delete,
}

impl fmt::Display for WriteOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Insert => "insert",
            Self::Update => "update",
            Self::Delete => "delete",
        })
    }
}

/// Category of a configured external effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalCategory {
    MessagePublish,
    HttpRequest,
    Email,
    Payment,
    FileWrite,
    Other,
}

impl fmt::Display for ExternalCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MessagePublish => "message-publish",
            Self::HttpRequest => "http-request",
            Self::Email => "email",
            Self::Payment => "payment",
            Self::FileWrite => "file-write",
            Self::Other => "other",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UnknownReason {
    /// An awaited call that matches no configured or recognized effect.
    UnrecognizedCall,
    /// A method on a recognized database handle that CommitRift does not model.
    UnrecognizedDatabaseMethod,
    /// Raw SQL (for example `db.execute(sql\`...\`)`); it may read or write.
    RawSql,
    /// A transaction whose callback body cannot be analyzed.
    OpaqueTransaction,
    /// A call on a member-expression receiver such as `this.db` or `ctx.queue`
    /// that resembles a configured receiver, but whose path is not configured.
    UnsupportedReceiver,
    /// A database operation issued on a parameter of an inlined function,
    /// where the caller's argument is not a recognized database or
    /// transaction handle.
    UnresolvedHandle,
}

impl UnknownReason {
    /// Whether the operation is reported even when no recognized effect is
    /// nearby: it certainly involves the database or a configured receiver.
    pub fn is_reported_alone(self) -> bool {
        !matches!(self, Self::UnrecognizedCall | Self::UnresolvedHandle)
    }
}

impl fmt::Display for UnknownReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnrecognizedCall => "unrecognized call",
            Self::UnrecognizedDatabaseMethod => "unmodeled database method",
            Self::RawSql => "raw SQL",
            Self::OpaqueTransaction => "transaction body not analyzable",
            Self::UnsupportedReceiver => "unsupported receiver expression",
            Self::UnresolvedHandle => "database handle argument not recognized",
        })
    }
}

/// How an effect is executed relative to the code that follows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Execution {
    /// `await`ed: later statements run after it settles.
    Awaited,
    /// Returned from an `async` function, which adopts and awaits it.
    Returned,
    /// Started without being awaited; it may complete after later operations.
    FireAndForget,
    /// Started together with other operations (e.g. `Promise.all`).
    Concurrent,
}

impl Execution {
    pub fn is_ordered(self) -> bool {
        matches!(self, Self::Awaited | Self::Returned)
    }
}

/// Evidence for why an operation was recognized.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recognition {
    pub basis: RecognitionBasis,
    /// The receiver or callee identifier as written in the source.
    pub receiver: String,
    /// Human-readable explanation of how the identifier was resolved.
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecognitionBasis {
    /// Matched a configured name only.
    ConfiguredName,
    /// Matched a configured member path such as `this.db` or `ctx.db`: only
    /// the names along the path are checked, not what the object holds.
    /// Weakest evidence.
    ConfiguredMemberPath,
    /// Imported from the module a configuration entry names.
    ConfiguredImport,
    /// Initialized in the same file by `drizzle(...)` imported from `drizzle-orm`.
    DrizzleConstructor,
    /// The parameter of a recognized transaction callback.
    TransactionHandle,
    /// A Drizzle-shaped call on a parameter whose name is not configured: a
    /// database operation only if a caller passes a database handle for it.
    Parameter,
}

/// How strong the evidence for a recognized operation is, from weakest to
/// strongest. Used to cap the confidence of diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EvidenceStrength {
    /// Not recognized (unknown effect).
    Unknown,
    /// Only the names along a member path were checked (`this.db`).
    MemberPath,
    /// A configured identifier name, without checking where it comes from.
    ConfiguredName,
    /// Imported from the module a configuration entry names.
    ConfiguredImport,
    /// Proven by provenance: a transaction callback parameter or a
    /// `drizzle(...)` instance.
    Provenance,
}

impl EvidenceStrength {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::MemberPath => "memberPath",
            Self::ConfiguredName => "configuredName",
            Self::ConfiguredImport => "configuredImport",
            Self::Provenance => "provenance",
        }
    }
}

impl RecognitionBasis {
    pub fn strength(self) -> EvidenceStrength {
        match self {
            Self::ConfiguredMemberPath => EvidenceStrength::MemberPath,
            Self::ConfiguredName => EvidenceStrength::ConfiguredName,
            Self::ConfiguredImport => EvidenceStrength::ConfiguredImport,
            Self::DrizzleConstructor | Self::TransactionHandle => EvidenceStrength::Provenance,
            Self::Parameter => EvidenceStrength::Unknown,
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::Parameter => {
                "Drizzle-shaped call on a parameter (a database operation only where a caller passes a database handle)"
            }
            Self::ConfiguredName => "name-based match against the configuration",
            Self::ConfiguredMemberPath => {
                "member-path match against the configuration (only the names along the path are checked)"
            }
            Self::ConfiguredImport => "import-based match against the configuration",
            Self::DrizzleConstructor => "created by `drizzle(...)` from `drizzle-orm` in this file",
            Self::TransactionHandle => "parameter of a recognized transaction callback",
        }
    }
}

/// A recognized database transaction. Writes issued on its handle become
/// durable only when the outermost enclosing transaction commits, which happens
/// when its callback completes normally.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionScope {
    pub transaction: TransactionId,
    pub execution: Execution,
    pub body: Vec<Node>,
}

/// The inlined body of a function declared in the same file, at an awaited
/// call site. A `return` inside ends the callee, not the enclosing function;
/// an exception propagates out of the call.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallScope {
    pub call: CallId,
    /// Name of the called function.
    pub callee: std::sync::Arc<str>,
    /// The call as written, such as `notify(id)`.
    pub label: std::sync::Arc<str>,
    /// Location of the call expression.
    pub location: Location,
    /// Location of the callee's declaration.
    pub callee_location: Location,
    pub execution: Execution,
    pub body: Vec<Node>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Box<CallResolution>>,
    /// The callee was proven inert (no database write, transaction,
    /// external effect or unknown call on any path; see
    /// `docs/phase5/INTERPROCEDURAL-SUMMARIES.md`): the body is empty and
    /// the call is resolved, not unknown.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub inert: bool,
}

/// A two-or-more-way choice (`if`, `?:`, `&&`, `||`, `??`).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub kind: BranchKind,
    /// Source text of the condition (possibly truncated).
    pub condition: String,
    pub location: Location,
    pub arms: Vec<Arm>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BranchKind {
    If,
    Conditional,
    Logical,
}

impl fmt::Display for BranchKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::If => "if",
            Self::Conditional => "conditional expression",
            Self::Logical => "logical expression",
        })
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Arm {
    pub label: String,
    pub body: Vec<Node>,
    /// What taking this arm implies about variables that never change in the
    /// function. Used to discard paths whose decisions contradict each other
    /// (`if (a) ...; if (!a) ...`). Empty when nothing is known: an empty
    /// list never makes a path infeasible.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<Fact>,
}

impl Arm {
    pub fn new(label: impl Into<String>, body: Vec<Node>) -> Self {
        Self {
            label: label.into(),
            body,
            facts: Vec::new(),
        }
    }
}

/// A fact implied by taking a branch arm. Facts only describe *variables*:
/// bindings the frontend has proven are never reassigned, so that every read
/// in the function sees the same value. Member accesses, calls and anything
/// else are never facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Fact {
    /// The arm can never be taken (its condition is a constant).
    Never,
    /// The variable is truthy (`value: true`) or falsy.
    Truthy { variable: String, value: bool },
    /// The variable is strictly equal (`===`) to the literal.
    Equals { variable: String, literal: Literal },
    /// The variable is not strictly equal to the literal.
    NotEquals { variable: String, literal: Literal },
    /// The variable holds an object (an object or array literal, a function):
    /// truthy and not equal to any primitive literal.
    Object { variable: String },
}

impl Fact {
    pub fn variable(&self) -> Option<&str> {
        match self {
            Self::Never => None,
            Self::Truthy { variable, .. }
            | Self::Equals { variable, .. }
            | Self::NotEquals { variable, .. }
            | Self::Object { variable } => Some(variable),
        }
    }

    /// The same fact about a renamed variable.
    pub fn renamed(&self, rename: impl Fn(&str) -> String) -> Self {
        match self {
            Self::Never => Self::Never,
            Self::Truthy { variable, value } => Self::Truthy {
                variable: rename(variable),
                value: *value,
            },
            Self::Equals { variable, literal } => Self::Equals {
                variable: rename(variable),
                literal: literal.clone(),
            },
            Self::NotEquals { variable, literal } => Self::NotEquals {
                variable: rename(variable),
                literal: literal.clone(),
            },
            Self::Object { variable } => Self::Object {
                variable: rename(variable),
            },
        }
    }

    /// Whether `self` and `other` cannot hold at the same time.
    pub fn contradicts(&self, other: &Self) -> bool {
        if matches!(self, Self::Never) || matches!(other, Self::Never) {
            return true;
        }
        if self.variable() != other.variable() {
            return false;
        }
        match (self, other) {
            (Self::Truthy { value: a, .. }, Self::Truthy { value: b, .. }) => a != b,
            (Self::Equals { literal: a, .. }, Self::Equals { literal: b, .. }) => a != b,
            (Self::Equals { literal: a, .. }, Self::NotEquals { literal: b, .. })
            | (Self::NotEquals { literal: b, .. }, Self::Equals { literal: a, .. }) => a == b,
            (Self::Equals { literal, .. }, Self::Truthy { value, .. })
            | (Self::Truthy { value, .. }, Self::Equals { literal, .. }) => {
                literal.is_truthy() != *value
            }
            (Self::Object { .. }, Self::Equals { .. })
            | (Self::Equals { .. }, Self::Object { .. }) => true,
            (Self::Object { .. }, Self::Truthy { value, .. })
            | (Self::Truthy { value, .. }, Self::Object { .. }) => !*value,
            _ => false,
        }
    }
}

/// A primitive literal a variable can be compared with.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum Literal {
    Boolean(bool),
    /// A string literal's value.
    String(String),
    /// A finite number in canonical form (`1`, `0.5`); `0` and `-0` are both `0`.
    Number(String),
    Null,
    Undefined,
}

impl Literal {
    /// The literal's truthiness in JavaScript.
    pub fn is_truthy(&self) -> bool {
        match self {
            Self::Boolean(value) => *value,
            Self::String(value) => !value.is_empty(),
            Self::Number(value) => value != "0",
            Self::Null | Self::Undefined => false,
        }
    }
}

/// A `switch` statement. Execution enters at one case and falls through
/// subsequent cases until a `break` or another exit.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Switch {
    pub discriminant: String,
    pub location: Location,
    pub cases: Vec<Arm>,
    pub has_default: bool,
}

/// A loop. The analysis models it as executing zero times or once
/// (`runs_at_least_once` loops: exactly once); further iterations are not modeled.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Loop {
    pub kind: LoopKind,
    pub header: String,
    pub location: Location,
    pub runs_at_least_once: bool,
    pub body: Vec<Node>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LoopKind {
    For,
    ForOf,
    ForIn,
    While,
    DoWhile,
    /// A callback run once per element by an array method (`map`,
    /// `forEach`): zero or more times, never awaited by the method itself.
    Callback,
}

impl fmt::Display for LoopKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::For => "for",
            Self::ForOf => "for-of",
            Self::ForIn => "for-in",
            Self::While => "while",
            Self::DoWhile => "do-while",
            Self::Callback => "array callback",
        })
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Try {
    pub location: Location,
    pub block: Vec<Node>,
    pub handler: Option<Handler>,
    pub finalizer: Option<Vec<Node>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Handler {
    pub location: Location,
    pub body: Vec<Node>,
}

/// A statement that ends the current path or scope.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exit {
    pub kind: ExitKind,
    pub location: Location,
    /// For [`ExitKind::Rollback`]: the transaction being rolled back.
    pub transaction: Option<TransactionId>,
    /// For [`ExitKind::Unsupported`]: what was not understood.
    pub detail: Option<String>,
    /// For a `return` of an inline transaction callback whose result is
    /// bound to a variable (`const result = await db.transaction(...)`):
    /// what the returned value implies about that variable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<Fact>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExitKind {
    Return,
    Throw,
    /// An explicit transaction rollback (it throws inside the callback).
    Rollback,
    Break,
    Continue,
    /// Control flow the frontend cannot model. Paths stop here and the
    /// analysis reports that it is incomplete.
    Unsupported,
}

// ---------------------------------------------------------------------------
// Text rendering (used by `commitrift ir` and in tests).
// ---------------------------------------------------------------------------

impl FileIr {
    /// Render the IR as indented text.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "file {}", self.path);
        for function in &self.functions {
            function.write_text(&mut out, 1, &self.sources);
        }
        out
    }
}

/// Formats a location for IR text: `line:column`, or `file:line:column` for
/// a location in another file.
struct At<'a> {
    location: Location,
    sources: &'a [String],
}

impl fmt::Display for At<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self
            .location
            .file
            .and_then(|id| self.sources.get(id.0 as usize))
        {
            Some(file) => write!(f, "{file}:{}", self.location),
            None => match self.location.file {
                Some(FileId(id)) => write!(f, "<file {id}>:{}", self.location),
                None => write!(f, "{}", self.location),
            },
        }
    }
}

impl FunctionIr {
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        self.write_text(&mut out, 0, &[]);
        out
    }

    fn write_text(&self, out: &mut String, depth: usize, sources: &[String]) {
        let asyncness = if self.is_async { "async " } else { "" };
        line(
            out,
            depth,
            format_args!("{asyncness}fn {} @ {}", self.name, self.location),
        );
        if self.body.is_empty() {
            line(out, depth + 1, format_args!("(no effects)"));
        }
        self.write_nodes(out, &self.body, depth + 1, sources);
    }

    fn write_nodes(&self, out: &mut String, nodes: &[Node], depth: usize, sources: &[String]) {
        for node in nodes {
            self.write_node(out, node, depth, sources);
        }
    }

    fn write_node(&self, out: &mut String, node: &Node, depth: usize, sources: &[String]) {
        let at = |location: Location| At { location, sources };
        match node {
            Node::Effect(effect) => {
                let handle = match &effect.kind {
                    EffectKind::DatabaseRead { handle, .. }
                    | EffectKind::DatabaseWrite { handle, .. } => match handle {
                        DbHandle::Root { name } => format!(" via {name}"),
                        DbHandle::Transaction { id } => format!(" via {id}"),
                        DbHandle::Parameter { index, name, .. } => {
                            format!(" via parameter #{index} `{name}`")
                        }
                    },
                    _ => String::new(),
                };
                let basis = effect
                    .recognition
                    .as_ref()
                    .map(|r| format!(" [{:?}]", r.basis))
                    .unwrap_or_default();
                let mut summary = effect
                    .summary
                    .as_ref()
                    .map(summary_note)
                    .unwrap_or_default();
                if let Some(provenance) = &effect.provenance {
                    let inputs = provenance
                        .inputs
                        .iter()
                        .map(|input| {
                            if input.exact {
                                input.atom.to_string()
                            } else {
                                format!("~{}", input.atom)
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    let unknown = if provenance.unknown {
                        "; some unknown"
                    } else {
                        ""
                    };
                    summary.push_str(&format!(" <inputs: {inputs}{unknown}>"));
                }
                line(
                    out,
                    depth,
                    format_args!(
                        "effect#{} {}: {}{handle} @ {} ({:?}){basis}{summary}",
                        effect.id.0,
                        effect.kind.describe(),
                        effect.label,
                        at(effect.location),
                        effect.execution,
                    ),
                );
            }
            Node::Transaction(scope) => {
                let info = self.transaction(scope.transaction);
                let parent = info
                    .parent
                    .map(|p| format!(" nested in {p}"))
                    .unwrap_or_default();
                let handle = info.handle.as_deref().unwrap_or("<none>");
                let summary = info.summary.as_ref().map(summary_note).unwrap_or_default();
                line(
                    out,
                    depth,
                    format_args!(
                        "{} begin {} handle={handle}{parent} @ {} ({:?}){summary}",
                        info.id,
                        info.label,
                        at(info.location),
                        scope.execution
                    ),
                );
                self.write_nodes(out, &scope.body, depth + 1, sources);
                line(
                    out,
                    depth,
                    format_args!("{} commit (on normal completion)", info.id),
                );
            }
            Node::Branch(branch) => {
                line(
                    out,
                    depth,
                    format_args!(
                        "{} `{}` @ {}",
                        branch.kind,
                        branch.condition,
                        at(branch.location)
                    ),
                );
                for arm in &branch.arms {
                    line(
                        out,
                        depth + 1,
                        format_args!("{}:{}", arm.label, facts_note(&arm.facts)),
                    );
                    self.write_body(out, &arm.body, depth + 2, sources);
                }
            }
            Node::Switch(switch) => {
                line(
                    out,
                    depth,
                    format_args!("switch `{}` @ {}", switch.discriminant, at(switch.location)),
                );
                for case in &switch.cases {
                    line(
                        out,
                        depth + 1,
                        format_args!("{}:{}", case.label, facts_note(&case.facts)),
                    );
                    self.write_body(out, &case.body, depth + 2, sources);
                }
            }
            Node::Loop(lp) => {
                let once = if lp.runs_at_least_once {
                    " (runs at least once)"
                } else {
                    ""
                };
                line(
                    out,
                    depth,
                    format_args!(
                        "loop {} `{}`{once} @ {}",
                        lp.kind,
                        lp.header,
                        at(lp.location)
                    ),
                );
                self.write_body(out, &lp.body, depth + 1, sources);
            }
            Node::Try(try_node) => {
                line(out, depth, format_args!("try @ {}", at(try_node.location)));
                self.write_body(out, &try_node.block, depth + 1, sources);
                if let Some(handler) = &try_node.handler {
                    line(out, depth, format_args!("catch @ {}", at(handler.location)));
                    self.write_body(out, &handler.body, depth + 1, sources);
                }
                if let Some(finalizer) = &try_node.finalizer {
                    line(out, depth, format_args!("finally"));
                    self.write_body(out, finalizer, depth + 1, sources);
                }
            }
            Node::Call(call) => {
                let resolution = match call.resolution.as_deref() {
                    None => String::new(),
                    Some(CallResolution::Method {
                        class,
                        exact,
                        assumption,
                    }) => format!(
                        " [method of {class}, {}{}]",
                        if *exact { "exact receiver" } else { "via this" },
                        assumption
                            .as_ref()
                            .map(|a| format!("; assumes {a}"))
                            .unwrap_or_default()
                    ),
                    Some(CallResolution::Callback { parameter }) => {
                        format!(" [callback passed for parameter #{parameter}]")
                    }
                    Some(CallResolution::Immediate) => " [immediately invoked]".to_owned(),
                };
                let verb = if call.inert {
                    "proven inert:"
                } else {
                    "inlines"
                };
                line(
                    out,
                    depth,
                    format_args!(
                        "{} {} {verb} `{}` (declared @ {}) @ {} ({:?}){resolution}",
                        call.call,
                        call.label,
                        call.callee,
                        at(call.callee_location),
                        at(call.location),
                        call.execution
                    ),
                );
                if !call.inert {
                    self.write_body(out, &call.body, depth + 1, sources);
                }
                line(out, depth, format_args!("{} end", call.call));
            }
            Node::Exit(exit) => {
                let mut text = format!("{:?}", exit.kind).to_lowercase();
                if let Some(tx) = exit.transaction {
                    let _ = write!(text, " {tx}");
                }
                if let Some(detail) = &exit.detail {
                    let _ = write!(text, " ({detail})");
                }
                text.push_str(&facts_note(&exit.facts));
                line(out, depth, format_args!("{text} @ {}", at(exit.location)));
            }
        }
    }

    fn write_body(&self, out: &mut String, nodes: &[Node], depth: usize, sources: &[String]) {
        if nodes.is_empty() {
            line(out, depth, format_args!("(nothing)"));
        } else {
            self.write_nodes(out, nodes, depth, sources);
        }
    }
}

fn facts_note(facts: &[Fact]) -> String {
    if facts.is_empty() {
        return String::new();
    }
    let text = facts
        .iter()
        .map(|fact| match fact {
            Fact::Never => "never taken".to_owned(),
            Fact::Truthy { variable, value } => {
                format!("{variable} is {}", if *value { "truthy" } else { "falsy" })
            }
            Fact::Equals { variable, literal } => format!("{variable} === {literal:?}"),
            Fact::NotEquals { variable, literal } => format!("{variable} !== {literal:?}"),
            Fact::Object { variable } => format!("{variable} is an object"),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(" [{text}]")
}

fn summary_note(summary: &SummaryOrigin) -> String {
    let chain = summary
        .calls
        .iter()
        .map(|site| format!("{} {}", site.call, site.callee))
        .collect::<Vec<_>>()
        .join(" > ");
    let paths = if summary.on_every_path {
        "every path"
    } else {
        "some paths"
    };
    format!(" <summary via {chain}; {paths}>")
}

fn line(out: &mut String, depth: usize, args: fmt::Arguments<'_>) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    let _ = out.write_fmt(args);
    out.push('\n');
}

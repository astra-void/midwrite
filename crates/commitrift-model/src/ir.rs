//! The concurrency-oriented IR: what an *actor* does, one scheduling-relevant
//! operation at a time.
//!
//! This IR is derived from (and kept separate from) the Effect IR of
//! `commitrift-core`. It is deliberately small: a bounded set of database
//! statements, transaction control, external effects, shared application
//! state, task spawning, and local computation. Anything the extractor does
//! not understand becomes [`Op::Unknown`], which the explorer reports as
//! unsupported if it is reachable; it is never a no-op.
//!
//! Local operations ([`Op::Let`], [`Op::JumpIf`], [`Op::Jump`]) are not
//! scheduling points: they commute with every other actor's operations and
//! are folded into the preceding visible transition (see
//! `docs/phase6/SEMANTICS.md`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::value::{Expr, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Isolation {
    ReadCommitted,
    RepeatableRead,
    Serializable,
}

impl Isolation {
    pub fn sql(self) -> &'static str {
        match self {
            Self::ReadCommitted => "READ COMMITTED",
            Self::RepeatableRead => "REPEATABLE READ",
            Self::Serializable => "SERIALIZABLE",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::ReadCommitted => "read-committed",
            Self::RepeatableRead => "repeatable-read",
            Self::Serializable => "serializable",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().replace([' ', '_'], "-").as_str() {
            "read-committed" => Some(Self::ReadCommitted),
            "repeatable-read" => Some(Self::RepeatableRead),
            "serializable" => Some(Self::Serializable),
            _ => None,
        }
    }
}

/// Where a statement runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Conn {
    /// The actor's open transaction (a `tx` handle).
    Tx,
    /// A pooled connection in autocommit mode (the root `db` handle): the
    /// statement is its own single-statement transaction.
    Pool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LockMode {
    None,
    ForUpdate,
    /// `FOR UPDATE SKIP LOCKED`
    SkipLocked,
    /// `FOR UPDATE NOWAIT`
    NoWait,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnConflict {
    /// A unique violation is an error (SQLSTATE 23505).
    Error,
    /// `ON CONFLICT DO NOTHING`.
    DoNothing,
}

/// `INNER JOIN` / `LEFT JOIN` (Phase 7): only equality of two columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JoinKind {
    Inner,
    Left,
}

/// One join of a [`StmtKind::Select`]: `JOIN table ON left = right`, where
/// `left` and `right` are qualified columns (`table.column`) of tables
/// already in the query.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Join {
    pub kind: JoinKind,
    pub table: String,
    pub left: String,
    pub right: String,
}

/// One field of a projected result row (Phase 7).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjItem {
    /// The whole row of a table, nested (`null` for an unmatched `LEFT
    /// JOIN`, as Drizzle returns it).
    Table(String),
    /// One column: plain (`email`) without joins, qualified with joins.
    Column(String),
}

/// A database statement of the supported subset.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StmtKind {
    /// `SELECT * FROM table WHERE pred ORDER BY <primary key> LIMIT 1
    /// [FOR UPDATE ...]`. The first matching row (primary-key order), or
    /// `null`, is assigned to `into`.
    Select {
        table: String,
        pred: Expr,
        lock: LockMode,
        into: String,
        /// Joined tables (Phase 7). With joins, columns in `pred` are
        /// qualified (`table.column`) and the result row is keyed by table
        /// name unless `projection` says otherwise.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        joins: Vec<Join>,
        /// Projected result fields (Phase 7); `None` reads the full row.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        projection: Option<Vec<(String, ProjItem)>>,
    },
    /// `SELECT count(*) FROM table WHERE pred`.
    SelectCount {
        table: String,
        pred: Expr,
        into: String,
    },
    /// `UPDATE table SET col = expr, ... WHERE pred`; `affected` receives the
    /// number of rows changed.
    Update {
        table: String,
        pred: Expr,
        set: Vec<(String, Expr)>,
        affected: Option<String>,
    },
    /// `INSERT INTO table (cols) VALUES (...)`.
    Insert {
        table: String,
        values: Vec<(String, Expr)>,
        on_conflict: OnConflict,
        affected: Option<String>,
        /// `RETURNING *` (Phase 7): the inserted row, including generated
        /// and default values, or `null` when nothing was inserted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        returning: Option<String>,
        /// Phase 12: `RETURNING col, ...` (a projection, as Drizzle sends
        /// `returning({ alias: table.col })`): the columns of the returned
        /// row. Absent: every column.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        returning_columns: Option<Vec<String>>,
    },
    /// `DELETE FROM table WHERE pred`.
    Delete {
        table: String,
        pred: Expr,
        affected: Option<String>,
    },
}

impl StmtKind {
    pub fn table(&self) -> &str {
        match self {
            Self::Select { table, .. }
            | Self::SelectCount { table, .. }
            | Self::Update { table, .. }
            | Self::Insert { table, .. }
            | Self::Delete { table, .. } => table,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stmt {
    pub conn: Conn,
    #[serde(flatten)]
    pub kind: StmtKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Op {
    /// Open the actor's transaction. `retry` is the number of times the
    /// transaction body is re-executed after a serialization failure or
    /// deadlock (SQLSTATE 40001 / 40P01).
    Begin {
        isolation: Option<Isolation>,
        #[serde(default)]
        retry: u32,
    },
    Commit,
    Rollback,
    Db {
        #[serde(flatten)]
        stmt: Stmt,
    },
    /// Local computation (not a scheduling point).
    Let {
        local: String,
        expr: Expr,
    },
    /// Jump to `target` (an index in the same code vector) if `cond` is
    /// truthy (not a scheduling point).
    JumpIf {
        cond: Expr,
        target: usize,
    },
    Jump {
        target: usize,
    },
    /// An application exception: the actor fails; an open transaction is
    /// rolled back (as a Drizzle `transaction` callback would).
    Throw {
        message: String,
    },
    /// Normal completion.
    Return,
    /// An externally visible effect (publish, HTTP call, email). `key` is an
    /// idempotency key when the call carries one.
    External {
        channel: String,
        payload: Vec<(String, Expr)>,
        key: Option<Expr>,
    },
    /// Read a shared in-process variable.
    AppRead {
        var: String,
        into: String,
    },
    /// Write a shared in-process variable.
    AppWrite {
        var: String,
        expr: Expr,
    },
    /// Start concurrent child tasks of the same actor (`Promise.all`, or a
    /// fire-and-forget call when `detached`). Children are code indices.
    Spawn {
        children: Vec<usize>,
        #[serde(default)]
        detached: bool,
    },
    /// Wait for every non-detached child spawned by this thread.
    Join,
    /// Enter a `try` block (Phase 7; local, not a scheduling point): a
    /// failure of this thread before the matching [`Op::EndTry`] resumes at
    /// `handler` instead of failing the thread. The failure's reason is
    /// bound to `error` when given (an opaque value: the extractor never lets
    /// code inspect it). A transaction opened inside the block is rolled
    /// back before the handler runs, as Drizzle's `transaction` does before
    /// re-throwing. A process crash never runs handlers.
    Try {
        handler: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Leave the innermost `try` block (local).
    EndTry,
    /// Fail with the reason held in `local` (re-throw after a `finally`
    /// block; local).
    Rethrow {
        local: String,
    },
    /// Behavior the extractor did not understand.
    Unknown {
        detail: String,
        /// Phase 12: locals the unsupported operation would have assigned
        /// (`const [row] = await <unsupported query>`). They count as
        /// assigned for the loader's checks, but no value is produced:
        /// reaching this operation makes the path unsupported, so nothing
        /// after it on that path is explored.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        defines: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    /// Derived from TypeScript by CommitRift.
    Extracted,
    /// Written in the scenario (a fixture).
    Declared,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRef {
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
    #[serde(default)]
    pub line: u32,
    #[serde(default)]
    pub column: u32,
    /// Last line of the operation's source span (Phase 7; 0 = unknown).
    /// Runtime statements are attributed to an operation when their
    /// application frame lies in `line..=end_line`.
    /// In memory only: reports keep the v1 `sourceRef` shape.
    #[serde(default, skip_serializing)]
    pub end_line: u32,
    /// The function the operation is written in.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub function: String,
    /// Callers through which the function was inlined, outermost first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub call_chain: Vec<String>,
    /// The Effect IR effect this operation corresponds to (its id within
    /// `function`'s Effect IR), when the effect is at the same location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_id: Option<u32>,
    /// Existing CommitRift findings (`CP1001@<fingerprint>`) that involve
    /// the effect at this location. Linked for traceability only: a
    /// sequential finding is never promoted to a concurrency finding.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
    /// A short source excerpt.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
}

impl SourceRef {
    pub fn declared(line: u32, text: &str) -> Self {
        Self {
            origin: Origin::Declared,
            file: String::new(),
            line,
            column: 0,
            end_line: 0,
            function: String::new(),
            call_chain: Vec::new(),
            effect_id: None,
            diagnostics: Vec::new(),
            text: text.to_owned(),
        }
    }

    pub fn label(&self) -> String {
        if self.file.is_empty() {
            if self.line == 0 {
                String::new()
            } else {
                format!("scenario:{}", self.line)
            }
        } else {
            format!("{}:{}:{}", self.file, self.line, self.column)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Step {
    pub op: Op,
    pub src: SourceRef,
}

/// Which attempts of an actor the environment repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RetryTrigger {
    /// A client or scheduler re-runs the operation after it failed.
    OnFailure,
    /// ... after the process crashed.
    OnCrash,
    OnFailureOrCrash,
    /// At-least-once redelivery: the operation may also be re-run after it
    /// *succeeded* (acknowledgement lost).
    AtLeastOnce,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationRetry {
    pub trigger: RetryTrigger,
    /// The environment *guarantees* the retry happens (bounded by
    /// `maxRetries`), as a queue that redelivers until acknowledged or a
    /// reconciliation job does. A state in which a guaranteed retry is still
    /// enabled is not quiescent, so final invariants (including
    /// "eventually" style ones) are not evaluated before it ran. A
    /// non-guaranteed retry is an optional environment choice.
    #[serde(default)]
    pub guaranteed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionRef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorSpec {
    pub name: String,
    pub function: FunctionRef,
    /// Arguments bound as locals before the first operation, in order.
    pub arguments: Vec<(String, Value)>,
    /// Code vectors; index 0 is the main task, others are spawned children.
    pub codes: Vec<Vec<Step>>,
    pub retry: Option<OperationRetry>,
    #[serde(default)]
    pub description: String,
}

impl ActorSpec {
    pub fn has_unknown(&self) -> Vec<(String, SourceRef)> {
        let mut out = Vec::new();
        for code in &self.codes {
            for step in code {
                if let Op::Unknown { detail, .. } = &step.op {
                    out.push((detail.clone(), step.src.clone()));
                }
            }
        }
        out
    }
}

/// How an external channel treats repeated calls that carry the same
/// idempotency key. A key name alone proves nothing: the scenario must say
/// who enforces it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Dedup {
    /// The channel ignores keys: every call is a visible effect.
    #[default]
    None,
    /// The channel (for example a payment provider) enforces uniqueness of
    /// the key: a repeated key produces no second visible effect.
    ByKey,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelSpec {
    #[serde(default)]
    pub dedup: Dedup,
}

pub type Channels = BTreeMap<String, ChannelSpec>;

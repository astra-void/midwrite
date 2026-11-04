//! The concurrency-oriented IR: what an *actor* does, one scheduling-relevant
//! operation at a time.
//!
//! This IR is derived from (and kept separate from) the Effect IR of
//! `midwrite-core`. It is deliberately small: a bounded set of database
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
    /// Phase 13: a read routed to a read replica (a `withReplicas`
    /// database's `select`/`query`). Only reads; the statement sees the
    /// replica's replayed prefix of the primary's commits, which may lag
    /// (`docs/phase13/REPLICA-CONSISTENCY.md`).
    Replica,
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
    /// Phase 13: `FOR NO KEY UPDATE`.
    ForNoKeyUpdate,
    /// Phase 13: `FOR SHARE`.
    ForShare,
    /// Phase 13: `FOR KEY SHARE`.
    ForKeyShare,
}

impl LockMode {
    /// The SQL suffix of a `SELECT` with this lock (with a leading space).
    pub fn sql(self) -> &'static str {
        match self {
            Self::None => "",
            Self::ForUpdate => " FOR UPDATE",
            Self::SkipLocked => " FOR UPDATE SKIP LOCKED",
            Self::NoWait => " FOR UPDATE NOWAIT",
            Self::ForNoKeyUpdate => " FOR NO KEY UPDATE",
            Self::ForShare => " FOR SHARE",
            Self::ForKeyShare => " FOR KEY SHARE",
        }
    }

    /// The row-lock mode the statement takes on each row it returns.
    pub fn row_lock(self) -> Option<crate::db::RowLock> {
        use crate::db::RowLock;
        match self {
            Self::None => None,
            Self::ForUpdate | Self::SkipLocked | Self::NoWait => Some(RowLock::Update),
            Self::ForNoKeyUpdate => Some(RowLock::NoKeyUpdate),
            Self::ForShare => Some(RowLock::Share),
            Self::ForKeyShare => Some(RowLock::KeyShare),
        }
    }

    /// Whether the lock excludes other lockers of its strength: `FOR UPDATE`
    /// (with any option) and `FOR NO KEY UPDATE` serialize two such
    /// readers; `FOR SHARE` / `FOR KEY SHARE` do not (Phase 13).
    pub fn is_exclusive(self) -> bool {
        matches!(
            self,
            Self::ForUpdate | Self::SkipLocked | Self::NoWait | Self::ForNoKeyUpdate
        )
    }

    /// Whether the mode needs lock modes beyond "exclusive" (Phase 13).
    pub fn is_shared(self) -> bool {
        matches!(self, Self::ForShare | Self::ForKeyShare)
    }
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

/// Phase 13: the `DO UPDATE` arm of `INSERT ... ON CONFLICT (target)`.
/// `set` and `filter` are evaluated on the existing (conflicting) row;
/// `Col("excluded.c")` is column `c` of the row proposed for insertion.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Upsert {
    /// The arbiter: the columns of a unique key (or the primary key).
    pub target: Vec<String>,
    pub set: Vec<(String, Expr)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<Expr>,
}

/// Phase 16: the multi-row mode of a [`StmtKind::Select`]: `SELECT * FROM
/// table WHERE pred [ORDER BY <primary key>] [LIMIT n] [FOR ...]`, whose
/// result is the list of returned rows (a JavaScript array of row objects).
///
/// The model returns and locks rows in primary-key order. That is
/// PostgreSQL's order when the statement says `ORDER BY` the key
/// (`order_by`; LockRows runs above the sort). Without it PostgreSQL's order
/// is its plan's: key order for an index scan, physical order for a bitmap
/// or sequential scan (measured in `docs/phase16/MULTI-ROW-LOCKS.md`), which
/// the model does not represent. The number of rows is bounded by the rows
/// of the model's finite world.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManyRows {
    /// `LIMIT n` (a literal in the source); absent: every matching row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// `ORDER BY column` (ascending) written in the source (Drizzle
    /// `.orderBy(t.column)`); the loader accepts only the single-column
    /// primary key. Absent: no `ORDER BY` in the statement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_by: Option<String>,
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
        /// Phase 16: the multi-row mode (`docs/phase16/MULTI-ROW-LOCKS.md`).
        /// Absent: the Phase 6-15 `LIMIT 1` statement above. Present: every
        /// matching row (at most `limit`), in primary-key order, is
        /// assigned to `into` as a list of rows; with a lock the rows are
        /// locked one at a time in that order.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        many: Option<ManyRows>,
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
        /// Phase 13: `ON CONFLICT (target) DO UPDATE SET ... [WHERE ...]`
        /// (`on_conflict` is then `Error`: a conflict on another unique key
        /// is still SQLSTATE 23505).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        upsert: Option<Upsert>,
        /// Phase 14: the rows after the first of a multi-row `INSERT ...
        /// VALUES (...), (...)` (Drizzle `values([{...}, {...}])`; `values`
        /// is the first row). One atomic statement: rows are inserted in
        /// order, each with its own defaults; any error aborts the whole
        /// statement. With rows here, `returning` receives the list of
        /// returned rows and `affected` the number of rows inserted.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        more_rows: Vec<Vec<(String, Expr)>>,
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

    /// Phase 14: whether the statement is a multi-row `INSERT`.
    pub fn is_multi_row(&self) -> bool {
        matches!(self, Self::Insert { more_rows, .. } if !more_rows.is_empty())
    }
}

/// Phase 14: the most rows one multi-row `INSERT` may propose (the
/// extractor refuses longer literal lists; the loader refuses more).
pub const MAX_INSERT_ROWS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stmt {
    pub conn: Conn,
    #[serde(flatten)]
    pub kind: StmtKind,
}

/// Phase 14: the generator index of values generated by package calls
/// ([`Op::Generate`]); column generators are numbered from 0.
pub const CALL_GENERATOR: u16 = 0x800;

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
    /// Phase 13: the value the main task returns (local, not a scheduling
    /// point; emitted right before the main task's [`Op::Return`]). An
    /// expression that cannot be evaluated records an *unknown* result
    /// instead of failing the task: return values never change what the
    /// database does, and an invariant over an unknown result is not
    /// evaluated.
    ReturnValue {
        expr: Expr,
    },
    /// Phase 13: the request's response is delivered to the client (a
    /// scheduling point, before the main task's [`Op::Return`]). Inserted by
    /// the scenario loader for actors declared as request handlers
    /// (`"response": true`): a crash between the last database operation
    /// and this step is a crash after the work committed and before the
    /// client learned it.
    Respond,
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
        /// Phase 16: the handler inspects the caught error (`e.code`,
        /// `e.cause?.code`, `e instanceof Error`): `error` receives the error
        /// object as the application sees it when the failure's origin is
        /// modeled (a database statement or COMMIT, as the driver and ORM
        /// surface it; a `throw new Error(...)`), and an object whose
        /// properties are unknown otherwise ([`CaughtErrors`]). Absent: the
        /// Phase 7 opaque binding (the failure's text).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        inspect: Option<CaughtErrors>,
    },
    /// Leave the innermost `try` block (local).
    EndTry,
    /// Fail with the reason held in `local` (re-throw after a `finally`
    /// block; local).
    Rethrow {
        local: String,
    },
    /// Phase 14: a fresh random value from a package call with a verified
    /// contract (`nanoid()`, `crypto.randomUUID()`), assigned to `local`: a
    /// generation event of the actor ([`crate::value::Value::Gen`] with
    /// generator [`CALL_GENERATOR`]; the actor's counter is shared with
    /// column defaults, so every event has its own identity). Local, not a
    /// scheduling point. `source` names the call for reports.
    Generate {
        local: String,
        source: String,
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

/// Phase 16: how a database error reaches application code, by where it
/// was raised. `None` means the shape is not verified for the project's
/// ORM and driver versions: reading a property of such an error is an
/// unsupported evaluation, never a guess (`docs/phase16/LOOP-SEMANTICS.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaughtErrors {
    /// A statement's error (and, through the transaction callback, a
    /// statement's error inside a transaction).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statement: Option<ErrorShape>,
    /// The error of a transaction's COMMIT.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<ErrorShape>,
}

/// Phase 16: the object a database error is, as the application catches it.
/// Only `code` (the SQLSTATE) is modeled; other properties (message text,
/// query text, severity, ...) read as unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorShape {
    /// The driver's error itself (node-postgres `DatabaseError`,
    /// postgres-js `PostgresError`): `code` is the SQLSTATE.
    Driver,
    /// drizzle-orm's `DrizzleQueryError` wrapping the driver's error:
    /// `code` is `undefined`, `cause.code` is the SQLSTATE.
    DrizzleQueryError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    /// Derived from TypeScript by Midwrite.
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
    /// Existing Midwrite findings (`MW1001@<fingerprint>`) that involve
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

//! The abstract database: a schema with explicitly declared constraints and
//! multi-versioned rows.
//!
//! Rows are identified by a [`RowId`] that is derived from *who inserted
//! them* (initial data, or actor + per-actor insert counter), never from a
//! global counter, so that independent inserts commute (this matters for
//! partial-order reduction and state merging).
//!
//! Each row has a chain of [`Version`]s in commit order. A version is
//! committed (`commit = Some(sequence)`) or the single uncommitted version of
//! its creator. PostgreSQL's row locking guarantees at most one uncommitted
//! version per row and that later versions are created only after earlier
//! writers finished, so chain order equals commit order.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::value::{Expr, Row, Value, predicate_matches};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TxnId(pub u32);

impl TxnId {
    pub fn actor(self) -> u16 {
        (self.0 >> 12) as u16
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RowId {
    pub table: u16,
    /// `u16::MAX` for rows of the initial state.
    pub actor: u16,
    pub n: u32,
}

impl RowId {
    pub const INITIAL: u16 = u16::MAX;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ColumnSchema {
    pub name: String,
    #[serde(default)]
    pub not_null: bool,
    /// Column type for the PostgreSQL oracle: `int`, `text` or `bool`. The
    /// abstract model is untyped; when omitted the oracle infers it.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub sql_type: Option<String>,
    /// The value an `INSERT` that omits the column stores. Without
    /// a declared default an omitted column is `NULL`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<ColumnDefault>,
    /// The column's name in the application's SQL schema when it differs
    /// from the model name (Drizzle property `teamId` → column `team_id`).
    /// Used only to compare the model with the original application's
    /// database; the model and its oracle use `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql_name: Option<String>,
    /// The value Drizzle computes for the column when an `UPDATE`
    /// (or an upsert's `DO UPDATE SET`) does not assign it, and for an
    /// `INSERT` that omits a column without a default (`$onUpdate`). Only
    /// [`ColumnDefault::Clock`] (`$onUpdate(() => new Date())`) is modeled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_update: Option<ColumnDefault>,
}

/// A column default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum ColumnDefault {
    /// A constant (`DEFAULT 'pending'`).
    Value(Value),
    /// A sequence (`serial`, `GENERATED ... AS IDENTITY`): every insert that
    /// omits the column takes the next value. As in PostgreSQL, allocation
    /// is not transactional: a value taken by an insert that later fails or
    /// is rolled back is not reused. Values are concrete only because the
    /// disposable database starts the sequence at a known value (`start`,
    /// default: one more than the largest initial value, else 1); they say
    /// nothing about identifiers in another database.
    Sequence(SequenceSpec),
    /// A generated identifier (`uuid().defaultRandom()`,
    /// `$defaultFn(() => crypto.randomUUID())`): every execution of an
    /// insert that omits the column produces a new value, a
    /// [`crate::value::Value::Gen`] whose identity is the generation event.
    /// Unlike a sequence, generation reads and writes no shared state, so
    /// inserts of different actors stay independent.
    Generated(GeneratedSpec),
    /// PostgreSQL's `now()` (`defaultNow()`, ``default(sql`now()`)``):
    /// the start time of the inserting transaction, the same value for every
    /// statement of that transaction ([`now_value`]).
    Now,
    /// An application clock reading (`$defaultFn(() => new
    /// Date())`, `$onUpdate(() => new Date())`): Drizzle reads the clock
    /// when it builds the statement, once per row and execution.
    Clock,
}

/// The reserved generator of [`crate::value::GenId`]s that are
/// PostgreSQL transaction timestamps (`now()`): `actor` is the transaction's
/// actor and `n` the transaction's number within the actor, so every
/// statement of one transaction reads the same value and two transactions
/// read different ones.
pub const CLOCK_DB: u16 = 0xFFF0;
/// The reserved generator of application clock readings (`new
/// Date()`): `n` is the actor's generation counter, as for Phase 12
/// generated values.
pub const CLOCK_APP: u16 = 0xFFF1;
/// The actor of a clock *placeholder*: a literal the extractor
/// writes where a statement reads the clock (`set({ at: new Date() })`,
/// ``sql`now()` ``); the engine replaces it when the statement starts.
pub const CLOCK_PLACEHOLDER: u16 = u16::MAX;

/// Whether a generated value is a clock reading.
pub fn is_clock(g: &crate::value::GenId) -> bool {
    matches!(g.generator, CLOCK_DB | CLOCK_APP)
}

/// The value of `now()` in transaction `txn`.
pub fn now_value(txn: TxnId) -> Value {
    Value::Gen(crate::value::GenId {
        generator: CLOCK_DB,
        actor: txn.actor(),
        n: txn.0 & 0xFFF,
    })
}

/// The placeholder of `now()` (``sql`now()` ``) in a statement.
pub fn now_placeholder() -> Value {
    Value::Gen(crate::value::GenId {
        generator: CLOCK_DB,
        actor: CLOCK_PLACEHOLDER,
        n: 0,
    })
}

/// The placeholder of the `k`-th application clock reading (`new
/// Date()`) written in a statement's arguments.
pub fn clock_placeholder(k: u32) -> Value {
    Value::Gen(crate::value::GenId {
        generator: CLOCK_APP,
        actor: CLOCK_PLACEHOLDER,
        n: k,
    })
}

/// The column types whose values the model checks on every write
/// (they are modeled only with the scenario features `timestamps` / `json`).
pub fn is_time_type(sql_type: Option<&str>) -> bool {
    matches!(sql_type, Some("timestamp" | "timestamptz" | "date"))
}

pub fn is_json_type(sql_type: Option<&str>) -> bool {
    matches!(sql_type, Some("json" | "jsonb"))
}

/// The deepest JSON value (objects and arrays nested in each other) the
/// model stores in a `json`/`jsonb` column.
pub const MAX_JSON_DEPTH: usize = 4;

/// Whether `text` is the canonical form of an explicit time value of a
/// column of `sql_type`: `YYYY-MM-DDTHH:MM:SS.sssZ` (JavaScript's
/// `toISOString()`, what a `Date` read through Drizzle serializes to) for
/// `timestamp`/`timestamptz`, `YYYY-MM-DD` for `date`. Canonical values of
/// one type compare as their instants do.
pub fn is_canonical_time(sql_type: &str, text: &str) -> bool {
    let b = text.as_bytes();
    let digits = |r: std::ops::Range<usize>| r.into_iter().all(|i| b[i].is_ascii_digit());
    let date = b.len() >= 10
        && digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10);
    if !date {
        return false;
    }
    let month = (b[5] - b'0') * 10 + (b[6] - b'0');
    let day = (b[8] - b'0') * 10 + (b[9] - b'0');
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return false;
    }
    if sql_type == "date" {
        return b.len() == 10;
    }
    b.len() == 24
        && b[10] == b'T'
        && digits(11..13)
        && b[13] == b':'
        && digits(14..16)
        && b[16] == b':'
        && digits(17..19)
        && b[19] == b'.'
        && digits(20..23)
        && b[23] == b'Z'
        && (b[11] - b'0') * 10 + (b[12] - b'0') < 24
        && (b[14] - b'0') * 10 + (b[15] - b'0') < 60
        && (b[17] - b'0') * 10 + (b[18] - b'0') < 60
}

/// Why a value cannot be written to a typed column (`None`: it
/// can). Time columns hold `NULL`, a clock reading or a canonical explicit
/// value; JSON columns hold `NULL` or a JSON value of bounded depth (objects,
/// arrays, integers, strings, booleans, JSON `null` inside). Other columns
/// are not checked (the Phase 6-13 behavior).
pub fn typed_value_problem(column: &ColumnSchema, v: &Value) -> Option<String> {
    let ty = column.sql_type.as_deref();
    if is_time_type(ty) {
        let ty = ty.unwrap_or_default();
        return match v {
            Value::Null => None,
            Value::Gen(g) if is_clock(g) && g.actor != CLOCK_PLACEHOLDER && ty != "date" => None,
            Value::Gen(g) if is_clock(g) => Some(format!(
                "column `{}` ({ty}): a clock reading stored as a date (truncated to its day) is not modeled",
                column.name
            )),
            Value::Str(s) if is_canonical_time(ty, s) => None,
            other => Some(format!(
                "column `{}` ({ty}): the value {} is not a modeled time value (NULL, a clock reading, or the canonical text {})",
                column.name,
                other.render(),
                if ty == "date" {
                    "YYYY-MM-DD"
                } else {
                    "YYYY-MM-DDTHH:MM:SS.sssZ"
                }
            )),
        };
    }
    if is_json_type(ty) {
        fn ok(v: &Value, depth: usize) -> bool {
            match v {
                Value::Null | Value::Bool(_) | Value::Int(_) | Value::Str(_) => true,
                Value::Row(r) => depth < MAX_JSON_DEPTH && r.values().all(|x| ok(x, depth + 1)),
                Value::List(items) => {
                    depth < MAX_JSON_DEPTH && items.iter().all(|x| ok(x, depth + 1))
                }
                _ => false,
            }
        }
        if matches!(v, Value::Null) || (!matches!(v, Value::Gen(_)) && ok(v, 0)) {
            return None;
        }
        return Some(format!(
            "column `{}` ({}): the value {} is not a modeled JSON value (objects, arrays, integers, strings, booleans; depth at most {MAX_JSON_DEPTH})",
            column.name,
            ty.unwrap_or_default(),
            v.render()
        ));
    }
    if let Value::Gen(g) = v
        && is_clock(g)
    {
        return Some(format!(
            "column `{}`: a clock reading in a column that is not a time column is not modeled",
            column.name
        ));
    }
    None
}

/// A comparison (`=`, `<`, ...) of a JSON column in a predicate:
/// not modeled (`json` has no equality in PostgreSQL; `jsonb` compares
/// structurally, which the value layer does not implement).
pub fn json_comparison(schema: &TableSchema, pred: &Expr) -> Option<String> {
    fn json_col(schema: &TableSchema, e: &Expr) -> Option<String> {
        match e {
            Expr::Col(c) => schema
                .columns
                .iter()
                .find(|x| &x.name == c && is_json_type(x.sql_type.as_deref()))
                .map(|x| x.name.clone()),
            _ => None,
        }
    }
    match pred {
        Expr::Bin(op, a, b) => {
            if !matches!(op, crate::value::BinOp::And | crate::value::BinOp::Or)
                && let Some(c) = json_col(schema, a).or_else(|| json_col(schema, b))
            {
                return Some(format!(
                    "a comparison of the JSON column `{c}` (`{}`) is not modeled",
                    op.symbol()
                ));
            }
            json_comparison(schema, a).or_else(|| json_comparison(schema, b))
        }
        Expr::Not(x) | Expr::IsNull(x) | Expr::Field(x, _) => json_comparison(schema, x),
        Expr::Call { args, .. } | Expr::Array(args) => args.iter().find_map(|a| {
            json_col(schema, a)
                .map(|c| format!("an operator over the JSON column `{c}` is not modeled"))
                .or_else(|| json_comparison(schema, a))
        }),
        _ => None,
    }
}

/// The first problem of a row image for the typed columns of
/// `schema`.
pub fn typed_row_problem(schema: &TableSchema, row: &Row) -> Option<String> {
    schema
        .columns
        .iter()
        .find_map(|c| typed_value_problem(c, row.get(&c.name).unwrap_or(&Value::Null)))
}

/// Who computes a generated default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GeneratedSide {
    /// PostgreSQL, while executing the INSERT (`DEFAULT gen_random_uuid()`):
    /// the application sees the value only through `RETURNING`.
    Database,
    /// The application, when Drizzle builds the INSERT (`$defaultFn`): the
    /// value is a bound parameter of the statement.
    Application,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GeneratedSpec {
    pub side: GeneratedSide,
    /// The recognized generator: `gen_random_uuid()`, `crypto.randomUUID()`,
    /// `@paralleldrive/cuid2 createId()`.
    pub generator: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SequenceSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableSchema {
    pub name: String,
    pub columns: Vec<ColumnSchema>,
    #[serde(default)]
    pub primary_key: Vec<String>,
    #[serde(default)]
    pub unique: Vec<Vec<String>>,
    /// Unique sets declared `NULLS NOT DISTINCT` (PostgreSQL 15+)
    /// over a nullable column, listed next to (not in) `unique`: for these
    /// rules a NULL key value equals another NULL, so two rows whose keys
    /// agree with NULLs in the same columns conflict (SQLSTATE 23505, an
    /// `ON CONFLICT` arbiter match, a wait on an uncommitted equal key).
    /// The sets in `unique` keep PostgreSQL's default `NULLS DISTINCT`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nulls_not_distinct: Vec<Vec<String>>,
    /// Simple CHECK constraints: SQL-mode expressions over `Col`.
    #[serde(default)]
    pub checks: Vec<Expr>,
    /// Foreign keys this table declares (it is the referencing
    /// table). Enforced as PostgreSQL does: the referencing side locks the
    /// referenced row `FOR KEY SHARE`; deleting or re-keying a referenced
    /// row fails while referencing rows remain (`docs/architecture/postgres-model.md`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreign_keys: Vec<ForeignKeySchema>,
}

/// A referential action. Only `NO ACTION` and `RESTRICT` are
/// executed by the model; a scenario whose statements would run another
/// action is refused by the loader.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RefAction {
    #[default]
    NoAction,
    Restrict,
    Cascade,
    SetNull,
    SetDefault,
}

impl RefAction {
    pub fn supported(self) -> bool {
        matches!(self, Self::NoAction | Self::Restrict)
    }
    pub fn sql(self) -> &'static str {
        match self {
            Self::NoAction => "NO ACTION",
            Self::Restrict => "RESTRICT",
            Self::Cascade => "CASCADE",
            Self::SetNull => "SET NULL",
            Self::SetDefault => "SET DEFAULT",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().replace([' ', '_'], "-").as_str() {
            "no-action" => Some(Self::NoAction),
            "restrict" => Some(Self::Restrict),
            "cascade" => Some(Self::Cascade),
            "set-null" => Some(Self::SetNull),
            "set-default" => Some(Self::SetDefault),
            _ => None,
        }
    }
}

/// A foreign key, `MATCH SIMPLE`: a referencing row with a NULL
/// in any key column is not checked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForeignKeySchema {
    /// Columns of this (referencing) table.
    pub columns: Vec<String>,
    /// The referenced table (a model table name).
    pub references: String,
    /// Columns of the referenced table: its primary key or a unique set.
    pub referenced_columns: Vec<String>,
    #[serde(default, skip_serializing_if = "is_no_action")]
    pub on_delete: RefAction,
    #[serde(default, skip_serializing_if = "is_no_action")]
    pub on_update: RefAction,
    /// `DEFERRABLE INITIALLY DEFERRED`: checked at COMMIT, not modeled (the
    /// loader refuses scenarios that write the referencing table).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub initially_deferred: bool,
}

fn is_no_action(a: &RefAction) -> bool {
    *a == RefAction::NoAction
}

impl ForeignKeySchema {
    /// PostgreSQL's default constraint name.
    pub fn name(&self, table: &str) -> String {
        format!("{table}_{}_fkey", self.columns.join("_"))
    }
}

/// A PostgreSQL row-level lock mode, weakest first.
///
/// | requested \ held | KEY SHARE | SHARE | NO KEY UPDATE | UPDATE |
/// | --- | --- | --- | --- | --- |
/// | KEY SHARE | | | | X |
/// | SHARE | | | X | X |
/// | NO KEY UPDATE | | X | X | X |
/// | UPDATE | X | X | X | X |
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RowLock {
    /// `FOR KEY SHARE`: taken by a foreign-key check on the referenced row.
    KeyShare,
    /// `FOR SHARE`.
    Share,
    /// `FOR NO KEY UPDATE`: an `UPDATE` that changes no key column.
    NoKeyUpdate,
    /// `FOR UPDATE`: `SELECT ... FOR UPDATE`, `DELETE`, an `UPDATE` that
    /// changes a key column.
    Update,
}

impl RowLock {
    pub fn conflicts(self, other: RowLock) -> bool {
        use RowLock::*;
        matches!(
            (self, other),
            (Update, _) | (_, Update) | (NoKeyUpdate, NoKeyUpdate | Share) | (Share, NoKeyUpdate)
        )
    }
    pub fn sql(self) -> &'static str {
        match self {
            Self::KeyShare => "FOR KEY SHARE",
            Self::Share => "FOR SHARE",
            Self::NoKeyUpdate => "FOR NO KEY UPDATE",
            Self::Update => "FOR UPDATE",
        }
    }
}

impl TableSchema {
    pub fn has_column(&self, name: &str) -> bool {
        self.columns.iter().any(|c| c.name == name)
    }

    /// Unique key sets: the primary key first, then declared unique sets
    /// (Phase 16: then the `NULLS NOT DISTINCT` sets).
    pub fn key_sets(&self) -> Vec<(String, Vec<String>)> {
        self.key_rules()
            .into_iter()
            .map(|r| (r.name, r.columns))
            .collect()
    }

    /// The unique rules with their NULL semantics, in
    /// [`TableSchema::key_sets`] order. Names follow PostgreSQL's defaults
    /// (`<table>_pkey`, `<table>_<columns>_key`, a repeated name gets the
    /// next free numeric suffix as `ChooseConstraintName` gives it).
    pub fn key_rules(&self) -> Vec<KeyRule> {
        let mut out: Vec<KeyRule> = Vec::new();
        if !self.primary_key.is_empty() {
            out.push(KeyRule {
                name: format!("{}_pkey", self.name),
                columns: self.primary_key.clone(),
                nulls_not_distinct: false,
            });
        }
        let sets = self
            .unique
            .iter()
            .map(|u| (u, false))
            .chain(self.nulls_not_distinct.iter().map(|u| (u, true)));
        for (u, nulls_not_distinct) in sets {
            let base = format!("{}_{}_key", self.name, u.join("_"));
            let mut name = base.clone();
            let mut n = 0;
            while out.iter().any(|r| r.name == name) {
                n += 1;
                name = format!("{base}{n}");
            }
            out.push(KeyRule {
                name,
                columns: u.clone(),
                nulls_not_distinct,
            });
        }
        out
    }

    /// Whether NULLs are equal for an `ON CONFLICT (target)`
    /// arbiter. PostgreSQL infers as arbiters every unique index whose key
    /// columns are exactly the target's (in any order); a row conflicts when
    /// it conflicts on one of them, so a `NULLS NOT DISTINCT` arbiter makes
    /// NULL keys match.
    pub fn arbiter_nulls_not_distinct(&self, target: &[String]) -> bool {
        let mut wanted = target.to_vec();
        wanted.sort();
        self.key_rules().into_iter().any(|r| {
            let mut cols = r.columns;
            cols.sort();
            r.nulls_not_distinct && cols == wanted
        })
    }
}

/// One unique rule of a table (the primary key, a `UNIQUE` set or
/// a `UNIQUE NULLS NOT DISTINCT` set).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyRule {
    pub name: String,
    pub columns: Vec<String>,
    /// NULLs are equal for this rule (PostgreSQL 15+ `NULLS NOT DISTINCT`).
    pub nulls_not_distinct: bool,
}

impl KeyRule {
    /// Whether a key tuple of this rule can conflict at all: with the
    /// default `NULLS DISTINCT` a tuple with a NULL never equals another
    /// (PostgreSQL skips the uniqueness check); with `NULLS NOT DISTINCT`
    /// every tuple is checked, NULL equal to NULL.
    pub fn checks(&self, key: &[Value]) -> bool {
        self.nulls_not_distinct || !key.iter().any(Value::is_null)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schema {
    pub tables: Vec<TableSchema>,
}

impl Schema {
    pub fn index(&self, table: &str) -> Option<u16> {
        self.tables
            .iter()
            .position(|t| t.name == table)
            .map(|i| i as u16)
    }
    pub fn table(&self, index: u16) -> &TableSchema {
        &self.tables[index as usize]
    }
    pub fn by_name(&self, name: &str) -> Option<&TableSchema> {
        self.tables.iter().find(|t| t.name == name)
    }
}

/// One version of a row. `values == None` is a deletion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub values: Option<Row>,
    pub creator: TxnId,
    pub commit: Option<u64>,
}

/// Which versions a statement or transaction can see.
#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    pub seq: u64,
    pub own: TxnId,
}

impl Snapshot {
    pub fn sees(&self, v: &Version) -> bool {
        v.creator == self.own || v.commit.is_some_and(|c| c <= self.seq)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Store {
    /// Version chains, shared between states until written (Phase 8: a
    /// transition writes few rows; cloning a state does not copy the
    /// others).
    pub rows: BTreeMap<RowId, std::sync::Arc<Vec<Version>>>,
    /// Row locks held by active transactions (`UPDATE`, `DELETE`,
    /// `SELECT ... FOR UPDATE`, Phase 13: foreign-key checks and `FOR
    /// [KEY] SHARE`), released when the holder ends. Holders of one row are
    /// kept in acquisition order (PostgreSQL's multixact member order: a
    /// waiter waits for them in this order); one entry per transaction, with
    /// its strongest mode.
    pub locks: BTreeMap<RowId, Vec<(TxnId, RowLock)>>,
}

pub const INITIAL_TXN: TxnId = TxnId(u32::MAX);

impl Store {
    /// Index of the version of `row` visible in `snap`, if any version is.
    pub fn visible_index(&self, row: RowId, snap: Snapshot) -> Option<usize> {
        let chain = self.rows.get(&row)?;
        chain.iter().rposition(|v| snap.sees(v))
    }

    /// The row image visible in `snap` (None if absent or deleted).
    pub fn visible_values(&self, row: RowId, snap: Snapshot) -> Option<&Row> {
        let chain = self.rows.get(&row)?;
        chain
            .iter()
            .rev()
            .find(|v| snap.sees(v))
            .and_then(|v| v.values.as_ref())
    }

    /// Row ids of `table` in key order (used to make scans deterministic).
    pub fn table_rows(&self, table: u16) -> impl Iterator<Item = RowId> + '_ {
        let lo = RowId {
            table,
            actor: 0,
            n: 0,
        };
        let hi = RowId {
            table,
            actor: u16::MAX,
            n: u32::MAX,
        };
        self.rows.range(lo..=hi).map(|(id, _)| *id)
    }

    /// The first holder of a lock on `row`, other than `own`, whose mode
    /// conflicts with `mode` and that `active` says is still running.
    pub fn conflicting_holder(
        &self,
        row: RowId,
        own: TxnId,
        mode: RowLock,
        active: impl Fn(TxnId) -> bool,
    ) -> Option<TxnId> {
        self.locks.get(&row).and_then(|holders| {
            holders
                .iter()
                .find(|(t, m)| *t != own && m.conflicts(mode) && active(*t))
                .map(|(t, _)| *t)
        })
    }

    /// Grant `mode` on `row` to `txn` (upgrading a weaker lock it holds).
    pub fn grant(&mut self, row: RowId, txn: TxnId, mode: RowLock) {
        let holders = self.locks.entry(row).or_default();
        match holders.iter_mut().find(|(t, _)| *t == txn) {
            Some((_, m)) => *m = (*m).max(mode),
            None => holders.push((txn, mode)),
        }
    }

    /// Release every lock of `txn`; returns the rows.
    pub fn release(&mut self, txn: TxnId) -> Vec<RowId> {
        let mut out = Vec::new();
        self.locks.retain(|row, holders| {
            let before = holders.len();
            holders.retain(|(t, _)| *t != txn);
            if holders.len() != before {
                out.push(*row);
            }
            !holders.is_empty()
        });
        out
    }

    /// Latest committed version of a row.
    pub fn latest_committed(&self, row: RowId) -> Option<(usize, &Version)> {
        let chain = self.rows.get(&row)?;
        chain
            .iter()
            .enumerate()
            .rev()
            .find(|(_, v)| v.commit.is_some())
    }
}

/// Sort key for deterministic scans: primary-key values (then row id).
pub fn pk_key(schema: &TableSchema, values: &Row) -> Vec<Value> {
    let cols: Vec<&String> = if schema.primary_key.is_empty() {
        schema.columns.iter().map(|c| &c.name).collect()
    } else {
        schema.primary_key.iter().collect()
    };
    cols.iter()
        .map(|c| values.get(*c).cloned().unwrap_or(Value::Null))
        .collect()
}

pub fn key_tuple(values: &Row, cols: &[String]) -> Vec<Value> {
    cols.iter()
        .map(|c| values.get(c).cloned().unwrap_or(Value::Null))
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[error("{sqlstate}: {message}")]
pub struct DbError {
    pub sqlstate: String,
    pub message: String,
}

impl DbError {
    pub fn new(sqlstate: &str, message: impl Into<String>) -> Self {
        Self {
            sqlstate: sqlstate.to_owned(),
            message: message.into(),
        }
    }
    pub fn serialization() -> Self {
        Self::new(
            "40001",
            "could not serialize access due to concurrent update",
        )
    }
    pub fn deadlock() -> Self {
        Self::new("40P01", "deadlock detected")
    }
    pub fn retryable(&self) -> bool {
        self.sqlstate == "40001" || self.sqlstate == "40P01"
    }

    /// An evaluation the model does not represent (the order of
    /// generated or clock values, a SQL operator on an unsupported value):
    /// not a database error, the engine reports the path as unsupported.
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(UNSUPPORTED_EVALUATION, message)
    }

    pub fn is_unsupported(&self) -> bool {
        self.sqlstate == UNSUPPORTED_EVALUATION
    }
}

/// The marker of [`DbError::unsupported`] (never a PostgreSQL
/// SQLSTATE: the engine turns it into an unsupported operation before any
/// result is recorded).
pub const UNSUPPORTED_EVALUATION: &str = "CR000";

/// The database error of an evaluation error inside a statement:
/// an unsupported evaluation stays unsupported; other errors keep their
/// Phase 6-13 SQLSTATE (`fallback`).
pub fn eval_db_error(e: &crate::value::EvalError, fallback: &str, context: &str) -> DbError {
    match e {
        crate::value::EvalError::Unknown(_) => DbError::unsupported(format!("{context}{e}")),
        _ => DbError::new(fallback, format!("{context}{e}")),
    }
}

/// Check NOT NULL and CHECK constraints of a new row image.
pub fn check_row(schema: &TableSchema, values: &Row) -> Result<(), DbError> {
    for column in &schema.columns {
        let not_null = column.not_null || schema.primary_key.contains(&column.name);
        if not_null && values.get(&column.name).is_none_or(Value::is_null) {
            return Err(DbError::new(
                "23502",
                format!(
                    "null value in column \"{}\" of relation \"{}\" violates not-null constraint",
                    column.name, schema.name
                ),
            ));
        }
    }
    for (i, check) in schema.checks.iter().enumerate() {
        // A CHECK passes unless it evaluates to false (unknown passes).
        let empty = crate::value::Locals::new();
        let result = check.eval(&empty, Some(values), crate::value::Mode::Sql);
        match result {
            Ok(Value::Bool(false)) => {
                return Err(DbError::new(
                    "23514",
                    format!(
                        "new row for relation \"{}\" violates check constraint \"{}_check{}\"",
                        schema.name,
                        schema.name,
                        i + 1
                    ),
                ));
            }
            Ok(_) => {}
            Err(e) => {
                return Err(DbError::new(
                    "XX000",
                    format!("cannot evaluate CHECK constraint: {e}"),
                ));
            }
        }
    }
    Ok(())
}

pub fn matches_row(pred: &Expr, row: &Row) -> Result<bool, DbError> {
    predicate_matches(pred, row).map_err(|e| eval_db_error(&e, "XX000", "predicate: "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::BinOp;

    fn table() -> TableSchema {
        TableSchema {
            name: "products".into(),
            columns: vec![
                ColumnSchema {
                    name: "id".into(),
                    not_null: true,
                    sql_type: None,
                    default: None,
                    sql_name: None,
                    on_update: None,
                },
                ColumnSchema {
                    name: "stock".into(),
                    not_null: false,
                    sql_type: None,
                    default: None,
                    sql_name: None,
                    on_update: None,
                },
            ],
            primary_key: vec!["id".into()],
            unique: vec![],
            nulls_not_distinct: Vec::new(),
            checks: vec![Expr::bin(BinOp::Ge, Expr::col("stock"), Expr::lit(0))],
            foreign_keys: Vec::new(),
        }
    }

    #[test]
    fn check_constraints_reject_false_but_accept_unknown() {
        let t = table();
        let mut row = Row::new();
        row.insert("id".into(), Value::str("a"));
        row.insert("stock".into(), Value::Int(-1));
        assert_eq!(check_row(&t, &row).unwrap_err().sqlstate, "23514");
        row.insert("stock".into(), Value::Null);
        assert!(check_row(&t, &row).is_ok());
        row.insert("id".into(), Value::Null);
        assert_eq!(check_row(&t, &row).unwrap_err().sqlstate, "23502");
    }

    #[test]
    fn snapshot_sees_own_and_committed_up_to_sequence() {
        let own = TxnId(1);
        let other = TxnId(2);
        let snap = Snapshot { seq: 5, own };
        let committed_old = Version {
            values: None,
            creator: other,
            commit: Some(5),
        };
        let committed_new = Version {
            values: None,
            creator: other,
            commit: Some(6),
        };
        let mine = Version {
            values: None,
            creator: own,
            commit: None,
        };
        let theirs = Version {
            values: None,
            creator: other,
            commit: None,
        };
        assert!(snap.sees(&committed_old));
        assert!(!snap.sees(&committed_new));
        assert!(snap.sees(&mine));
        assert!(!snap.sees(&theirs));
    }
}

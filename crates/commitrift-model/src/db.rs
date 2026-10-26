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
    /// The value an `INSERT` that omits the column stores (Phase 7). Without
    /// a declared default an omitted column is `NULL`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<ColumnDefault>,
    /// The column's name in the application's SQL schema when it differs
    /// from the model name (Drizzle property `teamId` → column `team_id`).
    /// Used only to compare the model with the original application's
    /// database; the model and its oracle use `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql_name: Option<String>,
}

/// A column default (Phase 7).
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
    /// Simple CHECK constraints: SQL-mode expressions over `Col`.
    #[serde(default)]
    pub checks: Vec<Expr>,
}

impl TableSchema {
    pub fn has_column(&self, name: &str) -> bool {
        self.columns.iter().any(|c| c.name == name)
    }

    /// Unique key sets: the primary key first, then declared unique sets.
    pub fn key_sets(&self) -> Vec<(String, Vec<String>)> {
        let mut out = Vec::new();
        if !self.primary_key.is_empty() {
            out.push((format!("{}_pkey", self.name), self.primary_key.clone()));
        }
        for u in &self.unique {
            out.push((format!("{}_{}_key", self.name, u.join("_")), u.clone()));
        }
        out
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
    /// `SELECT ... FOR UPDATE`), released when the holder ends.
    pub locks: BTreeMap<RowId, TxnId>,
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
    predicate_matches(pred, row).map_err(|e| DbError::new("XX000", format!("predicate: {e}")))
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
                },
                ColumnSchema {
                    name: "stock".into(),
                    not_null: false,
                    sql_type: None,
                    default: None,
                    sql_name: None,
                },
            ],
            primary_key: vec!["id".into()],
            unique: vec![],
            checks: vec![Expr::bin(BinOp::Ge, Expr::col("stock"), Expr::lit(0))],
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

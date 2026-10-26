//! The complete, validated input of one model-checking run.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::db::{RowId, Schema};
use crate::invariant::Invariant;
use crate::ir::{ActorSpec, Channels, Isolation};
use crate::value::{Row, Value};

/// Exploration bounds. Every bound that was reached is reported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Bounds {
    /// Maximum number of actors in the scenario.
    #[serde(default = "d_actors")]
    pub max_actors: usize,
    /// Maximum number of transitions on one schedule.
    #[serde(default = "d_steps")]
    pub max_steps: u32,
    /// Maximum number of distinct (canonical) states.
    #[serde(default = "d_states")]
    pub max_states: u64,
    /// Maximum process crashes on one schedule.
    #[serde(default)]
    pub max_crashes: u32,
    /// Maximum retries (operation re-executions and transaction retries) on
    /// one schedule.
    #[serde(default)]
    pub max_retries: u32,
    /// Phase 9: maximum operations on one schedule whose acknowledgment is
    /// lost in flight (a COMMIT or an autocommit write that reached the
    /// server, or did not, while the application learns nothing). `0`
    /// (default): no in-flight faults, exactly the Phase 8 transition system.
    #[serde(default)]
    pub max_in_flight: u32,
    /// Maximum context switches on one schedule. Reduction is disabled when
    /// this is set (see `docs/phase6/PARTIAL-ORDER-REDUCTION.md`).
    #[serde(default)]
    pub max_context_switches: Option<u32>,
    /// Maximum number of executed transitions over the whole search.
    #[serde(default)]
    pub max_transitions: Option<u64>,
    /// Maximum wall-clock time in milliseconds.
    #[serde(default)]
    pub max_wall_ms: Option<u64>,
    /// Maximum approximate memory of the visited-state table, in bytes.
    #[serde(default)]
    pub max_memory_bytes: Option<u64>,
}

fn d_actors() -> usize {
    4
}
fn d_steps() -> u32 {
    64
}
fn d_states() -> u64 {
    200_000
}

impl Default for Bounds {
    fn default() -> Self {
        Self {
            max_actors: d_actors(),
            max_steps: d_steps(),
            max_states: d_states(),
            max_crashes: 0,
            max_retries: 0,
            max_in_flight: 0,
            max_context_switches: None,
            max_transitions: None,
            max_wall_ms: None,
            max_memory_bytes: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Strategy {
    Bfs,
    Dfs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    /// Sleep-set partial-order reduction (forces depth-first search).
    pub por: bool,
    pub strategy: Strategy,
    /// Minimize the counterexample after the search.
    pub minimize: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            por: true,
            strategy: Strategy::Dfs,
            minimize: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Model {
    pub name: String,
    pub description: String,
    pub schema: Schema,
    /// Initial rows per table (index by schema table position).
    pub initial_rows: Vec<Vec<Row>>,
    pub isolation: Isolation,
    pub actors: Vec<ActorSpec>,
    /// Process index of each actor (actors with the same index share
    /// in-process application state and crash together).
    pub process_of: Vec<usize>,
    pub process_names: Vec<String>,
    pub channels: Channels,
    pub app_initial: BTreeMap<String, Value>,
    pub invariants: Vec<Invariant>,
    pub bounds: Bounds,
    pub options: Options,
    /// Modeling assumptions to print with every result.
    pub assumptions: Vec<String>,
}

impl Model {
    pub fn uses_serializable(&self) -> bool {
        self.isolation == Isolation::Serializable
            || self.actors.iter().any(|a| {
                a.codes.iter().flatten().any(|s| {
                    matches!(
                        s.op,
                        crate::ir::Op::Begin {
                            isolation: Some(Isolation::Serializable),
                            ..
                        }
                    )
                })
            })
    }

    pub fn actor_index(&self, name: &str) -> Option<usize> {
        self.actors.iter().position(|a| a.name == name)
    }

    /// Sequence-backed columns `(table, column, first value)`, in schema
    /// order: the index into `State::seqs`. The first value is the declared
    /// start, else one more than the largest initial value, else 1.
    pub fn sequences(&self) -> Vec<(u16, String, i64)> {
        let mut out = Vec::new();
        for (ti, t) in self.schema.tables.iter().enumerate() {
            for c in &t.columns {
                if let Some(crate::db::ColumnDefault::Sequence(spec)) = &c.default {
                    let start = spec.start.unwrap_or_else(|| {
                        self.initial_rows[ti]
                            .iter()
                            .filter_map(|r| match r.get(&c.name) {
                                Some(Value::Int(n)) => Some(*n),
                                _ => None,
                            })
                            .max()
                            .map_or(1, |m| m.saturating_add(1))
                    });
                    out.push((ti as u16, c.name.clone(), start));
                }
            }
        }
        out
    }

    /// Row ids of the initial state, in table order.
    pub fn initial_row_ids(&self) -> Vec<(RowId, Row)> {
        let mut out = Vec::new();
        for (t, rows) in self.initial_rows.iter().enumerate() {
            for (i, row) in rows.iter().enumerate() {
                out.push((
                    RowId {
                        table: t as u16,
                        actor: RowId::INITIAL,
                        n: i as u32,
                    },
                    row.clone(),
                ));
            }
        }
        out
    }
}

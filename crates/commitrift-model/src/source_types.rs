//! Serializable types of Phase 7 source-level verification and extraction
//! reports (always compiled; the verification itself needs the `postgres`
//! feature, see `source_verify`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::provider::EntryInfo;
use crate::value::Row;

/// How one actor's original code is invoked (from the scenario).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceActor {
    /// The harness entry (`entries` in the harness manifest).
    pub entry: String,
    /// The input the harness passes to the original function.
    pub input: Json,
}

/// The scenario's `sourceVerification` section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSpec {
    /// Path of the harness manifest, relative to the scenario file.
    #[serde(default)]
    pub harness: Option<String>,
    pub actors: BTreeMap<String, SourceActor>,
}

/// One model statement and what the original code did for it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementComparison {
    pub transition: usize,
    pub actor: String,
    /// The model operation (description and source).
    pub model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model_source: String,
    pub predicted: String,
    /// The SQL Drizzle actually sent (empty if none was matched).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sql: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub runtime_source: String,
    pub observed: String,
    /// `agree`, `differ`, `not-observed`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub differences: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub run: u32,
    /// `followed`, `diverged`, `inconclusive`, `blocked` (from the
    /// coordinator).
    pub coordination: String,
    pub statements_agree: bool,
    pub tables_agree: bool,
    pub outcomes_agree: bool,
    /// The invariant evaluated on the observed final rows: `violated`,
    /// `holds`, or `not-evaluated`.
    pub invariant: String,
    pub reproduced: bool,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceVerification {
    pub performed: bool,
    /// `reproduced`, `not-reproduced`, `diverged`, `inconclusive`,
    /// `unsupported`, `blocked`.
    pub status: String,
    pub harness: String,
    pub runs: Vec<RunSummary>,
    /// Details of the first run (statements, final rows, actors).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub statements: Vec<StatementComparison>,
    /// Statements the application sent outside the modeled functions
    /// (wrapper preconditions), with their source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outside_model: Vec<String>,
    /// Statements nobody expected (after the plan or instead of a step).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unexpected: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_tables: BTreeMap<String, Vec<Row>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub runtime_tables: BTreeMap<String, Vec<Row>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actors: Vec<(String, String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assumptions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postgres_version: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub isolation: String,
    pub note: String,
}

/// Semantic operation coverage of one extracted actor (Phase 7). Counts
/// operations of the concurrency IR, not AST nodes.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    /// Database statements, transaction control and external effects
    /// represented exactly within the modeled subset.
    pub represented_operations: usize,
    /// Operations whose behavior rests on a declared assumption (wrapper
    /// invocation, declared preconditions).
    pub assumed_operations: usize,
    /// Operations lowered to `Unknown` (reaching one makes the result
    /// `UNSUPPORTED`).
    pub unsupported_operations: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsupported: Vec<String>,
}

/// The machine-readable extraction report of one actor (Phase 7).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractionReport {
    pub actor: String,
    pub entry: EntryInfo,
    pub source_coverage: Coverage,
    /// Facts the extractor derived from source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_facts: Vec<String>,
    /// Facts the scenario supplied (not proven from source).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scenario_assumptions: Vec<String>,
}

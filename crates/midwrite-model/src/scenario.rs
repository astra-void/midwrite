//! The declarative scenario format and its validation.
//!
//! A scenario states *everything* the model checker assumes: the schema and
//! its constraints, the initial rows, the isolation level, the actors (which
//! function each runs, with which arguments, in which process), external
//! channels and their deduplication, retry and crash bounds, and the
//! invariants. Nothing is inferred or invented: unknown tables, columns,
//! functions, invariants and unsupported constructs are rejected with the
//! path of the offending field.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;

use crate::db::{ColumnSchema, Schema, TableSchema};
use crate::dsl;
use crate::invariant::Invariant;
use crate::ir::{
    ActorSpec, Channels, FunctionRef, Isolation, Op, OperationRetry, Origin, Step, StmtKind,
};
use crate::model::{Bounds, Model, Options, Strategy};
use crate::provider::{
    ActorProvider, ExternalBinding, ExtractRequest, TableBinding, TransactionRetryBinding,
};
use crate::value::{Expr, Row, Value};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {message}")]
pub struct ScenarioError {
    pub path: String,
    pub message: String,
}

fn bad<T>(path: impl Into<String>, message: impl Into<String>) -> Result<T, ScenarioError> {
    Err(ScenarioError {
        path: path.into(),
        message: message.into(),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawScenario {
    schema_version: u32,
    name: String,
    #[serde(default)]
    description: String,
    database: RawDatabase,
    #[serde(default)]
    channels: Channels,
    #[serde(default)]
    app: BTreeMap<String, Value>,
    /// Bounded connection pools, process name → capacity.
    #[serde(default)]
    pools: BTreeMap<String, u16>,
    actors: Vec<RawActor>,
    #[serde(default)]
    bounds: Bounds,
    #[serde(default)]
    options: RawOptions,
    #[serde(default)]
    invariants: Vec<Invariant>,
    #[serde(default)]
    extraction: RawExtraction,
    /// How to run each actor's original code
    /// (`midwrite model-check --verify-source`).
    #[serde(default)]
    source_verification: Option<crate::source_types::SourceSpec>,
    /// Model features beyond the Phase 12 semantics, opted into
    /// explicitly so that earlier scenarios keep their meaning:
    /// `foreign-keys` (enforce the source schema's foreign keys and their
    /// row locks), `replicas` (reads through a replicated database see a
    /// lagging replica).
    #[serde(default)]
    features: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawDatabase {
    #[serde(default = "default_iso")]
    isolation: String,
    tables: Vec<RawTable>,
    #[serde(default)]
    initial_state: BTreeMap<String, Vec<Row>>,
}

fn default_iso() -> String {
    "read-committed".into()
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawTable {
    /// Required unless `from` names the table in the extracted schema.
    #[serde(default)]
    name: String,
    /// With `from`: the columns to model (names, or full declarations that
    /// must agree with the source); empty: every modelable column.
    #[serde(default)]
    columns: Vec<RawColumn>,
    #[serde(default)]
    primary_key: Vec<String>,
    #[serde(default)]
    unique: Vec<Vec<String>>,
    /// Unique sets declared `NULLS NOT DISTINCT` (PostgreSQL 15+),
    /// kept apart from `unique` (whose sets are NULLS DISTINCT). With
    /// `from`, a non-empty list must agree with the source.
    #[serde(default)]
    nulls_not_distinct: Vec<Vec<String>>,
    /// CHECK constraints as expressions over `@column`.
    #[serde(default)]
    checks: Vec<String>,
    /// The Drizzle table binding (or SQL name) in
    /// `extraction.schema` this table is read from.
    #[serde(default)]
    from: Option<String>,
    /// Declared facts that intentionally differ from the source.
    #[serde(default)]
    overrides: Option<RawOverrides>,
    /// Declared foreign keys (tables written in the scenario; a
    /// table read from the source gets its foreign keys from the source,
    /// with the `foreign-keys` feature).
    #[serde(default)]
    foreign_keys: Vec<crate::db::ForeignKeySchema>,
}

/// Facts a scenario states against its source, with the reason.
/// Every override is listed in the report as `scenario-override`.
#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawOverrides {
    reason: String,
    #[serde(default)]
    unique: Option<Vec<Vec<String>>>,
    #[serde(default)]
    primary_key: Option<Vec<String>>,
    #[serde(default)]
    not_null: BTreeMap<String, bool>,
}

#[derive(Deserialize, Clone)]
#[serde(untagged)]
enum RawColumn {
    Name(String),
    Full(ColumnSchema),
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawOptions {
    por: Option<bool>,
    strategy: Option<Strategy>,
    minimize: Option<bool>,
    /// Dynamic partial-order reduction instead of sleep sets.
    dpor: Option<bool>,
    /// Source-set DPOR (`stateful-sleep`, `stateful`,
    /// `stateless-sleep`).
    source_dpor: Option<crate::model::SourceDpor>,
    /// Report states in which tasks wait forever.
    termination: Option<bool>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawExtraction {
    /// Project root, relative to the scenario file.
    root: Option<String>,
    /// Drizzle table identifier → table name (default: same name).
    #[serde(default)]
    tables: BTreeMap<String, String>,
    #[serde(default)]
    externals: Vec<RawExternal>,
    /// The Drizzle schema module (relative to `root`) that tables
    /// with `from` are read from. Phase 12: or a list of modules; a table's
    /// `from` is then `binding` (unique across them) or `module#binding`.
    schema: Option<SchemaModules>,
    /// Helpers that re-run a transaction after 40001/40P01.
    #[serde(default)]
    transaction_retries: Vec<RawTransactionRetry>,
    max_inline_depth: Option<usize>,
    /// Project functions declared to return a replicated
    /// database (`withReplicas` routing).
    #[serde(default)]
    replica_factories: Vec<crate::provider::ReplicaFactoryBinding>,
}

#[derive(Deserialize, Clone)]
#[serde(untagged)]
enum SchemaModules {
    One(String),
    Many(Vec<String>),
}

impl SchemaModules {
    fn files(&self) -> Vec<String> {
        match self {
            Self::One(f) => vec![f.clone()],
            Self::Many(v) => v.clone(),
        }
    }
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawTransactionRetry {
    callee: String,
    #[serde(default)]
    callback_argument: usize,
    options_argument: Option<usize>,
    /// Total attempts when the call's options do not give `attempts`.
    #[serde(default = "one_attempt")]
    attempts: u32,
}

fn one_attempt() -> u32 {
    1
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawExternal {
    callee: String,
    channel: String,
    key_argument: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawActor {
    name: Option<String>,
    /// The function's name (for display), or a source selector when
    /// `entry` is not given.
    #[serde(default)]
    function: String,
    #[serde(default)]
    file: Option<String>,
    /// Select the entry function by a source selector (anonymous
    /// callbacks, methods, returned functions) with optional declared
    /// invocation assumptions.
    #[serde(default)]
    entry: Option<RawEntry>,
    /// Positional (`[..]`, for extracted functions) or named (`{..}`).
    #[serde(default)]
    arguments: RawArguments,
    /// A declared program in the scenario DSL.
    #[serde(default)]
    program: Option<String>,
    /// Pre-lowered code vectors.
    #[serde(default)]
    codes: Option<Vec<Vec<Step>>>,
    #[serde(default)]
    process: Option<String>,
    #[serde(default)]
    retry: Option<OperationRetry>,
    #[serde(default)]
    description: String,
    /// The actor is a request handler whose response is a
    /// separate step after its last operation ([`Op::Respond`]): a crash can
    /// happen after the work committed and before the client got the
    /// response.
    #[serde(default)]
    response: bool,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawEntry {
    selector: String,
    #[serde(default)]
    invocation: Option<String>,
    #[serde(default)]
    preconditions: Vec<String>,
    /// A tRPC procedure resolver's declared context.
    #[serde(default)]
    trpc: Option<crate::provider::TrpcContext>,
}

#[derive(Deserialize, Default)]
#[serde(untagged)]
enum RawArguments {
    #[default]
    None,
    Positional(Vec<Value>),
    Named(BTreeMap<String, Value>),
}

/// What the loader learned beyond the model itself.
#[derive(Debug, Default, Clone)]
pub struct LoadReport {
    /// Extraction assumptions per actor.
    pub assumptions: Vec<(String, Vec<String>)>,
    /// Selected entry functions per actor.
    pub entries: Vec<(String, crate::provider::EntryInfo)>,
    /// The scenario's `sourceVerification` section.
    pub source: Option<crate::source_types::SourceSpec>,
    /// The facts behind each table read from the source schema.
    pub schema: Vec<crate::schema_facts::TableFacts>,
    /// The extracted schema (for the catalog comparison).
    pub extracted_schema: Option<crate::schema_facts::ExtractedSchema>,
}

pub fn load_file(
    path: &Path,
    provider: &dyn ActorProvider,
) -> Result<(Model, LoadReport), ScenarioError> {
    let text = std::fs::read_to_string(path).map_err(|e| ScenarioError {
        path: path.display().to_string(),
        message: format!("cannot read the scenario: {e}"),
    })?;
    let base = path.parent().unwrap_or(Path::new("."));
    load_str(&text, base, provider)
}

pub fn load_str(
    text: &str,
    base_dir: &Path,
    provider: &dyn ActorProvider,
) -> Result<(Model, LoadReport), ScenarioError> {
    let raw: RawScenario = serde_json::from_str(text).map_err(|e| ScenarioError {
        path: format!("line {}, column {}", e.line(), e.column()),
        message: e.to_string(),
    })?;
    build(raw, base_dir, provider)
}

fn build(
    raw: RawScenario,
    base_dir: &Path,
    provider: &dyn ActorProvider,
) -> Result<(Model, LoadReport), ScenarioError> {
    if raw.schema_version != SCHEMA_VERSION {
        return bad(
            "schemaVersion",
            format!(
                "unsupported version {}; this build reads version {SCHEMA_VERSION}",
                raw.schema_version
            ),
        );
    }
    let isolation = Isolation::parse(&raw.database.isolation).ok_or_else(|| ScenarioError {
        path: "database.isolation".into(),
        message: format!(
            "unknown isolation level `{}` (read-committed, repeatable-read or serializable)",
            raw.database.isolation
        ),
    })?;
    let mut features = crate::model::Features::default();
    for (i, f) in raw.features.iter().enumerate() {
        match f.as_str() {
            "foreign-keys" => features.foreign_keys = true,
            "replicas" => features.replicas = true,
            "js-values" => features.js_values = true,
            "timestamps" => features.timestamps = true,
            "json" => features.json = true,
            other => {
                return bad(
                    format!("features[{i}]"),
                    format!(
                        "unknown feature `{other}` (foreign-keys, replicas, js-values, timestamps, json)"
                    ),
                );
            }
        }
    }
    let (tables, schema_facts, extracted_schema) = link_source_schema(
        &raw.database.tables,
        &raw.extraction,
        base_dir,
        provider,
        features,
    )?;
    let schema = build_schema(&tables, features)?;
    let initial_rows = build_initial(&schema, &raw.database.initial_state)?;
    if raw.actors.is_empty() {
        return bad("actors", "a scenario needs at least one actor");
    }
    if raw.actors.len() > raw.bounds.max_actors {
        return bad(
            "actors",
            format!(
                "{} actors exceed bounds.maxActors = {}",
                raw.actors.len(),
                raw.bounds.max_actors
            ),
        );
    }
    let mut report = LoadReport {
        source: raw.source_verification.clone(),
        schema: schema_facts,
        extracted_schema,
        ..LoadReport::default()
    };
    let mut actors = Vec::new();
    let mut process_names: Vec<String> = Vec::new();
    let mut process_of = Vec::new();
    let mut seen = BTreeSet::new();
    // Tables read from the source are known to the actor extractor by
    // their Drizzle binding.
    let mut extraction = raw.extraction.clone();
    for facts in &report.schema {
        if let Some(binding) = &facts.binding
            && !extraction.tables.contains_key(binding)
        {
            extraction
                .tables
                .insert(binding.clone(), facts.table.clone());
        }
    }
    let extraction = &extraction;
    for (i, a) in raw.actors.iter().enumerate() {
        let path = format!("actors[{i}]");
        let name = a
            .name
            .clone()
            .unwrap_or_else(|| ((b'A' + (i % 26) as u8) as char).to_string());
        if !seen.insert(name.clone()) {
            return bad(
                format!("{path}.name"),
                format!("duplicate actor name `{name}`"),
            );
        }
        if a.function.is_empty() && a.entry.is_none() {
            return bad(
                format!("{path}.function"),
                "an actor needs `function` or `entry`",
            );
        }
        let wants = Wants {
            replicas: features.replicas,
            js_values: features.js_values,
            return_values: raw.invariants.iter().any(|i| i.formula.reads_returned()),
        };
        let (params, mut codes, arguments, assumptions, entry) = actor_program(
            a,
            &path,
            extraction,
            &schema,
            &report.schema,
            base_dir,
            provider,
            wants,
        )?;
        seal_unknowns(&mut codes);
        if a.response {
            insert_responses(&mut codes);
        }
        if !assumptions.is_empty() {
            report.assumptions.push((name.clone(), assumptions));
        }
        if let Some(entry) = entry {
            report.entries.push((name.clone(), entry));
        }
        let process = a.process.clone().unwrap_or_else(|| name.clone());
        let pi = match process_names.iter().position(|p| *p == process) {
            Some(p) => p,
            None => {
                process_names.push(process);
                process_names.len() - 1
            }
        };
        process_of.push(pi);
        let _ = params;
        actors.push(ActorSpec {
            name,
            function: FunctionRef {
                name: if a.function.is_empty() {
                    a.entry
                        .as_ref()
                        .map(|e| e.selector.clone())
                        .unwrap_or_default()
                } else {
                    a.function.clone()
                },
                file: a.file.clone().or_else(|| {
                    a.entry
                        .as_ref()
                        .and_then(|e| e.selector.split(['#', '@']).next().map(str::to_owned))
                }),
            },
            arguments,
            codes,
            retry: a.retry,
            description: a.description.clone(),
        });
    }
    let mut options = Options::default();
    if let Some(p) = raw.options.por {
        options.por = p;
    }
    if let Some(s) = raw.options.strategy {
        options.strategy = s;
    }
    if let Some(d) = raw.options.dpor {
        options.dpor = d;
    }
    if let Some(d) = raw.options.source_dpor {
        options.source_dpor = Some(d);
    }
    if let Some(t) = raw.options.termination {
        options.termination = t;
    }
    if let Some(m) = raw.options.minimize {
        options.minimize = m;
    }
    let raw_pools = raw.pools.clone();
    let mut model = Model {
        name: raw.name,
        description: raw.description,
        schema,
        initial_rows,
        isolation,
        actors,
        process_of,
        pools: Vec::new(),
        process_names,
        channels: raw.channels,
        app_initial: raw.app,
        invariants: raw.invariants,
        bounds: raw.bounds,
        options,
        assumptions: Vec::new(),
        features,
    };
    for (name, cap) in &raw_pools {
        let Some(i) = model.process_names.iter().position(|p| p == name) else {
            return bad(
                "pools",
                format!("`{name}` is not the name of an actor or a process"),
            );
        };
        if *cap == 0 {
            return bad(
                "pools",
                format!("the pool of `{name}` needs a capacity of 1 or more"),
            );
        }
        if model.pools.is_empty() {
            model.pools = vec![None; model.process_names.len()];
        }
        model.pools[i] = Some(*cap);
    }
    if (model.options.termination || model.has_pools())
        && !model
            .invariants
            .iter()
            .any(|i| i.name == crate::model::TERMINATION)
    {
        model.invariants.push(Invariant {
            name: crate::model::TERMINATION.to_owned(),
            when: crate::invariant::When::Final,
            description: "no task waits forever: not for a connection of a bounded pool, and not for a lock that only the application's own waiting could release".to_owned(),
            formula: crate::invariant::Formula::Eq([
                crate::invariant::Term::Op(crate::invariant::TermOp::Stuck(())),
                crate::invariant::Term::Int(0),
            ]),
        });
    }
    validate(&model)?;
    check_replica_reads(&model)?;
    check_foreign_keys(&model)?;
    check_return_values(&model)?;
    model.assumptions = standard_assumptions(&model, &report);
    Ok((model, report))
}

// ---------------------------------------------------------------------------
// Tables read from the application's Drizzle schema
// ---------------------------------------------------------------------------

/// The model type of an extracted column, if the model represents it.
/// Time and JSON columns with the features `timestamps` / `json`
/// (Drizzle's default modes only: a `Date` for `timestamp`, a string for
/// `date`, a parsed value for `json`/`jsonb`).
fn model_type(
    c: &crate::schema_facts::SchemaColumn,
    features: crate::model::Features,
) -> Option<&'static str> {
    match c.builder.as_str() {
        "serial" | "bigserial" | "smallserial" | "integer" | "smallint" | "bigint" => Some("int"),
        "text" | "varchar" | "char" | "uuid" => Some("text"),
        "boolean" => Some("bool"),
        "timestamp" if features.timestamps && matches!(c.mode.as_deref(), None | Some("date")) => {
            Some(if c.sql_type == "timestamp with time zone" {
                "timestamptz"
            } else {
                "timestamp"
            })
        }
        "date" if features.timestamps && matches!(c.mode.as_deref(), None | Some("string")) => {
            Some("date")
        }
        "json" if features.json && c.mode.is_none() => Some("json"),
        "jsonb" if features.json && c.mode.is_none() => Some("jsonb"),
        _ if c.sql_type.starts_with("enum ") => Some("text"),
        _ => None,
    }
}

/// `() => new Date()` (a Drizzle `$defaultFn` / `$onUpdate`
/// callback reading the application clock), as the extractor recorded it.
fn is_clock_callback(text: &str) -> bool {
    let t: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    matches!(
        t.as_str(),
        "()=>newDate()" | "()=>{returnnewDate();}" | "()=>{returnnewDate()}"
    )
}

fn literal_value(l: &crate::schema_facts::SchemaLiteral) -> Value {
    use crate::schema_facts::SchemaLiteral;
    match l {
        SchemaLiteral::Integer(n) => Value::Int(*n),
        SchemaLiteral::Text(t) => Value::Str(t.clone()),
        SchemaLiteral::Boolean(b) => Value::Bool(*b),
        SchemaLiteral::Null => Value::Null,
    }
}

type Linked = (
    Vec<RawTable>,
    Vec<crate::schema_facts::TableFacts>,
    Option<crate::schema_facts::ExtractedSchema>,
);

/// Resolve the tables declared with `from` against the extracted schema:
/// every fact comes from the source unless the scenario overrides it
/// visibly; a declared fact that contradicts the source is an error.
fn link_source_schema(
    raw: &[RawTable],
    extraction: &RawExtraction,
    base_dir: &Path,
    provider: &dyn ActorProvider,
    features: crate::model::Features,
) -> Result<Linked, ScenarioError> {
    let foreign_keys = features.foreign_keys;
    if raw.iter().all(|t| t.from.is_none()) {
        for (i, t) in raw.iter().enumerate() {
            if t.overrides.is_some() {
                return bad(
                    format!("database.tables[{i}].overrides"),
                    "overrides need a table read from the source (`from`)",
                );
            }
        }
        return Ok((raw.to_vec(), Vec::new(), None));
    }
    let Some(modules) = &extraction.schema else {
        return bad(
            "extraction.schema",
            "tables with `from` need `extraction.schema` (the Drizzle schema module)",
        );
    };
    let files = modules.files();
    if files.is_empty() {
        return bad("extraction.schema", "the list of schema modules is empty");
    }
    let root = base_dir.join(extraction.root.as_deref().unwrap_or("."));
    let mut modules: Vec<(String, crate::schema_facts::ExtractedSchema)> = Vec::new();
    for file in &files {
        if modules.iter().any(|(f, _)| f == file) {
            return bad("extraction.schema", format!("`{file}` is listed twice"));
        }
        let extracted = provider
            .extract_schema(&root, file)
            .map_err(|e| ScenarioError {
                path: "extraction.schema".into(),
                message: e,
            })?;
        modules.push((file.clone(), extracted));
    }
    let mut tables = Vec::new();
    let mut facts = Vec::new();
    let mut used: Vec<(String, String)> = Vec::new();
    // (index in `tables`, index in `facts`, module, declaration)
    // of every table read from the source.
    let mut linked: Vec<(usize, usize, String, crate::schema_facts::SchemaTable)> = Vec::new();
    for (i, t) in raw.iter().enumerate() {
        if t.from.is_some() && !t.foreign_keys.is_empty() {
            return bad(
                format!("database.tables[{i}].foreignKeys"),
                "a table read from the source takes its foreign keys from the source (feature `foreign-keys`)",
            );
        }
        match &t.from {
            None => tables.push(t.clone()),
            Some(from) => {
                // `module#binding`, or a binding (or SQL name) that exactly
                // one listed module declares.
                let (wanted_file, binding) = match from.rsplit_once('#') {
                    Some((f, b)) => (Some(f), b),
                    None => (None, from.as_str()),
                };
                let found: Vec<(&String, &crate::schema_facts::SchemaTable)> = modules
                    .iter()
                    .filter(|(f, _)| wanted_file.is_none_or(|w| w == f))
                    .filter_map(|(f, x)| x.table(binding).map(|t| (f, t)))
                    .collect();
                let (file, x) = match found.as_slice() {
                    [one] => *one,
                    [] => {
                        return bad(
                            format!("database.tables[{i}].from"),
                            match wanted_file {
                                Some(f) if !files.iter().any(|x| x == f) => {
                                    format!("`{f}` is not listed in `extraction.schema`")
                                }
                                _ => format!(
                                    "`{}` declares no table `{binding}` the extractor understood",
                                    wanted_file.map_or_else(|| files.join("`, `"), str::to_owned)
                                ),
                            },
                        );
                    }
                    many => {
                        return bad(
                            format!("database.tables[{i}].from"),
                            format!(
                                "`{binding}` is declared by several schema modules ({}); write `module#binding`",
                                many.iter()
                                    .map(|(f, _)| f.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        );
                    }
                };
                let key = (file.clone(), x.binding.clone());
                if used.contains(&key) {
                    return bad(
                        format!("database.tables[{i}].from"),
                        format!(
                            "`{}#{}` is read twice (two references to one declaration are one table)",
                            key.0, key.1
                        ),
                    );
                }
                used.push(key);
                let (table, mut table_facts) = link_table(t, i, x, file, features)?;
                // Reported for a list of modules only: single-module reports
                // stay as they were.
                if matches!(extraction.schema, Some(SchemaModules::Many(_))) {
                    table_facts.module = Some(file.clone());
                }
                linked.push((tables.len(), facts.len(), file.clone(), x.clone()));
                tables.push(table);
                facts.push(table_facts);
            }
        }
    }
    // Two declarations with the same SQL name in one schema are a conflict
    // the model cannot represent (they would be one table).
    let mut names: BTreeMap<(Option<String>, String), String> = BTreeMap::new();
    for (file, x) in &modules {
        for t in &x.tables {
            let key = (t.schema.clone(), t.name.clone());
            let here = format!("{file}#{}", t.binding);
            if let Some(other) = names.insert(key, here.clone())
                && facts.iter().any(|f| f.table == t.name)
            {
                return bad(
                    "extraction.schema",
                    format!(
                        "the SQL table `{}` is declared twice ({other} and {here})",
                        t.name
                    ),
                );
            }
        }
    }
    if foreign_keys {
        // The frontend resolves imports relative to the canonical root.
        let canonical = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        link_foreign_keys(&mut tables, &mut facts, &linked, &canonical, provider);
    }
    let mut merged = crate::schema_facts::ExtractedSchema::default();
    for (_, x) in modules {
        merged.tables.extend(x.tables);
        merged.enums.extend(x.enums);
        merged.unsupported.extend(x.unsupported);
    }
    Ok((tables, facts, Some(merged)))
}

#[allow(clippy::too_many_lines)]
fn link_table(
    t: &RawTable,
    i: usize,
    x: &crate::schema_facts::SchemaTable,
    file: &str,
    features: crate::model::Features,
) -> Result<(RawTable, crate::schema_facts::TableFacts), ScenarioError> {
    use crate::db::{ColumnDefault, SequenceSpec};
    use crate::schema_facts::{FactOrigin, SchemaDefault, SchemaFact, TableFacts};
    let path = format!("database.tables[{i}]");
    let at = |line: u32| format!("{file}:{line}");
    let mut facts: Vec<SchemaFact> = Vec::new();
    let fact = |facts: &mut Vec<SchemaFact>, fact: String, origin, source: String| {
        facts.push(SchemaFact {
            fact,
            origin,
            source,
        });
    };
    if !t.name.is_empty() && t.name != x.name {
        return bad(
            format!("{path}.name"),
            format!(
                "the scenario names the table `{}`, the source (`{}` at {}) `{}`",
                t.name,
                x.binding,
                at(x.line),
                x.name
            ),
        );
    }
    fact(
        &mut facts,
        format!(
            "table `{}`{} from binding `{}`",
            x.name,
            x.schema
                .as_ref()
                .map(|s| format!(" in schema `{s}`"))
                .unwrap_or_default(),
            x.binding
        ),
        FactOrigin::ExtractedFromSource,
        at(x.line),
    );
    let overrides = t.overrides.clone().unwrap_or_default();
    let reason = &overrides.reason;
    // Which columns the scenario models.
    let declared: Vec<(String, Option<ColumnSchema>)> = t
        .columns
        .iter()
        .map(|c| match c {
            RawColumn::Name(n) => (n.clone(), None),
            RawColumn::Full(c) => (c.name.clone(), Some(c.clone())),
        })
        .collect();
    for (name, _) in &declared {
        if x.column(name).is_none() {
            return bad(
                format!("{path}.columns"),
                format!(
                    "`{}` has no column `{name}` (TypeScript property names: {})",
                    x.binding,
                    x.columns
                        .iter()
                        .map(|c| c.property.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
    }
    let explicit = !declared.is_empty();
    let mut columns = Vec::new();
    for c in &x.columns {
        let declared_column = declared.iter().find(|(n, _)| *n == c.property);
        if explicit && declared_column.is_none() {
            fact(
                &mut facts,
                format!("column `{}`: not selected by the scenario", c.sql_name),
                FactOrigin::NotModeled,
                at(c.line),
            );
            continue;
        }
        let Some(ty) = model_type(c, features) else {
            if explicit {
                return bad(
                    format!("{path}.columns"),
                    format!(
                        "column `{}` has type `{}`, which the model does not represent",
                        c.property, c.sql_type
                    ),
                );
            }
            fact(
                &mut facts,
                format!(
                    "column `{}`: type `{}` is not modeled",
                    c.sql_name, c.sql_type
                ),
                FactOrigin::NotModeled,
                at(c.line),
            );
            if c.not_null && c.default.is_none() {
                fact(
                    &mut facts,
                    format!(
                        "column `{}` is NOT NULL without a default and is not modeled: an INSERT the model accepts may fail in PostgreSQL",
                        c.sql_name
                    ),
                    FactOrigin::NotModeled,
                    at(c.line),
                );
            }
            continue;
        };
        let generated = c
            .generation
            .as_ref()
            .and_then(|g| g.fresh_identifier().map(|id| (g, id)));
        // Clock defaults of time columns (feature `timestamps`).
        let time = crate::db::is_time_type(Some(ty));
        let clock_default = match &c.default {
            Some(SchemaDefault::Expression { text })
                if time && ty != "date" && text.trim().eq_ignore_ascii_case("now()") =>
            {
                Some(ColumnDefault::Now)
            }
            Some(SchemaDefault::Application { text })
                if time && ty != "date" && is_clock_callback(text) =>
            {
                Some(ColumnDefault::Clock)
            }
            _ => None,
        };
        let on_update = match &c.on_update {
            None => None,
            Some(text) if time && ty != "date" && is_clock_callback(text) => {
                Some(ColumnDefault::Clock)
            }
            Some(text) => {
                if explicit {
                    return bad(
                        format!("{path}.columns"),
                        format!(
                            "column `{}` has `$onUpdate({text})`, which the model does not represent",
                            c.property
                        ),
                    );
                }
                fact(
                    &mut facts,
                    format!(
                        "column `{}`: `$onUpdate({text})` is not modeled",
                        c.sql_name
                    ),
                    FactOrigin::NotModeled,
                    at(c.line),
                );
                continue;
            }
        };
        let default = match &c.default {
            Some(_) if clock_default.is_some() => clock_default.clone(),
            // A recognized identifier generator, database side
            // (`defaultRandom()`) or application side (`$defaultFn` with a
            // recognized callback). Other defaults keep their earlier
            // treatment.
            Some(_) if generated.is_some() => {
                let (g, id) = generated.expect("checked");
                Some(ColumnDefault::Generated(crate::db::GeneratedSpec {
                    side: if g.class == crate::schema_facts::GeneratorClass::ApplicationGenerated {
                        crate::db::GeneratedSide::Application
                    } else {
                        crate::db::GeneratedSide::Database
                    },
                    generator: id.to_owned(),
                }))
            }
            None => None,
            Some(SchemaDefault::Constant { value }) => {
                Some(ColumnDefault::Value(literal_value(value)))
            }
            Some(SchemaDefault::Sequence { .. }) => {
                Some(ColumnDefault::Sequence(SequenceSpec::default()))
            }
            Some(SchemaDefault::Expression { text } | SchemaDefault::Application { text }) => {
                let by = if matches!(c.default, Some(SchemaDefault::Application { .. })) {
                    "the application (Drizzle)"
                } else {
                    "the database"
                };
                if explicit {
                    return bad(
                        format!("{path}.columns"),
                        format!(
                            "column `{}` takes a default computed by {by} (`{text}`), which the model does not represent",
                            c.property
                        ),
                    );
                }
                fact(
                    &mut facts,
                    format!(
                        "column `{}`: default `{text}` computed by {by} is not modeled",
                        c.sql_name
                    ),
                    FactOrigin::NotModeled,
                    at(c.line),
                );
                continue;
            }
        };
        let mut not_null = c.not_null;
        if let Some((_, Some(d))) = declared_column {
            if d.not_null != c.not_null && !overrides.not_null.contains_key(&c.property) {
                return bad(
                    format!("{path}.columns"),
                    format!(
                        "column `{}`: the scenario says {}, the source ({}) says {}; state an override with a reason to model the difference",
                        c.property,
                        if d.not_null { "NOT NULL" } else { "nullable" },
                        at(c.line),
                        if c.not_null { "NOT NULL" } else { "nullable" }
                    ),
                );
            }
            if let Some(sql) = &d.sql_name
                && *sql != c.sql_name
            {
                return bad(
                    format!("{path}.columns"),
                    format!(
                        "column `{}`: the scenario's sqlName `{sql}` differs from the source's `{}`",
                        c.property, c.sql_name
                    ),
                );
            }
            if let Some(dt) = &d.sql_type
                && dt != ty
            {
                return bad(
                    format!("{path}.columns"),
                    format!(
                        "column `{}`: type `{dt}` declared, `{ty}` in the source",
                        c.property
                    ),
                );
            }
            if d.default.is_some() && d.default != default {
                return bad(
                    format!("{path}.columns"),
                    format!(
                        "column `{}`: the declared default differs from the source",
                        c.property
                    ),
                );
            }
        }
        if let Some(value) = overrides.not_null.get(&c.property) {
            not_null = *value;
            fact(
                &mut facts,
                format!(
                    "column `{}`: {} (source: {}) because {reason}",
                    c.sql_name,
                    if *value { "NOT NULL" } else { "nullable" },
                    if c.not_null { "NOT NULL" } else { "nullable" }
                ),
                FactOrigin::ScenarioOverride,
                format!("{path}.overrides.notNull"),
            );
        }
        fact(
            &mut facts,
            format!(
                "column `{}` → `{}` {ty}{}{}",
                c.property,
                c.sql_name,
                if not_null { " NOT NULL" } else { "" },
                match &default {
                    Some(ColumnDefault::Value(v)) => format!(" DEFAULT {}", v.render()),
                    Some(ColumnDefault::Sequence(_)) => " generated (sequence)".to_owned(),
                    Some(ColumnDefault::Generated(g)) => format!(
                        " generated by the {} (`{}`)",
                        match g.side {
                            crate::db::GeneratedSide::Database => "database",
                            crate::db::GeneratedSide::Application => "application",
                        },
                        g.generator
                    ),
                    Some(ColumnDefault::Now) =>
                        " DEFAULT now() (the transaction's start time, an opaque clock reading)"
                            .to_owned(),
                    Some(ColumnDefault::Clock) =>
                        " computed by the application (`$defaultFn(() => new Date())`, an opaque clock reading)"
                            .to_owned(),
                    None => String::new(),
                } + if on_update.is_some() {
                    " ON UPDATE application clock (`$onUpdate(() => new Date())`)"
                } else {
                    ""
                }
            ),
            if declared_column.is_some_and(|(_, d)| d.is_some()) {
                FactOrigin::DeclaredByScenario
            } else {
                FactOrigin::ExtractedFromSource
            },
            at(c.line),
        );
        if !c.enum_values.is_empty() {
            fact(
                &mut facts,
                format!(
                    "column `{}`: enum domain ({}) is not enforced by the model",
                    c.sql_name,
                    c.enum_values.join(", ")
                ),
                FactOrigin::NotModeled,
                at(c.line),
            );
        }
        columns.push(ColumnSchema {
            name: c.property.clone(),
            not_null,
            sql_type: Some(ty.to_owned()),
            default,
            sql_name: (c.sql_name != c.property).then(|| c.sql_name.clone()),
            on_update,
        });
    }
    let property = |sql: &str| -> Option<String> {
        x.column_by_sql(sql)
            .map(|c| c.property.clone())
            .filter(|p| columns.iter().any(|m| &m.name == p))
    };
    // Primary key.
    let mut primary_key: Vec<String> = Vec::new();
    for sql in &x.primary_key {
        match property(sql) {
            Some(p) => primary_key.push(p),
            None => {
                return bad(
                    path.clone(),
                    format!("primary key column `{sql}` of `{}` is not modeled", x.name),
                );
            }
        }
    }
    if !primary_key.is_empty() {
        fact(
            &mut facts,
            format!("primary key ({})", x.primary_key.join(", ")),
            FactOrigin::ExtractedFromSource,
            at(x.line),
        );
    }
    if let Some(pk) = &overrides.primary_key {
        fact(
            &mut facts,
            format!(
                "primary key ({}) instead of ({}) because {reason}",
                pk.join(", "),
                primary_key.join(", ")
            ),
            FactOrigin::ScenarioOverride,
            format!("{path}.overrides.primaryKey"),
        );
        primary_key = pk.clone();
    } else if !t.primary_key.is_empty() && t.primary_key != primary_key {
        return bad(
            format!("{path}.primaryKey"),
            format!(
                "the scenario's primary key ({}) differs from the source's ({}); state an override with a reason",
                t.primary_key.join(", "),
                primary_key.join(", ")
            ),
        );
    }
    // Uniqueness: only global rules over modeled columns.
    let mut unique: Vec<Vec<String>> = Vec::new();
    // NULLS NOT DISTINCT rules over a nullable column.
    let mut nulls_not_distinct: Vec<Vec<String>> = Vec::new();
    for u in &x.unique {
        let name = u.name.clone().unwrap_or_else(|| u.columns.join("_"));
        let mapped: Option<Vec<String>> = u.columns.iter().map(|c| property(c)).collect();
        let nullable = u
            .columns
            .iter()
            .any(|c| x.column_by_sql(c).is_some_and(|c| !c.not_null));
        if let Some(predicate) = &u.predicate {
            fact(
                &mut facts,
                format!(
                    "partial unique {} `{name}` on ({}) WHERE {predicate}: not modeled (it is not a global uniqueness rule)",
                    u.kind,
                    u.columns.join(", ")
                ),
                FactOrigin::NotModeled,
                at(u.line),
            );
        } else if let (true, true, Some(cols)) = (u.nulls_not_distinct, nullable, &mapped) {
            // Enforced with NULL equal to NULL.
            fact(
                &mut facts,
                format!(
                    "unique {} `{name}` NULLS NOT DISTINCT on nullable ({}): NULLs are equal for this rule (PostgreSQL 15+); a second row with the same key, NULLs included, conflicts",
                    u.kind,
                    u.columns.join(", ")
                ),
                FactOrigin::ExtractedFromSource,
                at(u.line),
            );
            nulls_not_distinct.push(cols.clone());
        } else if let Some(cols) = mapped {
            fact(
                &mut facts,
                format!(
                    "unique {} `{name}` on ({}){}",
                    u.kind,
                    u.columns.join(", "),
                    if nullable {
                        " (nullable: NULLs never conflict)"
                    } else {
                        ""
                    }
                ),
                FactOrigin::ExtractedFromSource,
                at(u.line),
            );
            unique.push(cols);
        } else {
            fact(
                &mut facts,
                format!(
                    "unique {} `{name}` on ({}){}: a column is not modeled",
                    u.kind,
                    u.columns.join(", "),
                    if u.nulls_not_distinct && nullable {
                        " NULLS NOT DISTINCT"
                    } else {
                        ""
                    }
                ),
                FactOrigin::NotModeled,
                at(u.line),
            );
        }
    }
    let sorted = |v: &[Vec<String>]| {
        let mut v: Vec<Vec<String>> = v.to_vec();
        v.sort();
        v
    };
    if let Some(declared_unique) = &overrides.unique {
        fact(
            &mut facts,
            format!(
                "unique sets {:?} instead of the source's {:?} because {reason}",
                declared_unique, unique
            ),
            FactOrigin::ScenarioOverride,
            format!("{path}.overrides.unique"),
        );
        unique = declared_unique.clone();
    } else if !t.unique.is_empty() && sorted(&t.unique) != sorted(&unique) {
        return bad(
            format!("{path}.unique"),
            format!(
                "the scenario's unique sets {:?} differ from the source's {:?}; state an override with a reason",
                t.unique, unique
            ),
        );
    }
    // Declared NULLS NOT DISTINCT sets agree with the source.
    if !t.nulls_not_distinct.is_empty()
        && sorted(&t.nulls_not_distinct) != sorted(&nulls_not_distinct)
    {
        return bad(
            format!("{path}.nullsNotDistinct"),
            format!(
                "the scenario's NULLS NOT DISTINCT sets {:?} differ from the source's {:?}",
                t.nulls_not_distinct, nulls_not_distinct
            ),
        );
    }
    for fk in &x.foreign_keys {
        fact(
            &mut facts,
            format!(
                "foreign key ({}) → {}({}): not modeled (a violation, SQLSTATE 23503, is not predicted)",
                fk.columns.join(", "),
                fk.table,
                fk.foreign_columns.join(", ")
            ),
            FactOrigin::NotModeled,
            at(fk.line),
        );
    }
    for check in &x.checks {
        fact(
            &mut facts,
            format!("CHECK `{check}`: not interpreted"),
            FactOrigin::NotModeled,
            at(x.line),
        );
    }
    for u in &x.unsupported {
        fact(&mut facts, u.clone(), FactOrigin::Unknown, file.to_owned());
    }
    Ok((
        RawTable {
            name: x.name.clone(),
            columns: columns.into_iter().map(RawColumn::Full).collect(),
            primary_key,
            unique,
            nulls_not_distinct,
            checks: t.checks.clone(),
            from: None,
            overrides: None,
            foreign_keys: Vec::new(),
        },
        TableFacts {
            table: x.name.clone(),
            binding: Some(x.binding.clone()),
            module: None,
            facts,
        },
    ))
}

fn build_schema(
    raw: &[RawTable],
    features: crate::model::Features,
) -> Result<Schema, ScenarioError> {
    let mut tables = Vec::new();
    let mut names = BTreeSet::new();
    for (i, t) in raw.iter().enumerate() {
        let path = format!("database.tables[{i}]");
        if t.name.is_empty() {
            return bad(format!("{path}.name"), "a table needs `name` (or `from`)");
        }
        if !names.insert(t.name.clone()) {
            return bad(
                format!("{path}.name"),
                format!("duplicate table `{}`", t.name),
            );
        }
        let columns: Vec<ColumnSchema> = t
            .columns
            .iter()
            .map(|c| match c {
                RawColumn::Name(n) => ColumnSchema {
                    name: n.clone(),
                    not_null: false,
                    sql_type: None,
                    default: None,
                    sql_name: None,
                    on_update: None,
                },
                RawColumn::Full(c) => c.clone(),
            })
            .collect();
        let mut seen = BTreeSet::new();
        for c in &columns {
            let time = crate::db::is_time_type(c.sql_type.as_deref());
            let json = crate::db::is_json_type(c.sql_type.as_deref());
            if let Some(ty) = &c.sql_type
                && !matches!(ty.as_str(), "int" | "text" | "bool")
                && !(time && features.timestamps)
                && !(json && features.json)
            {
                return bad(
                    format!("{path}.columns"),
                    format!(
                        "column `{}`: unsupported type `{ty}` (int, text or bool; timestamp, timestamptz and date with the feature `timestamps`; json and jsonb with the feature `json`)",
                        c.name
                    ),
                );
            }
            // Clock defaults belong to `timestamp`/`timestamptz`
            // columns.
            let clock_ok = time && c.sql_type.as_deref() != Some("date");
            if matches!(
                c.default,
                Some(crate::db::ColumnDefault::Now | crate::db::ColumnDefault::Clock)
            ) && !clock_ok
            {
                return bad(
                    format!("{path}.columns"),
                    format!(
                        "column `{}`: a clock default needs a timestamp or timestamptz column",
                        c.name
                    ),
                );
            }
            match &c.on_update {
                None => {}
                Some(crate::db::ColumnDefault::Clock) if clock_ok => {}
                Some(_) => {
                    return bad(
                        format!("{path}.columns"),
                        format!(
                            "column `{}`: only `onUpdate: \"clock\"` on a timestamp or timestamptz column is modeled",
                            c.name
                        ),
                    );
                }
            }
            if let Some(crate::db::ColumnDefault::Value(v)) = &c.default
                && let Some(why) = crate::db::typed_value_problem(c, v)
            {
                return bad(format!("{path}.columns"), why);
            }
            if !seen.insert(c.name.clone()) {
                return bad(
                    format!("{path}.columns"),
                    format!("duplicate column `{}`", c.name),
                );
            }
        }
        let has = |c: &String| columns.iter().any(|x| &x.name == c);
        for c in t
            .primary_key
            .iter()
            .chain(t.unique.iter().flatten())
            .chain(t.nulls_not_distinct.iter().flatten())
        {
            if !has(c) {
                return bad(
                    path.clone(),
                    format!("constraint refers to unknown column `{c}`"),
                );
            }
        }
        let mut checks = Vec::new();
        for (j, text) in t.checks.iter().enumerate() {
            let e = dsl::parse_expr(text).map_err(|e| ScenarioError {
                path: format!("{path}.checks[{j}]"),
                message: e.to_string(),
            })?;
            let mut cols = Vec::new();
            e.columns(&mut cols);
            let mut locals = Vec::new();
            e.locals(&mut locals);
            if !locals.is_empty() {
                return bad(
                    format!("{path}.checks[{j}]"),
                    format!(
                        "a CHECK may only reference columns (`@name`), not `{}`",
                        locals[0]
                    ),
                );
            }
            if let Some(c) = cols.iter().find(|c| !has(c)) {
                return bad(
                    format!("{path}.checks[{j}]"),
                    format!("unknown column `{c}`"),
                );
            }
            if e.unknown_detail().is_some() {
                return bad(format!("{path}.checks[{j}]"), "unsupported expression");
            }
            checks.push(e);
        }
        tables.push(TableSchema {
            name: t.name.clone(),
            columns,
            primary_key: t.primary_key.clone(),
            unique: t.unique.clone(),
            nulls_not_distinct: t.nulls_not_distinct.clone(),
            checks,
            foreign_keys: t.foreign_keys.clone(),
        });
    }
    let schema = Schema { tables };
    // A foreign key references a key of a modeled table.
    for (i, t) in schema.tables.iter().enumerate() {
        for (j, fk) in t.foreign_keys.iter().enumerate() {
            let path = format!("database.tables[{i}].foreignKeys[{j}]");
            let Some(parent) = schema.by_name(&fk.references) else {
                return bad(path, format!("unknown table `{}`", fk.references));
            };
            if fk.columns.is_empty() || fk.columns.len() != fk.referenced_columns.len() {
                return bad(
                    path,
                    "`columns` and `referencedColumns` need the same, non-zero length",
                );
            }
            if let Some(c) = fk.columns.iter().find(|c| !t.has_column(c)) {
                return bad(path, format!("unknown column `{c}`"));
            }
            if let Some(c) = fk.referenced_columns.iter().find(|c| !parent.has_column(c)) {
                return bad(path, format!("unknown column `{c}` of `{}`", fk.references));
            }
            let mut wanted = fk.referenced_columns.clone();
            wanted.sort();
            if !parent.key_sets().into_iter().any(|(_, mut k)| {
                k.sort();
                k == wanted
            }) {
                return bad(
                    path,
                    format!(
                        "({}) is not the primary key or a unique set of `{}` (PostgreSQL requires one)",
                        fk.referenced_columns.join(", "),
                        fk.references
                    ),
                );
            }
        }
    }
    Ok(schema)
}

/// The source schema's foreign keys between modeled tables
/// (feature `foreign-keys`). A foreign key whose referenced table or
/// columns are not modeled stays a `not-modeled` fact.
fn link_foreign_keys(
    tables: &mut [RawTable],
    facts: &mut [crate::schema_facts::TableFacts],
    linked: &[(usize, usize, String, crate::schema_facts::SchemaTable)],
    root: &Path,
    provider: &dyn ActorProvider,
) {
    use crate::schema_facts::{FactOrigin, SchemaFact};
    let names: Vec<String> = tables.iter().map(|t| t.name.clone()).collect();
    for (ti, fi, file, x) in linked {
        for fk in &x.foreign_keys {
            let written = format!(
                "foreign key ({}) → {}({})",
                fk.columns.join(", "),
                fk.table,
                fk.foreign_columns.join(", ")
            );
            // The referenced declaration: a binding of the same module, or
            // an import resolved by the frontend.
            let target = linked
                .iter()
                .find(|(_, _, f, t)| f == file && t.binding == fk.table)
                .or_else(|| {
                    let (m, b) = provider.resolve_reference(root, file, &fk.table)?;
                    linked.iter().find(|(_, _, f, t)| *f == m && t.binding == b)
                });
            let columns: Option<Vec<String>> = fk
                .columns
                .iter()
                .map(|c| x.column_by_sql(c).map(|c| c.property.clone()))
                .collect();
            let action = |a: &Option<String>| match a {
                None => Ok(crate::db::RefAction::NoAction),
                Some(text) => crate::db::RefAction::parse(text).ok_or(text.clone()),
            };
            let outcome: Result<crate::db::ForeignKeySchema, String> = (|| {
                let (pi, _, _, _) =
                    target.ok_or("the referenced table is not modeled".to_owned())?;
                let columns = columns.ok_or("a column is not understood".to_owned())?;
                let has = |t: &RawTable, c: &String| {
                    t.columns.iter().any(|rc| match rc {
                        RawColumn::Name(n) => n == c,
                        RawColumn::Full(f) => &f.name == c,
                    })
                };
                if let Some(c) = columns.iter().find(|c| !has(&tables[*ti], c)) {
                    return Err(format!("column `{c}` is not modeled"));
                }
                if let Some(c) = fk.foreign_columns.iter().find(|c| !has(&tables[*pi], c)) {
                    return Err(format!("referenced column `{c}` is not modeled"));
                }
                Ok(crate::db::ForeignKeySchema {
                    columns,
                    references: names[*pi].clone(),
                    referenced_columns: fk.foreign_columns.clone(),
                    on_delete: action(&fk.on_delete)
                        .map_err(|a| format!("ON DELETE `{a}` is not understood"))?,
                    on_update: action(&fk.on_update)
                        .map_err(|a| format!("ON UPDATE `{a}` is not understood"))?,
                    initially_deferred: false,
                })
            })();
            let table_facts = &mut facts[*fi];
            let source = format!("{file}:{}", fk.line);
            table_facts
                .facts
                .retain(|f| !(f.source == source && f.fact.starts_with(&written)));
            match outcome {
                Ok(model_fk) => {
                    table_facts.facts.push(SchemaFact {
                        fact: format!(
                            "{written}{}{}: enforced (the check locks the referenced row FOR KEY SHARE; a violation is SQLSTATE 23503)",
                            if model_fk.on_delete == crate::db::RefAction::NoAction {
                                String::new()
                            } else {
                                format!(" ON DELETE {}", model_fk.on_delete.sql())
                            },
                            if model_fk.on_update == crate::db::RefAction::NoAction {
                                String::new()
                            } else {
                                format!(" ON UPDATE {}", model_fk.on_update.sql())
                            },
                        ),
                        origin: FactOrigin::ExtractedFromSource,
                        source,
                    });
                    tables[*ti].foreign_keys.push(model_fk);
                }
                Err(why) => table_facts.facts.push(SchemaFact {
                    fact: format!("{written}: not modeled ({why})"),
                    origin: FactOrigin::NotModeled,
                    source,
                }),
            }
        }
    }
}

/// Reads routed to the replica need the `replicas` feature and
/// are plain reads (a replica refuses writes and row locks).
fn check_replica_reads(model: &Model) -> Result<(), ScenarioError> {
    for (ai, a) in model.actors.iter().enumerate() {
        for (ci, code) in a.codes.iter().enumerate() {
            for (pc, step) in code.iter().enumerate() {
                let Op::Db { stmt } = &step.op else { continue };
                if stmt.conn != crate::ir::Conn::Replica {
                    continue;
                }
                let path = format!("actors[{ai}].codes[{ci}][{pc}]");
                if !model.features.replicas {
                    return bad(
                        path,
                        "a read routed to a read replica needs the feature `replicas` (otherwise it is not modeled)",
                    );
                }
                let plain = match &stmt.kind {
                    StmtKind::Select { lock, .. } => *lock == crate::ir::LockMode::None,
                    StmtKind::SelectCount { .. } => true,
                    _ => false,
                };
                if !plain {
                    return bad(
                        path,
                        "only plain reads run on a read replica (writes and row locks are refused there)",
                    );
                }
            }
        }
    }
    Ok(())
}

/// An invariant may read what an actor returned only if every
/// return value of its main task is modeled; an unknown one is reported,
/// never guessed.
fn check_return_values(model: &Model) -> Result<(), ScenarioError> {
    for (i, inv) in model.invariants.iter().enumerate() {
        for f in inv.formula.returned_functions() {
            for a in &model.actors {
                if f.as_ref().is_some_and(|f| *f != a.function.name) {
                    continue;
                }
                let Some(main) = a.codes.first() else {
                    continue;
                };
                for step in main {
                    if let Op::ReturnValue { expr } = &step.op
                        && let Some(detail) = expr.unknown_detail()
                    {
                        return bad(
                            format!("invariants[{i}]"),
                            format!(
                                "`{}` reads what `{}` returns, but the return value at {} is not modeled: {detail}",
                                inv.name,
                                a.name,
                                step.src.label()
                            ),
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

/// Refuse scenarios whose statements would run a foreign-key
/// behavior the model does not execute (a referential action other than NO
/// ACTION / RESTRICT, a check deferred to COMMIT).
fn check_foreign_keys(model: &Model) -> Result<(), ScenarioError> {
    let writes = |table: &str| {
        let mut out = (false, false, Vec::<String>::new());
        for a in &model.actors {
            for step in a.codes.iter().flatten() {
                if let Op::Db { stmt } = &step.op {
                    if stmt.kind.table() != table {
                        continue;
                    }
                    match &stmt.kind {
                        StmtKind::Insert { .. } => out.0 = true,
                        StmtKind::Update { set, .. } => {
                            out.0 = true;
                            out.2.extend(set.iter().map(|(c, _)| c.clone()));
                        }
                        StmtKind::Delete { .. } => out.1 = true,
                        _ => {}
                    }
                }
            }
        }
        out
    };
    for t in &model.schema.tables {
        for fk in &t.foreign_keys {
            let name = fk.name(&t.name);
            if fk.initially_deferred && writes(&t.name).0 {
                return bad(
                    "database",
                    format!(
                        "`{name}` is DEFERRABLE INITIALLY DEFERRED (checked at COMMIT): not modeled"
                    ),
                );
            }
            let (_, deletes, updated) = writes(&fk.references);
            if deletes && !fk.on_delete.supported() {
                return bad(
                    "database",
                    format!(
                        "`{name}` is ON DELETE {} and `{}` rows are deleted: only NO ACTION and RESTRICT are modeled",
                        fk.on_delete.sql(),
                        fk.references
                    ),
                );
            }
            if !fk.on_update.supported()
                && updated.iter().any(|c| fk.referenced_columns.contains(c))
            {
                return bad(
                    "database",
                    format!(
                        "`{name}` is ON UPDATE {} and the referenced key of `{}` is updated: only NO ACTION and RESTRICT are modeled",
                        fk.on_update.sql(),
                        fk.references
                    ),
                );
            }
        }
    }
    Ok(())
}

fn contains_generated(v: &Value) -> bool {
    match v {
        Value::Gen(_) => true,
        Value::Row(r) => r.values().any(contains_generated),
        _ => false,
    }
}

fn build_initial(
    schema: &Schema,
    raw: &BTreeMap<String, Vec<Row>>,
) -> Result<Vec<Vec<Row>>, ScenarioError> {
    for name in raw.keys() {
        if schema.by_name(name).is_none() {
            return bad(
                format!("database.initialState.{name}"),
                format!("unknown table `{name}`: declare it in database.tables"),
            );
        }
    }
    let mut out = Vec::new();
    for (ti, table) in schema.tables.iter().enumerate() {
        let mut rows = Vec::new();
        for (ri, row) in raw
            .get(&table.name)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .enumerate()
        {
            let path = format!("database.initialState.{}[{ri}]", table.name);
            for (k, v) in &row {
                if !table.has_column(k) {
                    return bad(&path, format!("unknown column `{k}`"));
                }
                // A generated value is produced by an insert of
                // the explored execution; it is not data of the initial
                // state (which holds concrete values only).
                if contains_generated(v) {
                    return bad(
                        &path,
                        format!(
                            "column `{k}`: a generated value cannot be part of the initial state"
                        ),
                    );
                }
            }
            let mut full = row;
            for c in &table.columns {
                full.entry(c.name.clone()).or_insert(Value::Null);
            }
            crate::db::check_row(table, &full).map_err(|e| ScenarioError {
                path: path.clone(),
                message: e.message,
            })?;
            // Time and JSON columns hold the values the model
            // represents.
            if let Some(why) = crate::db::typed_row_problem(table, &full) {
                return bad(&path, why);
            }
            for rule in table.key_rules() {
                let key = crate::db::key_tuple(&full, &rule.columns);
                // NULLS NOT DISTINCT rules check NULL keys too.
                if !rule.checks(&key) {
                    continue;
                }
                if rows
                    .iter()
                    .any(|r: &Row| crate::db::key_tuple(r, &rule.columns) == key)
                {
                    return bad(&path, format!("duplicate key violates `{}`", rule.name));
                }
            }
            rows.push(full);
        }
        let _ = ti;
        out.push(rows);
    }
    Ok(out)
}

type ActorBuild = (
    Vec<String>,
    Vec<Vec<Step>>,
    Vec<(String, Value)>,
    Vec<String>,
    Option<crate::provider::EntryInfo>,
);

/// What the extraction of an actor must produce beyond Phase 12.
#[derive(Clone, Copy)]
struct Wants {
    replicas: bool,
    js_values: bool,
    return_values: bool,
}

/// Insert [`Op::Respond`] before every return of the main task
/// (code 0), keeping jump targets: a jump to a return now reaches its
/// response first.
fn insert_responses(codes: &mut [Vec<Step>]) {
    let Some(main) = codes.first_mut() else {
        return;
    };
    let returns: Vec<usize> = main
        .iter()
        .enumerate()
        .filter(|(_, s)| matches!(s.op, Op::Return))
        .map(|(i, _)| i)
        .collect();
    // New index of an old position `t` that is a jump target: shifted by
    // the responses inserted strictly before it (a response inserted AT `t`
    // takes `t`'s place, so a jump to the return reaches it first).
    let moved = |t: usize| t + returns.iter().filter(|r| **r < t).count();
    for step in main.iter_mut() {
        match &mut step.op {
            Op::Jump { target } | Op::JumpIf { target, .. } => *target = moved(*target),
            Op::Try { handler, .. } => *handler = moved(*handler),
            _ => {}
        }
    }
    for (k, r) in returns.iter().enumerate() {
        let at = r + k;
        let src = main[at].src.clone();
        main.insert(
            at,
            Step {
                op: Op::Respond,
                src,
            },
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn actor_program(
    a: &RawActor,
    path: &str,
    extraction: &RawExtraction,
    schema: &Schema,
    facts: &[crate::schema_facts::TableFacts],
    base_dir: &Path,
    provider: &dyn ActorProvider,
    wants: Wants,
) -> Result<ActorBuild, ScenarioError> {
    if a.program.is_some() && a.codes.is_some() {
        return bad(path, "give either `program` or `codes`, not both");
    }
    if let Some(text) = &a.program {
        let codes = dsl::parse(text).map_err(|e| ScenarioError {
            path: format!("{path}.program"),
            message: e.to_string(),
        })?;
        let RawArguments::Named(map) = &a.arguments else {
            if matches!(a.arguments, RawArguments::None) {
                return Ok((Vec::new(), codes, Vec::new(), Vec::new(), None));
            }
            return bad(
                format!("{path}.arguments"),
                "a declared program needs named arguments (an object)",
            );
        };
        return Ok((
            map.keys().cloned().collect(),
            codes,
            map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            Vec::new(),
            None,
        ));
    }
    if let Some(codes) = &a.codes {
        let RawArguments::Named(map) = &a.arguments else {
            return Ok((Vec::new(), codes.clone(), Vec::new(), Vec::new(), None));
        };
        return Ok((
            map.keys().cloned().collect(),
            codes.clone(),
            map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            Vec::new(),
            None,
        ));
    }
    // Extraction from TypeScript.
    let selector_file = a
        .entry
        .as_ref()
        .and_then(|e| e.selector.split(['#', '@']).next().map(str::to_owned));
    let Some(file) = a.file.clone().or(selector_file) else {
        return bad(
            path,
            "an actor needs `program`, `codes`, `entry`, or `file` + `function` to extract from TypeScript",
        );
    };
    let file = &file;
    let root = base_dir.join(extraction.root.as_deref().unwrap_or("."));
    let args = match &a.arguments {
        RawArguments::Positional(v) => v.clone(),
        RawArguments::None => Vec::new(),
        RawArguments::Named(_) => {
            return bad(
                format!("{path}.arguments"),
                "an extracted function takes positional arguments (an array)",
            );
        }
    };
    let tables: Vec<TableBinding> = schema
        .tables
        .iter()
        .map(|t| TableBinding {
            identifier: extraction
                .tables
                .iter()
                .find(|(_, v)| **v == t.name)
                .map(|(k, _)| k.clone())
                .unwrap_or_else(|| t.name.clone()),
            table: t.name.clone(),
            columns: t.columns.iter().map(|c| c.name.clone()).collect(),
            // A table read from a schema module of a list is
            // bound by declaration identity. With a single module (Phase
            // 8–11 scenarios) binding stays by name, unchanged.
            source: facts
                .iter()
                .find(|f| f.table == t.name)
                .filter(|_| matches!(extraction.schema, Some(SchemaModules::Many(_))))
                .and_then(|f| Some((f.module.clone()?, f.binding.clone()?))),
        })
        .collect();
    let externals = extraction
        .externals
        .iter()
        .map(|e| ExternalBinding {
            callee: e.callee.clone(),
            channel: e.channel.clone(),
            key_argument: e.key_argument,
        })
        .collect();
    let transaction_retries = extraction
        .transaction_retries
        .iter()
        .map(|r| TransactionRetryBinding {
            callee: r.callee.clone(),
            callback_argument: r.callback_argument,
            options_argument: r.options_argument,
            attempts: r.attempts.max(1),
        })
        .collect();
    let request = ExtractRequest {
        root,
        file: file.clone(),
        function: a.function.clone(),
        arguments: args.clone(),
        tables,
        externals,
        transaction_retries,
        max_inline_depth: extraction.max_inline_depth.unwrap_or(3),
        selector: a.entry.as_ref().map(|e| e.selector.clone()),
        contract: a.entry.as_ref().and_then(|e| {
            (e.invocation.is_some() || !e.preconditions.is_empty() || e.trpc.is_some()).then(|| {
                crate::provider::InvocationContract {
                    invocation: e.invocation.clone(),
                    preconditions: e.preconditions.clone(),
                    trpc: e.trpc.clone(),
                }
            })
        }),
        replicas: wants.replicas,
        replica_factories: extraction.replica_factories.clone(),
        return_values: wants.return_values,
        js_values: wants.js_values,
    };
    let extracted = provider.extract(&request).map_err(|e| ScenarioError {
        path: path.to_owned(),
        message: format!("cannot extract `{}` from `{file}`: {e}", a.function),
    })?;
    if extracted.params.len() < args.len() {
        return bad(
            format!("{path}.arguments"),
            format!(
                "`{}` takes {} parameter(s) but {} argument(s) were given",
                a.function,
                extracted.params.len(),
                args.len()
            ),
        );
    }
    if extracted.params.len() > args.len() {
        return bad(
            format!("{path}.arguments"),
            format!(
                "`{}` takes {} parameter(s) ({}) but {} argument(s) were given: do not leave inputs unspecified",
                a.function,
                extracted.params.len(),
                extracted.params.join(", "),
                args.len()
            ),
        );
    }
    let bound = extracted.params.iter().cloned().zip(args).collect();
    Ok((
        extracted.params,
        extracted.codes,
        bound,
        extracted.assumptions,
        extracted.entry,
    ))
}

/// Make every operation whose expression is unknown an [`Op::Unknown`]
///. Evaluating an unknown expression used to fail the actor, which
/// silently turned unsupported source into a modeled exception; reaching it
/// now makes the result `UNSUPPORTED`. Indices are kept, so jumps stay valid.
pub fn seal_unknowns(codes: &mut [Vec<Step>]) {
    for code in codes {
        for step in code {
            let mut exprs: Vec<&crate::value::Expr> = Vec::new();
            match &step.op {
                Op::Let { expr, .. } | Op::AppWrite { expr, .. } => exprs.push(expr),
                Op::JumpIf { cond, .. } => exprs.push(cond),
                Op::External { payload, key, .. } => {
                    exprs.extend(payload.iter().map(|(_, e)| e));
                    exprs.extend(key.iter());
                }
                Op::Db { stmt } => match &stmt.kind {
                    StmtKind::Select { pred, .. }
                    | StmtKind::SelectCount { pred, .. }
                    | StmtKind::Delete { pred, .. } => exprs.push(pred),
                    StmtKind::Update { pred, set, .. } => {
                        exprs.push(pred);
                        exprs.extend(set.iter().map(|(_, e)| e));
                    }
                    StmtKind::Insert {
                        values, more_rows, ..
                    } => {
                        exprs.extend(values.iter().map(|(_, e)| e));
                        exprs.extend(more_rows.iter().flatten().map(|(_, e)| e));
                    }
                },
                _ => {}
            }
            if let Some(detail) = exprs.iter().find_map(|e| e.unknown_detail()) {
                // What the operation would have assigned.
                let defines: Vec<String> = match &step.op {
                    Op::Let { local, .. } => vec![local.clone()],
                    Op::Db { stmt } => match &stmt.kind {
                        StmtKind::Select { into, .. } | StmtKind::SelectCount { into, .. } => {
                            vec![into.clone()]
                        }
                        StmtKind::Update { affected, .. } | StmtKind::Delete { affected, .. } => {
                            affected.iter().cloned().collect()
                        }
                        StmtKind::Insert {
                            affected,
                            returning,
                            ..
                        } => affected.iter().chain(returning.iter()).cloned().collect(),
                    },
                    _ => Vec::new(),
                };
                step.op = Op::Unknown {
                    detail: detail.to_owned(),
                    defines,
                };
            }
        }
    }
}

/// Static validation of the assembled model.
pub fn validate(model: &Model) -> Result<(), ScenarioError> {
    for (ai, actor) in model.actors.iter().enumerate() {
        let path = format!("actors[{ai}] ({})", actor.name);
        if actor.codes.is_empty() || actor.codes[0].is_empty() {
            return bad(&path, "the actor has no operations");
        }
        let mut defined: BTreeSet<String> =
            actor.arguments.iter().map(|(k, _)| k.clone()).collect();
        for code in &actor.codes {
            for step in code {
                match &step.op {
                    Op::Let { local, .. } | Op::Generate { local, .. } => {
                        defined.insert(local.clone());
                    }
                    Op::AppRead { into, .. } => {
                        defined.insert(into.clone());
                    }
                    // Assigned by an unsupported operation (no
                    // value: the path ends there as unsupported).
                    Op::Unknown { defines, .. } => {
                        defined.extend(defines.iter().cloned());
                    }
                    Op::Try {
                        error: Some(local), ..
                    } => {
                        defined.insert(local.clone());
                    }
                    Op::Db { stmt } => match &stmt.kind {
                        StmtKind::Select { into, .. } | StmtKind::SelectCount { into, .. } => {
                            defined.insert(into.clone());
                        }
                        StmtKind::Insert {
                            affected,
                            returning: Some(row),
                            ..
                        } => {
                            defined.insert(row.clone());
                            defined.extend(affected.clone());
                        }
                        StmtKind::Update {
                            affected: Some(n), ..
                        }
                        | StmtKind::Insert {
                            affected: Some(n), ..
                        }
                        | StmtKind::Delete {
                            affected: Some(n), ..
                        } => {
                            defined.insert(n.clone());
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
        let n_codes = actor.codes.len();
        for (ci, code) in actor.codes.iter().enumerate() {
            for (si, step) in code.iter().enumerate() {
                let at = format!("{path}, code {ci}, step {si} ({})", step.src.label());
                check_step(model, step, &defined, code.len(), n_codes, &at)?;
            }
            if !matches!(
                code.last().map(|s| &s.op),
                Some(Op::Return | Op::Throw { .. } | Op::Jump { .. })
            ) {
                return bad(&path, format!("code {ci} does not end with return/throw"));
            }
        }
        for c in &model.channels {
            let _ = c;
        }
        for code in &actor.codes {
            for step in code {
                if let Op::External { channel, .. } = &step.op
                    && !model.channels.contains_key(channel)
                {
                    return bad(
                        &path,
                        format!(
                            "external channel `{channel}` is not declared in `channels` (declare how it treats idempotency keys)"
                        ),
                    );
                }
            }
        }
    }
    for inv in &model.invariants {
        inv.validate(model).map_err(|e| ScenarioError {
            path: format!("invariants[{}]", inv.name),
            message: e.message,
        })?;
    }
    if model.invariants.is_empty() {
        return bad(
            "invariants",
            "declare at least one invariant: nothing is inferred",
        );
    }
    let mut names = BTreeSet::new();
    for inv in &model.invariants {
        if !names.insert(&inv.name) {
            return bad(
                "invariants",
                format!("duplicate invariant name `{}`", inv.name),
            );
        }
    }
    Ok(())
}

fn check_expr(
    model: &Model,
    e: &Expr,
    table: Option<&TableSchema>,
    defined: &BTreeSet<String>,
    at: &str,
) -> Result<(), ScenarioError> {
    let mut locals = Vec::new();
    e.locals(&mut locals);
    if let Some(l) = locals.iter().find(|l| !defined.contains(*l)) {
        return bad(
            at,
            format!("`{l}` is never assigned (arguments and `let` define locals)"),
        );
    }
    let mut cols = Vec::new();
    e.columns(&mut cols);
    match table {
        Some(t) => {
            if let Some(c) = cols.iter().find(|c| !t.has_column(c)) {
                return bad(at, format!("unknown column `{}.{c}`", t.name));
            }
        }
        None => {
            if let Some(c) = cols.first() {
                return bad(at, format!("column reference `@{c}` outside a statement"));
            }
        }
    }
    let _ = model;
    Ok(())
}

fn check_step(
    model: &Model,
    step: &Step,
    defined: &BTreeSet<String>,
    code_len: usize,
    n_codes: usize,
    at: &str,
) -> Result<(), ScenarioError> {
    let table_of = |name: &str| {
        model.schema.by_name(name).ok_or_else(|| ScenarioError {
            path: at.to_owned(),
            message: format!("unknown table `{name}`"),
        })
    };
    match &step.op {
        Op::Let { expr, .. } => check_expr(model, expr, None, defined, at),
        Op::JumpIf { cond, target } => {
            if *target >= code_len {
                return bad(at, "jump target out of range");
            }
            check_expr(model, cond, None, defined, at)
        }
        Op::Jump { target }
        | Op::Try {
            handler: target, ..
        } => {
            if *target >= code_len {
                return bad(at, "jump target out of range");
            }
            Ok(())
        }
        Op::Spawn { children, .. } => {
            if children.iter().any(|c| *c == 0 || *c >= n_codes) {
                return bad(at, "spawn refers to an unknown task");
            }
            Ok(())
        }
        Op::External { payload, key, .. } => {
            for (_, e) in payload {
                check_expr(model, e, None, defined, at)?;
            }
            if let Some(k) = key {
                check_expr(model, k, None, defined, at)?;
            }
            Ok(())
        }
        Op::AppWrite { expr, .. } => check_expr(model, expr, None, defined, at),
        // An unknown return value is allowed here (it is refused
        // only when an invariant reads it).
        Op::ReturnValue { expr } if expr.unknown_detail().is_none() => {
            check_expr(model, expr, None, defined, at)
        }
        Op::Db { stmt } => match &stmt.kind {
            // The multi-row mode reads one table (joins are not
            // modeled with it).
            StmtKind::Select {
                joins,
                many: Some(_),
                ..
            } if !joins.is_empty() => bad(at, "a multi-row select with joins is not modeled"),
            // `ORDER BY` only by the single-column primary key (the
            // model's order).
            StmtKind::Select {
                table,
                many:
                    Some(crate::ir::ManyRows {
                        order_by: Some(column),
                        ..
                    }),
                ..
            } if model
                .schema
                .by_name(table)
                .is_none_or(|t| t.primary_key.as_slice() != std::slice::from_ref(column)) =>
            {
                bad(
                    at,
                    format!(
                        "`ORDER BY {column}` on a multi-row select is modeled only when `{column}` is the table's single-column primary key"
                    ),
                )
            }
            StmtKind::Select {
                table,
                pred,
                joins,
                projection,
                ..
            } if !joins.is_empty() || projection.is_some() => {
                // Joined selects use qualified columns (`table.column`).
                let mut tables = vec![table_of(table)?];
                for j in joins {
                    tables.push(table_of(&j.table)?);
                }
                let known = |q: &str| -> bool {
                    match q.split_once('.') {
                        Some((t, c)) => tables.iter().any(|x| x.name == t && x.has_column(c)),
                        None => joins.is_empty() && tables[0].has_column(q),
                    }
                };
                for j in joins {
                    for c in [&j.left, &j.right] {
                        if !known(c) || !c.contains('.') {
                            return bad(
                                at,
                                format!(
                                    "join column `{c}` is not a qualified column of a joined table"
                                ),
                            );
                        }
                    }
                }
                for (field, item) in projection.iter().flatten() {
                    let ok = match item {
                        crate::ir::ProjItem::Column(c) => known(c),
                        crate::ir::ProjItem::Table(t) => tables.iter().any(|x| &x.name == t),
                    };
                    if !ok {
                        return bad(
                            at,
                            format!(
                                "projected field `{field}` refers to an unknown column or table"
                            ),
                        );
                    }
                }
                let mut locals = Vec::new();
                pred.locals(&mut locals);
                if let Some(l) = locals.iter().find(|l| !defined.contains(*l)) {
                    return bad(at, format!("`{l}` is never assigned"));
                }
                let mut cols = Vec::new();
                pred.columns(&mut cols);
                if let Some(c) = cols.iter().find(|c| !known(c)) {
                    return bad(at, format!("unknown column `{c}`"));
                }
                Ok(())
            }
            StmtKind::Select { table, pred, .. } | StmtKind::SelectCount { table, pred, .. } => {
                check_expr(model, pred, Some(table_of(table)?), defined, at)
            }
            StmtKind::Delete { table, pred, .. } => {
                check_expr(model, pred, Some(table_of(table)?), defined, at)
            }
            StmtKind::Update {
                table, pred, set, ..
            } => {
                let t = table_of(table)?;
                check_expr(model, pred, Some(t), defined, at)?;
                for (c, e) in set {
                    if !t.has_column(c) {
                        return bad(at, format!("unknown column `{table}.{c}`"));
                    }
                    check_expr(model, e, Some(t), defined, at)?;
                }
                Ok(())
            }
            StmtKind::Insert {
                table,
                values,
                more_rows,
                ..
            } => {
                let t = table_of(table)?;
                // A multi-row insert, bounded.
                if more_rows.len() + 1 > crate::ir::MAX_INSERT_ROWS {
                    return bad(
                        at,
                        format!(
                            "a multi-row insert of {} rows exceeds the bound of {}",
                            more_rows.len() + 1,
                            crate::ir::MAX_INSERT_ROWS
                        ),
                    );
                }
                for row in std::iter::once(values).chain(more_rows.iter()) {
                    for (c, e) in row {
                        if !t.has_column(c) {
                            return bad(at, format!("unknown column `{table}.{c}`"));
                        }
                        check_expr(model, e, None, defined, at)?;
                    }
                }
                Ok(())
            }
        },
        _ => Ok(()),
    }
}

/// The assumptions printed with every result.
fn standard_assumptions(model: &Model, report: &LoadReport) -> Vec<String> {
    let mut v = vec![
        format!(
            "database default isolation: {} (PostgreSQL semantics, see docs/architecture/postgres-model.md)",
            model.isolation.name()
        ),
        "a database statement is atomic unless it must wait for another transaction; waiting statements resume with their original snapshot".into(),
        "autocommit (root `db`) statements are single-statement transactions at the default isolation level".into(),
        "local computation is not a scheduling point; it is folded into the preceding transition".into(),
        "rows are read in primary-key order for LIMIT 1 (the declared programs and generated SQL both use ORDER BY the primary key)".into(),
        "tasks of different actors never share a transaction, connection or event loop; actors listed with the same process share in-process variables and crash together".into(),
        "a process crash rolls back open transactions and loses local state; committed data and performed external effects remain".into(),
    ];
    if model.uses_serializable() {
        v.push("SERIALIZABLE is modeled by a serialization-graph check at commit (conservative over-approximation of the histories PostgreSQL allows; PostgreSQL's SSI may abort more); see docs/architecture/postgres-model.md".into());
    }
    if model.bounds.max_context_switches.is_some() {
        v.push("partial-order reduction is disabled because maxContextSwitches is set".into());
    }
    let nnd: Vec<String> = model
        .schema
        .tables
        .iter()
        .flat_map(|t| {
            t.nulls_not_distinct
                .iter()
                .map(move |u| format!("{}({})", t.name, u.join(", ")))
        })
        .collect();
    if !nnd.is_empty() {
        v.push(format!(
            "UNIQUE NULLS NOT DISTINCT (PostgreSQL 15+): for {} a NULL key value equals another NULL, so an insert or update whose key matches an existing or uncommitted row's, NULLs included, fails with 23505, waits for the uncommitted row's transaction, or is an ON CONFLICT match; non-NULL values compare by model value equality as for every unique key",
            nnd.join(", ")
        ));
    }
    let generators = model.generators();
    if !generators.is_empty() {
        let names: Vec<String> = generators
            .iter()
            .map(|(t, c, g)| {
                format!(
                    "{}.{c} ({}, {})",
                    model.schema.table(*t).name,
                    g.generator,
                    match g.side {
                        crate::db::GeneratedSide::Database => "database",
                        crate::db::GeneratedSide::Application => "application",
                    }
                )
            })
            .collect();
        v.push(format!(
            "generated-values-distinct: every execution of an insert that omits a generated column produces a new value, distinct from every other value (random identifiers are assumed not to collide): {}",
            names.join(", ")
        ));
        v.push("generated values have no known order: rows with generated keys are ordered by generation event where a LIMIT 1 read needs an order (PostgreSQL orders them by their random value)".into());
    }
    // Clock readings.
    if model
        .schema
        .tables
        .iter()
        .flat_map(|t| &t.columns)
        .any(|c| crate::db::is_time_type(c.sql_type.as_deref()))
    {
        v.push("clock-readings-opaque: `now()` is the transaction's start time (the same value for every statement of one transaction, PostgreSQL's transaction_timestamp()); an application clock reading (`new Date()`, `$defaultFn`/`$onUpdate(() => new Date())`) is one value per evaluation; readings are opaque: they have no known order (an order comparison is unsupported) and two different readings are assumed to differ from each other and from every explicit value (clock-readings-distinct)".into());
        v.push("explicit time values are canonical text (`YYYY-MM-DDTHH:MM:SS.sssZ` for timestamp/timestamptz, what a Drizzle `Date` serializes to; `YYYY-MM-DD` for date) and compare as the instants they denote; any other value written to a time column is unsupported".into());
    }
    for a in &model.actors {
        if a.codes
            .iter()
            .flatten()
            .any(|s| s.src.origin == Origin::Declared)
        {
            v.push(format!(
                "actor {}: program declared in the scenario (fixture), not extracted from source",
                a.name
            ));
        }
    }
    for (actor, notes) in &report.assumptions {
        for n in notes {
            v.push(format!("actor {actor}: {n}"));
        }
    }
    v
}

pub mod testing {
    //! Helpers for tests: build a model for the `products`/`orders`
    //! workload from declared programs.
    use super::*;
    use crate::provider::NoProvider;

    /// (actor name, declared program, named arguments)
    pub type Declared<'a> = (&'a str, &'a str, &'a [(&'a str, Value)]);

    pub fn model_from_programs(iso: &str, actors: &[Declared<'_>]) -> Model {
        model_with(iso, actors, serde_json::json!({}), serde_json::json!({}))
    }

    /// Build a model from a scenario document (panics when it is invalid).
    pub fn model_from_json(doc: serde_json::Value) -> Model {
        match load_str(&doc.to_string(), Path::new("."), &NoProvider) {
            Ok((m, _)) => m,
            Err(e) => panic!("test scenario is invalid: {e}"),
        }
    }

    pub fn model_with(
        iso: &str,
        actors: &[Declared<'_>],
        bounds: serde_json::Value,
        extra: serde_json::Value,
    ) -> Model {
        let actors_json: Vec<serde_json::Value> = actors
            .iter()
            .map(|(name, program, args)| {
                let named: serde_json::Map<String, serde_json::Value> = args
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), serde_json::to_value(v).unwrap()))
                    .collect();
                serde_json::json!({
                    "name": name, "function": "purchase", "program": program, "arguments": named
                })
            })
            .collect();
        let mut doc = serde_json::json!({
            "schemaVersion": 1,
            "name": "test",
            "database": {
                "isolation": iso,
                "tables": [
                    { "name": "products", "columns": [{"name":"id","notNull":true}, "stock"], "primaryKey": ["id"] },
                    { "name": "orders", "columns": [{"name":"id","notNull":true}, "product"], "primaryKey": ["id"] }
                ],
                "initialState": { "products": [{"id": "p1", "stock": 1}], "orders": [] }
            },
            "actors": actors_json,
            "bounds": bounds,
            "invariants": [{
                "name": "no-oversell",
                "assert": { "le": [
                    { "actors": { "function": "purchase", "outcome": "ok" } },
                    { "initial": { "table": "products", "where": {"id": "p1"}, "column": "stock" } }
                ] }
            }]
        });
        if let (Some(d), Some(e)) = (doc.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                d.insert(k.clone(), v.clone());
            }
        }
        match load_str(&doc.to_string(), Path::new("."), &NoProvider) {
            Ok((m, _)) => m,
            Err(e) => panic!("test scenario is invalid: {e}"),
        }
    }
}

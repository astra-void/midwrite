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
    actors: Vec<RawActor>,
    #[serde(default)]
    bounds: Bounds,
    #[serde(default)]
    options: RawOptions,
    #[serde(default)]
    invariants: Vec<Invariant>,
    #[serde(default)]
    extraction: RawExtraction,
    /// Phase 7: how to run each actor's original code
    /// (`commitrift model-check --verify-source`).
    #[serde(default)]
    source_verification: Option<crate::source_types::SourceSpec>,
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
    /// CHECK constraints as expressions over `@column`.
    #[serde(default)]
    checks: Vec<String>,
    /// Phase 8: the Drizzle table binding (or SQL name) in
    /// `extraction.schema` this table is read from.
    #[serde(default)]
    from: Option<String>,
    /// Phase 8: declared facts that intentionally differ from the source.
    #[serde(default)]
    overrides: Option<RawOverrides>,
}

/// Facts a scenario states against its source, with the reason (Phase 8).
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
    /// Phase 8: the Drizzle schema module (relative to `root`) that tables
    /// with `from` are read from.
    schema: Option<String>,
    /// Phase 8: helpers that re-run a transaction after 40001/40P01.
    #[serde(default)]
    transaction_retries: Vec<RawTransactionRetry>,
    max_inline_depth: Option<usize>,
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
    /// Phase 7: select the entry function by a source selector (anonymous
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
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawEntry {
    selector: String,
    #[serde(default)]
    invocation: Option<String>,
    #[serde(default)]
    preconditions: Vec<String>,
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
    /// Selected entry functions per actor (Phase 7).
    pub entries: Vec<(String, crate::provider::EntryInfo)>,
    /// The scenario's `sourceVerification` section (Phase 7).
    pub source: Option<crate::source_types::SourceSpec>,
    /// Phase 8: the facts behind each table read from the source schema.
    pub schema: Vec<crate::schema_facts::TableFacts>,
    /// Phase 8: the extracted schema (for the catalog comparison).
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
    let (tables, schema_facts, extracted_schema) =
        link_source_schema(&raw.database.tables, &raw.extraction, base_dir, provider)?;
    let schema = build_schema(&tables)?;
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
        let (params, mut codes, arguments, assumptions, entry) =
            actor_program(a, &path, extraction, &schema, base_dir, provider)?;
        seal_unknowns(&mut codes);
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
    if let Some(m) = raw.options.minimize {
        options.minimize = m;
    }
    let mut model = Model {
        name: raw.name,
        description: raw.description,
        schema,
        initial_rows,
        isolation,
        actors,
        process_of,
        process_names,
        channels: raw.channels,
        app_initial: raw.app,
        invariants: raw.invariants,
        bounds: raw.bounds,
        options,
        assumptions: Vec::new(),
    };
    validate(&model)?;
    model.assumptions = standard_assumptions(&model, &report);
    Ok((model, report))
}

// ---------------------------------------------------------------------------
// Phase 8: tables read from the application's Drizzle schema
// ---------------------------------------------------------------------------

/// The model type of an extracted column, if the model represents it.
fn model_type(c: &crate::schema_facts::SchemaColumn) -> Option<&'static str> {
    match c.builder.as_str() {
        "serial" | "bigserial" | "smallserial" | "integer" | "smallint" | "bigint" => Some("int"),
        "text" | "varchar" | "char" | "uuid" => Some("text"),
        "boolean" => Some("bool"),
        _ if c.sql_type.starts_with("enum ") => Some("text"),
        _ => None,
    }
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
) -> Result<Linked, ScenarioError> {
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
    let Some(file) = &extraction.schema else {
        return bad(
            "extraction.schema",
            "tables with `from` need `extraction.schema` (the Drizzle schema module)",
        );
    };
    let root = base_dir.join(extraction.root.as_deref().unwrap_or("."));
    let extracted = provider
        .extract_schema(&root, file)
        .map_err(|e| ScenarioError {
            path: "extraction.schema".into(),
            message: e,
        })?;
    let mut tables = Vec::new();
    let mut facts = Vec::new();
    for (i, t) in raw.iter().enumerate() {
        match &t.from {
            None => tables.push(t.clone()),
            Some(binding) => {
                let Some(x) = extracted.table(binding) else {
                    return bad(
                        format!("database.tables[{i}].from"),
                        format!("`{file}` declares no table `{binding}` the extractor understood"),
                    );
                };
                let (table, table_facts) = link_table(t, i, x, file)?;
                tables.push(table);
                facts.push(table_facts);
            }
        }
    }
    Ok((tables, facts, Some(extracted)))
}

#[allow(clippy::too_many_lines)]
fn link_table(
    t: &RawTable,
    i: usize,
    x: &crate::schema_facts::SchemaTable,
    file: &str,
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
        let Some(ty) = model_type(c) else {
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
        let default = match &c.default {
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
                    None => String::new(),
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
        } else if u.nulls_not_distinct && nullable {
            fact(
                &mut facts,
                format!(
                    "unique {} `{name}` NULLS NOT DISTINCT on nullable ({}): not modeled (the model treats NULLs as distinct)",
                    u.kind,
                    u.columns.join(", ")
                ),
                FactOrigin::NotModeled,
                at(u.line),
            );
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
                    "unique {} `{name}` on ({}): a column is not modeled",
                    u.kind,
                    u.columns.join(", ")
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
            checks: t.checks.clone(),
            from: None,
            overrides: None,
        },
        TableFacts {
            table: x.name.clone(),
            binding: Some(x.binding.clone()),
            facts,
        },
    ))
}

fn build_schema(raw: &[RawTable]) -> Result<Schema, ScenarioError> {
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
                },
                RawColumn::Full(c) => c.clone(),
            })
            .collect();
        let mut seen = BTreeSet::new();
        for c in &columns {
            if let Some(ty) = &c.sql_type
                && !matches!(ty.as_str(), "int" | "text" | "bool")
            {
                return bad(
                    format!("{path}.columns"),
                    format!(
                        "column `{}`: unsupported type `{ty}` (int, text or bool)",
                        c.name
                    ),
                );
            }
            if !seen.insert(c.name.clone()) {
                return bad(
                    format!("{path}.columns"),
                    format!("duplicate column `{}`", c.name),
                );
            }
        }
        let has = |c: &String| columns.iter().any(|x| &x.name == c);
        for c in t.primary_key.iter().chain(t.unique.iter().flatten()) {
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
            checks,
        });
    }
    Ok(Schema { tables })
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
            for k in row.keys() {
                if !table.has_column(k) {
                    return bad(&path, format!("unknown column `{k}`"));
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
            for (cname, cols) in table.key_sets() {
                let key = crate::db::key_tuple(&full, &cols);
                if key.iter().any(Value::is_null) {
                    continue;
                }
                if rows
                    .iter()
                    .any(|r: &Row| crate::db::key_tuple(r, &cols) == key)
                {
                    return bad(&path, format!("duplicate key violates `{cname}`"));
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

fn actor_program(
    a: &RawActor,
    path: &str,
    extraction: &RawExtraction,
    schema: &Schema,
    base_dir: &Path,
    provider: &dyn ActorProvider,
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
            (e.invocation.is_some() || !e.preconditions.is_empty()).then(|| {
                crate::provider::InvocationContract {
                    invocation: e.invocation.clone(),
                    preconditions: e.preconditions.clone(),
                }
            })
        }),
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
/// (Phase 7). Evaluating an unknown expression used to fail the actor, which
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
                    StmtKind::Insert { values, .. } => exprs.extend(values.iter().map(|(_, e)| e)),
                },
                _ => {}
            }
            if let Some(detail) = exprs.iter().find_map(|e| e.unknown_detail()) {
                step.op = Op::Unknown {
                    detail: detail.to_owned(),
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
                    Op::Let { local, .. } => {
                        defined.insert(local.clone());
                    }
                    Op::AppRead { into, .. } => {
                        defined.insert(into.clone());
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
        Op::Db { stmt } => match &stmt.kind {
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
            StmtKind::Insert { table, values, .. } => {
                let t = table_of(table)?;
                for (c, e) in values {
                    if !t.has_column(c) {
                        return bad(at, format!("unknown column `{table}.{c}`"));
                    }
                    check_expr(model, e, None, defined, at)?;
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
            "database default isolation: {} (PostgreSQL semantics, see docs/phase6/POSTGRES-ISOLATION.md)",
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
        v.push("SERIALIZABLE is modeled by a serialization-graph check at commit (conservative over-approximation of the histories PostgreSQL allows; PostgreSQL's SSI may abort more); see docs/phase6/POSTGRES-ISOLATION.md".into());
    }
    if model.bounds.max_context_switches.is_some() {
        v.push("partial-order reduction is disabled because maxContextSwitches is set".into());
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

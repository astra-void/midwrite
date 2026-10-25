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
use crate::provider::{ActorProvider, ExternalBinding, ExtractRequest, TableBinding};
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawTable {
    name: String,
    columns: Vec<RawColumn>,
    #[serde(default)]
    primary_key: Vec<String>,
    #[serde(default)]
    unique: Vec<Vec<String>>,
    /// CHECK constraints as expressions over `@column`.
    #[serde(default)]
    checks: Vec<String>,
}

#[derive(Deserialize)]
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

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawExtraction {
    /// Project root, relative to the scenario file.
    root: Option<String>,
    /// Drizzle table identifier → table name (default: same name).
    #[serde(default)]
    tables: BTreeMap<String, String>,
    #[serde(default)]
    externals: Vec<RawExternal>,
    max_inline_depth: Option<usize>,
}

#[derive(Deserialize)]
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
    function: String,
    #[serde(default)]
    file: Option<String>,
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
    let schema = build_schema(&raw.database.tables)?;
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
    let mut report = LoadReport::default();
    let mut actors = Vec::new();
    let mut process_names: Vec<String> = Vec::new();
    let mut process_of = Vec::new();
    let mut seen = BTreeSet::new();
    let extraction = &raw.extraction;
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
        let (params, codes, arguments, assumptions) =
            actor_program(a, &path, extraction, &schema, base_dir, provider)?;
        if !assumptions.is_empty() {
            report.assumptions.push((name.clone(), assumptions));
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
                name: a.function.clone(),
                file: a.file.clone(),
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

fn build_schema(raw: &[RawTable]) -> Result<Schema, ScenarioError> {
    let mut tables = Vec::new();
    let mut names = BTreeSet::new();
    for (i, t) in raw.iter().enumerate() {
        let path = format!("database.tables[{i}]");
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
                return Ok((Vec::new(), codes, Vec::new(), Vec::new()));
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
        ));
    }
    if let Some(codes) = &a.codes {
        let RawArguments::Named(map) = &a.arguments else {
            return Ok((Vec::new(), codes.clone(), Vec::new(), Vec::new()));
        };
        return Ok((
            map.keys().cloned().collect(),
            codes.clone(),
            map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            Vec::new(),
        ));
    }
    // Extraction from TypeScript.
    let Some(file) = &a.file else {
        return bad(
            path,
            "an actor needs `program`, `codes`, or `file` + `function` to extract from TypeScript",
        );
    };
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
    let request = ExtractRequest {
        root,
        file: file.clone(),
        function: a.function.clone(),
        arguments: args.clone(),
        tables,
        externals,
        max_inline_depth: extraction.max_inline_depth.unwrap_or(3),
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
    ))
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
                    Op::Db { stmt } => match &stmt.kind {
                        StmtKind::Select { into, .. } | StmtKind::SelectCount { into, .. } => {
                            defined.insert(into.clone());
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
        Op::Jump { target } => {
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

//! Initial-state synthesis and scenario construction.
//!
//! A generated scenario is written in the existing scenario format and
//! loaded by the existing loader; this module only decides its contents.
//!
//! **Arguments** come from the parameter paths the extracted code reads,
//! typed by the columns they are compared with or written to.
//!
//! **Rows** are found by running each actor alone through the model's own
//! engine (`engine::apply`), from an empty database: when the actor stops
//! before the candidate's operations because a read found nothing, a row
//! satisfying that read's closed predicate is added (with its primary key,
//! not-null columns, defaults and the parent rows its foreign keys need) and
//! the actor is run again. A read the candidate needs to come back empty (the
//! check of a check-then-insert) is never satisfied. The loop is bounded;
//! when it cannot reach the candidate's operations it says why. The rows are
//! then checked against every modeled constraint by the loader and against
//! the source's foreign keys here.
//!
//! **Serial outcomes** (the `SerialCell` property) are computed the same way:
//! the actors run one after the other in the engine, in every order.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;
use serde_json::{Value as Json, json};

use midwrite_model::engine::{Action, Recorder, apply, enabled, initial_state};
use midwrite_model::invariant::{
    ActorSel, ActorsRef, ExternalsRef, Formula, GeneratedBy, GeneratorSel, Invariant, OutcomeKind,
    RowRef, Term, TermOp, When,
};
use midwrite_model::ir::{Op, SourceRef};
use midwrite_model::model::Model;
use midwrite_model::provider::ActorProvider;
use midwrite_model::schema_facts::{SchemaDefault, SchemaTable};
use midwrite_model::state::{ClosedStmt, State, TKind};
use midwrite_model::value::{BinOp, Builtin, Expr, GenId, Row, Value};

use crate::candidate::{Candidate, Class, Property, Relation, Spec};
use crate::detect::Analyzed;
use crate::effects::Val;
use crate::project::ProjectIndex;

pub const MAX_SEED_ITERATIONS: usize = 16;
pub const MAX_SOLO_STEPS: usize = 2_000;

/// The kind of a scenario variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VariantKind {
    /// The application as analyzed: its schema, its code, a synthesized
    /// state and the declared environment.
    Actual,
    /// The same with actors that differ in the candidate's key: a control
    /// that should not interact.
    Control,
    /// A schema or isolation the application does NOT have (a mitigation or
    /// a sensitivity question). Never a finding about the application.
    Hypothetical,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Variant {
    pub name: String,
    pub kind: VariantKind,
    pub relation: Relation,
    pub description: String,
    /// Hypothetical unique constraint (table, columns).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unique_override: Option<(String, Vec<String>)>,
}

/// The variants built for a candidate.
pub fn variants(c: &Candidate, hypothetical: bool) -> Vec<Variant> {
    let mut out = vec![Variant {
        name: "actual".to_owned(),
        kind: VariantKind::Actual,
        relation: c.environment.relation,
        description: "the application as analyzed".to_owned(),
        unique_override: None,
    }];
    if c.environment.actors > 1
        && matches!(
            c.class,
            Class::CheckThenAct | Class::ReadModifyWrite | Class::StatusTransition
        )
    {
        out.push(Variant {
            name: "distinct-key".to_owned(),
            kind: VariantKind::Control,
            relation: Relation::DistinctKey,
            description: "the actors use different keys: a control that should not interact"
                .to_owned(),
            unique_override: None,
        });
    }
    if c.environment.actors > 1 && !c.request_keys.is_empty() {
        out.push(Variant {
            name: "distinct-request-key".to_owned(),
            kind: VariantKind::Actual,
            relation: Relation::DistinctRequestKey,
            description: format!(
                "the same requests with different idempotency keys ({}): a client that makes a new key per attempt (assumed environment, not established)",
                c.request_keys.join(", ")
            ),
            unique_override: None,
        });
    }
    if hypothetical
        && c.class == Class::CheckThenAct
        && !c.protections.iter().any(|p| p.starts_with("UNIQUE"))
    {
        out.push(Variant {
            name: "hypothetical-unique".to_owned(),
            kind: VariantKind::Hypothetical,
            relation: c.environment.relation,
            description: format!(
                "HYPOTHETICAL: UNIQUE({}) on `{}`, which the analyzed schema does not have",
                c.resource.columns.join(", "),
                c.resource.table
            ),
            unique_override: Some((c.resource.table.clone(), c.resource.columns.clone())),
        });
    }
    out
}

/// One actor of a scenario.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorPlan {
    pub name: String,
    pub selector: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invocation: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub preconditions: Vec<String>,
    pub arguments: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<String>,
    /// Phase 13: the entry's response is a separate step (a crash can
    /// follow its last statement): actors whose caller retries after a
    /// crash.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub response: bool,
    /// Source locations of the candidate's operations this actor must reach.
    #[serde(skip)]
    pub targets: Vec<String>,
    /// Source locations of reads that must find nothing.
    #[serde(skip)]
    pub keep_empty: Vec<String>,
    /// Rows this actor's reads returned in its solo run on the final
    /// rows, by source location (values a property key may depend on).
    #[serde(skip)]
    pub observed: BTreeMap<String, Row>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SynthesisReport {
    /// `satisfied`, `unsatisfiable` or `unsupported`.
    pub status: String,
    pub detail: String,
    pub iterations: usize,
    /// Rows added, per table.
    pub rows: BTreeMap<String, usize>,
    pub notes: Vec<String>,
}

/// Everything a generated scenario contains.
#[derive(Clone, Debug)]
pub struct Plan {
    pub name: String,
    pub description: String,
    pub schema_file: String,
    /// Phase 12: every schema module the tables come from.
    pub schema_files: Vec<String>,
    /// Table bindings to read with `from` (`module#binding` when the
    /// project has several schema modules).
    pub tables: Vec<String>,
    pub unique_override: Option<(String, Vec<String>, String)>,
    pub isolation: String,
    pub actors: Vec<ActorPlan>,
    pub rows: BTreeMap<String, Vec<Row>>,
    pub max_steps: u32,
    pub max_states: u64,
    pub max_crashes: u32,
    pub max_retries: u32,
    pub max_in_flight: u32,
    pub max_wall_ms: Option<u64>,
    pub max_memory_bytes: Option<u64>,
    pub invariants: Vec<Invariant>,
    pub source_verification: Option<Json>,
    /// Phase 13: an actor reads through a read replica (the scenario gets
    /// the `replicas` feature).
    pub replica_reads: bool,
    /// Phase 14: an actor's extracted code uses JavaScript value forms (the
    /// scenario gets the `js-values` feature, so that the loader extracts
    /// it as discovery did; without such forms both extractions are the
    /// Phase 13 one).
    pub js_values: bool,
}

impl Plan {
    /// The scenario document. `root` is the project root as the scenario
    /// file sees it.
    pub fn to_json(&self, index: &ProjectIndex, root: &str) -> Json {
        let mut tables = Vec::new();
        for binding in &self.tables {
            let mut t = json!({ "from": binding });
            if let Some((table, cols, reason)) = &self.unique_override
                && index.by_from(binding).is_some_and(|m| &m.name == table)
            {
                let source = index.source_table(table);
                let mut unique: Vec<Vec<String>> = source
                    .map(|s| {
                        s.unique
                            .iter()
                            .filter(|u| u.predicate.is_none())
                            .map(|u| {
                                u.columns
                                    .iter()
                                    .map(|c| {
                                        s.column_by_sql(c)
                                            .map_or_else(|| c.clone(), |x| x.property.clone())
                                    })
                                    .collect()
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                unique.push(cols.clone());
                t["overrides"] = json!({ "reason": reason, "unique": unique });
            }
            tables.push(t);
        }
        let mut channels = serde_json::Map::new();
        for e in &index.contracts.externals {
            channels.insert(e.channel.clone(), json!({ "dedup": e.dedup }));
        }
        let externals: Vec<Json> = index
            .contracts
            .externals
            .iter()
            .map(|e| {
                let mut x = json!({ "callee": e.callee, "channel": e.channel });
                if let Some(k) = e.key_argument {
                    x["keyArgument"] = json!(k);
                }
                x
            })
            .collect();
        let retries: Vec<Json> = index
            .bindings
            .transaction_retries
            .iter()
            .map(|r| {
                let mut x = json!({
                    "callee": r.callee,
                    "callbackArgument": r.callback_argument,
                    "attempts": r.attempts,
                });
                if let Some(o) = r.options_argument {
                    x["optionsArgument"] = json!(o);
                }
                x
            })
            .collect();
        let actors: Vec<Json> = self
            .actors
            .iter()
            .map(|a| {
                let mut entry = json!({ "selector": a.selector });
                if let Some(i) = &a.invocation {
                    entry["invocation"] = json!(i);
                }
                if !a.preconditions.is_empty() {
                    entry["preconditions"] = json!(a.preconditions);
                }
                let mut x = json!({
                    "name": a.name,
                    "entry": entry,
                    "arguments": a.arguments,
                });
                if let Some(r) = &a.retry {
                    x["retry"] = json!({ "trigger": retry_trigger(r) });
                }
                if a.response {
                    x["response"] = json!(true);
                }
                x
            })
            .collect();
        let mut bounds = json!({
            "maxSteps": self.max_steps,
            "maxStates": self.max_states,
        });
        if self.max_crashes > 0 {
            bounds["maxCrashes"] = json!(self.max_crashes);
        }
        if self.max_retries > 0 {
            bounds["maxRetries"] = json!(self.max_retries);
        }
        if self.max_in_flight > 0 {
            bounds["maxInFlight"] = json!(self.max_in_flight);
        }
        if let Some(ms) = self.max_wall_ms {
            bounds["maxWallMs"] = json!(ms);
        }
        if let Some(b) = self.max_memory_bytes {
            bounds["maxMemoryBytes"] = json!(b);
        }
        // Phase 12: a project with several schema modules gets the list
        // (tables are then bound by declaration); otherwise the single
        // module, as in Phase 11.
        let schema = if index.schema_files.len() > 1 {
            json!(self.schema_files)
        } else {
            json!(self.schema_file)
        };
        let mut extraction = json!({ "root": root, "schema": schema });
        if !externals.is_empty() {
            extraction["externals"] = json!(externals);
        }
        if !retries.is_empty() {
            extraction["transactionRetries"] = json!(retries);
        }
        // Phase 13: the declared replicated-database factories, so that the
        // loader extracts exactly what discovery extracted.
        if !index.bindings.replica_factories.is_empty() {
            extraction["replicaFactories"] = json!(index.bindings.replica_factories);
        }
        let mut doc = json!({
            "schemaVersion": 1,
            "name": self.name,
            "description": self.description,
            "extraction": extraction,
            "database": {
                "isolation": self.isolation,
                "tables": tables,
                "initialState": self.rows,
            },
            "actors": actors,
            "bounds": bounds,
            "invariants": self.invariants,
        });
        if !channels.is_empty() {
            doc["channels"] = Json::Object(channels);
        }
        if let Some(sv) = &self.source_verification {
            doc["sourceVerification"] = sv.clone();
        }
        // Phase 13: the source schema's foreign keys (and their row locks)
        // are enforced when the scenario's tables carry any.
        let has_foreign_keys = self.tables.iter().any(|r| {
            index
                .by_from(r)
                .and_then(|t| index.source_table(&t.name))
                .is_some_and(|t| {
                    !t.foreign_keys.is_empty() || t.columns.iter().any(|c| c.references.is_some())
                })
        });
        // Phase 14: time and JSON columns are modeled when the scenario
        // opts in (the discovery index reads them so).
        let builders = |names: &[&str]| {
            self.tables.iter().any(|r| {
                index
                    .by_from(r)
                    .and_then(|t| index.source_table(&t.name))
                    .is_some_and(|t| {
                        t.columns
                            .iter()
                            .any(|c| names.contains(&c.builder.as_str()))
                    })
            })
        };
        let mut features = Vec::new();
        if has_foreign_keys {
            features.push("foreign-keys");
        }
        if builders(&["timestamp", "date"]) {
            features.push("timestamps");
        }
        if builders(&["json", "jsonb"]) {
            features.push("json");
        }
        if self.replica_reads {
            features.push("replicas");
        }
        if self.js_values {
            features.push("js-values");
        }
        if !features.is_empty() {
            doc["features"] = json!(features);
        }
        doc
    }

    /// Load the scenario with the model checker's loader (project root as
    /// base directory).
    pub fn load(
        &self,
        index: &ProjectIndex,
        provider: &dyn ActorProvider,
    ) -> Result<(Model, midwrite_model::scenario::LoadReport), String> {
        let doc = self.to_json(index, ".");
        midwrite_model::scenario::load_str(&doc.to_string(), &index.root, provider)
            .map_err(|e| e.to_string())
    }
}

/// Phase 14: the effects of one extraction in a comparable form: every
/// database statement (source, connection, kind, table, lock), transaction
/// boundary, external call and unknown operation, per code vector, in code
/// order. Local computation, jumps and the steps only the loader adds (the
/// response step and the recorded return value) are not effects.
fn effect_trace(codes: &[Vec<midwrite_model::ir::Step>]) -> Vec<Vec<String>> {
    use midwrite_model::ir::{Conn, StmtKind};
    codes
        .iter()
        .map(|code| {
            code.iter()
                .filter_map(|s| {
                    let at = s.src.label();
                    Some(match &s.op {
                        Op::Db { stmt } => {
                            let conn = match stmt.conn {
                                Conn::Tx => "tx",
                                Conn::Pool => "pool",
                                Conn::Replica => "replica",
                            };
                            let kind = match &stmt.kind {
                                StmtKind::Select { lock, .. } => format!("select {}", lock.sql()),
                                StmtKind::SelectCount { .. } => "count".to_owned(),
                                StmtKind::Update { .. } => "update".to_owned(),
                                StmtKind::Insert { upsert, .. } => {
                                    if upsert.is_some() { "upsert" } else { "insert" }.to_owned()
                                }
                                StmtKind::Delete { .. } => "delete".to_owned(),
                            };
                            format!("{at} {conn} {} {}", kind.trim(), stmt.kind.table())
                        }
                        Op::Begin { .. } => format!("{at} begin"),
                        Op::Commit => format!("{at} commit"),
                        Op::Rollback => format!("{at} rollback"),
                        Op::External { channel, .. } => format!("{at} external {channel}"),
                        Op::Unknown { .. } => format!("{at} unknown"),
                        _ => return None,
                    })
                })
                .collect()
        })
        .collect()
}

/// Phase 14: compare discovery's extraction of an entry with the scenario
/// loader's (the round trip source → discovery → scenario → loader).
fn round_trip(
    discovered: &[Vec<midwrite_model::ir::Step>],
    loaded: &[Vec<midwrite_model::ir::Step>],
) -> Result<(), String> {
    let (d, l) = (effect_trace(discovered), effect_trace(loaded));
    if d == l {
        return Ok(());
    }
    if d.len() != l.len() {
        return Err(format!(
            "{} code vector(s) in discovery, {} in the loader",
            d.len(),
            l.len()
        ));
    }
    for (i, (dc, lc)) in d.iter().zip(&l).enumerate() {
        if let Some(j) = (0..dc.len().max(lc.len())).find(|&j| dc.get(j) != lc.get(j)) {
            let show = |v: Option<&String>| v.map_or("nothing".to_owned(), |s| format!("`{s}`"));
            return Err(format!(
                "code {i}, effect {j}: discovery has {}, the loader has {}",
                show(dc.get(j)),
                show(lc.get(j))
            ));
        }
    }
    Ok(())
}

fn retry_trigger(r: &str) -> &'static str {
    match r {
        "on-crash" => "onCrash",
        "on-failure" => "onFailure",
        "on-failure-or-crash" => "onFailureOrCrash",
        _ => "atLeastOnce",
    }
}

// ---------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ty {
    Int,
    Text,
    Bool,
    /// Phase 12: a `uuid` column: the runtime database accepts only UUIDs.
    Uuid,
    /// Phase 14: an array (of at most [`MAX_LIST`] strings, or integers).
    List {
        ints: bool,
    },
}

/// Phase 14: the most elements a synthesized array has.
pub const MAX_LIST: usize = 3;

/// Parameter paths the code reads, each with a type and whether it is a key.
pub(crate) fn param_paths(a: &Analyzed, index: &ProjectIndex) -> BTreeMap<String, (Ty, bool)> {
    let s = &a.summary;
    let mut out: BTreeMap<String, (Ty, bool)> = BTreeMap::new();
    let mut record = |path: &str, ty: Ty, key: bool| {
        let e = out.entry(path.to_owned()).or_insert((ty, key));
        e.1 |= key;
        if e.0 == Ty::Text && ty != Ty::Text {
            e.0 = ty;
        }
    };
    // Every path mentioned anywhere.
    let mut all = BTreeSet::new();
    for code in &s.codes {
        for step in code {
            for e in exprs_of(&step.op) {
                collect_paths(e, &s.params, &mut all);
            }
        }
    }
    for p in &all {
        record(p, Ty::Text, false);
    }
    let column_type = |table: &str, column: &str| -> Option<(Ty, bool)> {
        let t = index.source_table(table)?;
        let c = t.column(column)?;
        let ty = match c.builder.as_str() {
            "serial" | "bigserial" | "smallserial" | "integer" | "smallint" | "bigint" => Ty::Int,
            "boolean" => Ty::Bool,
            "uuid" => Ty::Uuid,
            _ => Ty::Text,
        };
        let key = c.primary_key
            || c.references.is_some()
            || t.foreign_keys
                .iter()
                .any(|f| f.columns.contains(&c.sql_name))
            || t.primary_key.contains(&c.sql_name)
            || t.unique.iter().any(|u| u.columns.contains(&c.sql_name));
        Some((ty, key))
    };
    for e in &s.db {
        for (c, v) in e.eq.iter().chain(e.writes.iter()) {
            let mut leaves = Vec::new();
            v.leaves(&mut leaves);
            let direct = matches!(v, Val::Param { .. });
            for l in leaves {
                if let Val::Param { path } = l
                    && let Some((ty, key)) = column_type(&e.table, c)
                {
                    // A parameter compared with a key column is a key; one that
                    // only feeds arithmetic is a quantity.
                    let is_key = key || (direct && e.eq.iter().any(|(ec, _)| ec == c));
                    record(&path, ty, direct && is_key);
                }
            }
        }
        for (text, _) in &e.other {
            let _ = text;
        }
    }
    for x in &s.externals {
        if let Some(Val::Param { path }) = &x.key {
            record(path, Ty::Text, true);
        }
    }
    // Paths compared with integers or used in arithmetic are integers.
    for code in &s.codes {
        for step in code {
            for e in exprs_of(&step.op) {
                numeric_paths(e, &s.params, &mut |p| record(p, Ty::Int, false));
            }
        }
    }
    // A path that is a prefix of another is an object, not a leaf.
    let leaves: BTreeMap<String, (Ty, bool)> = out
        .iter()
        .filter(|(p, _)| !out.keys().any(|q| q.starts_with(&format!("{p}."))))
        .map(|(p, v)| (p.clone(), *v))
        .collect();
    leaves
}

/// Phase 14: whether extracted code uses a JavaScript value form (an array,
/// a built-in call, a callback, `undefined`, a list/map/set literal), which
/// only an extraction with `js_values` produces.
fn uses_js_values(codes: &[Vec<midwrite_model::ir::Step>]) -> bool {
    fn js(e: &Expr) -> bool {
        match e {
            Expr::Array(_) | Expr::Call { .. } | Expr::Each { .. } => true,
            Expr::Lit(v) => matches!(
                v,
                Value::Undefined(_) | Value::List(_) | Value::Map(_) | Value::Set(_)
            ),
            Expr::Local(_) | Expr::Col(_) | Expr::Unknown(_) => false,
            Expr::Field(e, _) | Expr::Not(e) | Expr::IsNull(e) => js(e),
            Expr::Bin(_, a, b) => js(a) || js(b),
            Expr::Object(fields) => fields.iter().any(|(_, e)| js(e)),
        }
    }
    codes.iter().flatten().any(|s| {
        exprs_of(&s.op).into_iter().any(js) || matches!(&s.op, Op::ReturnValue { expr } if js(expr))
    })
}

pub(crate) fn exprs_of(op: &Op) -> Vec<&Expr> {
    use midwrite_model::ir::StmtKind;
    match op {
        Op::Let { expr, .. } => vec![expr],
        Op::JumpIf { cond, .. } => vec![cond],
        Op::External { payload, key, .. } => {
            payload.iter().map(|(_, e)| e).chain(key.iter()).collect()
        }
        Op::AppWrite { expr, .. } => vec![expr],
        Op::Db { stmt } => match &stmt.kind {
            StmtKind::Select { pred, .. }
            | StmtKind::SelectCount { pred, .. }
            | StmtKind::Delete { pred, .. } => vec![pred],
            StmtKind::Update { pred, set, .. } => std::iter::once(pred)
                .chain(set.iter().map(|(_, e)| e))
                .collect(),
            // Phase 14: every row of a multi-row INSERT.
            StmtKind::Insert {
                values, more_rows, ..
            } => values
                .iter()
                .chain(more_rows.iter().flatten())
                .map(|(_, e)| e)
                .collect(),
        },
        _ => Vec::new(),
    }
}

pub(crate) fn path_of(e: &Expr, params: &[String]) -> Option<String> {
    match e {
        Expr::Local(l) if params.contains(l) => Some(l.clone()),
        // Phase 14: `At(base, "f")` is the property read of JavaScript values.
        _ => {
            let (base, f) = e.as_property()?;
            path_of(base, params).map(|p| format!("{p}.{f}"))
        }
    }
}

fn collect_paths(e: &Expr, params: &[String], out: &mut BTreeSet<String>) {
    if let Some(p) = path_of(e, params) {
        out.insert(p);
        return;
    }
    match e {
        Expr::Field(b, _) | Expr::Not(b) | Expr::IsNull(b) => collect_paths(b, params, out),
        Expr::Bin(_, a, b) => {
            collect_paths(a, params, out);
            collect_paths(b, params, out);
        }
        Expr::Object(fields) => {
            for (_, v) in fields {
                collect_paths(v, params, out);
            }
        }
        // Phase 14: arguments of JavaScript built-ins and array callbacks
        // (SQL built-ins are left to their own typing).
        Expr::Call { func, .. } if func.is_sql() => {}
        Expr::Array(items) | Expr::Call { args: items, .. } => {
            for v in items {
                collect_paths(v, params, out);
            }
        }
        Expr::Each { list, body, .. } => {
            collect_paths(list, params, out);
            collect_paths(body, params, out);
        }
        _ => {}
    }
}

fn numeric_paths(e: &Expr, params: &[String], f: &mut dyn FnMut(&str)) {
    match e {
        Expr::Bin(op, a, b) => {
            let numeric = matches!(
                op,
                BinOp::Add
                    | BinOp::Sub
                    | BinOp::Mul
                    | BinOp::Lt
                    | BinOp::Le
                    | BinOp::Gt
                    | BinOp::Ge
            );
            if numeric {
                for side in [a, b] {
                    if let Some(p) = path_of(side, params) {
                        f(&p);
                    }
                }
            }
            numeric_paths(a, params, f);
            numeric_paths(b, params, f);
        }
        Expr::Field(b, _) | Expr::Not(b) | Expr::IsNull(b) => numeric_paths(b, params, f),
        Expr::Object(fields) => {
            for (_, v) in fields {
                numeric_paths(v, params, f);
            }
        }
        Expr::Call { func, .. } if func.is_sql() => {}
        Expr::Array(items) | Expr::Call { args: items, .. } => {
            for v in items {
                numeric_paths(v, params, f);
            }
        }
        Expr::Each { list, body, .. } => {
            numeric_paths(list, params, f);
            numeric_paths(body, params, f);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Phase 14: string and array arguments
// ---------------------------------------------------------------------------

/// How the extracted code uses one parameter path as a JavaScript value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Usage {
    /// Array-only operations (`map`/`filter`/... callbacks, `join`).
    array: Vec<String>,
    /// Element access, iteration, destructuring (arrays, also strings).
    element: Vec<String>,
    /// String-only operations (`split`, `trim`, `startsWith`, ...).
    string: Vec<String>,
    /// Operations of both strings and arrays (`includes`, `indexOf`,
    /// `length`).
    shared: Vec<String>,
    /// Literal arguments of `includes`/`indexOf`: candidate elements.
    needles: Vec<Value>,
    /// Elements are read as objects (`x.id` in a callback or a loop).
    object_elements: bool,
}

/// Phase 14: the usage of every parameter path the code applies a
/// JavaScript built-in to. A local defined once from a path (the snapshot
/// of a `for...of` or of a destructuring) stands for that path.
fn value_usage(codes: &[Vec<Op>], params: &[String]) -> BTreeMap<String, Usage> {
    let mut defs: BTreeMap<&str, Vec<&Expr>> = BTreeMap::new();
    for op in codes.iter().flatten() {
        if let Op::Let { local, expr } = op {
            defs.entry(local.as_str()).or_default().push(expr);
        }
    }
    let path = |e: &Expr| -> Option<String> {
        if let Some(p) = path_of(e, params) {
            return Some(p);
        }
        match e {
            Expr::Local(l) => match defs.get(l.as_str()).map(Vec::as_slice) {
                Some([one]) => path_of(one, params),
                _ => None,
            },
            _ => None,
        }
    };
    // Locals holding an element of a path (`x = At(list, i)`).
    let mut elements: BTreeMap<String, String> = BTreeMap::new();
    for op in codes.iter().flatten() {
        if let Op::Let { local, expr } = op
            && let Expr::Call {
                func: Builtin::At,
                args,
            } = expr
            && let [recv, index] = args.as_slice()
            && !matches!(index, Expr::Lit(Value::Str(_)))
            && let Some(p) = path(recv)
        {
            elements.insert(local.clone(), p);
        }
    }
    let mut out: BTreeMap<String, Usage> = BTreeMap::new();
    fn walk(
        e: &Expr,
        path: &dyn Fn(&Expr) -> Option<String>,
        elements: &BTreeMap<String, String>,
        out: &mut BTreeMap<String, Usage>,
    ) {
        match e {
            Expr::Call { func, args } => {
                if let Some(recv) = args.first()
                    && let Some(p) = path(recv)
                {
                    let name = func.name().to_owned();
                    match func {
                        Builtin::Join => out.entry(p).or_default().array.push(name),
                        Builtin::Split
                        | Builtin::StartsWith
                        | Builtin::EndsWith
                        | Builtin::ToLowerCase
                        | Builtin::ToUpperCase
                        | Builtin::Trim => out.entry(p).or_default().string.push(name),
                        Builtin::Includes | Builtin::IndexOf | Builtin::Length => {
                            let u = out.entry(p).or_default();
                            u.shared.push(name);
                            if let Some(Expr::Lit(v)) = args.get(1)
                                && matches!(v, Value::Str(_) | Value::Int(_))
                                && !u.needles.contains(v)
                            {
                                u.needles.push(v.clone());
                            }
                        }
                        Builtin::At if !matches!(args.get(1), Some(Expr::Lit(Value::Str(_)))) => {
                            out.entry(p).or_default().element.push("[index]".to_owned())
                        }
                        _ => {}
                    }
                }
                // An element read as an object.
                if let Some((base, _)) = e.as_property()
                    && let Expr::Local(l) = base
                    && let Some(p) = elements.get(l)
                {
                    out.entry(p.clone()).or_default().object_elements = true;
                }
                for a in args {
                    walk(a, path, elements, out);
                }
            }
            Expr::Each {
                op,
                list,
                var,
                body,
            } => {
                if let Some(p) = path(list) {
                    let u = out.entry(p).or_default();
                    u.array.push(format!("{op:?}").to_lowercase());
                    if reads_property_of(body, var) {
                        u.object_elements = true;
                    }
                }
                walk(list, path, elements, out);
                walk(body, path, elements, out);
            }
            Expr::Field(b, _) | Expr::Not(b) | Expr::IsNull(b) => {
                if let Expr::Field(base, _) = e
                    && let Expr::Local(l) = base.as_ref()
                    && let Some(p) = elements.get(l)
                {
                    out.entry(p.clone()).or_default().object_elements = true;
                }
                walk(b, path, elements, out);
            }
            Expr::Bin(_, a, b) => {
                walk(a, path, elements, out);
                walk(b, path, elements, out);
            }
            Expr::Object(fields) => fields
                .iter()
                .for_each(|(_, v)| walk(v, path, elements, out)),
            Expr::Array(items) => items.iter().for_each(|v| walk(v, path, elements, out)),
            _ => {}
        }
    }
    for op in codes.iter().flatten() {
        for e in exprs_of(op) {
            walk(e, &path, &elements, &mut out);
        }
    }
    out
}

fn reads_property_of(e: &Expr, var: &str) -> bool {
    if let Some((base, _)) = e.as_property()
        && matches!(base, Expr::Local(l) if l == var)
    {
        return true;
    }
    match e {
        Expr::Field(b, _) | Expr::Not(b) | Expr::IsNull(b) => reads_property_of(b, var),
        Expr::Bin(_, a, b) => reads_property_of(a, var) || reads_property_of(b, var),
        Expr::Object(fields) => fields.iter().any(|(_, v)| reads_property_of(v, var)),
        Expr::Array(items) | Expr::Call { args: items, .. } => {
            items.iter().any(|v| reads_property_of(v, var))
        }
        Expr::Each { list, body, .. } => {
            reads_property_of(list, var) || reads_property_of(body, var)
        }
        _ => false,
    }
}

/// The type a written TypeScript annotation gives a value, when it is one
/// synthesis builds: `string`, `number`, `string[]`, `Array<number>`, ...
fn annotated_ty(written: &str) -> Option<Ty> {
    let t = written.trim();
    let t = t.strip_prefix("readonly ").unwrap_or(t).trim();
    let element = t
        .strip_suffix("[]")
        .or_else(|| t.strip_prefix("Array<").and_then(|r| r.strip_suffix('>')))
        .or_else(|| {
            t.strip_prefix("ReadonlyArray<")
                .and_then(|r| r.strip_suffix('>'))
        });
    match element.map(str::trim) {
        Some("string") => Some(Ty::List { ints: false }),
        Some("number") => Some(Ty::List { ints: true }),
        Some(_) => None,
        None => match t {
            "string" => Some(Ty::Text),
            "number" => Some(Ty::Int),
            "boolean" => Some(Ty::Bool),
            _ => None,
        },
    }
}

/// Phase 14: the type of a path used as a JavaScript value, or why it is
/// not synthesized. `Ok(None)`: no decision (the Phase 12 typing stands).
fn usage_ty(path: &str, u: &Usage, annotation: Option<&str>) -> Result<Option<Ty>, String> {
    let declared = annotation.and_then(annotated_ty);
    if u.object_elements {
        return Err(format!(
            "the parameter path `{path}` is an array of objects: synthesized arrays hold strings or integers only"
        ));
    }
    let list_ints = declared == Some(Ty::List { ints: true })
        || (declared.is_none()
            && u.needles.iter().all(|v| matches!(v, Value::Int(_)))
            && !u.needles.is_empty());
    let list = Ty::List { ints: list_ints };
    if !u.array.is_empty() && !u.string.is_empty() {
        return Err(format!(
            "the parameter path `{path}` is used both as an array ({}) and as a string ({})",
            u.array.join(", "),
            u.string.join(", ")
        ));
    }
    let used = if !u.array.is_empty() {
        Some(list)
    } else if !u.string.is_empty() {
        Some(Ty::Text)
    } else if !u.element.is_empty() {
        Some(list)
    } else {
        None
    };
    match (used, declared) {
        // The annotation agrees, or decides between the shared uses.
        (Some(Ty::List { .. }), Some(Ty::List { .. })) | (None, Some(Ty::List { .. })) => {
            Ok(declared)
        }
        (Some(Ty::Text), Some(Ty::Text)) | (None, Some(Ty::Text)) => Ok(Some(Ty::Text)),
        // Element access on a declared string indexes the string.
        (Some(Ty::List { .. }), Some(Ty::Text)) if u.array.is_empty() => Ok(Some(Ty::Text)),
        (Some(used), Some(declared)) if used != declared => Err(format!(
            "the parameter path `{path}` is used as {} but declared `{}`",
            if matches!(used, Ty::List { .. }) {
                "an array"
            } else {
                "a string"
            },
            annotation.unwrap_or_default()
        )),
        (Some(used), _) => Ok(Some(used)),
        (None, _) if !u.shared.is_empty() => Err(format!(
            "the parameter path `{path}` is used only with operations of both strings and arrays ({}), and no `string`/`string[]` annotation says which it is",
            u.shared.join(", ")
        )),
        (None, _) => Ok(None),
    }
}

/// A synthesized array: the literal arguments of its `includes`/`indexOf`
/// (so a membership guard can pass), else one placeholder element.
fn list_value(last: &str, u: Option<&Usage>, ints: bool) -> Value {
    let mut items: Vec<Value> = u
        .map(|u| {
            u.needles
                .iter()
                .filter(|v| matches!(v, Value::Int(_)) == ints)
                .take(MAX_LIST)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if items.is_empty() {
        items.push(if ints {
            Value::Int(1)
        } else {
            Value::Str(format!("{last}-1"))
        });
    }
    Value::List(items)
}

fn set_path(root: &mut [Value], params: &[String], path: &str, value: Value) {
    let mut parts = path.split('.');
    let Some(first) = parts.next() else { return };
    let Some(i) = params.iter().position(|p| p == first) else {
        return;
    };
    let rest: Vec<&str> = parts.collect();
    if rest.is_empty() {
        root[i] = value;
        return;
    }
    if !matches!(root[i], Value::Row(_)) {
        root[i] = Value::Row(Row::new());
    }
    let mut cur = &mut root[i];
    for (k, part) in rest.iter().enumerate() {
        let Value::Row(map) = cur else { return };
        if k + 1 == rest.len() {
            map.insert((*part).to_owned(), value);
            return;
        }
        cur = map
            .entry((*part).to_owned())
            .or_insert_with(|| Value::Row(Row::new()));
        if !matches!(cur, Value::Row(_)) {
            *cur = Value::Row(Row::new());
        }
    }
}

/// Phase 16: remove the key at `path` (a property below a parameter) from
/// its object.
fn remove_path(root: &mut [Value], params: &[String], path: &str) {
    let Some((parent, key)) = path.rsplit_once('.') else {
        return;
    };
    let mut parts = parent.split('.');
    let Some(i) = parts
        .next()
        .and_then(|first| params.iter().position(|p| p == first))
    else {
        return;
    };
    let mut cur = &mut root[i];
    for part in parts {
        let Value::Row(map) = cur else { return };
        let Some(next) = map.get_mut(part) else {
            return;
        };
        cur = next;
    }
    if let Value::Row(map) = cur {
        map.remove(key);
    }
}

/// Phase 16: make the arguments of `a` valid for the zod schemas that
/// validate them (`midwrite_model::zodfn::conform`: valid values are
/// kept, required keys added, keys a strict object rejects removed), then
/// restore the values `midwrite-contracts.json` declares. A schema no
/// synthesized value satisfies (a `.regex()` the placeholders miss) leaves
/// the arguments as they are: the run stops at the validation and says so.
fn conform_to_zod(a: &Analyzed, args: &mut [Value]) {
    let params = &a.summary.params;
    for z in crate::domains::zod_inputs(a) {
        let current = get_path(args, params, &z.path);
        let name = z.path.rsplit('.').next().unwrap_or(&z.path);
        if let Some(v) =
            midwrite_model::zodfn::conform(z.version, &z.schema, current.as_ref(), name)
            && Some(&v) != current.as_ref()
        {
            set_path(args, params, &z.path, v);
        }
    }
    if let Some(ct) = &a.contract {
        for (path, value) in &ct.values {
            set_path(args, params, path, value.clone());
        }
    }
}

pub fn get_path(args: &[Value], params: &[String], path: &str) -> Option<Value> {
    let mut parts = path.split('.');
    let first = parts.next()?;
    let i = params.iter().position(|p| p == first)?;
    let mut cur = args.get(i)?;
    for part in parts {
        let Value::Row(map) = cur else { return None };
        cur = map.get(part)?;
    }
    Some(cur.clone())
}

/// Paths of an entry that carry the candidate's logical key, or that the
/// relation varies.
pub(crate) fn varied_paths(c: &Candidate, a: &Analyzed, relation: Relation) -> Vec<String> {
    let mut vals: Vec<Val> = Vec::new();
    match relation {
        Relation::Same => {}
        Relation::DistinctKey => {
            for p in &c.properties {
                match &p.spec {
                    Spec::AtMostOne { key, .. } | Spec::SerialCell { key, .. } => {
                        vals.extend(key.iter().map(|(_, v)| v.clone()));
                    }
                    _ => {}
                }
            }
            if vals.is_empty()
                && let Some(op) = c.conflict.first()
                && !op.is_external
                && let Some(e) = a.summary.db.get(op.effect)
            {
                // The checked row's key.
                vals.extend(
                    e.eq.iter()
                        .map(|(_, v)| crate::detect::canon(v, &a.summary)),
                );
            }
        }
        Relation::DistinctRequestKey => {
            return c.request_keys.clone();
        }
        Relation::DistinctRow | Relation::Swapped => {
            for op in c.conflict.iter().filter(|o| !o.is_external) {
                // Phase 16: with swapped keys a row lock taken by a read
                // (`SELECT ... FOR UPDATE`) is part of the conflict too.
                if let Some(e) = a.summary.db.get(op.effect)
                    && (e.access.writes()
                        || (relation == Relation::Swapped
                            && e.lock != midwrite_model::ir::LockMode::None))
                {
                    vals.extend(
                        e.eq.iter()
                            .map(|(_, v)| crate::detect::canon(v, &a.summary)),
                    );
                }
            }
        }
    }
    let mut out: Vec<String> = Vec::new();
    // Phase 12: a key read from a row (`owner.id` of the user found by
    // email) varies with the parameters that select that row; without
    // following them, a "distinct key" control kept the same key and
    // silently repeated the actual scenario.
    let mut seen = BTreeSet::new();
    let mut i = 0;
    while i < vals.len() {
        let v = vals[i].clone();
        i += 1;
        match v {
            Val::Param { path } => {
                if !out.contains(&path) {
                    out.push(path);
                }
            }
            Val::Read { effect, .. } | Val::ReadRow { effect } => {
                if seen.insert(effect)
                    && let Some(e) = a.summary.db.get(effect)
                {
                    vals.extend(
                        e.eq.iter()
                            .map(|(_, v)| crate::detect::canon(v, &a.summary)),
                    );
                }
            }
            _ => {}
        }
    }
    out
}

/// Synthesize positional arguments for `a` (actor number `k`), or say why
/// a parameter cannot be synthesized (Phase 14: a value whose type the code
/// does not determine).
fn arguments(
    c: &Candidate,
    a: &Analyzed,
    index: &ProjectIndex,
    relation: Relation,
    k: usize,
) -> Result<Vec<Value>, String> {
    if let Some(declared) = a.contract.as_ref().and_then(|c| c.arguments.clone()) {
        return Ok(declared);
    }
    let params = &a.summary.params;
    let mut args = vec![Value::Null; params.len()];
    let mut paths = param_paths(a, index);
    // Phase 14: strings and arrays, typed by the built-ins applied to them
    // and the parameters' TypeScript annotations.
    let ops: Vec<Vec<Op>> = a
        .summary
        .codes
        .iter()
        .map(|code| code.iter().map(|s| s.op.clone()).collect())
        .collect();
    let usage = value_usage(&ops, params);
    let annotations = midwrite_analyzer::TsActorProvider::new(&index.root)
        .param_annotations(&index.root, &a.fact.selector);
    // Phase 15: a string-literal union (`kind: RouteKind`) admits only its
    // members.
    let mut literals: BTreeMap<String, Vec<String>> = annotations
        .iter()
        .filter_map(|(p, t)| crate::domains::literal_members(t).map(|m| (p.clone(), m)))
        .collect();
    // Phase 16: so does a zod enum (base values take its first member,
    // "different value" choices another one).
    for (p, members) in crate::domains::zod_enum_members(a) {
        literals.entry(p).or_insert(members);
    }
    if !usage.is_empty() {
        for (path, u) in &usage {
            if let Some(ty) = usage_ty(path, u, annotations.get(path).map(String::as_str))? {
                paths.entry(path.clone()).or_insert((ty, false)).0 = ty;
                // An array or string is a leaf: drop paths below it.
                let below = format!("{path}.");
                paths.retain(|p, _| !p.starts_with(&below));
            }
        }
    }
    let varied = varied_paths(c, a, relation);
    let mut next_key = 1;
    let mut key_values: BTreeMap<String, i64> = BTreeMap::new();
    for (path, (ty, key)) in &paths {
        let last = path.rsplit('.').next().unwrap_or(path);
        let value = match ty {
            Ty::Int if *key => {
                let n = next_key;
                next_key += 1;
                key_values.insert(path.clone(), n);
                Value::Int(n)
            }
            Ty::Int => Value::Int(3),
            Ty::Bool => Value::Bool(true),
            Ty::Uuid => uuid_literal(last, 1),
            Ty::Text if literals.contains_key(path) => Value::Str(literals[path][0].clone()),
            Ty::Text if last.to_ascii_lowercase().contains("email") => {
                Value::Str("alice@example.test".to_owned())
            }
            Ty::Text => Value::Str(format!("{last}-1")),
            Ty::List { ints } => list_value(last, usage.get(path), *ints),
        };
        set_path(&mut args, params, path, value);
    }
    if let Some(ct) = &a.contract {
        for (path, value) in &ct.values {
            set_path(&mut args, params, path, value.clone());
        }
    }
    // Phase 16: arguments a zod schema validates satisfy it.
    conform_to_zod(a, &mut args);
    if k > 0 {
        match relation {
            Relation::Same => {}
            Relation::DistinctKey | Relation::DistinctRow | Relation::DistinctRequestKey => {
                for path in &varied {
                    let new = match get_path(&args, params, path) {
                        // Phase 15: another member of a literal union, if any.
                        Some(Value::Str(s)) if literals.contains_key(path) => {
                            match literals[path].iter().find(|m| **m != s) {
                                Some(m) => Value::Str(m.clone()),
                                None => continue,
                            }
                        }
                        Some(Value::Int(n)) => Value::Int(n + 100),
                        // Phase 12: another valid UUID.
                        Some(Value::Str(s)) if s.len() == 36 && s.matches('-').count() == 4 => {
                            Value::Str(format!("{}b{}", &s[..24], &s[25..]))
                        }
                        Some(Value::Str(s)) if s.contains("alice") => {
                            Value::Str(s.replace("alice", "bob"))
                        }
                        Some(Value::Str(s)) => Value::Str(format!("{s}-b")),
                        _ => continue,
                    };
                    set_path(&mut args, params, path, new);
                }
            }
            Relation::Swapped => {
                if let [p1, p2, ..] = varied.as_slice() {
                    let v1 = get_path(&args, params, p1);
                    let v2 = get_path(&args, params, p2);
                    if let (Some(v1), Some(v2)) = (v1, v2) {
                        set_path(&mut args, params, p1, v2);
                        set_path(&mut args, params, p2, v1);
                    }
                }
            }
        }
        // Phase 16: a varied value the schema rejects is replaced by a
        // valid one.
        conform_to_zod(a, &mut args);
    }
    Ok(args)
}

/// Phase 15: the synthesized arguments of every actor of a variant (the
/// base world's), or why one cannot be synthesized.
pub fn base_arguments(
    c: &Candidate,
    variant: &Variant,
    analyzed: &[&Analyzed],
    index: &ProjectIndex,
) -> Result<Vec<Vec<Value>>, String> {
    c.actor_entries
        .iter()
        .enumerate()
        .map(|(k, &ai)| arguments(c, analyzed[ai], index, variant.relation, k))
        .collect()
}

// ---------------------------------------------------------------------------
// Running actors in the engine
// ---------------------------------------------------------------------------

/// One executed transition of a solo run.
pub struct Executed {
    pub src: SourceRef,
    pub actions: Vec<Action>,
    /// Phase 15: the branches evaluated in this transition (local
    /// operations are folded into it): (location, source condition's value).
    pub branches: Vec<(String, bool)>,
}

pub enum Solo {
    Finished {
        executed: Vec<Executed>,
        state: Box<State>,
    },
    Unsupported {
        detail: String,
        location: String,
    },
}

/// Phase 15: where a solo run ended (for a developer): the last executed
/// operation and the failure the actor ended with, if any.
fn stopped_at(executed: &[Executed], state: &State) -> String {
    let last = executed
        .last()
        .map(|e| e.src.label())
        .unwrap_or_else(|| "its first operation".to_owned());
    let failure = state
        .actors
        .first()
        .and_then(|a| a.threads.iter().find_map(|t| t.failure.clone()));
    match failure {
        Some(f) => format!(" (the actor ended after {last} with: {f})"),
        None => format!(" (the actor ended after {last})"),
    }
}

/// Phase 15: branch outcomes among recorded events (`CHECK` events: the
/// branch's source condition and its value).
fn branch_events(events: &[midwrite_model::engine::Event]) -> Vec<(String, bool)> {
    events
        .iter()
        .filter(|e| e.text.starts_with("CHECK "))
        .filter_map(|e| match (&e.src, &e.result) {
            (Some(src), Some(Value::Bool(b))) => Some((src.label(), *b)),
            _ => None,
        })
        .collect()
}

/// Run `actor` alone from `state` until it cannot proceed.
pub fn run_alone(model: &Model, state: State, actor: usize) -> Solo {
    let mut s = state;
    let mut executed = Vec::new();
    for _ in 0..MAX_SOLO_STEPS {
        let next = enabled(model, &s)
            .into_iter()
            .find(|t| t.actor as usize == actor && matches!(t.kind, TKind::Step | TKind::Resume));
        let Some(t) = next else { break };
        let thread = &s.actors[actor].threads[t.thread as usize];
        let src = model.actors[actor].codes[thread.code as usize][thread.pc as usize]
            .src
            .clone();
        let mut rec = Recorder::on();
        match apply(model, &s, t, &mut rec) {
            Ok(applied) => {
                s = applied.state;
                // Phase 15: branch outcomes, from the recorder's `CHECK`
                // events (a branch's source condition and its value).
                executed.push(Executed {
                    src,
                    branches: branch_events(&rec.events),
                    actions: rec.actions,
                });
            }
            Err(u) => {
                return Solo::Unsupported {
                    detail: u.detail,
                    location: u.src.label(),
                };
            }
        }
    }
    Solo::Finished {
        executed,
        state: Box::new(s),
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

struct Rows<'a> {
    index: &'a ProjectIndex,
    rows: BTreeMap<String, Vec<Row>>,
    added: BTreeMap<String, usize>,
    notes: Vec<String>,
}

impl Rows<'_> {
    fn source(&self, table: &str) -> Option<&SchemaTable> {
        self.index.source_table(table)
    }

    fn model_columns(&self, table: &str) -> Vec<String> {
        self.index
            .table_by_name(table)
            .map(|t| t.columns.clone())
            .unwrap_or_default()
    }

    /// The table a foreign key written as `binding` in `module` references
    /// (Phase 12: resolved by declaration).
    fn table_of_binding(&self, module: &str, binding: &str) -> Option<String> {
        self.index
            .referenced_table(module, binding)
            .map(|t| t.name.clone())
    }

    fn next_int(&self, table: &str, column: &str) -> i64 {
        self.rows
            .get(table)
            .into_iter()
            .flatten()
            .filter_map(|r| match r.get(column) {
                Some(Value::Int(n)) => Some(*n),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            + 1
    }

    /// Add a row with the given values, completing keys, not-null columns,
    /// defaults and foreign-key parents. Returns false if impossible.
    fn add(&mut self, table: &str, given: Row, depth: u32) -> Result<(), String> {
        // Phase 12: a value generated during the run (the key an actor's own
        // insert produced) cannot be required of the initial state: the row
        // it belongs to is created by the execution itself.
        if given.values().any(|v| matches!(v, Value::Gen(_))) {
            return Err(format!(
                "a `{table}` row would need a value generated during the execution"
            ));
        }
        if depth > 6 {
            return Err(format!("foreign-key chain from `{table}` is too deep"));
        }
        let Some(source) = self.source(table).cloned() else {
            return Err(format!("`{table}` is not a table of the analyzed schema"));
        };
        let columns = self.model_columns(table);
        let mut row = Row::new();
        let n = self.rows.get(table).map_or(0, Vec::len) + 1;
        for col in &source.columns {
            if !columns.contains(&col.property) {
                if col.not_null && col.default.is_none() && !col.primary_key {
                    self.notes.push(format!(
                        "`{table}.{}` is NOT NULL without default and not modeled: the runtime database needs a value the model does not carry",
                        col.property
                    ));
                }
                continue;
            }
            let value = if let Some(v) = given.get(&col.property) {
                v.clone()
            } else if col.primary_key || source.primary_key.contains(&col.sql_name) {
                match model_ty(&col.builder) {
                    Ty::Int => Value::Int(self.next_int(table, &col.property)),
                    // Phase 12: a `uuid` column needs a valid UUID in the
                    // runtime database.
                    _ if col.builder == "uuid" => uuid_literal(table, n),
                    _ => Value::Str(format!("{table}-{n}")),
                }
            } else if let Some(d) = &col.default {
                match d {
                    SchemaDefault::Constant { value } => literal(value),
                    SchemaDefault::Sequence { .. } => {
                        Value::Int(self.next_int(table, &col.property))
                    }
                    _ => fill(&col.builder, &col.property, n),
                }
            } else if col.not_null {
                fill(&col.builder, &col.property, n)
            } else {
                Value::Null
            };
            row.insert(col.property.clone(), value);
        }
        // Foreign keys: the parent row must exist.
        let mut parents: Vec<(String, String, Value)> = Vec::new();
        for col in &source.columns {
            if let Some((ptable, pcol)) = &col.references
                && let Some(v) = row.get(&col.property)
                && !v.is_null()
            {
                parents.push((ptable.clone(), pcol.clone(), v.clone()));
            }
        }
        for fk in &source.foreign_keys {
            if let ([c], [pc]) = (fk.columns.as_slice(), fk.foreign_columns.as_slice())
                && let Some(prop) = source.column_by_sql(c).map(|x| x.property.clone())
                && let Some(v) = row.get(&prop)
                && !v.is_null()
            {
                parents.push((fk.table.clone(), pc.clone(), v.clone()));
            }
        }
        for (pbinding, pcol, v) in parents {
            // Phase 12: a generated parent key is created by the execution.
            if matches!(v, Value::Gen(_)) {
                continue;
            }
            let Some(ptable) = self.table_of_binding(&source.file, &pbinding) else {
                self.notes.push(format!(
                    "foreign key of `{table}` to `{pbinding}`: the parent table is not modeled"
                ));
                continue;
            };
            let exists = self
                .rows
                .get(&ptable)
                .is_some_and(|rs| rs.iter().any(|r| r.get(&pcol) == Some(&v)));
            if !exists {
                let mut g = Row::new();
                g.insert(pcol.clone(), v.clone());
                self.add(&ptable, g, depth + 1)?;
            }
        }
        // Keys must stay unique. Phase 16: a `NULLS NOT DISTINCT` rule
        // compares NULLs as equal (the model enforces it, so a world that
        // repeated a NULL key would be refused by the loader).
        let nulls_not_distinct = crate::detect::nulls_not_distinct_sets(self.index, table);
        for (set, predicate) in crate::detect::unique_sets(self.index, table) {
            if predicate.is_some() {
                continue;
            }
            let key: Vec<Option<&Value>> = set.iter().map(|c| row.get(c)).collect();
            if key.iter().any(Option::is_none)
                || (key.iter().any(|v| v.is_some_and(Value::is_null))
                    && !nulls_not_distinct.contains(&set))
            {
                continue;
            }
            if self.rows.get(table).is_some_and(|rs| {
                rs.iter()
                    .any(|r| set.iter().map(|c| r.get(c)).collect::<Vec<_>>() == key)
            }) {
                return Err(format!(
                    "a `{table}` row with ({}) = ({}) already exists",
                    set.join(", "),
                    key.iter()
                        .map(|v| v.map(Value::render).unwrap_or_default())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        *self.added.entry(table.to_owned()).or_default() += 1;
        self.rows.entry(table.to_owned()).or_default().push(row);
        Ok(())
    }

    /// Foreign keys of every row point at an existing row (the model does not
    /// enforce foreign keys; PostgreSQL does).
    fn fk_violations(&self, extra: &BTreeMap<String, Vec<Row>>) -> Vec<String> {
        let mut out = Vec::new();
        let all = |t: &str| -> Vec<&Row> {
            self.rows
                .get(t)
                .into_iter()
                .flatten()
                .chain(extra.get(t).into_iter().flatten())
                .collect()
        };
        for (table, rows) in self.rows.iter().chain(extra.iter()) {
            let Some(source) = self.source(table) else {
                continue;
            };
            for col in &source.columns {
                let Some((pb, pc)) = &col.references else {
                    continue;
                };
                let Some(pt) = self.table_of_binding(&source.file, pb) else {
                    continue;
                };
                for r in rows {
                    // Phase 12: a generated key references a row the
                    // execution itself inserted.
                    let Some(v) = r
                        .get(&col.property)
                        .filter(|v| !v.is_null() && !matches!(v, Value::Gen(_)))
                    else {
                        continue;
                    };
                    if !all(&pt).iter().any(|p| p.get(pc) == Some(v)) {
                        out.push(format!(
                            "`{table}.{}` = {} has no `{pt}.{pc}`",
                            col.property,
                            v.render()
                        ));
                    }
                }
            }
        }
        out
    }
}

fn model_ty(builder: &str) -> Ty {
    match builder {
        "serial" | "bigserial" | "smallserial" | "integer" | "smallint" | "bigint" => Ty::Int,
        "boolean" => Ty::Bool,
        _ => Ty::Text,
    }
}

/// A valid, readable UUID for the `n`-th synthesized row of `name`: the
/// first group is a stable hash of the name, the last the row number.
pub(crate) fn uuid_literal(name: &str, n: usize) -> Value {
    let h = midwrite_core::diagnostic::stable_hash(&[name]);
    Value::Str(format!(
        "{:08x}-0000-4000-a000-{:012}",
        h & 0xffff_ffff,
        n % 1_000_000_000_000
    ))
}

fn fill(builder: &str, property: &str, n: usize) -> Value {
    if builder == "uuid" {
        return uuid_literal(property, n);
    }
    // Phase 14: time and JSON columns (modeled under the `timestamps` and
    // `json` features) need values of their type: an explicit canonical
    // time (the text a Drizzle `Date` serializes to) and an empty object.
    match builder {
        "timestamp" => return Value::str("2026-01-01T00:00:00.000Z"),
        "date" => return Value::str("2026-01-01"),
        "json" | "jsonb" => return Value::Row(Row::new()),
        _ => {}
    }
    match model_ty(builder) {
        Ty::Int => Value::Int(10),
        Ty::Bool => Value::Bool(true),
        Ty::Text | Ty::Uuid | Ty::List { .. } => Value::Str(format!("{property}-{n}")),
    }
}

fn literal(l: &midwrite_model::schema_facts::SchemaLiteral) -> Value {
    use midwrite_model::schema_facts::SchemaLiteral;
    match l {
        SchemaLiteral::Integer(n) => Value::Int(*n),
        SchemaLiteral::Text(t) => Value::Str(t.clone()),
        SchemaLiteral::Boolean(b) => Value::Bool(*b),
        SchemaLiteral::Null => Value::Null,
    }
}

/// A row satisfying a closed single-table predicate, if the predicate is a
/// conjunction of comparisons of a column with a literal.
fn row_for(pred: &Expr) -> Result<Row, String> {
    let mut parts = Vec::new();
    fn split(e: &Expr, out: &mut Vec<Expr>) {
        match e {
            Expr::Bin(BinOp::And, a, b) => {
                split(a, out);
                split(b, out);
            }
            Expr::Lit(Value::Bool(true)) => {}
            other => out.push(other.clone()),
        }
    }
    split(pred, &mut parts);
    let mut row = Row::new();
    for p in parts {
        match &p {
            Expr::Bin(op, a, b) => {
                let (col, lit, flipped) = match (a.as_ref(), b.as_ref()) {
                    (Expr::Col(c), Expr::Lit(v)) => (c, v, false),
                    (Expr::Lit(v), Expr::Col(c)) => (c, v, true),
                    _ => {
                        return Err(format!(
                            "`{}` is not a column compared with a value",
                            p.render()
                        ));
                    }
                };
                let op = if flipped { flip(*op) } else { *op };
                let value = match (op, lit) {
                    (BinOp::Eq, v) => v.clone(),
                    (BinOp::Ge, Value::Int(n)) => Value::Int((*n).max(10)),
                    (BinOp::Gt, Value::Int(n)) => Value::Int((n + 1).max(10)),
                    (BinOp::Le, Value::Int(n)) => Value::Int(*n),
                    (BinOp::Lt, Value::Int(n)) => Value::Int(n - 1),
                    (BinOp::Ne, Value::Int(n)) => Value::Int(n + 1),
                    (BinOp::Ne, Value::Str(s)) => Value::Str(format!("{s}-other")),
                    (BinOp::Ne, Value::Bool(b)) => Value::Bool(!b),
                    _ => return Err(format!("cannot satisfy `{}`", p.render())),
                };
                let name = c_name(col);
                if let Some(previous) = row.get(&name)
                    && *previous != value
                {
                    return Err(format!(
                        "contradictory conditions on `{name}` ({} and {})",
                        previous.render(),
                        value.render()
                    ));
                }
                row.insert(name, value);
            }
            Expr::IsNull(c) => {
                if let Expr::Col(c) = c.as_ref() {
                    row.insert(c_name(c), Value::Null);
                } else {
                    return Err(format!("cannot satisfy `{}`", p.render()));
                }
            }
            other => return Err(format!("cannot satisfy `{}`", other.render())),
        }
    }
    Ok(row)
}

fn c_name(c: &str) -> String {
    c.rsplit('.').next().unwrap_or(c).to_owned()
}

fn flip(op: BinOp) -> BinOp {
    match op {
        BinOp::Lt => BinOp::Gt,
        BinOp::Le => BinOp::Ge,
        BinOp::Gt => BinOp::Lt,
        BinOp::Ge => BinOp::Le,
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Building a scenario for a candidate variant
// ---------------------------------------------------------------------------

/// Budgets that end up in a scenario's bounds.
#[derive(Clone, Copy, Debug)]
pub struct ScenarioBudget {
    pub max_states: u64,
    pub max_wall_ms: Option<u64>,
    pub max_memory_bytes: Option<u64>,
}

pub struct Built {
    pub plan: Plan,
    pub synthesis: SynthesisReport,
    /// Phase 15: what the actors' solo runs on the final rows observed.
    pub observations: crate::worlds::Observations,
}

fn table_closure(
    index: &ProjectIndex,
    tables: &BTreeSet<String>,
) -> Result<(Vec<String>, Vec<String>), String> {
    let mut names: BTreeSet<String> = tables.clone();
    let mut todo: Vec<String> = names.iter().cloned().collect();
    while let Some(t) = todo.pop() {
        let Some(source) = index.source_table(&t) else {
            continue;
        };
        let mut parents: Vec<String> = source
            .columns
            .iter()
            .filter_map(|c| c.references.as_ref().map(|(b, _)| b.clone()))
            .collect();
        parents.extend(source.foreign_keys.iter().map(|f| f.table.clone()));
        for b in parents {
            if let Some(m) = index.referenced_table(&source.file, &b)
                && names.insert(m.name.clone())
            {
                todo.push(m.name.clone());
            }
        }
    }
    // Phase 12: the closure may span several schema modules; every one is
    // listed in the scenario.
    let mut files = BTreeSet::new();
    let mut refs = Vec::new();
    for n in &names {
        let Some(m) = index.table_by_name(n) else {
            return Err(format!("table `{n}` is not modeled"));
        };
        files.insert(m.schema_file.clone());
        refs.push(index.from_ref(m));
    }
    if files.is_empty() {
        return Err("no table".to_owned());
    }
    Ok((parents_first(index, refs), files.into_iter().collect()))
}

/// Phase 12: synthesized rows may reach a table the scenario does not list
/// yet (a parent found while satisfying a read); extend the tables by the
/// closure of every table with rows, so the scenario loads.
fn cover_rows(index: &ProjectIndex, plan: &mut Plan) {
    let mut names: BTreeSet<String> = plan
        .tables
        .iter()
        .filter_map(|r| index.by_from(r).map(|t| t.name.clone()))
        .collect();
    let before = names.len();
    names.extend(
        plan.rows
            .iter()
            .filter(|(_, rows)| !rows.is_empty())
            .map(|(t, _)| t.clone()),
    );
    if names.len() == before {
        return;
    }
    if let Ok((tables, files)) = table_closure(index, &names) {
        plan.tables = tables;
        plan.schema_file = files[0].clone();
        plan.schema_files = files;
    }
}

/// Order table bindings so that every table comes after the tables its
/// foreign keys reference (a database seeds rows in this order), ties by
/// name.
fn parents_first(index: &ProjectIndex, mut bindings: Vec<String>) -> Vec<String> {
    bindings.sort();
    let parents = |b: &str| -> BTreeSet<String> {
        index
            .by_from(b)
            .and_then(|t| index.source_table(&t.name))
            .map(|st| {
                st.columns
                    .iter()
                    .filter_map(|c| c.references.as_ref().map(|(pb, _)| pb.clone()))
                    .chain(st.foreign_keys.iter().map(|f| f.table.clone()))
                    .filter_map(|pb| index.referenced_table(&st.file, &pb))
                    .map(|t| index.from_ref(t))
                    .filter(|pb| pb != b)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut out: Vec<String> = Vec::new();
    while !bindings.is_empty() {
        let pos = bindings
            .iter()
            .position(|b| {
                parents(b)
                    .iter()
                    .all(|p| out.contains(p) || !bindings.contains(p))
            })
            .unwrap_or(0);
        out.push(bindings.remove(pos));
    }
    out
}

/// Build the scenario of one candidate variant (rows, arguments, invariants).
pub fn build(
    c: &Candidate,
    variant: &Variant,
    analyzed: &[&Analyzed],
    index: &ProjectIndex,
    provider: &dyn ActorProvider,
    budget: ScenarioBudget,
) -> Result<Built, SynthesisReport> {
    build_world(
        c,
        variant,
        analyzed,
        index,
        provider,
        budget,
        &crate::worlds::WorldSpec::base(),
    )
}

/// Phase 15: [`build`] for one initial world: its argument overrides are
/// applied before rows are synthesized (rows then follow the arguments),
/// its cell overrides after every actor's first solo run (the rows the
/// actors need), and every actor must still reach the candidate's
/// operations alone on the final rows.
#[allow(clippy::too_many_arguments)]
pub fn build_world(
    c: &Candidate,
    variant: &Variant,
    analyzed: &[&Analyzed],
    index: &ProjectIndex,
    provider: &dyn ActorProvider,
    budget: ScenarioBudget,
    world: &crate::worlds::WorldSpec,
) -> Result<Built, SynthesisReport> {
    let fail = |status: &str, detail: String| SynthesisReport {
        status: status.to_owned(),
        detail,
        iterations: 0,
        rows: BTreeMap::new(),
        notes: Vec::new(),
    };
    // Tables: everything the actors touch, the property's table, parents.
    let mut touched = BTreeSet::new();
    for a in analyzed {
        for t in a.summary.tables() {
            touched.insert(t.to_owned());
        }
    }
    for p in &c.properties {
        match &p.spec {
            Spec::AtMostOne { table, .. }
            | Spec::AtLeast { table, .. }
            | Spec::NonNegative { table, .. }
            | Spec::SerialCell { table, .. } => {
                touched.insert(table.clone());
            }
            _ => {}
        }
    }
    if touched.is_empty() {
        // An actor without database effects (externals only) still needs a
        // table for the scenario to load: none is required by the loader.
    }
    let (tables, schema_files) = match table_closure(index, &touched) {
        Ok(x) => x,
        Err(e) => return Err(fail("unsupported", e)),
    };
    // Actors.
    let mut actors = Vec::new();
    for (k, &ai) in c.actor_entries.iter().enumerate() {
        let a = analyzed[ai];
        let name = format!(
            "{}{}",
            actor_base(a),
            if c.actor_entries.len() > 1 {
                format!("-{}", k + 1)
            } else {
                String::new()
            }
        );
        let (invocation, preconditions) = match &a.contract {
            Some(ct) if ct.invocation.is_some() => {
                (ct.invocation.clone(), ct.preconditions.clone())
            }
            _ => match a.invocation().as_str() {
                "wrapper-awaited-when-guards-pass" => {
                    let w = a.info.as_ref().and_then(|i| i.wrapper.as_ref());
                    (
                        Some("awaited".to_owned()),
                        vec![format!(
                            "SYNTHESIZED by discovery from the wrapper's source: the guards of `{}` pass ({})",
                            w.map(|w| w.wrapper.as_str()).unwrap_or("the wrapper"),
                            w.map(|w| w.guards.join("; ")).unwrap_or_default()
                        )],
                    )
                }
                "wrapper-awaited" => (Some("awaited".to_owned()), Vec::new()),
                "unknown" => {
                    return Err(fail(
                        "unsupported",
                        format!(
                            "how the wrapper of `{}` invokes the callback is unknown: declare `invocation` in midwrite-contracts.json",
                            a.fact.selector
                        ),
                    ));
                }
                _ => (None, Vec::new()),
            },
        };
        // With swapped arguments the second actor may take another branch to
        // its locks: it only has to run.
        let targets: Vec<String> = if k > 0 && variant.relation == Relation::Swapped {
            Vec::new()
        } else {
            c.conflict
                .iter()
                .filter(|o| c.actor_entries.get(k).is_some_and(|e| *e == o.entry))
                // Phase 12: the write a handler repeats after an unknown
                // outcome runs only under the fault the scenario explores;
                // a fault-free run alone cannot reach it.
                .filter(|o| o.role != "retried-write")
                .map(|o| o.location.clone())
                .collect()
        };
        let keep_empty: Vec<String> = c
            .conflict
            .iter()
            .filter(|o| {
                o.role == "check" && c.class == Class::CheckThenAct && c.actor_entries[k] == o.entry
            })
            .map(|o| o.location.clone())
            .collect();
        actors.push(ActorPlan {
            name,
            selector: a.fact.selector.clone(),
            invocation,
            preconditions,
            arguments: match arguments(c, a, index, variant.relation, k) {
                Ok(mut args) => {
                    for o in world.args.iter().filter(|o| o.actors.contains(&k)) {
                        if o.absent {
                            remove_path(&mut args, &a.summary.params, &o.path);
                        } else {
                            set_path(&mut args, &a.summary.params, &o.path, o.value.clone());
                        }
                    }
                    args
                }
                Err(why) => return Err(fail("unsupported", why)),
            },
            retry: c.environment.retry.clone(),
            response: c
                .environment
                .retry
                .as_deref()
                .is_some_and(|r| matches!(r, "on-crash" | "on-failure-or-crash")),
            targets,
            keep_empty,
            observed: BTreeMap::new(),
        });
    }
    let total_ops: usize = analyzed
        .iter()
        .map(|a| a.summary.codes.iter().map(Vec::len).sum::<usize>())
        .sum();
    let mut plan = Plan {
        name: String::new(),
        description: String::new(),
        schema_file: schema_files[0].clone(),
        schema_files,
        tables,
        unique_override: variant.unique_override.as_ref().map(|(t, cols)| {
            (
                t.clone(),
                cols.clone(),
                format!(
                    "HYPOTHETICAL ({}): not the application's schema",
                    variant.name
                ),
            )
        }),
        isolation: "read-committed".to_owned(),
        actors,
        rows: BTreeMap::new(),
        max_steps: (64usize).max(total_ops * 3 * (1 + c.environment.max_retries as usize)) as u32,
        max_states: budget.max_states,
        max_crashes: c.environment.max_crashes,
        // Transaction retries of the code's helpers use the retry bound.
        max_retries: analyzed
            .iter()
            .flat_map(|a| a.summary.transactions.iter().map(|t| t.retries))
            .max()
            .unwrap_or(0)
            .min(2)
            .max(c.environment.max_retries),
        max_in_flight: c.environment.max_in_flight,
        max_wall_ms: budget.max_wall_ms,
        max_memory_bytes: budget.max_memory_bytes,
        invariants: vec![Invariant {
            name: "synthesis".to_owned(),
            when: When::Final,
            description: String::new(),
            formula: Formula::Eq([Term::Int(0), Term::Int(0)]),
        }],
        source_verification: None,
        js_values: c.actor_entries.iter().any(|&ai| {
            uses_js_values(&analyzed[ai].summary.codes)
                // Phase 16: unrolled loops, inspected caught errors and
                // request bodies exist only with JavaScript values.
                || analyzed[ai].summary.assumptions.iter().any(|a| {
                    a.starts_with("loop ")
                        || a.starts_with("caught-errors: ")
                        || a.starts_with("request: ")
                })
        }),
        replica_reads: c.actor_entries.iter().any(|&ai| {
            analyzed[ai].summary.codes.iter().flatten().any(|s| {
                matches!(&s.op, midwrite_model::ir::Op::Db { stmt }
                    if stmt.conn == midwrite_model::ir::Conn::Replica)
            })
        }),
    };
    let mut rows = Rows {
        index,
        rows: BTreeMap::new(),
        added: BTreeMap::new(),
        notes: Vec::new(),
    };
    preseed_argument_rows(c, &plan, analyzed, &mut rows);
    preseed_written_rows(c, &plan, analyzed, &mut rows);
    // Phase 16: arguments validated by a zod schema were chosen to satisfy
    // it: stated, never presented as observed.
    for (k, actor) in plan.actors.iter().enumerate() {
        let Some(a) = c.actor_entries.get(k).map(|&ai| analyzed[ai]) else {
            continue;
        };
        for z in crate::domains::zod_inputs(a) {
            if let Some(v) = get_path(&actor.arguments, &a.summary.params, &z.path) {
                rows.notes.push(format!(
                    "SYNTHESIZED input of `{}`: `{}` = {} satisfies its zod {} schema `{}` (chosen by discovery to pass validation; not observed from callers)",
                    actor.selector,
                    z.path,
                    v.render(),
                    z.version.tag(),
                    midwrite_model::zodfn::render(&z.schema)
                ));
            }
        }
    }
    let mut iterations = 0;
    let mut observations = crate::worlds::Observations::default();
    let mut guard_fixes: BTreeSet<(String, String, String, Value)> = BTreeSet::new();
    // Each actor alone, rows accumulating; then every actor again on the
    // final rows.
    let order: Vec<usize> = (0..plan.actors.len()).chain(0..plan.actors.len()).collect();
    for (round, k) in order.into_iter().enumerate() {
        // Phase 15: the world's changes to the rows the actors need.
        if round == plan.actors.len()
            && !world.cells.is_empty()
            && let Err(e) = crate::worlds::apply_cells(&mut rows.rows, &world.cells)
        {
            return Err(SynthesisReport {
                status: "unsatisfiable".to_owned(),
                detail: format!("world `{}`: {e}", world.id),
                iterations,
                rows: rows.added.clone(),
                notes: rows.notes.clone(),
            });
        }
        let mut solo = plan.clone();
        solo.actors = vec![plan.actors[k].clone()];
        solo.max_crashes = 0;
        solo.max_retries = 0;
        solo.max_in_flight = 0;
        let final_round = round >= plan.actors.len();
        let mut reached = false;
        for _ in 0..MAX_SEED_ITERATIONS {
            iterations += 1;
            solo.rows = rows.rows.clone();
            cover_rows(index, &mut solo);
            let (model, _) = match solo.load(index, provider) {
                Ok(m) => m,
                Err(e) => {
                    return Err(SynthesisReport {
                        status: "unsatisfiable".to_owned(),
                        detail: format!("the synthesized scenario does not load: {e}"),
                        iterations,
                        rows: rows.added.clone(),
                        notes: rows.notes.clone(),
                    });
                }
            };
            // Phase 14: the loader must extract what discovery extracted
            // (connections, statements, transactions, sources); a scenario
            // whose re-extraction differs would check another program.
            if let Err(e) = round_trip(
                &analyzed[c.actor_entries[k]].summary.codes,
                &model.actors[0].codes,
            ) {
                return Err(SynthesisReport {
                    status: "unsupported".to_owned(),
                    detail: format!(
                        "scenario round-trip: the loader's extraction of actor `{}` differs from discovery's: {e}",
                        solo.actors[0].name
                    ),
                    iterations,
                    rows: rows.added.clone(),
                    notes: rows.notes.clone(),
                });
            }
            // Phase 15: the local operations before the first visible step
            // (branches included) run while the initial state settles.
            let mut settle = Recorder::on();
            let start = midwrite_model::engine::initial_state_recorded(&model, &mut settle);
            let run = run_alone(&model, start, 0);
            let (executed, state) = match run {
                Solo::Unsupported { detail, location } => {
                    return Err(SynthesisReport {
                        status: "unsupported".to_owned(),
                        detail: format!(
                            "actor `{}` reaches an operation the model does not support at {location}: {detail}",
                            solo.actors[0].name
                        ),
                        iterations,
                        rows: rows.added.clone(),
                        notes: rows.notes.clone(),
                    });
                }
                Solo::Finished { executed, state } => (executed, state),
            };
            // A target UPDATE/DELETE that changed no row did not exercise
            // the conflict.
            let done: BTreeSet<String> = executed
                .iter()
                .filter(|e| !e.actions.iter().any(changed_nothing))
                .map(|e| e.src.label())
                .collect();
            let targets = solo.actors[0].targets.clone();
            if targets.iter().all(|t| done.contains(t)) {
                // Rows the actor inserts need their parents too.
                let inserted = inserted_rows(&model, &executed);
                let missing = rows.fk_violations(&inserted);
                if missing.is_empty() {
                    reached = true;
                    let _ = state;
                    if final_round {
                        plan.actors[k].observed = observed_rows(&executed);
                        let selector = plan.actors[k].selector.clone();
                        let settled = branch_events(&settle.events);
                        let all = settled
                            .iter()
                            .chain(executed.iter().flat_map(|e| e.branches.iter()));
                        for (at, value) in all {
                            let b = observations
                                .branches
                                .entry((selector.clone(), at.clone()))
                                .or_default();
                            if *value {
                                b.1 = true;
                            } else {
                                b.0 = true;
                            }
                        }
                        for e in &executed {
                            observations
                                .reached
                                .entry(selector.clone())
                                .or_default()
                                .insert(e.src.label());
                        }
                    }
                    break;
                }
                let mut fixed = false;
                for (table, list) in &inserted {
                    let Some(source) = index.source_table(table) else {
                        continue;
                    };
                    for r in list {
                        for col in &source.columns {
                            let Some((pb, pc)) = &col.references else {
                                continue;
                            };
                            let Some(pt) = rows.table_of_binding(&source.file, pb) else {
                                continue;
                            };
                            let Some(v) = r.get(&col.property).filter(|v| !v.is_null()) else {
                                continue;
                            };
                            // Phase 12: a parent the execution inserts
                            // itself (its key generated, or taken from the
                            // request) is not initial data.
                            let inserted_parent = inserted
                                .get(&pt)
                                .is_some_and(|rs| rs.iter().any(|p| p.get(pc) == Some(v)));
                            if !inserted_parent
                                && !matches!(v, Value::Gen(_))
                                && !rows
                                    .rows
                                    .get(&pt)
                                    .is_some_and(|rs| rs.iter().any(|p| p.get(pc) == Some(v)))
                            {
                                let mut g = Row::new();
                                g.insert(pc.clone(), v.clone());
                                if rows.add(&pt, g, 0).is_ok() {
                                    fixed = true;
                                }
                            }
                        }
                    }
                }
                if !fixed {
                    return Err(SynthesisReport {
                        status: "unsatisfiable".to_owned(),
                        detail: format!(
                            "foreign keys of inserted rows cannot be satisfied: {}",
                            missing.join("; ")
                        ),
                        iterations,
                        rows: rows.added.clone(),
                        notes: rows.notes.clone(),
                    });
                }
                continue;
            }
            if final_round {
                return Err(SynthesisReport {
                    status: "unsatisfiable".to_owned(),
                    detail: format!(
                        "with the rows other actors need, actor `{}` no longer reaches {}",
                        solo.actors[0].name,
                        targets.join(", ")
                    ),
                    iterations,
                    rows: rows.added.clone(),
                    notes: rows.notes.clone(),
                });
            }
            // Find the last read that came back empty and is not required
            // to stay empty.
            let seeded = seed_from(&model, &executed, &solo.actors[0].keep_empty, &mut rows)
                .or_else(|why| {
                    // Phase 15: every read found a row; a guard over a read
                    // row may have stopped the actor.
                    satisfy_guard(
                        &model,
                        &settle.events,
                        &executed,
                        &mut rows,
                        &mut guard_fixes,
                        &solo.actors[0].name,
                    )
                    .map_err(|g| {
                        if g.is_empty() {
                            why
                        } else {
                            format!("{why}; {g}")
                        }
                    })
                });
            // Phase 15: a guard over an argument (`actor.kind === "staff"`).
            let seeded = match seeded {
                Ok(()) => Ok(()),
                Err(why) => {
                    let a = analyzed[c.actor_entries[k]];
                    let fixed: BTreeSet<String> = a
                        .contract
                        .as_ref()
                        .map(|ct| {
                            if ct.arguments.is_some() {
                                a.summary.params.to_vec()
                            } else {
                                ct.values.keys().cloned().collect::<Vec<_>>()
                            }
                        })
                        .unwrap_or_default()
                        .into_iter()
                        .chain(world.args.iter().map(|o| o.path.clone()))
                        .collect();
                    match argument_guard(
                        &model,
                        &settle.events,
                        &executed,
                        &a.summary.params,
                        &solo.actors[0].arguments,
                        &fixed,
                        &mut guard_fixes,
                    ) {
                        Some((path, value, at)) => {
                            let selector = plan.actors[k].selector.clone();
                            for actor in plan.actors.iter_mut().filter(|x| x.selector == selector) {
                                set_path(
                                    &mut actor.arguments,
                                    &a.summary.params,
                                    &path,
                                    value.clone(),
                                );
                            }
                            solo.actors[0].arguments = plan.actors[k].arguments.clone();
                            rows.notes.push(format!(
                                "SYNTHESIZED precondition of `{selector}`: argument `{path}` = {}, so that the guard at {at} lets the call continue (the scenario explores callers that satisfy this guard; it is not assumed of every caller)",
                                value.render()
                            ));
                            Ok(())
                        }
                        None => Err(why),
                    }
                }
            };
            match seeded {
                Ok(()) => continue,
                Err(why) => {
                    return Err(SynthesisReport {
                        status: "unsatisfiable".to_owned(),
                        detail: format!(
                            "actor `{}` does not reach {} and no row can be added: {why}{}",
                            solo.actors[0].name,
                            targets.join(", "),
                            stopped_at(&executed, &state)
                        ),
                        iterations,
                        rows: rows.added.clone(),
                        notes: rows.notes.clone(),
                    });
                }
            }
        }
        if !reached {
            return Err(SynthesisReport {
                status: "unsatisfiable".to_owned(),
                detail: format!(
                    "actor `{}` does not reach the candidate's operations within {MAX_SEED_ITERATIONS} iterations",
                    plan.actors[k].name
                ),
                iterations,
                rows: rows.added.clone(),
                notes: rows.notes.clone(),
            });
        }
    }
    plan.rows = rows.rows.clone();
    cover_rows(index, &mut plan);
    let synthesis = SynthesisReport {
        status: "satisfied".to_owned(),
        detail: format!(
            "every actor alone reaches the candidate's operations from {} synthesized row(s)",
            rows.added.values().sum::<usize>()
        ),
        iterations,
        rows: rows.added.clone(),
        notes: rows.notes.clone(),
    };
    Ok(Built {
        plan,
        synthesis,
        observations,
    })
}

fn actor_base(a: &Analyzed) -> String {
    let name = a.fact.name.split('(').next().unwrap_or("actor");
    name.replace(['.', '/', ' '], "-")
}

/// Rows the run inserted, per table.
fn inserted_rows(model: &Model, executed: &[Executed]) -> BTreeMap<String, Vec<Row>> {
    let mut out: BTreeMap<String, Vec<Row>> = BTreeMap::new();
    for e in executed {
        for a in &e.actions {
            match a {
                Action::Stmt {
                    stmt: ClosedStmt::Insert { table, values, .. },
                    ..
                } => {
                    out.entry(model.schema.table(*table).name.clone())
                        .or_default()
                        .push(values.clone());
                }
                // Phase 14: every row of a multi-row INSERT.
                Action::Stmt {
                    stmt: ClosedStmt::InsertMany { table, rows, .. },
                    ..
                } => {
                    out.entry(model.schema.table(*table).name.clone())
                        .or_default()
                        .extend(rows.iter().cloned());
                }
                _ => {}
            }
        }
    }
    out
}

fn changed_nothing(a: &Action) -> bool {
    use midwrite_model::engine::Expect;
    use midwrite_model::exec::StmtResult;
    matches!(
        a,
        Action::Stmt {
            stmt: ClosedStmt::Update { .. } | ClosedStmt::Delete { .. },
            expect: Expect::Done(StmtResult::Affected(0)),
            ..
        }
    ) || matches!(
        // Phase 16: a multi-row lock that locked no row.
        a,
        Action::Stmt {
            stmt: ClosedStmt::SelectMany { lock, .. },
            expect: Expect::Done(StmtResult::Rows(rows)),
            ..
        } if *lock != midwrite_model::ir::LockMode::None && rows.is_empty()
    )
}

/// The equality conditions of the last read of `table` executed before
/// `before`.
fn preceding_read(executed: &[Executed], before: &Executed, table: u16) -> Option<Row> {
    let at = executed.iter().position(|e| std::ptr::eq(e, before))?;
    executed[..at].iter().rev().find_map(|e| {
        e.actions.iter().find_map(|a| match a {
            Action::Stmt {
                stmt:
                    ClosedStmt::Select {
                        table: t,
                        pred,
                        joins,
                        ..
                    },
                ..
            } if *t == table && joins.is_empty() => eq_literals(pred),
            Action::Stmt {
                stmt: ClosedStmt::Count { table: t, pred },
                ..
            } if *t == table => eq_literals(pred),
            _ => None,
        })
    })
}

/// `column = literal` conjuncts of a closed predicate.
fn eq_literals(pred: &Expr) -> Option<Row> {
    let mut out = Row::new();
    let mut todo = vec![pred];
    while let Some(e) = todo.pop() {
        match e {
            Expr::Bin(BinOp::And, a, b) => {
                todo.push(a);
                todo.push(b);
            }
            Expr::Bin(BinOp::Eq, a, b) => {
                if let (Expr::Col(c), Expr::Lit(v)) | (Expr::Lit(v), Expr::Col(c)) =
                    (a.as_ref(), b.as_ref())
                {
                    out.insert(c_name(c), v.clone());
                }
            }
            _ => {}
        }
    }
    Some(out)
}

/// An object argument whose fields are columns of one table, including its
/// single-column primary key, is taken to be that table's row (the wrapper
/// of a Server Action passes the signed-in member it loaded): the row is
/// seeded with the same values. Recorded as a note; a different table or no
/// table makes no row.
fn preseed_argument_rows(c: &Candidate, plan: &Plan, analyzed: &[&Analyzed], rows: &mut Rows<'_>) {
    for (k, actor) in plan.actors.iter().enumerate() {
        let a = analyzed[c.actor_entries[k]];
        for (param, value) in a.summary.params.iter().zip(&actor.arguments) {
            let Value::Row(fields) = value else { continue };
            let matches: Vec<&crate::project::ModeledTable> = rows
                .index
                .tables
                .iter()
                .filter(|t| {
                    let pk: Vec<String> = crate::detect::unique_sets(rows.index, &t.name)
                        .into_iter()
                        .next()
                        .map(|(k, _)| k)
                        .unwrap_or_default();
                    pk.len() == 1
                        && fields.contains_key(&pk[0])
                        && fields.keys().all(|f| t.columns.contains(f))
                })
                .collect();
            // Source evidence first: the argument's key is written to or
            // compared with a column whose foreign key names the table.
            let by_fk: Vec<&crate::project::ModeledTable> = matches
                .iter()
                .copied()
                .filter(|t| {
                    let pk = crate::detect::unique_sets(rows.index, &t.name)
                        .into_iter()
                        .next()
                        .map(|(k, _)| k)
                        .unwrap_or_default();
                    let path = format!("{param}.{}", pk[0]);
                    a.summary.db.iter().any(|e| {
                        e.eq.iter().chain(e.writes.iter()).any(|(col, v)| {
                            matches!(v, Val::Param { path: p } if *p == path)
                                && rows.index.source_table(&e.table).is_some_and(|st| {
                                    st.column(col).is_some_and(|sc| {
                                        sc.references
                                            .as_ref()
                                            .is_some_and(|(b, pc)| *b == t.binding && *pc == pk[0])
                                    })
                                })
                        })
                    })
                })
                .collect();
            let chosen = if by_fk.len() == 1 { by_fk } else { matches };
            let [table] = chosen.as_slice() else { continue };
            let pk = crate::detect::unique_sets(rows.index, &table.name)
                .into_iter()
                .next()
                .map(|(k, _)| k)
                .unwrap_or_default();
            let exists = rows
                .rows
                .get(&table.name)
                .is_some_and(|rs| rs.iter().any(|r| r.get(&pk[0]) == fields.get(&pk[0])));
            if exists {
                continue;
            }
            let table_name = table.name.clone();
            if rows.add(&table_name, fields.clone(), 0).is_ok() {
                rows.notes.push(format!(
                    "argument `{param}` of `{}` is taken to be the `{table_name}` row with {} = {} (its fields are that table's columns)",
                    actor.name,
                    pk[0],
                    fields.get(&pk[0]).map(Value::render).unwrap_or_default()
                ));
            }
        }
    }
}

/// Rows the candidate's target UPDATE/DELETE statements write, computed
/// from each actor's arguments (with the equality conditions of the read
/// they follow): the rows the conflict is about.
fn preseed_written_rows(c: &Candidate, plan: &Plan, analyzed: &[&Analyzed], rows: &mut Rows<'_>) {
    use crate::effects::Access;
    for (k, actor) in plan.actors.iter().enumerate() {
        let entry = c.actor_entries[k];
        let a = analyzed[entry];
        let s = &a.summary;
        let ops: Vec<&crate::candidate::ConflictOp> = c
            .conflict
            .iter()
            .filter(|o| o.entry == entry && !o.is_external)
            .collect();
        let read = ops
            .iter()
            .map(|o| &s.db[o.effect])
            .find(|e| matches!(e.access, Access::Read | Access::Count));
        for op in &ops {
            let e = &s.db[op.effect];
            if !matches!(e.access, Access::Update | Access::Delete) {
                continue;
            }
            let mut row = Row::new();
            let mut ok = true;
            for (col, v) in &e.eq {
                match val_value(&crate::detect::canon(v, s), actor, s) {
                    Some(x) => {
                        row.insert(col.clone(), x);
                    }
                    None => ok = false,
                }
            }
            if !ok || row.is_empty() {
                continue;
            }
            if let Some(r) = read.filter(|r| r.table == e.table) {
                for (col, v) in &r.eq {
                    if !row.contains_key(col)
                        && let Some(x) = val_value(&crate::detect::canon(v, s), actor, s)
                    {
                        row.insert(col.clone(), x);
                    }
                }
            }
            let exists = rows.rows.get(&e.table).is_some_and(|rs| {
                rs.iter()
                    .any(|r| e.eq.iter().all(|(col, _)| r.get(col) == row.get(col)))
            });
            if !exists && rows.add(&e.table, row, 0).is_err() {
                rows.notes.push(format!(
                    "the row written at {} could not be pre-seeded",
                    e.src.label()
                ));
            }
        }
    }
}

/// `table.column = literal` conjuncts of a closed predicate, qualified.
fn qualified_literals(pred: &Expr) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let mut todo = vec![pred];
    while let Some(e) = todo.pop() {
        match e {
            Expr::Bin(BinOp::And, a, b) => {
                todo.push(a);
                todo.push(b);
            }
            Expr::Bin(BinOp::Eq, a, b) => {
                if let (Expr::Col(c), Expr::Lit(v)) | (Expr::Lit(v), Expr::Col(c)) =
                    (a.as_ref(), b.as_ref())
                {
                    out.push((c.clone(), v.clone()));
                }
            }
            _ => {}
        }
    }
    out
}

/// For a joined read: a missing base row, or a missing row of a joined table
/// that the join condition (`base.col = joined.col`) and the read's
/// conditions on the joined table describe. `None` when nothing is missing.
fn join_seed(
    model: &Model,
    table: u16,
    pred: &Expr,
    joins: &[midwrite_model::state::ClosedJoin],
    rows: &mut Rows<'_>,
) -> Option<Result<(), String>> {
    let base = model.schema.table(table).name.clone();
    let conds = qualified_literals(pred);
    let on = |t: &str| -> Row {
        conds
            .iter()
            .filter_map(|(c, v)| {
                c.strip_prefix(&format!("{t}."))
                    .map(|col| (col.to_owned(), v.clone()))
            })
            .collect()
    };
    let base_conds = on(&base);
    let matching: Vec<Row> = rows
        .rows
        .get(&base)
        .into_iter()
        .flatten()
        .filter(|r| base_conds.iter().all(|(k, v)| r.get(k) == Some(v)))
        .cloned()
        .collect();
    if matching.is_empty() {
        if base_conds.is_empty() {
            return None;
        }
        return Some(rows.add(&base, base_conds, 0));
    }
    for row in &matching {
        for j in joins {
            let joined = model.schema.table(j.table).name.clone();
            let split = |q: &str| q.split_once('.').map(|(t, c)| (t.to_owned(), c.to_owned()));
            let (Some((lt, lc)), Some((rt, rc))) = (split(&j.left), split(&j.right)) else {
                continue;
            };
            let (base_col, joined_col) = if rt == joined && lt == base {
                (lc, rc)
            } else if lt == joined && rt == base {
                (rc, lc)
            } else {
                continue;
            };
            let Some(value) = row.get(&base_col).filter(|v| !v.is_null()) else {
                continue;
            };
            let present = rows
                .rows
                .get(&joined)
                .is_some_and(|rs| rs.iter().any(|r| r.get(&joined_col) == Some(value)));
            if present {
                continue;
            }
            let mut given = on(&joined);
            given.insert(joined_col, value.clone());
            return Some(rows.add(&joined, given, 0));
        }
    }
    None
}

fn seed_from(
    model: &Model,
    executed: &[Executed],
    keep_empty: &[String],
    rows: &mut Rows<'_>,
) -> Result<(), String> {
    use midwrite_model::engine::Expect;
    use midwrite_model::exec::StmtResult;
    for e in executed.iter().rev() {
        let label = e.src.label();
        for a in &e.actions {
            let Action::Stmt { stmt, expect, .. } = a else {
                continue;
            };
            let (table, pred, empty) = match (stmt, expect) {
                (
                    ClosedStmt::Select {
                        table, pred, joins, ..
                    },
                    Expect::Done(StmtResult::Row(r)),
                ) => {
                    if !joins.is_empty() {
                        if keep_empty.contains(&label) {
                            continue;
                        }
                        match join_seed(model, *table, pred, joins, rows) {
                            Some(Ok(())) => return Ok(()),
                            Some(Err(why)) => return Err(format!("the read at {label}: {why}")),
                            None => {}
                        }
                        if r.is_none() {
                            return Err(format!(
                                "the read at {label} joins tables and no row of its tables can be added"
                            ));
                        }
                        continue;
                    }
                    (*table, pred, r.is_none())
                }
                // Phase 16: a multi-row select that returned nothing.
                (ClosedStmt::SelectMany { table, pred, .. }, Expect::Done(StmtResult::Rows(r))) => {
                    (*table, pred, r.is_empty())
                }
                (ClosedStmt::Count { table, pred }, Expect::Done(StmtResult::Count(n))) => {
                    (*table, pred, *n < 3)
                }
                (ClosedStmt::Update { table, pred, .. }, Expect::Done(StmtResult::Affected(0))) => {
                    (*table, pred, true)
                }
                _ => continue,
            };
            if !empty || keep_empty.contains(&label) {
                continue;
            }
            let name = model.schema.table(table).name.clone();
            let mut given = row_for(pred).map_err(|why| format!("the read at {label}: {why}"))?;
            if matches!(stmt, ClosedStmt::Update { .. }) {
                // The written row satisfies what the code checked before.
                if let Some(read) = preceding_read(executed, e, table) {
                    for (k, v) in read {
                        given.entry(k).or_insert(v);
                    }
                }
            }
            return rows
                .add(&name, given, 0)
                .map_err(|why| format!("the read at {label}: {why}"));
        }
    }
    Err("no read on the executed path came back empty".to_owned())
}

/// Phase 15: a guard the run evaluated that compares an argument path with
/// a literal its synthesized value differs from (`actor.kind === "staff"`
/// with `kind = "kind-1"`): the path, the literal, and the guard's location.
/// Paths in `fixed` (declared, or chosen by the world) are left alone; each
/// (path, value) is tried once.
fn argument_guard(
    model: &Model,
    settled: &[midwrite_model::engine::Event],
    executed: &[Executed],
    params: &[String],
    args: &[Value],
    fixed: &BTreeSet<String>,
    tried: &mut BTreeSet<(String, String, String, Value)>,
) -> Option<(String, Value, String)> {
    let evaluated: BTreeSet<String> = branch_events(settled)
        .into_iter()
        .chain(executed.iter().flat_map(|e| e.branches.iter().cloned()))
        .map(|(at, _)| at)
        .collect();
    // Locals defined exactly once, over every code vector (inlined
    // callees bind their parameters to such locals).
    let mut defs: BTreeMap<&str, Vec<&Expr>> = BTreeMap::new();
    for code in &model.actors[0].codes {
        for st in code {
            if let Op::Let { local, expr } = &st.op {
                defs.entry(local.as_str()).or_default().push(expr);
            }
        }
    }
    type Defs<'a> = BTreeMap<&'a str, Vec<&'a Expr>>;
    fn single<'a>(defs: &Defs<'a>, l: &str) -> Option<&'a Expr> {
        match defs.get(l).map(Vec::as_slice) {
            Some([one]) => Some(*one),
            _ => None,
        }
    }
    // A parameter path, through such locals (`isSchoolAdmin$actor.kind`).
    fn path_through(e: &Expr, params: &[String], defs: &Defs<'_>, depth: u32) -> Option<String> {
        if let Some(p) = path_of(e, params) {
            return Some(p);
        }
        if depth > 6 {
            return None;
        }
        if let Some((base, f)) = e.as_property() {
            return path_through(base, params, defs, depth + 1).map(|p| format!("{p}.{f}"));
        }
        match e {
            Expr::Local(l) => path_through(single(defs, l)?, params, defs, depth + 1),
            _ => None,
        }
    }
    fn walk(
        e: &Expr,
        params: &[String],
        defs: &Defs<'_>,
        depth: u32,
        out: &mut Vec<(String, Value)>,
    ) {
        if depth > 6 {
            return;
        }
        match e {
            Expr::Bin(BinOp::Eq | BinOp::Ne, a, b) => {
                if let (x, Expr::Lit(v)) | (Expr::Lit(v), x) = (a.as_ref(), b.as_ref())
                    && let Some(p) = path_through(x, params, defs, 0)
                    && matches!(v, Value::Str(_) | Value::Int(_) | Value::Bool(_))
                {
                    out.push((p, v.clone()));
                }
                walk(a, params, defs, depth, out);
                walk(b, params, defs, depth, out);
            }
            Expr::Bin(_, a, b) => {
                walk(a, params, defs, depth, out);
                walk(b, params, defs, depth, out);
            }
            Expr::Not(x) | Expr::Field(x, _) | Expr::IsNull(x) => {
                walk(x, params, defs, depth, out);
            }
            // A local holding a condition (`ok = a && b`, lowered as
            // several assignments): every definition.
            Expr::Local(l) => {
                for def in defs.get(l.as_str()).into_iter().flatten() {
                    walk(def, params, defs, depth + 1, out);
                }
            }
            // A membership guard (`roles.includes("admin")`): the element.
            Expr::Call {
                func: Builtin::Includes,
                args,
            } => {
                if let [list, Expr::Lit(v @ (Value::Str(_) | Value::Int(_)))] = args.as_slice()
                    && let Some(p) = path_through(list, params, defs, 0)
                {
                    out.push((p, Value::List(vec![v.clone()])));
                }
            }
            _ => {}
        }
    }
    for code in &model.actors[0].codes {
        for step in code {
            let Op::JumpIf { cond, .. } = &step.op else {
                continue;
            };
            let at = step.src.label();
            if !evaluated.contains(&at) {
                continue;
            }
            let mut tests = Vec::new();
            walk(cond, params, &defs, 0, &mut tests);
            for (path, literal) in tests {
                if fixed
                    .iter()
                    .any(|f| path == *f || path.starts_with(&format!("{f}.")))
                {
                    continue;
                }
                let current = get_path(args, params, &path);
                if current.as_ref() == Some(&literal) {
                    continue;
                }
                // A list that already holds the element passes.
                if let (Some(Value::List(have)), Value::List(want)) = (&current, &literal)
                    && want.iter().all(|w| have.contains(w))
                {
                    continue;
                }
                if !tried.insert((
                    "argument".to_owned(),
                    path.clone(),
                    String::new(),
                    literal.clone(),
                )) {
                    continue;
                }
                return Some((path, literal, at));
            }
        }
    }
    None
}

/// Phase 15: when an actor stops at a guard that compares a property of a
/// row it read with a literal (`membership.role !== "admin"`), set that
/// column of the stored row to the value that lets the guard pass, once per
/// (table, column, value). The requirement is recorded as a synthesized
/// precondition: the scenario explores callers who satisfy it, it is never
/// assumed of an arbitrary caller. Returns why nothing could be changed
/// (empty: no such guard).
fn satisfy_guard(
    model: &Model,
    settled: &[midwrite_model::engine::Event],
    executed: &[Executed],
    rows: &mut Rows<'_>,
    tried: &mut BTreeSet<(String, String, String, Value)>,
    actor: &str,
) -> Result<(), String> {
    use midwrite_model::engine::Expect;
    use midwrite_model::exec::StmtResult;
    // Branches the run evaluated, by location.
    let evaluated: BTreeSet<String> = branch_events(settled)
        .into_iter()
        .chain(executed.iter().flat_map(|e| e.branches.iter().cloned()))
        .map(|(at, _)| at)
        .collect();
    // (branch location, property, candidate values that may pass it).
    let mut tests: Vec<(String, String, Vec<Value>)> = Vec::new();
    fn walk(e: &Expr, at: &str, out: &mut Vec<(String, String, Vec<Value>)>) {
        match e {
            Expr::Bin(
                op @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge),
                a,
                b,
            ) => {
                let pair = match (a.as_ref(), b.as_ref()) {
                    (x, Expr::Lit(v)) | (Expr::Lit(v), x) => {
                        x.as_property().map(|(_, f)| (f.to_owned(), v.clone()))
                    }
                    _ => None,
                };
                match (op, pair) {
                    (BinOp::Eq | BinOp::Ne, Some((f, v)))
                        if matches!(v, Value::Str(_) | Value::Int(_) | Value::Bool(_)) =>
                    {
                        out.push((at.to_owned(), f, vec![v]));
                    }
                    // An integer ordering: the values around the operand.
                    (_, Some((f, Value::Int(n)))) => {
                        let vs = [n.checked_add(1), Some(n), n.checked_sub(1)]
                            .into_iter()
                            .flatten()
                            .map(Value::Int)
                            .collect();
                        out.push((at.to_owned(), f, vs));
                    }
                    _ => {}
                }
                walk(a, at, out);
                walk(b, at, out);
            }
            Expr::Bin(_, a, b) => {
                walk(a, at, out);
                walk(b, at, out);
            }
            Expr::Not(x) | Expr::Field(x, _) | Expr::IsNull(x) => walk(x, at, out),
            _ => {}
        }
    }
    fn locals_of(e: &Expr, out: &mut Vec<String>) {
        match e {
            Expr::Local(l) => out.push(l.clone()),
            Expr::Bin(_, a, b) => {
                locals_of(a, out);
                locals_of(b, out);
            }
            Expr::Not(x) | Expr::Field(x, _) | Expr::IsNull(x) => locals_of(x, out),
            _ => {}
        }
    }
    for code in &model.actors[0].codes {
        for step in code {
            if let Op::JumpIf { cond, .. } = &step.op {
                let at = step.src.label();
                if evaluated.contains(&at) {
                    walk(cond, &at, &mut tests);
                    // The locals the condition tests (`allowed = x > 0`).
                    let mut locals = Vec::new();
                    locals_of(cond, &mut locals);
                    for st in code {
                        if let Op::Let { local, expr } = &st.op
                            && locals.contains(local)
                        {
                            walk(expr, &at, &mut tests);
                        }
                    }
                }
            }
        }
    }
    if tests.is_empty() {
        return Err(String::new());
    }
    // Rows the run read, latest first.
    for e in executed.iter().rev() {
        for a in &e.actions {
            let Action::Stmt {
                stmt:
                    ClosedStmt::Select {
                        table, pred, joins, ..
                    },
                expect: Expect::Done(StmtResult::Row(Some(_))),
                ..
            } = a
            else {
                continue;
            };
            if !joins.is_empty() {
                continue;
            }
            let name = model.schema.table(*table).name.clone();
            let Some(key) = eq_literals(pred) else {
                continue;
            };
            for (at, column, values) in &tests {
                if crate::worlds::identity_column(rows.index, &name, column) {
                    continue;
                }
                for literal in values {
                    let Some(matching) = rows.rows.get_mut(&name) else {
                        continue;
                    };
                    let hits: Vec<usize> = matching
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| key.iter().all(|(k, v)| r.get(k) == Some(v)))
                        .map(|(i, _)| i)
                        .collect();
                    let Some(&first) = hits.first() else { continue };
                    let Some(current) = matching[first].get(column).cloned() else {
                        continue;
                    };
                    let value = if current == *literal {
                        if values.len() > 1 {
                            continue;
                        }
                        match crate::worlds::distinct_value(rows.index, &name, column, literal) {
                            Some(v) => v,
                            None => continue,
                        }
                    } else {
                        literal.clone()
                    };
                    let row_key = Value::Row(key.clone()).render();
                    if !tried.insert((name.clone(), row_key, column.clone(), value.clone())) {
                        continue;
                    }
                    let matching = rows.rows.get_mut(&name).expect("present");
                    for i in hits {
                        matching[i].insert(column.clone(), value.clone());
                    }
                    rows.notes.push(format!(
                    "SYNTHESIZED precondition of `{actor}`: the `{name}` row its read at {} returns has `{column}` = {}, so that the guard at {at} lets it continue (the scenario explores callers that satisfy this guard; it is not assumed of every caller)",
                    e.src.label(),
                    value.render()
                ));
                    return Ok(());
                }
            }
        }
    }
    Err("no stored row read by the actor can be changed to pass its guards".to_owned())
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

fn val_value(v: &Val, actor: &ActorPlan, s: &crate::effects::EffectSummary) -> Option<Value> {
    match v {
        Val::Param { path } => get_path(&actor.arguments, &s.params, path),
        Val::Lit { value } => Some(value.clone()),
        // A value the actor read: what the read returned in the actor's
        // solo run on the synthesized rows.
        Val::Read { effect, column } => {
            let row = actor.observed.get(&s.db.get(*effect)?.src.label())?;
            let mut parts = column.split('.');
            let mut cur = row.get(parts.next()?)?;
            for part in parts {
                let Value::Row(r) = cur else { return None };
                cur = r.get(part)?;
            }
            (!cur.is_null()).then(|| cur.clone())
        }
        _ => None,
    }
}

/// Rows the run's reads returned, by source location.
fn observed_rows(executed: &[Executed]) -> BTreeMap<String, Row> {
    use midwrite_model::engine::Expect;
    use midwrite_model::exec::StmtResult;
    let mut out = BTreeMap::new();
    for e in executed {
        for a in &e.actions {
            if let Action::Stmt {
                stmt: ClosedStmt::Select { .. },
                expect: Expect::Done(StmtResult::Row(Some(row))),
                ..
            } = a
            {
                out.insert(e.src.label(), row.clone());
            }
        }
    }
    out
}

fn count_le_one(table: &str, filter: BTreeMap<String, Value>) -> Formula {
    Formula::Le([
        Term::Op(TermOp::Count(RowRef {
            table: table.to_owned(),
            filter,
            column: None,
        })),
        Term::Int(1),
    ])
}

/// Turn the candidate's properties into invariants for a built plan.
pub fn invariants(
    c: &Candidate,
    plan: &Plan,
    relation: Relation,
    analyzed: &[&Analyzed],
    index: &ProjectIndex,
    provider: &dyn ActorProvider,
) -> (Vec<(Property, Invariant)>, Vec<String>) {
    let mut out = Vec::new();
    let mut notes = Vec::new();
    for p in &c.properties {
        let describe = |extra: &str| format!("[{}] {}{}", p.origin.as_str(), p.description, extra);
        let mut renamed_generated = false;
        let mut replica_serial = false;
        let formula = match &p.spec {
            Spec::AtMostOne { table, key, filter } => {
                let mut parts = Vec::new();
                let mut seen = BTreeSet::new();
                for (k, actor) in plan.actors.iter().enumerate() {
                    let a = analyzed[c.actor_entries[k]];
                    let mut f = filter.clone();
                    let mut ok = true;
                    for (col, v) in key {
                        match val_value(v, actor, &a.summary) {
                            Some(x) => {
                                f.insert(col.clone(), x);
                            }
                            None => ok = false,
                        }
                    }
                    // A partner's key is computed by its own code: only the
                    // first actor's key is known to be the shared one.
                    if !ok {
                        if k == 0 {
                            notes.push(format!(
                                "`{}`: the key is not computable from the arguments",
                                p.name
                            ));
                        }
                        continue;
                    }
                    if seen.insert(format!("{f:?}")) {
                        parts.push(count_le_one(table, f));
                    }
                }
                match parts.len() {
                    0 => continue,
                    1 => parts.pop().expect("one"),
                    _ => Formula::And(parts),
                }
            }
            Spec::AtLeast {
                table,
                group_by,
                filter,
                min,
            } => {
                let groups: BTreeSet<Vec<Value>> = plan
                    .rows
                    .get(table)
                    .into_iter()
                    .flatten()
                    .map(|r| {
                        group_by
                            .iter()
                            .map(|g| r.get(g).cloned().unwrap_or(Value::Null))
                            .collect()
                    })
                    .collect();
                let mut parts = Vec::new();
                for g in groups {
                    let mut f = filter.clone();
                    for (col, v) in group_by.iter().zip(g) {
                        f.insert(col.clone(), v);
                    }
                    let initial = plan
                        .rows
                        .get(table)
                        .into_iter()
                        .flatten()
                        .filter(|r| f.iter().all(|(k, v)| r.get(k) == Some(v)))
                        .count() as i64;
                    if initial < *min {
                        notes.push(format!(
                            "`{}`: group {f:?} violates the invariant in the initial state; not checked",
                            p.name
                        ));
                        continue;
                    }
                    parts.push(Formula::Ge([
                        Term::Op(TermOp::Count(RowRef {
                            table: table.clone(),
                            filter: f,
                            column: None,
                        })),
                        Term::Int(*min),
                    ]));
                }
                match parts.len() {
                    0 => continue,
                    1 => parts.pop().expect("one"),
                    _ => Formula::And(parts),
                }
            }
            Spec::NonNegative { table, column } => {
                let pk = crate::detect::unique_sets(index, table)
                    .into_iter()
                    .next()
                    .map(|(k, _)| k)
                    .unwrap_or_default();
                let parts: Vec<Formula> = plan
                    .rows
                    .get(table)
                    .into_iter()
                    .flatten()
                    .map(|r| {
                        let filter = pk
                            .iter()
                            .map(|k| (k.clone(), r.get(k).cloned().unwrap_or(Value::Null)))
                            .collect();
                        Formula::Ge([
                            Term::Op(TermOp::Row(RowRef {
                                table: table.clone(),
                                filter,
                                column: Some(column.clone()),
                            })),
                            Term::Int(0),
                        ])
                    })
                    .collect();
                match parts.len() {
                    0 => continue,
                    1 => parts.into_iter().next().expect("one"),
                    _ => Formula::And(parts),
                }
            }
            Spec::MaxExternal {
                channel,
                max,
                per_request,
            } => Formula::Le([
                Term::Op(TermOp::Externals(ExternalsRef {
                    channel: channel.clone(),
                    key: None,
                    distinct_keys: false,
                })),
                // In a control every actor has its own key, and a
                // per-request bound holds per actor.
                Term::Int(if relation == Relation::DistinctKey || *per_request {
                    *max * plan.actors.len() as i64
                } else {
                    *max
                }),
            ]),
            Spec::NoOrphan {
                child,
                column,
                parent,
                parent_column,
                key,
            } => {
                let a = analyzed[c.actor_entries[0]];
                let Some(k) = val_value(key, &plan.actors[0], &a.summary) else {
                    notes.push(format!(
                        "`{}`: the key is not computable from the arguments",
                        p.name
                    ));
                    continue;
                };
                let count = |table: &str, col: &str| {
                    Term::Op(TermOp::Count(RowRef {
                        table: table.to_owned(),
                        filter: BTreeMap::from([(col.to_owned(), k.clone())]),
                        column: None,
                    }))
                };
                Formula::Implies(
                    Box::new(Formula::Gt([count(child, column), Term::Int(0)])),
                    Box::new(Formula::Gt([count(parent, parent_column), Term::Int(0)])),
                )
            }
            Spec::ReturnsArgument { function, argument } => {
                let a = analyzed[c.actor_entries[0]];
                let Some(v) = val_value(argument, &plan.actors[0], &a.summary) else {
                    notes.push(format!(
                        "`{}`: the argument is not computable from the arguments",
                        p.name
                    ));
                    continue;
                };
                let ok = |returned: Option<Value>| {
                    Term::Op(TermOp::Actors(ActorsRef {
                        function: function.clone(),
                        outcome: OutcomeKind::Ok,
                        returned,
                    }))
                };
                Formula::Eq([ok(Some(v)), ok(None)])
            }
            Spec::NoFailedActors => Formula::Eq([
                Term::Op(TermOp::Actors(ActorsRef {
                    function: None,
                    outcome: OutcomeKind::Failed,
                    returned: None,
                })),
                Term::Int(0),
            ]),
            Spec::Formula { formula } => formula.clone(),
            Spec::SerialCell { table, column, key } => {
                let actor = &plan.actors[0];
                let a = analyzed[c.actor_entries[0]];
                let mut filter = BTreeMap::new();
                let mut ok = true;
                for (col, v) in key {
                    match val_value(v, actor, &a.summary) {
                        Some(x) => {
                            filter.insert(col.clone(), x);
                        }
                        None => ok = false,
                    }
                }
                if !ok {
                    notes.push(format!(
                        "`{}`: the row key is not computable from the arguments",
                        p.name
                    ));
                    continue;
                }
                match serial_values(plan, index, provider, table, column, &filter) {
                    Ok(serial) if !serial.values.is_empty() => {
                        // (completed == k and cell in V_k) for some k.
                        let term = || {
                            Term::Op(TermOp::Row(RowRef {
                                table: table.clone(),
                                filter: filter.clone(),
                                column: Some(column.clone()),
                            }))
                        };
                        // Phase 14: an outcome an invariant term cannot state
                        // (a boolean, a row) is not turned into `null` (which
                        // made every such outcome a violation). Phase 16: a
                        // generated identifier is stated by the actor and
                        // generator of its generation event, when that is
                        // faithful ([`SerialOutcomes::renaming`]).
                        let mut refused = None;
                        let mut by_count: Vec<Formula> = Vec::new();
                        for (k, vs) in &serial.values {
                            let mut cells: Vec<Formula> = Vec::new();
                            for v in vs {
                                match serial_cell(term(), v, &serial) {
                                    Ok(f) => cells.push(f),
                                    Err(why) => {
                                        refused.get_or_insert(why);
                                    }
                                }
                            }
                            let cell = if cells.len() == 1 {
                                cells.pop().expect("one")
                            } else {
                                Formula::Or(cells)
                            };
                            by_count.push(Formula::And(vec![
                                Formula::Eq([
                                    Term::Op(TermOp::Actors(ActorsRef {
                                        function: None,
                                        outcome: OutcomeKind::Ok,
                                        returned: None,
                                    })),
                                    Term::Int(*k),
                                ]),
                                cell,
                            ]));
                        }
                        if let Some(why) = refused {
                            notes.push(match why {
                                Refusal::Unstatable(v) => format!(
                                    "`{}`: a serial execution leaves {v} in `{table}.{column}`, which an invariant cannot state",
                                    p.name
                                ),
                                Refusal::NotFaithful(v, why) => format!(
                                    "`{}`: a serial execution leaves the generated value {v} in `{table}.{column}`, which cannot be stated by its actor and generator: {why}",
                                    p.name
                                ),
                            });
                            continue;
                        }
                        if serial.renamed {
                            renamed_generated = true;
                        }
                        if serial.replicas {
                            replica_serial = true;
                        }
                        if by_count.len() == 1 {
                            by_count.pop().expect("one")
                        } else {
                            Formula::Or(by_count)
                        }
                    }
                    Ok(_) => {
                        notes.push(format!("`{}`: no serial execution leaves the row", p.name));
                        continue;
                    }
                    Err(e) => {
                        notes.push(format!(
                            "`{}`: serial executions could not be computed: {e}",
                            p.name
                        ));
                        continue;
                    }
                }
            }
        };
        let mut extra = match &p.spec {
            Spec::SerialCell { .. } => {
                " (the value of a serial execution of the requests that completed, computed by the model)"
            }
            _ => "",
        }
        .to_owned();
        // Phase 16: how a serial outcome was stated (only when it applies,
        // so that earlier descriptions are unchanged).
        if renamed_generated {
            extra.push_str("; a generated value is stated as generated by the same actor and generator as in the serial execution (any of that actor's generation events of that generator: a retried attempt generates a new value), never by its text");
        }
        if replica_serial {
            extra.push_str("; the serial executions let the modeled asynchronous replica replay any prefix of the commits at every step (a stale read is permitted, as eventual consistency permits it)");
        }
        out.push((
            p.clone(),
            Invariant {
                name: p.name.clone(),
                when: When::Final,
                description: describe(&extra),
                formula,
            },
        ));
    }
    (out, notes)
}

fn value_term(v: &Value) -> Option<Term> {
    match v {
        Value::Int(n) => Some(Term::Int(*n)),
        Value::Str(s) => Some(Term::Str(s.clone())),
        Value::Null => Some(Term::Null(())),
        _ => None,
    }
}

/// Phase 16: why a serial outcome cannot be stated.
enum Refusal {
    /// A value no invariant term states (a boolean, a row, a clock
    /// reading): its rendering.
    Unstatable(String),
    /// A generated identifier whose actor and generator do not identify
    /// its generation event: its rendering and why.
    NotFaithful(String, String),
}

/// The serial outcomes of a cell ([`serial_values`]).
struct SerialOutcomes {
    /// The values by the number of completed requests.
    values: BTreeMap<i64, BTreeSet<Value>>,
    /// Phase 16: for each generated identifier among the values, the
    /// actor's name and the generator that state it, or why they do not.
    renaming: BTreeMap<GenId, Result<(String, GeneratorSel), String>>,
    /// Phase 16: a generated identifier was stated by actor and generator.
    renamed: bool,
    /// Phase 16: the serial executions explored replica progress.
    replicas: bool,
}

/// One serial outcome as a formula over the cell `term`.
fn serial_cell(term: Term, v: &Value, serial: &SerialOutcomes) -> Result<Formula, Refusal> {
    if let Some(t) = value_term(v) {
        return Ok(Formula::Eq([term, t]));
    }
    let Value::Gen(g) = v else {
        return Err(Refusal::Unstatable(v.render()));
    };
    match serial.renaming.get(g) {
        Some(Ok((actor, generator))) => Ok(Formula::GeneratedBy(Box::new(GeneratedBy {
            value: term,
            by: Some(ActorSel::Actor(actor.clone())),
            generator: Some(generator.clone()),
        }))),
        // A clock reading keeps the Phase 14 wording.
        Some(Err(_)) if is_clock_value(g) => Err(Refusal::Unstatable(v.render())),
        Some(Err(why)) => Err(Refusal::NotFaithful(v.render(), why.clone())),
        None => Err(Refusal::Unstatable(v.render())),
    }
}

fn is_clock_value(g: &GenId) -> bool {
    midwrite_model::db::is_clock(g) || g.actor == midwrite_model::db::CLOCK_PLACEHOLDER
}

/// Phase 16: the generation sites of `generator` in actor `actor`'s code:
/// package calls ([`Op::Generate`]) for the call generator, INSERT rows
/// into the column's table for a column generator. `None` when a backward
/// jump could run a site more than once in one attempt.
fn generation_sites(model: &Model, actor: usize, generator: u16) -> Option<usize> {
    use midwrite_model::ir::{CALL_GENERATOR, StmtKind};
    let gens = model.generators();
    let mut sites = 0;
    for code in &model.actors[actor].codes {
        for (i, step) in code.iter().enumerate() {
            match &step.op {
                Op::Jump { target } | Op::JumpIf { target, .. } if *target <= i => return None,
                Op::Generate { .. } if generator == CALL_GENERATOR => sites += 1,
                Op::Db { stmt } if generator != CALL_GENERATOR => {
                    if let StmtKind::Insert {
                        table, more_rows, ..
                    } = &stmt.kind
                        && gens
                            .get(generator as usize)
                            .is_some_and(|(t, _, _)| model.schema.table(*t).name == *table)
                    {
                        sites += 1 + more_rows.len();
                    }
                }
                _ => {}
            }
        }
    }
    Some(sites)
}

/// Phase 16: whether the generated identifier `g` of a serial execution can
/// be stated as "generated by actor A from generator G" faithfully: it is
/// an identifier (not a clock reading), and actor A's code has exactly one
/// generation site of G and no backward jump, so that every attempt of A
/// generates at most one value of G, at that site, in the serial model and
/// in the concurrent model alike (the same actors and code; only the fault
/// bounds differ). Generation-event numbers are not compared: they shift
/// with retries and with the actor's other generators.
fn renaming(model: &Model, g: &GenId) -> Result<(String, GeneratorSel), String> {
    if is_clock_value(g) {
        return Err("a clock reading is not a generated identifier".to_owned());
    }
    let Some(spec) = model.actors.get(g.actor as usize) else {
        return Err(format!("no actor {}", g.actor));
    };
    let generator = if g.generator == midwrite_model::ir::CALL_GENERATOR {
        GeneratorSel::Call
    } else {
        let gens = model.generators();
        let Some((t, c, _)) = gens.get(g.generator as usize) else {
            return Err(format!("no generator {}", g.generator));
        };
        GeneratorSel::Column(format!("{}.{c}", model.schema.table(*t).name))
    };
    match generation_sites(model, g.actor as usize, g.generator) {
        Some(1) => Ok((spec.name.clone(), generator)),
        Some(n) => Err(format!(
            "actor `{}` has {n} generation sites of that generator, so actor and generator do not identify the event",
            spec.name
        )),
        None => Err(format!(
            "actor `{}`'s code jumps backward, so one site may generate several values in one attempt",
            spec.name
        )),
    }
}

/// The values `table.column` of the row `filter` has after every serial
/// execution of a subset of the plan's actors (each actor alone, to
/// completion, in every order), by the number of actors that completed.
/// Phase 16: with the `replicas` feature, the replica may replay any prefix
/// of the primary's commits at every step of a serial execution (the
/// model's unbounded lag), so that a stale read and a fresh read are both
/// serial outcomes; without it, exactly the Phase 14 computation.
fn serial_values(
    plan: &Plan,
    index: &ProjectIndex,
    provider: &dyn ActorProvider,
    table: &str,
    column: &str,
    filter: &BTreeMap<String, Value>,
) -> Result<SerialOutcomes, String> {
    let mut p = plan.clone();
    p.max_crashes = 0;
    p.max_retries = 0;
    p.max_in_flight = 0;
    let (model, _) = p.load(index, provider)?;
    let t = model
        .schema
        .index(table)
        .ok_or_else(|| format!("no table `{table}`"))?;
    let n = model.actors.len();
    let mut values: BTreeMap<i64, BTreeSet<Value>> = BTreeMap::new();
    for mask in 0u32..(1 << n) {
        let subset: Vec<usize> = (0..n).filter(|a| mask & (1 << a) != 0).collect();
        let mut orders: Vec<Vec<usize>> = Vec::new();
        permutations(&subset, &mut Vec::new(), &mut orders);
        for order in orders {
            for s in serial_states(&model, &order)? {
                let ok = s
                    .actors
                    .iter()
                    .map(|a| i64::from(a.completions))
                    .sum::<i64>();
                for r in s.committed_rows(&model, t) {
                    if filter.iter().all(|(k, v)| r.get(k) == Some(v))
                        && let Some(v) = r.get(column)
                    {
                        values.entry(ok).or_default().insert(v.clone());
                    }
                }
            }
        }
    }
    let mut renamings = BTreeMap::new();
    for v in values.values().flatten() {
        if let Value::Gen(g) = v {
            renamings.insert(*g, renaming(&model, g));
        }
    }
    Ok(SerialOutcomes {
        renamed: renamings.values().any(Result::is_ok),
        values,
        renaming: renamings,
        replicas: model.features.replicas,
    })
}

/// Phase 16: the most states one serial order's replica-progress
/// exploration may visit.
const MAX_SERIAL_STATES: usize = 20_000;

/// The final states of the serial execution `order` (each actor alone, to
/// completion). One state without the `replicas` feature (exactly the
/// Phase 14 runs); with it, one per reachable choice of replica progress.
fn serial_states(model: &Model, order: &[usize]) -> Result<Vec<State>, String> {
    if !model.features.replicas {
        let mut s = initial_state(model);
        for &a in order {
            match run_alone(model, s, a) {
                Solo::Finished { state, .. } => s = *state,
                Solo::Unsupported { detail, location } => {
                    return Err(format!("{detail} at {location}"));
                }
            }
        }
        return Ok(vec![s]);
    }
    let mut out = Vec::new();
    let mut seen: BTreeSet<(usize, Vec<u8>)> = BTreeSet::new();
    let mut stack = vec![(initial_state(model), 0usize, 0usize)];
    while let Some((s, pos, steps)) = stack.pop() {
        if !seen.insert((pos, s.canonical(model))) {
            continue;
        }
        if seen.len() > MAX_SERIAL_STATES {
            return Err(format!(
                "more than {MAX_SERIAL_STATES} states with replica progress"
            ));
        }
        let Some(&a) = order.get(pos) else {
            out.push(s);
            continue;
        };
        let en = enabled(model, &s);
        if let Some(t) = en.iter().find(|t| t.kind == TKind::Replicate)
            && let Ok(applied) = apply(model, &s, *t, &mut Recorder::off())
        {
            stack.push((applied.state, pos, steps));
        }
        let next = en
            .iter()
            .find(|t| t.actor as usize == a && matches!(t.kind, TKind::Step | TKind::Resume));
        match next {
            Some(t) if steps < MAX_SOLO_STEPS => match apply(model, &s, *t, &mut Recorder::off()) {
                Ok(applied) => stack.push((applied.state, pos, steps + 1)),
                Err(u) => return Err(format!("{} at {}", u.detail, u.src.label())),
            },
            _ => stack.push((s, pos + 1, 0)),
        }
    }
    Ok(out)
}

fn permutations(items: &[usize], prefix: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    if items.is_empty() {
        out.push(prefix.clone());
        return;
    }
    for (i, x) in items.iter().enumerate() {
        let mut rest = items.to_vec();
        rest.remove(i);
        prefix.push(*x);
        permutations(&rest, prefix, out);
        prefix.pop();
    }
}

/// The `sourceVerification` section, when the contracts map every actor's
/// entry to the harness.
pub fn source_verification(
    plan: &Plan,
    analyzed: &[&Analyzed],
    c: &Candidate,
    index: &ProjectIndex,
    manifest_from_scenario: &str,
) -> Option<Json> {
    let harness = index.contracts.harness.as_ref()?;
    let mut actors = serde_json::Map::new();
    for (k, actor) in plan.actors.iter().enumerate() {
        let entry = harness.entries.get(&actor.selector)?;
        let a = analyzed[c.actor_entries[k]];
        let input = substitute(&entry.input, &actor.arguments, &a.summary.params)?;
        actors.insert(
            actor.name.clone(),
            json!({ "entry": entry.entry, "input": input }),
        );
    }
    Some(json!({ "harness": manifest_from_scenario, "actors": actors }))
}

fn substitute(template: &Json, args: &[Value], params: &[String]) -> Option<Json> {
    Some(match template {
        Json::String(s) if s.starts_with("$arg") => {
            let rest = &s[4..];
            let (n, path) = match rest.split_once('.') {
                Some((n, p)) => (n, Some(p)),
                None => (rest, None),
            };
            let n: usize = n.parse().ok()?;
            let name = params.get(n)?;
            let full = match path {
                Some(p) => format!("{name}.{p}"),
                None => name.clone(),
            };
            serde_json::to_value(get_path(args, params, &full)?).ok()?
        }
        Json::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                out.insert(k.clone(), substitute(v, args, params)?);
            }
            Json::Object(out)
        }
        Json::Array(items) => Json::Array(
            items
                .iter()
                .map(|v| substitute(v, args, params))
                .collect::<Option<Vec<_>>>()?,
        ),
        other => other.clone(),
    })
}

/// A path from directory `from` to `to` (both absolute), with `/`.
pub fn relative_path(from: &Path, to: &Path) -> String {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = Vec::new();
    for _ in common..from.len() {
        parts.push("..".to_owned());
    }
    for c in &to[common..] {
        parts.push(c.as_os_str().to_string_lossy().into_owned());
    }
    if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(conn: midwrite_model::ir::Conn, line: u32) -> midwrite_model::ir::Step {
        use midwrite_model::ir::{LockMode, Stmt, StmtKind};
        midwrite_model::ir::Step {
            op: Op::Db {
                stmt: Stmt {
                    conn,
                    kind: StmtKind::Select {
                        table: "profiles".into(),
                        pred: Expr::eq(Expr::col("id"), Expr::local("id")),
                        lock: LockMode::None,
                        into: "p".into(),
                        joins: Vec::new(),
                        projection: None,
                        many: None,
                    },
                },
            },
            src: SourceRef::declared(line, "db.select()"),
        }
    }

    #[test]
    fn round_trip_detects_a_replica_read_reextracted_as_a_primary_read() {
        use midwrite_model::ir::{Conn, Step};
        let local = |line| Step {
            op: Op::Let {
                local: "x".into(),
                expr: Expr::lit(1),
            },
            src: SourceRef::declared(line, "x"),
        };
        let discovered = vec![vec![local(1), read(Conn::Replica, 2)]];
        // The loader's extra steps (locals, the response step) are not
        // effects.
        let same = vec![vec![
            local(1),
            read(Conn::Replica, 2),
            Step {
                op: Op::Respond,
                src: SourceRef::declared(3, "return"),
            },
        ]];
        assert_eq!(round_trip(&discovered, &same), Ok(()));
        let primary = vec![vec![read(Conn::Pool, 2)]];
        let err = round_trip(&discovered, &primary).unwrap_err();
        assert!(
            err.contains("replica select") && err.contains("pool select"),
            "{err}"
        );
        let missing = vec![Vec::new()];
        assert!(
            round_trip(&discovered, &missing)
                .unwrap_err()
                .contains("nothing")
        );
    }

    #[test]
    fn rows_satisfy_closed_predicates_or_say_why() {
        let pred = Expr::bin(
            BinOp::And,
            Expr::eq(Expr::col("id"), Expr::lit(3)),
            Expr::bin(BinOp::Ge, Expr::col("stock"), Expr::lit(4)),
        );
        let row = row_for(&pred).unwrap();
        assert_eq!(row.get("id"), Some(&Value::Int(3)));
        assert_eq!(row.get("stock"), Some(&Value::Int(10)));
        let bad = Expr::bin(BinOp::Or, Expr::col("a"), Expr::col("b"));
        assert!(row_for(&bad).is_err());
        let both = Expr::eq(Expr::col("a"), Expr::col("b"));
        assert!(row_for(&both).is_err());
        let contradictory = Expr::bin(
            BinOp::And,
            Expr::eq(Expr::col("id"), Expr::lit(1)),
            Expr::eq(Expr::col("id"), Expr::lit(2)),
        );
        assert!(
            row_for(&contradictory)
                .unwrap_err()
                .contains("contradictory")
        );
    }

    #[test]
    fn paths_are_set_and_read_back() {
        let params = vec!["data".to_owned(), "member".to_owned()];
        let mut args = vec![Value::Null, Value::Null];
        set_path(&mut args, &params, "data.email", Value::str("a@b"));
        set_path(&mut args, &params, "member.id", Value::Int(1));
        set_path(&mut args, &params, "member.orgId", Value::Int(2));
        assert_eq!(
            get_path(&args, &params, "member.orgId"),
            Some(Value::Int(2))
        );
        assert_eq!(
            get_path(&args, &params, "data.email"),
            Some(Value::str("a@b"))
        );
        assert_eq!(get_path(&args, &params, "data.role"), None);
    }

    fn prop(base: Expr, name: &str) -> Expr {
        Expr::Call {
            func: Builtin::At,
            args: vec![base, Expr::Lit(Value::str(name))],
        }
    }

    fn call(func: Builtin, args: Vec<Expr>) -> Expr {
        Expr::Call { func, args }
    }

    fn test(cond: Expr) -> Op {
        Op::JumpIf { cond, target: 0 }
    }

    #[test]
    fn phase14_shared_uses_need_an_annotation_and_needles_become_elements() {
        let params = vec!["input".to_owned()];
        let roles = prop(Expr::local("input"), "roles");
        let ops = vec![vec![test(Expr::not(call(
            Builtin::Includes,
            vec![roles, Expr::Lit(Value::str("admin"))],
        )))]];
        let usage = value_usage(&ops, &params);
        let u = &usage["input.roles"];
        let refused = usage_ty("input.roles", u, None).unwrap_err();
        assert!(refused.contains("both strings and arrays"), "{refused}");
        assert_eq!(
            usage_ty("input.roles", u, Some("string[]")),
            Ok(Some(Ty::List { ints: false }))
        );
        assert_eq!(
            usage_ty("input.roles", u, Some("readonly string[]")),
            Ok(Some(Ty::List { ints: false }))
        );
        assert_eq!(
            usage_ty("input.roles", u, Some("Array<number>")),
            Ok(Some(Ty::List { ints: true }))
        );
        assert_eq!(
            usage_ty("input.roles", u, Some("string")),
            Ok(Some(Ty::Text))
        );
        // A union says nothing: still refused.
        assert!(usage_ty("input.roles", u, Some("string | string[]")).is_err());
        assert_eq!(
            list_value("roles", Some(u), false),
            Value::List(vec![Value::str("admin")])
        );
        assert_eq!(
            list_value("roles", None, false),
            Value::List(vec![Value::str("roles-1")])
        );
    }

    #[test]
    fn phase14_string_only_and_array_only_uses_decide_and_conflicts_are_refused() {
        let params = vec!["name".to_owned(), "tags".to_owned()];
        let ops = vec![vec![
            Op::Let {
                local: "n".to_owned(),
                expr: call(Builtin::Trim, vec![Expr::local("name")]),
            },
            Op::Let {
                local: "k".to_owned(),
                expr: Expr::Each {
                    op: midwrite_model::value::EachOp::Some,
                    list: Box::new(Expr::local("tags")),
                    var: "t".to_owned(),
                    body: Box::new(Expr::eq(Expr::local("t"), Expr::Lit(Value::str("x")))),
                },
            },
        ]];
        let usage = value_usage(&ops, &params);
        assert_eq!(usage_ty("name", &usage["name"], None), Ok(Some(Ty::Text)));
        assert_eq!(
            usage_ty("tags", &usage["tags"], None),
            Ok(Some(Ty::List { ints: false }))
        );
        // Declared as a string but used with an array callback.
        assert!(usage_ty("tags", &usage["tags"], Some("string")).is_err());
        let both = vec![vec![
            Op::Let {
                local: "a".to_owned(),
                expr: call(Builtin::Join, vec![Expr::local("tags")]),
            },
            Op::Let {
                local: "b".to_owned(),
                expr: call(Builtin::ToLowerCase, vec![Expr::local("tags")]),
            },
        ]];
        let usage = value_usage(&both, &params);
        let refused = usage_ty("tags", &usage["tags"], None).unwrap_err();
        assert!(refused.contains("both as an array"), "{refused}");
    }

    #[test]
    fn phase14_loops_type_their_list_and_object_elements_are_refused() {
        // `for (const item of items) { item.id }`: a snapshot local, an
        // element local, a property of the element.
        let params = vec!["items".to_owned()];
        let ops = vec![vec![
            Op::Let {
                local: "_iter1".to_owned(),
                expr: Expr::local("items"),
            },
            Op::Let {
                local: "_i2".to_owned(),
                expr: Expr::lit(0),
            },
            test(Expr::not(Expr::bin(
                BinOp::Lt,
                Expr::local("_i2"),
                call(Builtin::Length, vec![Expr::local("_iter1")]),
            ))),
            Op::Let {
                local: "item".to_owned(),
                expr: call(Builtin::At, vec![Expr::local("_iter1"), Expr::local("_i2")]),
            },
            Op::Let {
                local: "id".to_owned(),
                expr: prop(Expr::local("item"), "id"),
            },
        ]];
        let usage = value_usage(&ops, &params);
        let u = &usage["items"];
        assert!(u.object_elements);
        assert!(
            usage_ty("items", u, None)
                .unwrap_err()
                .contains("array of objects")
        );
        // Without the property read, iteration types an array.
        let plain: Vec<Vec<Op>> = vec![ops[0][..4].to_vec()];
        let usage = value_usage(&plain, &params);
        assert_eq!(
            usage_ty("items", &usage["items"], None),
            Ok(Some(Ty::List { ints: false }))
        );
        // At most three elements, whatever the code tests.
        let many = Usage {
            needles: (1..=5).map(Value::Int).collect(),
            ..Usage::default()
        };
        assert_eq!(
            list_value("ids", Some(&many), true),
            Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
        );
        assert_eq!(MAX_LIST, 3);
    }

    #[test]
    fn relative_paths_walk_up_and_down() {
        assert_eq!(
            relative_path(Path::new("/a/b/out"), Path::new("/a/b/app")),
            "../app"
        );
        assert_eq!(relative_path(Path::new("/a"), Path::new("/a")), ".");
        assert_eq!(relative_path(Path::new("/x/y"), Path::new("/a")), "../../a");
    }
}

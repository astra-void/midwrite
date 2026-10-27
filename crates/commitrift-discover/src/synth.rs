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

use commitrift_model::engine::{Action, Recorder, apply, enabled, initial_state};
use commitrift_model::invariant::{
    ActorsRef, ExternalsRef, Formula, Invariant, OutcomeKind, RowRef, Term, TermOp, When,
};
use commitrift_model::ir::{Op, SourceRef};
use commitrift_model::model::Model;
use commitrift_model::provider::ActorProvider;
use commitrift_model::schema_facts::{SchemaDefault, SchemaTable};
use commitrift_model::state::{ClosedStmt, State, TKind};
use commitrift_model::value::{BinOp, Expr, Row, Value};

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
        doc
    }

    /// Load the scenario with the model checker's loader (project root as
    /// base directory).
    pub fn load(
        &self,
        index: &ProjectIndex,
        provider: &dyn ActorProvider,
    ) -> Result<(Model, commitrift_model::scenario::LoadReport), String> {
        let doc = self.to_json(index, ".");
        commitrift_model::scenario::load_str(&doc.to_string(), &index.root, provider)
            .map_err(|e| e.to_string())
    }
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
enum Ty {
    Int,
    Text,
    Bool,
    /// Phase 12: a `uuid` column: the runtime database accepts only UUIDs.
    Uuid,
}

/// Parameter paths the code reads, each with a type and whether it is a key.
fn param_paths(a: &Analyzed, index: &ProjectIndex) -> BTreeMap<String, (Ty, bool)> {
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

fn exprs_of(op: &Op) -> Vec<&Expr> {
    use commitrift_model::ir::StmtKind;
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
            StmtKind::Insert { values, .. } => values.iter().map(|(_, e)| e).collect(),
        },
        _ => Vec::new(),
    }
}

fn path_of(e: &Expr, params: &[String]) -> Option<String> {
    match e {
        Expr::Local(l) if params.contains(l) => Some(l.clone()),
        Expr::Field(base, f) => path_of(base, params).map(|p| format!("{p}.{f}")),
        _ => None,
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
        _ => {}
    }
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
fn varied_paths(c: &Candidate, a: &Analyzed, relation: Relation) -> Vec<String> {
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
                if let Some(e) = a.summary.db.get(op.effect)
                    && e.access.writes()
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

/// Synthesize positional arguments for `a` (actor number `k`).
fn arguments(
    c: &Candidate,
    a: &Analyzed,
    index: &ProjectIndex,
    relation: Relation,
    k: usize,
) -> Vec<Value> {
    if let Some(declared) = a.contract.as_ref().and_then(|c| c.arguments.clone()) {
        return declared;
    }
    let params = &a.summary.params;
    let mut args = vec![Value::Null; params.len()];
    let paths = param_paths(a, index);
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
            Ty::Text if last.to_ascii_lowercase().contains("email") => {
                Value::Str("alice@example.test".to_owned())
            }
            Ty::Text => Value::Str(format!("{last}-1")),
        };
        set_path(&mut args, params, path, value);
    }
    if let Some(ct) = &a.contract {
        for (path, value) in &ct.values {
            set_path(&mut args, params, path, value.clone());
        }
    }
    if k > 0 {
        match relation {
            Relation::Same => {}
            Relation::DistinctKey | Relation::DistinctRow | Relation::DistinctRequestKey => {
                for path in &varied {
                    let new = match get_path(&args, params, path) {
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
    }
    args
}

// ---------------------------------------------------------------------------
// Running actors in the engine
// ---------------------------------------------------------------------------

/// One executed transition of a solo run.
pub struct Executed {
    pub src: SourceRef,
    pub actions: Vec<Action>,
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
                executed.push(Executed {
                    src,
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
        // Keys must stay unique.
        for (set, predicate) in crate::detect::unique_sets(self.index, table) {
            if predicate.is_some() {
                continue;
            }
            let key: Vec<Option<&Value>> = set.iter().map(|c| row.get(c)).collect();
            if key.iter().any(|v| v.is_none_or(Value::is_null)) {
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
fn uuid_literal(name: &str, n: usize) -> Value {
    let h = commitrift_core::diagnostic::stable_hash(&[name]);
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
    match model_ty(builder) {
        Ty::Int => Value::Int(10),
        Ty::Bool => Value::Bool(true),
        Ty::Text | Ty::Uuid => Value::Str(format!("{property}-{n}")),
    }
}

fn literal(l: &commitrift_model::schema_facts::SchemaLiteral) -> Value {
    use commitrift_model::schema_facts::SchemaLiteral;
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
#[allow(clippy::too_many_arguments)]
pub fn build(
    c: &Candidate,
    variant: &Variant,
    analyzed: &[&Analyzed],
    index: &ProjectIndex,
    provider: &dyn ActorProvider,
    budget: ScenarioBudget,
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
                            "how the wrapper of `{}` invokes the callback is unknown: declare `invocation` in commitrift-contracts.json",
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
            arguments: arguments(c, a, index, variant.relation, k),
            retry: c.environment.retry.clone(),
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
    };
    let mut rows = Rows {
        index,
        rows: BTreeMap::new(),
        added: BTreeMap::new(),
        notes: Vec::new(),
    };
    preseed_argument_rows(c, &plan, analyzed, &mut rows);
    preseed_written_rows(c, &plan, analyzed, &mut rows);
    let mut iterations = 0;
    // Each actor alone, rows accumulating; then every actor again on the
    // final rows.
    let order: Vec<usize> = (0..plan.actors.len()).chain(0..plan.actors.len()).collect();
    for (round, k) in order.into_iter().enumerate() {
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
            let run = run_alone(&model, initial_state(&model), 0);
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
            let targets = &solo.actors[0].targets;
            if targets.iter().all(|t| done.contains(t)) {
                // Rows the actor inserts need their parents too.
                let inserted = inserted_rows(&model, &executed);
                let missing = rows.fk_violations(&inserted);
                if missing.is_empty() {
                    reached = true;
                    let _ = state;
                    if final_round {
                        plan.actors[k].observed = observed_rows(&executed);
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
            match seed_from(&model, &executed, &solo.actors[0].keep_empty, &mut rows) {
                Ok(()) => continue,
                Err(why) => {
                    return Err(SynthesisReport {
                        status: "unsatisfiable".to_owned(),
                        detail: format!(
                            "actor `{}` does not reach {} and no row can be added: {why}",
                            solo.actors[0].name,
                            targets.join(", ")
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
    Ok(Built { plan, synthesis })
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
            if let Action::Stmt {
                stmt: ClosedStmt::Insert { table, values, .. },
                ..
            } = a
            {
                out.entry(model.schema.table(*table).name.clone())
                    .or_default()
                    .push(values.clone());
            }
        }
    }
    out
}

fn changed_nothing(a: &Action) -> bool {
    use commitrift_model::engine::Expect;
    use commitrift_model::exec::StmtResult;
    matches!(
        a,
        Action::Stmt {
            stmt: ClosedStmt::Update { .. } | ClosedStmt::Delete { .. },
            expect: Expect::Done(StmtResult::Affected(0)),
            ..
        }
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
    joins: &[commitrift_model::state::ClosedJoin],
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
    use commitrift_model::engine::Expect;
    use commitrift_model::exec::StmtResult;
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
    use commitrift_model::engine::Expect;
    use commitrift_model::exec::StmtResult;
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
            Spec::NoFailedActors => Formula::Eq([
                Term::Op(TermOp::Actors(ActorsRef {
                    function: None,
                    outcome: OutcomeKind::Failed,
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
                    Ok(values) if !values.is_empty() => {
                        // (completed == k and cell in V_k) for some k.
                        let term = || {
                            Term::Op(TermOp::Row(RowRef {
                                table: table.clone(),
                                filter: filter.clone(),
                                column: Some(column.clone()),
                            }))
                        };
                        let mut by_count: Vec<Formula> = values
                            .iter()
                            .map(|(k, vs)| {
                                let mut cells: Vec<Formula> = vs
                                    .iter()
                                    .map(|v| Formula::Eq([term(), value_term(v)]))
                                    .collect();
                                let cell = if cells.len() == 1 {
                                    cells.pop().expect("one")
                                } else {
                                    Formula::Or(cells)
                                };
                                Formula::And(vec![
                                    Formula::Eq([
                                        Term::Op(TermOp::Actors(ActorsRef {
                                            function: None,
                                            outcome: OutcomeKind::Ok,
                                        })),
                                        Term::Int(*k),
                                    ]),
                                    cell,
                                ])
                            })
                            .collect();
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
        let extra = match &p.spec {
            Spec::SerialCell { .. } => {
                " (the value of a serial execution of the requests that completed, computed by the model)"
            }
            _ => "",
        };
        out.push((
            p.clone(),
            Invariant {
                name: p.name.clone(),
                when: When::Final,
                description: describe(extra),
                formula,
            },
        ));
    }
    (out, notes)
}

fn value_term(v: &Value) -> Term {
    match v {
        Value::Int(n) => Term::Int(*n),
        Value::Str(s) => Term::Str(s.clone()),
        _ => Term::Null(()),
    }
}

/// The values `table.column` of the row `filter` has after every serial
/// execution of a subset of the plan's actors (each actor alone, to
/// completion, in every order), by the number of actors that completed.
fn serial_values(
    plan: &Plan,
    index: &ProjectIndex,
    provider: &dyn ActorProvider,
    table: &str,
    column: &str,
    filter: &BTreeMap<String, Value>,
) -> Result<BTreeMap<i64, BTreeSet<Value>>, String> {
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
            let mut s = initial_state(&model);
            for a in order {
                match run_alone(&model, s, a) {
                    Solo::Finished { state, .. } => s = *state,
                    Solo::Unsupported { detail, location } => {
                        return Err(format!("{detail} at {location}"));
                    }
                }
            }
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
    Ok(values)
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

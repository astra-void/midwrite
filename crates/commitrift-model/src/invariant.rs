//! Declared invariants: a small, compositional assertion language over the
//! *committed* database state, actor outcomes, the external effect log and
//! shared application variables. There is no embedded code: an invariant is
//! data, evaluated by the model checker.
//!
//! ```json
//! {
//!   "name": "no-oversell",
//!   "when": "final",
//!   "assert": { "le": [
//!     { "actors": { "function": "purchase", "outcome": "ok" } },
//!     { "initial": { "table": "products", "where": { "id": "product-1" }, "column": "stock" } }
//!   ] }
//! }
//! ```
//!
//! Semantics: a comparison involving `null` (for example the value of a row
//! that does not exist) is *false*, except `eq`/`ne` against `null`.
//! `when: "final"` invariants are evaluated in quiescent states (all actors
//! finished); `when: "always"` invariants in every reachable state.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::Model;
use crate::state::{Outcome, State};
use crate::value::{Row, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum When {
    Final,
    Always,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Invariant {
    pub name: String,
    #[serde(default = "default_when")]
    pub when: When,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "assert")]
    pub formula: Formula,
}

fn default_when() -> When {
    When::Final
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Formula {
    Eq([Term; 2]),
    Ne([Term; 2]),
    Lt([Term; 2]),
    Le([Term; 2]),
    Gt([Term; 2]),
    Ge([Term; 2]),
    And(Vec<Formula>),
    Or(Vec<Formula>),
    Not(Box<Formula>),
    Implies(Box<Formula>, Box<Formula>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Term {
    Int(i64),
    Str(String),
    Null(()),
    Op(TermOp),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RowRef {
    pub table: String,
    /// Equality filter on columns.
    #[serde(default, rename = "where")]
    pub filter: BTreeMap<String, Value>,
    #[serde(default)]
    pub column: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActorsRef {
    #[serde(default)]
    pub function: Option<String>,
    pub outcome: OutcomeKind,
    /// Phase 13: count only completions whose main task returned this
    /// value (what the caller was told: `"returned": true` for "claimed").
    /// Needs `outcome: "ok"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returned: Option<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OutcomeKind {
    /// Total number of times the operation returned normally (a retried
    /// operation that succeeds twice counts twice).
    Ok,
    /// Actors whose latest attempt failed.
    Failed,
    Crashed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExternalsRef {
    pub channel: String,
    #[serde(default)]
    pub key: Option<Value>,
    /// Count distinct idempotency keys instead of effects.
    #[serde(default)]
    pub distinct_keys: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TermOp {
    /// A column of the *initial* row matching the filter.
    Initial(RowRef),
    /// A column of the committed row matching the filter (`null` if none).
    Row(RowRef),
    /// Number of committed rows matching the filter.
    Count(RowRef),
    /// Sum of a column over committed rows matching the filter.
    Sum(RowRef),
    Actors(ActorsRef),
    /// Number of performed (not deduplicated) external effects.
    Externals(ExternalsRef),
    /// A shared in-process variable (of the first process).
    App(String),
    /// Phase 9: running tasks of a state in which nothing can proceed (zero
    /// unless the system is stuck: every task waits for a pool connection,
    /// or for a lock only the application's own waiting could release).
    Stuck(()),
    Add(Box<Term>, Box<Term>),
    Sub(Box<Term>, Box<Term>),
}

/// Whether an actor's function `name` is `f`. Phase 14: an actor built from
/// an entry selector (`src/a.ts#claim`, `src/a.ts#POST/arg[0]`) also answers
/// to its export's name (`claim`, `POST`), the name a contract uses.
fn names_function(name: &str, f: &str) -> bool {
    name == f
        || name
            .rsplit_once('#')
            .is_some_and(|(_, export)| export == f || export.split('/').next() == Some(f))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invariant `{invariant}`: {message}")]
pub struct InvariantError {
    pub invariant: String,
    pub message: String,
}

impl Invariant {
    /// Check names against the model; unsupported references are rejected.
    pub fn validate(&self, model: &Model) -> Result<(), InvariantError> {
        let err = |m: String| InvariantError {
            invariant: self.name.clone(),
            message: m,
        };
        let mut result = Ok(());
        self.formula.walk_terms(&mut |t| {
            if result.is_err() {
                return;
            }
            if let Term::Op(op) = t {
                let r = match op {
                    TermOp::Initial(r) | TermOp::Row(r) | TermOp::Count(r) | TermOp::Sum(r) => {
                        validate_ref(
                            model,
                            r,
                            matches!(op, TermOp::Sum(_) | TermOp::Initial(_) | TermOp::Row(_)),
                        )
                    }
                    TermOp::Actors(a) => match &a.function {
                        Some(f)
                            if !model
                                .actors
                                .iter()
                                .any(|x| names_function(&x.function.name, f)) =>
                        {
                            Err(format!("no actor runs function `{f}`"))
                        }
                        _ if a.returned.is_some() && a.outcome != OutcomeKind::Ok => {
                            Err("`returned` counts completions: it needs `outcome: \"ok\"`".into())
                        }
                        _ => Ok(()),
                    },
                    TermOp::Externals(_)
                    | TermOp::App(_)
                    | TermOp::Stuck(())
                    | TermOp::Add(..)
                    | TermOp::Sub(..) => Ok(()),
                };
                if let Err(m) = r {
                    result = Err(err(m));
                }
            }
        });
        result
    }

    pub fn holds(&self, model: &Model, s: &State) -> bool {
        self.formula.eval(model, s)
    }
}

fn validate_ref(model: &Model, r: &RowRef, needs_column: bool) -> Result<(), String> {
    let Some(table) = model.schema.by_name(&r.table) else {
        return Err(format!("unknown table `{}`", r.table));
    };
    for c in r.filter.keys() {
        if !table.has_column(c) {
            return Err(format!("unknown column `{}.{c}`", r.table));
        }
    }
    match (&r.column, needs_column) {
        (Some(c), _) if !table.has_column(c) => Err(format!("unknown column `{}.{c}`", r.table)),
        (None, true) => Err(format!("a `column` is required for table `{}`", r.table)),
        _ => Ok(()),
    }
}

impl Formula {
    fn walk_terms(&self, f: &mut dyn FnMut(&Term)) {
        match self {
            Self::Eq(t) | Self::Ne(t) | Self::Lt(t) | Self::Le(t) | Self::Gt(t) | Self::Ge(t) => {
                for x in t {
                    x.walk(f);
                }
            }
            Self::And(v) | Self::Or(v) => v.iter().for_each(|x| x.walk_terms(f)),
            Self::Not(x) => x.walk_terms(f),
            Self::Implies(a, b) => {
                a.walk_terms(f);
                b.walk_terms(f);
            }
        }
    }

    /// Phase 13: whether the formula reads what main tasks returned.
    pub fn reads_returned(&self) -> bool {
        let mut any = false;
        self.walk_terms(&mut |t| {
            if let Term::Op(TermOp::Actors(a)) = t
                && a.returned.is_some()
            {
                any = true;
            }
        });
        any
    }

    /// Phase 13: the functions whose return values the formula reads
    /// (`None`: every actor's).
    pub fn returned_functions(&self) -> Vec<Option<String>> {
        let mut out = Vec::new();
        self.walk_terms(&mut |t| {
            if let Term::Op(TermOp::Actors(a)) = t
                && a.returned.is_some()
            {
                out.push(a.function.clone());
            }
        });
        out
    }

    /// Whether the formula reads only database rows (initial or committed),
    /// i.e. nothing a database replay cannot observe (actor outcomes,
    /// external effects, in-process variables).
    pub fn reads_only_database(&self) -> bool {
        let mut only = true;
        self.walk_terms(&mut |t| {
            if let Term::Op(TermOp::Actors(_) | TermOp::Externals(_) | TermOp::App(_)) = t {
                only = false;
            }
        });
        only
    }

    /// Whether the formula reads only committed rows and the external
    /// effect log (Phase 8: both are observable after an original-code run).
    pub fn reads_only_database_and_externals(&self) -> bool {
        let mut only = true;
        self.walk_terms(&mut |t| {
            if let Term::Op(TermOp::Actors(_) | TermOp::App(_) | TermOp::Stuck(())) = t {
                only = false;
            }
        });
        only
    }

    /// Whether the formula reads only committed rows, the external effect
    /// log and actor outcomes (Phase 11: outcomes are observable after an
    /// original-code run without crashes or retries).
    pub fn reads_only_database_externals_and_outcomes(&self) -> bool {
        let mut only = true;
        self.walk_terms(&mut |t| {
            if let Term::Op(TermOp::App(_) | TermOp::Stuck(())) = t {
                only = false;
            }
        });
        only
    }

    /// Visit the non-literal comparison operands (for reports).
    pub fn visit_terms(&self, f: &mut dyn FnMut(&Term)) {
        match self {
            Self::Eq(t) | Self::Ne(t) | Self::Lt(t) | Self::Le(t) | Self::Gt(t) | Self::Ge(t) => {
                for x in t {
                    if matches!(x, Term::Op(_)) {
                        f(x);
                    }
                }
            }
            Self::And(v) | Self::Or(v) => v.iter().for_each(|x| x.visit_terms(f)),
            Self::Not(x) => x.visit_terms(f),
            Self::Implies(a, b) => {
                a.visit_terms(f);
                b.visit_terms(f);
            }
        }
    }

    pub fn eval(&self, model: &Model, s: &State) -> bool {
        match self {
            Self::And(v) => v.iter().all(|x| x.eval(model, s)),
            Self::Or(v) => v.iter().any(|x| x.eval(model, s)),
            Self::Not(x) => !x.eval(model, s),
            Self::Implies(a, b) => !a.eval(model, s) || b.eval(model, s),
            Self::Eq([a, b]) => cmp(a.eval(model, s), b.eval(model, s), |o| o.is_eq(), true),
            Self::Ne([a, b]) => cmp(a.eval(model, s), b.eval(model, s), |o| o.is_ne(), true),
            Self::Lt([a, b]) => cmp(a.eval(model, s), b.eval(model, s), |o| o.is_lt(), false),
            Self::Le([a, b]) => cmp(a.eval(model, s), b.eval(model, s), |o| o.is_le(), false),
            Self::Gt([a, b]) => cmp(a.eval(model, s), b.eval(model, s), |o| o.is_gt(), false),
            Self::Ge([a, b]) => cmp(a.eval(model, s), b.eval(model, s), |o| o.is_ge(), false),
        }
    }

    /// A readable rendering for reports.
    pub fn render(&self) -> String {
        let bin = |op: &str, t: &[Term; 2]| format!("{} {op} {}", t[0].render(), t[1].render());
        match self {
            Self::Eq(t) => bin("=", t),
            Self::Ne(t) => bin("!=", t),
            Self::Lt(t) => bin("<", t),
            Self::Le(t) => bin("<=", t),
            Self::Gt(t) => bin(">", t),
            Self::Ge(t) => bin(">=", t),
            Self::And(v) => v
                .iter()
                .map(|x| format!("({})", x.render()))
                .collect::<Vec<_>>()
                .join(" and "),
            Self::Or(v) => v
                .iter()
                .map(|x| format!("({})", x.render()))
                .collect::<Vec<_>>()
                .join(" or "),
            Self::Not(x) => format!("not ({})", x.render()),
            Self::Implies(a, b) => format!("({}) implies ({})", a.render(), b.render()),
        }
    }
}

fn cmp(a: Value, b: Value, test: impl Fn(std::cmp::Ordering) -> bool, null_ok: bool) -> bool {
    match (&a, &b) {
        (Value::Null, Value::Null) => null_ok && test(std::cmp::Ordering::Equal),
        (Value::Null, _) | (_, Value::Null) => null_ok && test(std::cmp::Ordering::Less),
        (Value::Int(x), Value::Int(y)) => test(x.cmp(y)),
        (Value::Str(x), Value::Str(y)) => test(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => test(x.cmp(y)),
        _ => false,
    }
}

impl Term {
    fn walk(&self, f: &mut dyn FnMut(&Term)) {
        f(self);
        if let Self::Op(TermOp::Add(a, b) | TermOp::Sub(a, b)) = self {
            a.walk(f);
            b.walk(f);
        }
    }

    pub fn render(&self) -> String {
        match self {
            Self::Int(n) => n.to_string(),
            Self::Str(s) => format!("{s:?}"),
            Self::Null(()) => "null".into(),
            Self::Op(op) => match op {
                TermOp::Initial(r) => format!("initial({})", r.render()),
                TermOp::Row(r) => format!("committed({})", r.render()),
                TermOp::Count(r) => format!("count({})", r.render()),
                TermOp::Sum(r) => format!("sum({})", r.render()),
                TermOp::Actors(a) => format!(
                    "{}{}",
                    format!(
                        "{}{:?}",
                        a.function
                            .as_deref()
                            .map(|f| format!("{f}:"))
                            .unwrap_or_default(),
                        a.outcome
                    )
                    .to_lowercase(),
                    a.returned
                        .as_ref()
                        .map(|v| format!(" returning {}", v.render()))
                        .unwrap_or_default()
                ),
                TermOp::Externals(e) => format!(
                    "externals({}{})",
                    e.channel,
                    if e.distinct_keys {
                        ", distinct keys"
                    } else {
                        ""
                    }
                ),
                TermOp::App(v) => format!("app.{v}"),
                TermOp::Stuck(()) => "stuck tasks".to_owned(),
                TermOp::Add(a, b) => format!("({} + {})", a.render(), b.render()),
                TermOp::Sub(a, b) => format!("({} - {})", a.render(), b.render()),
            },
        }
    }

    pub fn eval(&self, model: &Model, s: &State) -> Value {
        match self {
            Self::Int(n) => Value::Int(*n),
            Self::Str(x) => Value::Str(x.clone()),
            Self::Null(()) => Value::Null,
            Self::Op(op) => eval_op(op, model, s),
        }
    }
}

impl RowRef {
    fn render(&self) -> String {
        let filter: Vec<String> = self
            .filter
            .iter()
            .map(|(k, v)| format!("{k}={}", v.render()))
            .collect();
        match &self.column {
            Some(c) => format!("{}[{}].{c}", self.table, filter.join(",")).replace("[]", ""),
            None => format!("{}[{}]", self.table, filter.join(",")).replace("[]", ""),
        }
    }

    fn matches(&self, row: &Row) -> bool {
        self.filter.iter().all(|(c, v)| row.get(c) == Some(v))
    }
}

fn eval_op(op: &TermOp, model: &Model, s: &State) -> Value {
    match op {
        TermOp::Initial(r) => {
            let Some(t) = model.schema.index(&r.table) else {
                return Value::Null;
            };
            model.initial_rows[t as usize]
                .iter()
                .find(|row| r.matches(row))
                .and_then(|row| r.column.as_ref().and_then(|c| row.get(c).cloned()))
                .unwrap_or(Value::Null)
        }
        TermOp::Row(r) => {
            let Some(t) = model.schema.index(&r.table) else {
                return Value::Null;
            };
            s.committed_rows(model, t)
                .into_iter()
                .find(|row| r.matches(row))
                .and_then(|row| r.column.as_ref().and_then(|c| row.get(c).cloned()))
                .unwrap_or(Value::Null)
        }
        TermOp::Count(r) => {
            let Some(t) = model.schema.index(&r.table) else {
                return Value::Null;
            };
            Value::Int(
                s.committed_rows(model, t)
                    .iter()
                    .filter(|row| r.matches(row))
                    .count() as i64,
            )
        }
        TermOp::Sum(r) => {
            let (Some(t), Some(c)) = (model.schema.index(&r.table), r.column.as_ref()) else {
                return Value::Null;
            };
            let mut total = 0i64;
            for row in s
                .committed_rows(model, t)
                .iter()
                .filter(|row| r.matches(row))
            {
                if let Some(Value::Int(n)) = row.get(c) {
                    total = total.saturating_add(*n);
                }
            }
            Value::Int(total)
        }
        TermOp::Actors(a) => {
            let mut n = 0i64;
            for (i, st) in s.actors.iter().enumerate() {
                if let Some(f) = &a.function
                    && !names_function(&model.actors[i].function.name, f)
                {
                    continue;
                }
                if let Some(v) = &a.returned {
                    // Phase 13: completions that returned `v`. An unknown
                    // value is refused when the scenario is loaded.
                    n += st
                        .returned
                        .iter()
                        .filter(|r| r.as_ref().is_ok_and(|x| x == v))
                        .count() as i64;
                    continue;
                }
                n += match a.outcome {
                    OutcomeKind::Ok => i64::from(st.completions),
                    OutcomeKind::Failed => i64::from(matches!(st.outcome, Outcome::Failed(_))),
                    OutcomeKind::Crashed => i64::from(matches!(st.outcome, Outcome::Crashed)),
                };
            }
            Value::Int(n)
        }
        TermOp::Externals(e) => {
            let performed = s
                .externals
                .iter()
                .filter(|x| x.channel == e.channel && !x.deduplicated)
                .filter(|x| e.key.as_ref().is_none_or(|k| x.key.as_ref() == Some(k)));
            if e.distinct_keys {
                let keys: std::collections::BTreeSet<_> =
                    performed.map(|x| x.key.clone()).collect();
                Value::Int(keys.len() as i64)
            } else {
                Value::Int(performed.count() as i64)
            }
        }
        TermOp::Stuck(()) => {
            let en = crate::engine::enabled(model, s);
            if crate::engine::is_quiescent(model, &en) && !s.is_finished() {
                Value::Int(
                    s.actors
                        .iter()
                        .flat_map(|a| a.threads.iter())
                        .filter(|t| t.status == crate::state::TStatus::Running)
                        .count() as i64,
                )
            } else {
                Value::Int(0)
            }
        }
        TermOp::App(v) => s
            .apps
            .first()
            .and_then(|a| a.get(v))
            .cloned()
            .unwrap_or(Value::Null),
        TermOp::Add(a, b) | TermOp::Sub(a, b) => match (a.eval(model, s), b.eval(model, s)) {
            (Value::Int(x), Value::Int(y)) => {
                let r = if matches!(op, TermOp::Add(..)) {
                    x.checked_add(y)
                } else {
                    x.checked_sub(y)
                };
                r.map_or(Value::Null, Value::Int)
            }
            _ => Value::Null,
        },
    }
}

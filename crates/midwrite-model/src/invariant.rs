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
//!
//! Generated identities (`docs/concepts/invariants.md`).
//! A generated value ([`Value::Gen`]) has no text the model knows, so an
//! invariant never names one; it states *who generated* a value
//! (`generatedBy`: an actor and a generator), whether two values are the
//! *same* or *distinct* generation events (`sameGenerated`,
//! `distinctGenerated`, `same`), which rows *match* values read elsewhere
//! (`countMatching` / `rowMatching` with term-valued `match` and
//! `insertedBy`), and quantifies over actors (`forEachActor`, with
//! `returned` of the bound actor) and rows (`forEachRow`, with `field`).
//! Every Phase 6-15 invariant keeps its JSON and its meaning.

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
    /// The value is a generated identifier (not a clock reading)
    /// whose generation event belongs to the selected actor and generator.
    GeneratedBy(Box<GeneratedBy>),
    /// Both values are generated identifiers of the same
    /// generation event (false when either is not generated).
    SameGenerated([Term; 2]),
    /// Both values are generated identifiers of different
    /// generation events (false when either is not generated: this is not
    /// the negation of `sameGenerated`).
    DistinctGenerated([Term; 2]),
    /// Both values are present (neither `null` nor `undefined`)
    /// and equal; a generated value equals only itself.
    Same([Term; 2]),
    /// The formula holds for every actor (running `function`;
    /// by default only actors that completed at least once), with the actor
    /// bound as `self`.
    ForEachActor(Box<ForEachActor>),
    /// The formula holds for every committed row of `table`
    /// matching `where`, with the row bound for `field` terms.
    ForEachRow(Box<ForEachRow>),
}

/// Which actor a generation event or an inserted row belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActorSel {
    /// The actor bound by the innermost `forEachActor` (`"self"`).
    #[serde(rename = "self")]
    Bound,
    /// The scenario actor of this name (`{"actor": "A"}`).
    Actor(String),
    /// Any actor running this function (`{"function": "f"}`; an entry
    /// selector also answers to its export's name, as in `actors`).
    Function(String),
}

/// Which generator a generation event came from. A call site is
/// not part of a generation event's identity: every package call shares
/// one generator (`ir::CALL_GENERATOR`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GeneratorSel {
    /// A package call with a verified fresh-value contract (`nanoid()`,
    /// `crypto.randomUUID()`), at any call site (`"call"`).
    Call,
    /// The generated default of `table.column` (`{"column": "t.c"}`).
    Column(String),
}

/// `{"generatedBy": {"value": T, "by": ..., "generator": ...}}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GeneratedBy {
    pub value: Term,
    /// Absent: any actor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<ActorSel>,
    /// Absent: any identifier generator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator: Option<GeneratorSel>,
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

/// `{"forEachActor": {"function": f, "completed": true, "assert": F}}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForEachActor {
    /// Absent: every actor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// Only actors whose main task completed at least once (default): an
    /// actor that failed or crashed told its caller nothing.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub completed: bool,
    #[serde(rename = "assert")]
    pub formula: Formula,
}

/// `{"forEachRow": {"table": t, "where": {...}, "assert": F}}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForEachRow {
    pub table: String,
    #[serde(default, rename = "where", skip_serializing_if = "BTreeMap::is_empty")]
    pub filter: BTreeMap<String, Value>,
    #[serde(rename = "assert")]
    pub formula: Formula,
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
    /// Count only completions whose main task returned this
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

/// Committed rows selected by literal values (`where`), by the
/// values of other terms (`match`: a row matches when its column equals the
/// term's value, compared by identity for generated values; a `null` or
/// `undefined` term value matches no row, as SQL `=` does) and by the actor
/// whose INSERT created the row (`insertedBy`; rows of the initial state
/// were inserted by no actor).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RowMatch {
    pub table: String,
    #[serde(default, rename = "where", skip_serializing_if = "BTreeMap::is_empty")]
    pub filter: BTreeMap<String, Value>,
    #[serde(default, rename = "match", skip_serializing_if = "BTreeMap::is_empty")]
    pub matching: BTreeMap<String, Term>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inserted_by: Option<ActorSel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
}

/// What an actor's main task returned in its latest completed
/// attempt (`null` before any completion), optionally a field `path`
/// (`"a.b"`) of a returned object (`null` when absent).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReturnedRef {
    /// The actor's name; absent: the actor bound by `forEachActor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
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
    /// Running tasks of a state in which nothing can proceed (zero
    /// unless the system is stuck: every task waits for a pool connection,
    /// or for a lock only the application's own waiting could release).
    Stuck(()),
    Add(Box<Term>, Box<Term>),
    Sub(Box<Term>, Box<Term>),
    /// Number of committed rows selected by a [`RowMatch`].
    CountMatching(Box<RowMatch>),
    /// `column` of the first committed row (in key order)
    /// selected by a [`RowMatch`] (`null` if none).
    RowMatching(Box<RowMatch>),
    /// A returned value ([`ReturnedRef`]).
    Returned(ReturnedRef),
    /// A column of the row bound by `forEachRow`.
    Field(String),
    /// An actor's argument ([`ArgumentRef`]): what a declared
    /// property uses to name the row a call works on.
    Argument(ArgumentRef),
}

/// An actor's argument: `path` is the parameter's name, then
/// fields of an object argument (`params.customerId`); `null` when absent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArgumentRef {
    /// The actor's name; absent: the actor bound by `forEachActor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    pub path: String,
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
        check_formula(model, &self.formula, &Scope::default()).map_err(|m| InvariantError {
            invariant: self.name.clone(),
            message: m,
        })
    }

    pub fn holds(&self, model: &Model, s: &State) -> bool {
        self.formula.eval(model, s)
    }
}

/// What the enclosing quantifiers bind (for validation).
#[derive(Clone, Default)]
struct Scope {
    /// Inside `forEachActor` over this function (`Some(None)`: every
    /// actor).
    actor: Option<Option<String>>,
    /// The table of the innermost `forEachRow`.
    row: Option<String>,
}

fn check_formula(model: &Model, f: &Formula, scope: &Scope) -> Result<(), String> {
    match f {
        Formula::Eq(t)
        | Formula::Ne(t)
        | Formula::Lt(t)
        | Formula::Le(t)
        | Formula::Gt(t)
        | Formula::Ge(t)
        | Formula::SameGenerated(t)
        | Formula::DistinctGenerated(t)
        | Formula::Same(t) => t.iter().try_for_each(|x| check_term(model, x, scope)),
        Formula::And(v) | Formula::Or(v) => {
            v.iter().try_for_each(|x| check_formula(model, x, scope))
        }
        Formula::Not(x) => check_formula(model, x, scope),
        Formula::Implies(a, b) => {
            check_formula(model, a, scope)?;
            check_formula(model, b, scope)
        }
        Formula::GeneratedBy(g) => {
            check_term(model, &g.value, scope)?;
            if let Some(by) = &g.by {
                check_actor_sel(model, by, scope)?;
            }
            match &g.generator {
                Some(GeneratorSel::Column(c)) if column_generator(model, c).is_none() => Err(
                    format!("`{c}` is not a column with a generated default (`table.column`)"),
                ),
                _ => Ok(()),
            }
        }
        Formula::ForEachActor(q) => {
            if let Some(f) = &q.function
                && !model
                    .actors
                    .iter()
                    .any(|x| names_function(&x.function.name, f))
            {
                return Err(format!("no actor runs function `{f}`"));
            }
            let inner = Scope {
                actor: Some(q.function.clone()),
                row: scope.row.clone(),
            };
            check_formula(model, &q.formula, &inner)
        }
        Formula::ForEachRow(q) => {
            let Some(table) = model.schema.by_name(&q.table) else {
                return Err(format!("unknown table `{}`", q.table));
            };
            if let Some(c) = q.filter.keys().find(|c| !table.has_column(c)) {
                return Err(format!("unknown column `{}.{c}`", q.table));
            }
            let inner = Scope {
                actor: scope.actor.clone(),
                row: Some(q.table.clone()),
            };
            check_formula(model, &q.formula, &inner)
        }
    }
}

fn check_term(model: &Model, t: &Term, scope: &Scope) -> Result<(), String> {
    let Term::Op(op) = t else {
        return Ok(());
    };
    match op {
        TermOp::Initial(r) | TermOp::Row(r) | TermOp::Count(r) | TermOp::Sum(r) => validate_ref(
            model,
            r,
            matches!(op, TermOp::Sum(_) | TermOp::Initial(_) | TermOp::Row(_)),
        ),
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
        TermOp::Externals(_) | TermOp::App(_) | TermOp::Stuck(()) => Ok(()),
        TermOp::Add(a, b) | TermOp::Sub(a, b) => {
            check_term(model, a, scope)?;
            check_term(model, b, scope)
        }
        TermOp::CountMatching(m) | TermOp::RowMatching(m) => {
            let Some(table) = model.schema.by_name(&m.table) else {
                return Err(format!("unknown table `{}`", m.table));
            };
            for c in m.filter.keys().chain(m.matching.keys()) {
                if !table.has_column(c) {
                    return Err(format!("unknown column `{}.{c}`", m.table));
                }
            }
            match (&m.column, matches!(op, TermOp::RowMatching(_))) {
                (Some(c), _) if !table.has_column(c) => {
                    return Err(format!("unknown column `{}.{c}`", m.table));
                }
                (None, true) => {
                    return Err(format!("a `column` is required for table `{}`", m.table));
                }
                _ => {}
            }
            if let Some(by) = &m.inserted_by {
                check_actor_sel(model, by, scope)?;
            }
            m.matching
                .values()
                .try_for_each(|x| check_term(model, x, scope))
        }
        TermOp::Returned(r) => match &r.actor {
            Some(a) if model.actor_index(a).is_none() => Err(format!("no actor named `{a}`")),
            None if scope.actor.is_none() => Err(
                "`returned` without `actor` reads the actor bound by `forEachActor`: use it inside one, or name the actor"
                    .into(),
            ),
            _ => Ok(()),
        },
        TermOp::Argument(r) => {
            let head = r.path.split('.').next().unwrap_or_default();
            let has = |a: &crate::ir::ActorSpec| a.arguments.iter().any(|(n, _)| n == head);
            match (&r.actor, &scope.actor) {
                (Some(a), _) => match model.actor_index(a) {
                    None => Err(format!("no actor named `{a}`")),
                    Some(i) if !has(&model.actors[i]) => {
                        Err(format!("actor `{a}` has no argument `{head}`"))
                    }
                    Some(_) => Ok(()),
                },
                (None, None) => Err(
                    "`argument` without `actor` reads the actor bound by `forEachActor`: use it inside one, or name the actor"
                        .into(),
                ),
                (None, Some(f)) => match model.actors.iter().find(|a| {
                    f.as_ref()
                        .is_none_or(|f| names_function(&a.function.name, f))
                        && !has(a)
                }) {
                    Some(a) => Err(format!("actor `{}` has no argument `{head}`", a.name)),
                    None => Ok(()),
                },
            }
        }
        TermOp::Field(c) => match &scope.row {
            None => Err(format!(
                "`field` reads the row bound by `forEachRow`: `{c}` is used outside one"
            )),
            Some(table)
                if !model
                    .schema
                    .by_name(table)
                    .is_some_and(|t| t.has_column(c)) =>
            {
                Err(format!("unknown column `{table}.{c}`"))
            }
            Some(_) => Ok(()),
        },
    }
}

fn check_actor_sel(model: &Model, by: &ActorSel, scope: &Scope) -> Result<(), String> {
    match by {
        ActorSel::Bound if scope.actor.is_none() => Err(
            "`self` is the actor bound by `forEachActor`: use it inside one, or name the actor"
                .into(),
        ),
        ActorSel::Actor(a) if model.actor_index(a).is_none() => {
            Err(format!("no actor named `{a}`"))
        }
        ActorSel::Function(f)
            if !model
                .actors
                .iter()
                .any(|x| names_function(&x.function.name, f)) =>
        {
            Err(format!("no actor runs function `{f}`"))
        }
        _ => Ok(()),
    }
}

/// The generator index of `table.column`'s generated default.
fn column_generator(model: &Model, spec: &str) -> Option<u16> {
    let (table, column) = spec.split_once('.')?;
    let t = model.schema.index(table)?;
    model
        .generators()
        .iter()
        .position(|(ti, c, _)| *ti == t && c == column)
        .map(|i| i as u16)
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

/// What the enclosing quantifiers bind (for evaluation).
#[derive(Clone, Copy, Default)]
struct Env<'a> {
    actor: Option<usize>,
    row: Option<&'a Row>,
}

impl Formula {
    fn walk_terms(&self, f: &mut dyn FnMut(&Term)) {
        match self {
            Self::Eq(t)
            | Self::Ne(t)
            | Self::Lt(t)
            | Self::Le(t)
            | Self::Gt(t)
            | Self::Ge(t)
            | Self::SameGenerated(t)
            | Self::DistinctGenerated(t)
            | Self::Same(t) => {
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
            Self::GeneratedBy(g) => g.value.walk(f),
            Self::ForEachActor(q) => q.formula.walk_terms(f),
            Self::ForEachRow(q) => q.formula.walk_terms(f),
        }
    }

    /// Whether the formula uses a Phase 16 construct (generated
    /// identity, quantifiers, term-matched rows, returned values).
    pub fn uses_identity(&self) -> bool {
        let mut any = self.has_identity_formula();
        self.walk_terms(&mut |t| {
            if let Term::Op(
                TermOp::CountMatching(_)
                | TermOp::RowMatching(_)
                | TermOp::Returned(_)
                | TermOp::Field(_)
                | TermOp::Argument(_),
            ) = t
            {
                any = true;
            }
        });
        any
    }

    fn has_identity_formula(&self) -> bool {
        match self {
            Self::GeneratedBy(_)
            | Self::SameGenerated(_)
            | Self::DistinctGenerated(_)
            | Self::Same(_)
            | Self::ForEachActor(_)
            | Self::ForEachRow(_) => true,
            Self::And(v) | Self::Or(v) => v.iter().any(Self::has_identity_formula),
            Self::Not(x) => x.has_identity_formula(),
            Self::Implies(a, b) => a.has_identity_formula() || b.has_identity_formula(),
            Self::Eq(_) | Self::Ne(_) | Self::Lt(_) | Self::Le(_) | Self::Gt(_) | Self::Ge(_) => {
                false
            }
        }
    }

    /// Whether the formula reads what main tasks returned.
    pub fn reads_returned(&self) -> bool {
        let mut any = false;
        self.walk_terms(&mut |t| match t {
            Term::Op(TermOp::Actors(a)) if a.returned.is_some() => any = true,
            Term::Op(TermOp::Returned(_)) => any = true,
            _ => {}
        });
        any
    }

    /// The functions whose return values the formula reads
    /// (`None`: every actor's). Phase 16: `returned` of the actor bound by
    /// `forEachActor` reads that quantifier's function; `returned` of a
    /// named actor counts as every actor's.
    pub fn returned_functions(&self) -> Vec<Option<String>> {
        let mut out = Vec::new();
        self.collect_returned(None, &mut out);
        out
    }

    fn collect_returned(&self, bound: Option<&Option<String>>, out: &mut Vec<Option<String>>) {
        let mut visit = |t: &Term| {
            t.walk(&mut |x| match x {
                Term::Op(TermOp::Actors(a)) if a.returned.is_some() => {
                    out.push(a.function.clone());
                }
                Term::Op(TermOp::Returned(r)) => out.push(match (&r.actor, bound) {
                    (None, Some(f)) => f.clone(),
                    _ => None,
                }),
                _ => {}
            });
        };
        match self {
            Self::Eq(t)
            | Self::Ne(t)
            | Self::Lt(t)
            | Self::Le(t)
            | Self::Gt(t)
            | Self::Ge(t)
            | Self::SameGenerated(t)
            | Self::DistinctGenerated(t)
            | Self::Same(t) => t.iter().for_each(visit),
            Self::GeneratedBy(g) => visit(&g.value),
            Self::And(v) | Self::Or(v) => v.iter().for_each(|x| x.collect_returned(bound, out)),
            Self::Not(x) => x.collect_returned(bound, out),
            Self::Implies(a, b) => {
                a.collect_returned(bound, out);
                b.collect_returned(bound, out);
            }
            Self::ForEachActor(q) => q.formula.collect_returned(Some(&q.function), out),
            Self::ForEachRow(q) => q.formula.collect_returned(bound, out),
        }
    }

    /// Whether the formula reads only database rows (initial or committed),
    /// i.e. nothing a database replay cannot observe (actor outcomes,
    /// external effects, in-process variables). Phase 16: a formula with
    /// generated-identity constructs is never counted as database-only
    /// (which actor generated a value is not observed by a replay).
    pub fn reads_only_database(&self) -> bool {
        let mut only = !self.uses_identity();
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
        let mut only = !self.uses_identity();
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
        let mut only = !self.uses_identity();
        self.walk_terms(&mut |t| {
            if let Term::Op(TermOp::App(_) | TermOp::Stuck(())) = t {
                only = false;
            }
        });
        only
    }

    /// Visit the non-literal comparison operands (for reports). Phase 16:
    /// the operands of identity predicates are visited; the bodies of
    /// quantifiers are not (their terms depend on the binding; see
    /// [`Formula::diagnose`]).
    pub fn visit_terms(&self, f: &mut dyn FnMut(&Term)) {
        match self {
            Self::Eq(t)
            | Self::Ne(t)
            | Self::Lt(t)
            | Self::Le(t)
            | Self::Gt(t)
            | Self::Ge(t)
            | Self::SameGenerated(t)
            | Self::DistinctGenerated(t)
            | Self::Same(t) => {
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
            Self::GeneratedBy(g) => {
                if matches!(g.value, Term::Op(_)) {
                    f(&g.value);
                }
            }
            Self::ForEachActor(_) | Self::ForEachRow(_) => {}
        }
    }

    pub fn eval(&self, model: &Model, s: &State) -> bool {
        self.eval_in(model, s, Env::default())
    }

    fn eval_in(&self, model: &Model, s: &State, env: Env<'_>) -> bool {
        let t = |x: &Term| x.eval_in(model, s, env);
        match self {
            Self::And(v) => v.iter().all(|x| x.eval_in(model, s, env)),
            Self::Or(v) => v.iter().any(|x| x.eval_in(model, s, env)),
            Self::Not(x) => !x.eval_in(model, s, env),
            Self::Implies(a, b) => !a.eval_in(model, s, env) || b.eval_in(model, s, env),
            Self::Eq([a, b]) => cmp(t(a), t(b), |o| o.is_eq(), true),
            Self::Ne([a, b]) => cmp(t(a), t(b), |o| o.is_ne(), true),
            Self::Lt([a, b]) => cmp(t(a), t(b), |o| o.is_lt(), false),
            Self::Le([a, b]) => cmp(t(a), t(b), |o| o.is_le(), false),
            Self::Gt([a, b]) => cmp(t(a), t(b), |o| o.is_gt(), false),
            Self::Ge([a, b]) => cmp(t(a), t(b), |o| o.is_ge(), false),
            Self::GeneratedBy(g) => generated_by(model, env, &t(&g.value), g),
            Self::SameGenerated([a, b]) => match (t(a), t(b)) {
                (Value::Gen(x), Value::Gen(y)) => is_identifier(&x) && is_identifier(&y) && x == y,
                _ => false,
            },
            Self::DistinctGenerated([a, b]) => match (t(a), t(b)) {
                (Value::Gen(x), Value::Gen(y)) => is_identifier(&x) && is_identifier(&y) && x != y,
                _ => false,
            },
            Self::Same([a, b]) => {
                let (x, y) = (t(a), t(b));
                !x.is_nullish() && x == y
            }
            Self::ForEachActor(q) => bound_actors(model, s, q).all(|i| {
                q.formula.eval_in(
                    model,
                    s,
                    Env {
                        actor: Some(i),
                        ..env
                    },
                )
            }),
            Self::ForEachRow(q) => {
                let Some(ti) = model.schema.index(&q.table) else {
                    return false;
                };
                s.committed_rows(model, ti)
                    .iter()
                    .filter(|r| q.filter.iter().all(|(c, v)| r.get(c) == Some(v)))
                    .all(|r| {
                        q.formula.eval_in(
                            model,
                            s,
                            Env {
                                row: Some(r),
                                ..env
                            },
                        )
                    })
            }
        }
    }

    /// Why the formula is false in `s`, one line per failing
    /// identity predicate (inside quantifiers: per failing instance, with
    /// its binding), classified so that a generated-identity mismatch reads
    /// differently from an absent value (a missing row, or a read through a
    /// lagging replica that has not observed a write). Empty for formulas
    /// without Phase 16 constructs and for formulas that hold.
    pub fn diagnose(&self, model: &Model, s: &State) -> Vec<String> {
        let mut out = Vec::new();
        if self.uses_identity() && !self.eval(model, s) {
            self.diagnose_in(model, s, Env::default(), "", &mut out);
        }
        out
    }

    fn diagnose_in(
        &self,
        model: &Model,
        s: &State,
        env: Env<'_>,
        prefix: &str,
        out: &mut Vec<String>,
    ) {
        if self.eval_in(model, s, env) {
            return;
        }
        let val = |x: &Term| x.eval_in(model, s, env);
        let line = |text: String| format!("{prefix}{text}");
        match self {
            Self::And(v) => v
                .iter()
                .for_each(|x| x.diagnose_in(model, s, env, prefix, out)),
            Self::Or(v) => {
                // Every alternative failed: report each.
                v.iter()
                    .for_each(|x| x.diagnose_in(model, s, env, prefix, out));
            }
            Self::Implies(_, b) => b.diagnose_in(model, s, env, prefix, out),
            Self::Not(x) => out.push(line(format!("`{}` holds", x.render()))),
            Self::Eq(t) | Self::Ne(t) | Self::Lt(t) | Self::Le(t) | Self::Gt(t) | Self::Ge(t) => {
                out.push(line(format!(
                    "`{}` is false: {} = {}, {} = {}",
                    self.render(),
                    t[0].render(),
                    describe(model, &val(&t[0])),
                    t[1].render(),
                    describe(model, &val(&t[1]))
                )));
            }
            Self::GeneratedBy(g) => {
                let v = val(&g.value);
                out.push(line(format!(
                    "{}: {} = {}, expected a value generated{}",
                    classify(model, std::slice::from_ref(&v)),
                    g.value.render(),
                    describe(model, &v),
                    render_generated_by(g, model, env)
                )));
            }
            Self::SameGenerated(t) | Self::DistinctGenerated(t) | Self::Same(t) => {
                let (a, b) = (val(&t[0]), val(&t[1]));
                out.push(line(format!(
                    "{}: `{}` is false: {} = {}, {} = {}",
                    classify(model, &[a.clone(), b.clone()]),
                    self.render(),
                    t[0].render(),
                    describe(model, &a),
                    t[1].render(),
                    describe(model, &b)
                )));
            }
            Self::ForEachActor(q) => {
                for i in bound_actors(model, s, q) {
                    let inner = Env {
                        actor: Some(i),
                        ..env
                    };
                    let p = format!("{prefix}actor `{}`: ", model.actors[i].name);
                    q.formula.diagnose_in(model, s, inner, &p, out);
                }
            }
            Self::ForEachRow(q) => {
                let Some(ti) = model.schema.index(&q.table) else {
                    return;
                };
                for r in s
                    .committed_rows(model, ti)
                    .iter()
                    .filter(|r| q.filter.iter().all(|(c, v)| r.get(c) == Some(v)))
                {
                    let inner = Env {
                        row: Some(r),
                        ..env
                    };
                    let p = format!(
                        "{prefix}row {} of `{}`: ",
                        Value::Row(r.clone()).render(),
                        q.table
                    );
                    q.formula.diagnose_in(model, s, inner, &p, out);
                }
            }
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
            Self::GeneratedBy(g) => format!(
                "{} is generated{}",
                g.value.render(),
                render_generated_by_static(g)
            ),
            Self::SameGenerated(t) => format!(
                "{} is the same generated value as {}",
                t[0].render(),
                t[1].render()
            ),
            Self::DistinctGenerated(t) => format!(
                "{} and {} are distinct generated values",
                t[0].render(),
                t[1].render()
            ),
            Self::Same(t) => format!("{} is the same value as {}", t[0].render(), t[1].render()),
            Self::ForEachActor(q) => format!(
                "for every {}{}: ({})",
                if q.completed { "completed " } else { "" },
                q.function
                    .as_deref()
                    .map(|f| format!("`{f}` actor"))
                    .unwrap_or_else(|| "actor".to_owned()),
                q.formula.render()
            ),
            Self::ForEachRow(q) => format!(
                "for every row of {}: ({})",
                RowRef {
                    table: q.table.clone(),
                    filter: q.filter.clone(),
                    column: None,
                }
                .render(),
                q.formula.render()
            ),
        }
    }
}

/// The actors a `forEachActor` ranges over.
fn bound_actors<'a>(
    model: &'a Model,
    s: &'a State,
    q: &'a ForEachActor,
) -> impl Iterator<Item = usize> + 'a {
    (0..model.actors.len()).filter(move |&i| {
        q.function
            .as_ref()
            .is_none_or(|f| names_function(&model.actors[i].function.name, f))
            && (!q.completed || s.actors[i].completions > 0)
    })
}

/// A generated identifier, as opposed to a clock reading or a
/// clock placeholder (Phase 14 timestamps are also [`Value::Gen`]).
fn is_identifier(g: &crate::value::GenId) -> bool {
    !crate::db::is_clock(g) && g.actor != crate::db::CLOCK_PLACEHOLDER
}

fn actor_matches(model: &Model, env: Env<'_>, by: &ActorSel, actor: usize) -> bool {
    match by {
        ActorSel::Bound => env.actor == Some(actor),
        ActorSel::Actor(a) => model.actor_index(a) == Some(actor),
        ActorSel::Function(f) => model
            .actors
            .get(actor)
            .is_some_and(|x| names_function(&x.function.name, f)),
    }
}

fn generated_by(model: &Model, env: Env<'_>, v: &Value, g: &GeneratedBy) -> bool {
    let Value::Gen(id) = v else {
        return false;
    };
    if !is_identifier(id) {
        return false;
    }
    if let Some(by) = &g.by
        && !actor_matches(model, env, by, id.actor as usize)
    {
        return false;
    }
    match &g.generator {
        None => true,
        Some(GeneratorSel::Call) => id.generator == crate::ir::CALL_GENERATOR,
        Some(GeneratorSel::Column(c)) => column_generator(model, c) == Some(id.generator),
    }
}

fn render_actor_sel(by: &ActorSel) -> String {
    match by {
        ActorSel::Bound => "self".to_owned(),
        ActorSel::Actor(a) => format!("actor `{a}`"),
        ActorSel::Function(f) => format!("a `{f}` actor"),
    }
}

fn render_generator(g: &GeneratorSel) -> String {
    match g {
        GeneratorSel::Call => "a package call".to_owned(),
        GeneratorSel::Column(c) => format!("the default of `{c}`"),
    }
}

fn render_generated_by_static(g: &GeneratedBy) -> String {
    let mut out = String::new();
    if let Some(by) = &g.by {
        out.push_str(&format!(" by {}", render_actor_sel(by)));
    }
    if let Some(gen_sel) = &g.generator {
        out.push_str(&format!(" from {}", render_generator(gen_sel)));
    }
    out
}

/// As [`render_generated_by_static`], naming the bound actor.
fn render_generated_by(g: &GeneratedBy, model: &Model, env: Env<'_>) -> String {
    match (&g.by, env.actor) {
        (Some(ActorSel::Bound), Some(i)) => {
            let rest = GeneratedBy {
                value: g.value.clone(),
                by: Some(ActorSel::Actor(model.actors[i].name.clone())),
                generator: g.generator.clone(),
            };
            render_generated_by_static(&rest)
        }
        _ => render_generated_by_static(g),
    }
}

/// A value with, for a generated value, the generation event it
/// names (actor and generator), for diagnoses.
fn describe(model: &Model, v: &Value) -> String {
    match v {
        Value::Gen(g) if is_identifier(g) => {
            let actor = model
                .actors
                .get(g.actor as usize)
                .map_or_else(|| format!("#{}", g.actor), |a| format!("`{}`", a.name));
            let generator = if g.generator == crate::ir::CALL_GENERATOR {
                "a package call".to_owned()
            } else {
                model.generators().get(g.generator as usize).map_or_else(
                    || format!("generator {}", g.generator),
                    |(t, c, _)| format!("the default of `{}.{c}`", model.schema.table(*t).name),
                )
            };
            format!(
                "{} (generated by actor {actor} from {generator}, its generation event {})",
                v.render(),
                g.n
            )
        }
        Value::Gen(_) => format!("{} (a clock reading)", v.render()),
        _ => v.render(),
    }
}

/// The kind of an identity predicate's failure.
fn classify(model: &Model, values: &[Value]) -> String {
    if values.iter().any(Value::is_nullish) {
        if model.features.replicas {
            "absent value (a missing row or value; with the modeled asynchronous replica, also what a read that has not yet observed a write returns: stale, which eventual consistency permits unless a declared property requires the write to be visible)".to_owned()
        } else {
            "absent value (a missing row or value)".to_owned()
        }
    } else if values
        .iter()
        .all(|v| matches!(v, Value::Gen(g) if is_identifier(g)))
    {
        "generated-identity mismatch".to_owned()
    } else {
        "not a generated value of the expected identity".to_owned()
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
        match self {
            Self::Op(TermOp::Add(a, b) | TermOp::Sub(a, b)) => {
                a.walk(f);
                b.walk(f);
            }
            Self::Op(TermOp::CountMatching(m) | TermOp::RowMatching(m)) => {
                m.matching.values().for_each(|x| x.walk(f));
            }
            _ => {}
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
                TermOp::CountMatching(m) => format!("count({})", m.render()),
                TermOp::RowMatching(m) => format!(
                    "committed({}).{}",
                    m.render(),
                    m.column.as_deref().unwrap_or_default()
                ),
                TermOp::Returned(r) => format!(
                    "returned({}){}",
                    r.actor.as_deref().unwrap_or("self"),
                    r.path
                        .as_deref()
                        .map(|p| format!(".{p}"))
                        .unwrap_or_default()
                ),
                TermOp::Field(c) => format!("row.{c}"),
                TermOp::Argument(r) => format!(
                    "argument({}).{}",
                    r.actor.as_deref().unwrap_or("self"),
                    r.path
                ),
            },
        }
    }

    pub fn eval(&self, model: &Model, s: &State) -> Value {
        self.eval_in(model, s, Env::default())
    }

    fn eval_in(&self, model: &Model, s: &State, env: Env<'_>) -> Value {
        match self {
            Self::Int(n) => Value::Int(*n),
            Self::Str(x) => Value::Str(x.clone()),
            Self::Null(()) => Value::Null,
            Self::Op(op) => eval_op(op, model, s, env),
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

impl RowMatch {
    fn render(&self) -> String {
        let mut parts: Vec<String> = self
            .filter
            .iter()
            .map(|(k, v)| format!("{k}={}", v.render()))
            .collect();
        parts.extend(
            self.matching
                .iter()
                .map(|(k, t)| format!("{k}={}", t.render())),
        );
        let mut out = format!("{}[{}]", self.table, parts.join(",")).replace("[]", "");
        if let Some(by) = &self.inserted_by {
            out.push_str(&format!(" inserted by {}", render_actor_sel(by)));
        }
        out
    }

    /// The committed rows selected, in key order.
    fn rows(&self, model: &Model, s: &State, env: Env<'_>) -> Vec<Row> {
        let Some(t) = model.schema.index(&self.table) else {
            return Vec::new();
        };
        let wanted: Vec<(&String, Value)> = self
            .matching
            .iter()
            .map(|(c, x)| (c, x.eval_in(model, s, env)))
            .collect();
        if wanted.iter().any(|(_, v)| v.is_nullish()) {
            return Vec::new();
        }
        let snap = s.committed_snapshot();
        let schema = model.schema.table(t);
        let mut rows: Vec<Row> = s
            .store
            .table_rows(t)
            .filter(|id| {
                self.inserted_by.as_ref().is_none_or(|by| {
                    id.actor != crate::db::RowId::INITIAL
                        && actor_matches(model, env, by, id.actor as usize)
                })
            })
            .filter_map(|id| s.store.visible_values(id, snap).cloned())
            .filter(|r| self.filter.iter().all(|(c, v)| r.get(c) == Some(v)))
            .filter(|r| wanted.iter().all(|(c, v)| r.get(*c) == Some(v)))
            .collect();
        rows.sort_by_key(|r| crate::db::pk_key(schema, r));
        rows
    }
}

/// The field `path` (`a.b`) of a value (`null` when absent).
fn field_path(v: Value, path: Option<&str>) -> Value {
    let Some(path) = path else {
        return v;
    };
    let mut cur = v;
    for key in path.split('.') {
        cur = match cur {
            Value::Row(mut r) => r.remove(key).unwrap_or(Value::Null),
            _ => return Value::Null,
        };
    }
    cur
}

fn eval_op(op: &TermOp, model: &Model, s: &State, env: Env<'_>) -> Value {
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
                    // Completions that returned `v`. An unknown
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
        TermOp::Add(a, b) | TermOp::Sub(a, b) => {
            match (a.eval_in(model, s, env), b.eval_in(model, s, env)) {
                (Value::Int(x), Value::Int(y)) => {
                    let r = if matches!(op, TermOp::Add(..)) {
                        x.checked_add(y)
                    } else {
                        x.checked_sub(y)
                    };
                    r.map_or(Value::Null, Value::Int)
                }
                _ => Value::Null,
            }
        }
        TermOp::CountMatching(m) => Value::Int(m.rows(model, s, env).len() as i64),
        TermOp::RowMatching(m) => m
            .rows(model, s, env)
            .into_iter()
            .next()
            .and_then(|row| m.column.as_ref().and_then(|c| row.get(c).cloned()))
            .unwrap_or(Value::Null),
        TermOp::Returned(r) => {
            let actor = match &r.actor {
                Some(name) => model.actor_index(name),
                None => env.actor,
            };
            // The latest completed attempt; an unknown return value is
            // refused when the scenario is loaded.
            let v = actor
                .and_then(|i| s.actors.get(i))
                .and_then(|a| a.returned.last())
                .and_then(|r| r.as_ref().ok().cloned())
                .unwrap_or(Value::Null);
            field_path(v, r.path.as_deref())
        }
        TermOp::Field(c) => env
            .row
            .and_then(|r| r.get(c).cloned())
            .unwrap_or(Value::Null),
        TermOp::Argument(r) => {
            let actor = match &r.actor {
                Some(name) => model.actor_index(name),
                None => env.actor,
            };
            let (head, rest) = match r.path.split_once('.') {
                Some((h, rest)) => (h, Some(rest)),
                None => (r.path.as_str(), None),
            };
            let v = actor
                .and_then(|i| model.actors.get(i))
                .and_then(|a| a.arguments.iter().find(|(n, _)| n == head))
                .map(|(_, v)| v.clone())
                .unwrap_or(Value::Null);
            field_path(v, rest)
        }
    }
}

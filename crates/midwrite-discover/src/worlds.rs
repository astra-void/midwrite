//! Initial worlds and feedback-guided input search.
//!
//! A *world* is one choice of initial rows and actor arguments for a
//! scenario variant. The base world is the Phase 11–14 synthesis. Further
//! worlds change one input relationship at a time and are built by the same
//! synthesizer, so every world satisfies the modeled constraints and lets
//! each actor reach the candidate's operations (or is rejected and reported):
//!
//! * **domain worlds** take a parameter path to another class of its typed
//!   domain (`null` where the annotation admits it, an integer at a branch
//!   boundary, the literal a branch compares with), for every actor that
//!   reads the path (the actors' relationship is kept);
//! * **relationship worlds** come from the base world: where a stored value
//!   equals a value an actor writes to the same column, a world where it
//!   differs (and the converse), so that a stale read is distinguishable from
//!   a fresh one;
//! * **feedback** orders them: a world whose class flips a branch no solo
//!   run of an explored world took, or whose column an actor also reads back,
//!   comes first.
//!
//! Worlds are deduplicated by a fingerprint of everything the model checker
//! is given. The search stops at the first validated counterexample (unless
//! asked to explore every world) or at the world budget; everything not
//! explored is reported (`docs/architecture/world-search.md`).

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use midwrite_model::value::{Row, Value};

use crate::candidate::Candidate;
use crate::detect::Analyzed;
use crate::domains::{PathDomain, ValueClass};
use crate::effects::Val;
use crate::project::ProjectIndex;
use crate::synth::{Plan, get_path, uuid_literal};

/// Where a world comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorldOrigin {
    /// The Phase 11–14 synthesis.
    Base,
    /// Another class of a parameter's typed domain.
    Domain,
    /// A stored value related differently to a value an actor writes.
    Relationship,
}

/// A change of one argument path, for some actors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArgOverride {
    /// The actors (indexes in the scenario) whose argument changes.
    pub actors: Vec<usize>,
    pub path: String,
    pub value: Value,
    /// The key is removed from its object instead (`value` is
    /// then unused).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub absent: bool,
}

/// A change of one cell of the base world's synthesized rows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CellOverride {
    pub table: String,
    pub row: usize,
    pub column: String,
    pub value: Value,
}

/// One initial world to build.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldSpec {
    /// Stable name (`base`, `null:parentId`, `stored-distinct:accounts.displayName`).
    pub id: String,
    pub origin: WorldOrigin,
    /// The input relationships this world establishes, in words.
    pub relationships: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<ArgOverride>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cells: Vec<CellOverride>,
    /// Why it was explored in this order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
    /// Lower first.
    #[serde(skip)]
    pub priority: u8,
    /// The parameter paths it changes (for branch feedback).
    #[serde(skip)]
    pub paths: Vec<String>,
}

impl WorldSpec {
    pub fn base() -> Self {
        Self {
            id: "base".to_owned(),
            origin: WorldOrigin::Base,
            relationships: vec!["the synthesized initial state and arguments".to_owned()],
            args: Vec::new(),
            cells: Vec::new(),
            feedback: None,
            priority: 0,
            paths: Vec::new(),
        }
    }

    pub fn is_base(&self) -> bool {
        self.origin == WorldOrigin::Base
    }
}

/// What solo runs of the actors on a world's final rows observed.
#[derive(Clone, Debug, Default)]
pub struct Observations {
    /// Branch outcomes by (actor selector, location): (condition false
    /// seen, condition true seen).
    pub branches: BTreeMap<(String, String), (bool, bool)>,
    /// Source locations executed, per actor selector.
    pub reached: BTreeMap<String, BTreeSet<String>>,
}

impl Observations {
    pub fn merge(&mut self, other: &Observations) {
        for (k, (f, j)) in &other.branches {
            let e = self.branches.entry(k.clone()).or_default();
            e.0 |= f;
            e.1 |= j;
        }
        for (k, v) in &other.reached {
            self.reached
                .entry(k.clone())
                .or_default()
                .extend(v.iter().cloned());
        }
    }

    /// Branch outcomes taken (each branch has two).
    pub fn outcomes(&self) -> usize {
        self.branches
            .values()
            .map(|(f, j)| usize::from(*f) + usize::from(*j))
            .sum()
    }
}

/// Domain worlds of a candidate: for every path of every actor whose domain
/// has a class besides the synthesized value. Actors that read the same path
/// get the same class (their relationship to each other is kept).
pub fn domain_worlds(
    c: &Candidate,
    analyzed: &[&Analyzed],
    domains: &[Vec<PathDomain>],
    base_args: &[Vec<Value>],
) -> Vec<WorldSpec> {
    let mut by_class: BTreeMap<(String, ValueClass), Vec<usize>> = BTreeMap::new();
    for (k, &ai) in c.actor_entries.iter().enumerate() {
        let Some(ds) = domains.get(k) else { continue };
        let _ = ai;
        for d in ds {
            for class in &d.classes {
                by_class
                    .entry((d.path.clone(), class.clone()))
                    .or_default()
                    .push(k);
            }
        }
    }
    let mut out = Vec::new();
    for ((path, class), actors) in by_class {
        let mut args = Vec::new();
        let mut changed = Vec::new();
        for &k in &actors {
            let a = analyzed[c.actor_entries[k]];
            let at = |p: &str| {
                base_args
                    .get(k)
                    .and_then(|args| get_path(args, &a.summary.params, p))
            };
            let base = at(&path);
            // A key of a zod-validated object may be absent from
            // the base arguments (an optional key): it can be set when its
            // object exists.
            let parent_exists = match path.rsplit_once('.') {
                Some((parent, _)) => matches!(at(parent), Some(Value::Row(_))),
                None => true,
            };
            // `Some(None)`: remove the key.
            let new: Option<Option<Value>> = match (&class, base) {
                (ValueClass::Absent, Some(_)) => Some(None),
                (ValueClass::Present { value }, None) if parent_exists => Some(Some(value.clone())),
                (ValueClass::Null, None) if parent_exists => Some(Some(Value::Null)),
                (ValueClass::Undefined, None) if parent_exists => Some(Some(Value::undefined())),
                (ValueClass::Absent | ValueClass::Present { .. }, _) | (_, None) => None,
                (_, Some(base)) => class.value(&base).map(Some),
            };
            if let Some(v) = new {
                changed.push(k);
                args.push((k, v));
            }
        }
        if args.is_empty() {
            continue;
        }
        // One override per distinct value (actors with the same base value
        // get the same new value).
        let mut overrides: Vec<ArgOverride> = Vec::new();
        for (k, v) in args {
            let absent = v.is_none();
            let v = v.unwrap_or_else(Value::undefined);
            match overrides
                .iter_mut()
                .find(|o| o.value == v && o.absent == absent)
            {
                Some(o) => o.actors.push(k),
                None => overrides.push(ArgOverride {
                    actors: vec![k],
                    path: path.clone(),
                    value: v,
                    absent,
                }),
            }
        }
        let who = if changed.len() == c.actor_entries.len() {
            "every actor".to_owned()
        } else {
            format!(
                "actor(s) {}",
                changed
                    .iter()
                    .map(|k| (k + 1).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let (tag, priority) = match &class {
            ValueClass::Null => ("null", 20),
            ValueClass::Undefined => ("undefined", 22),
            ValueClass::EqualToOperand { .. } => ("operand", 30),
            ValueClass::BelowOperand { .. } => ("below", 32),
            ValueClass::AboveOperand { .. } => ("above", 32),
            ValueClass::Negated => ("negated", 34),
            ValueClass::Absent => ("absent", 21),
            ValueClass::Present { .. } => ("present", 23),
        };
        let suffix = match &class {
            ValueClass::EqualToOperand { operand } => format!("={}", operand.render()),
            ValueClass::BelowOperand { operand } | ValueClass::AboveOperand { operand } => {
                format!("={operand}")
            }
            _ => String::new(),
        };
        out.push(WorldSpec {
            id: format!("{tag}:{path}{suffix}"),
            origin: WorldOrigin::Domain,
            relationships: vec![format!("{} for {who}", class.describe(&path))],
            args: overrides,
            cells: Vec::new(),
            feedback: None,
            priority,
            paths: vec![path],
        });
    }
    out
}

/// Actor-relationship worlds: the actors of an actual (same-key) variant
/// share every argument; for each argument outside the candidate's key that
/// they share, a world where the later actors pass another valid value (the
/// actors still use the same key). A value that only selects behavior (a
/// kind, a flag, an amount) can decide whether a unique key covers the
/// conflict.
pub fn actor_worlds(
    c: &Candidate,
    analyzed: &[&Analyzed],
    index: &ProjectIndex,
    base_args: &[Vec<Value>],
) -> Vec<WorldSpec> {
    // Only where the actors are independent requests that share a key (the
    // classes that also get a distinct-key control): actors that model one
    // request delivered or retried more than once must keep identical
    // arguments.
    let independent = matches!(
        c.class,
        crate::candidate::Class::CheckThenAct
            | crate::candidate::Class::ReadModifyWrite
            | crate::candidate::Class::StatusTransition
    ) && c.environment.retry.is_none();
    if !independent
        || c.actor_entries.len() < 2
        || c.actor_entries.iter().any(|&e| e != c.actor_entries[0])
        || base_args.len() < 2
    {
        return Vec::new();
    }
    let a = analyzed[c.actor_entries[0]];
    let keys = crate::synth::varied_paths(c, a, crate::candidate::Relation::DistinctKey);
    // The column each path is written to or compared with.
    let mut column: BTreeMap<String, (String, String)> = BTreeMap::new();
    for e in &a.summary.db {
        for (col, v) in e.writes.iter().chain(e.eq.iter()) {
            if let Val::Param { path } = v {
                column
                    .entry(path.clone())
                    .or_insert_with(|| (e.table.clone(), col.clone()));
            }
        }
    }
    // Arguments that take part in a read's predicate without
    // being a recognized key: written to a column the predicate constrains
    // in a conjunct that is not a plain `column = value` (a null-aware
    // equality chosen by a conditional, `IS NULL`, a range), or compared
    // with a column through a computation (`name = trim(lower($name))`).
    // Varying such an argument between the actors changes what the check
    // means, so the actors would no longer "share the key".
    let mut checked: BTreeSet<(String, String)> = BTreeSet::new();
    let mut in_predicate: BTreeSet<String> = BTreeSet::new();
    for e in a
        .summary
        .db
        .iter()
        .filter(|e| matches!(e.access, crate::effects::Access::Read))
    {
        for (_, cols) in &e.other {
            checked.extend(cols.iter().map(|c| (e.table.clone(), c.clone())));
        }
        for (_, v) in &e.eq {
            if matches!(v, Val::Param { .. }) {
                continue;
            }
            let mut leaves = Vec::new();
            v.leaves(&mut leaves);
            in_predicate.extend(leaves.into_iter().filter_map(|l| match l {
                Val::Param { path } => Some(path),
                _ => None,
            }));
        }
    }
    let annotations = midwrite_analyzer::TsActorProvider::new(&index.root)
        .param_annotations(&index.root, &a.fact.selector);
    let mut out = Vec::new();
    for path in crate::synth::param_paths(a, index).keys() {
        if keys.contains(path)
            || in_predicate.contains(path)
            || column.get(path).is_some_and(|tc| checked.contains(tc))
        {
            continue;
        }
        // A zod enum is a literal domain too.
        let literals = annotations
            .get(path)
            .and_then(|t| crate::domains::literal_members(t))
            .or_else(|| crate::domains::zod_enum_members(a).remove(path));
        let first = get_path(&base_args[0], &a.summary.params, path);
        let Some(first) = first.filter(|v| !v.is_null()) else {
            continue;
        };
        if (1..base_args.len())
            .any(|k| get_path(&base_args[k], &a.summary.params, path).as_ref() != Some(&first))
        {
            continue;
        }
        let other = match (&literals, column.get(path)) {
            // A literal union admits only its members.
            (Some(m), _) => m
                .iter()
                .map(|x| Value::Str(x.clone()))
                .find(|x| *x != first),
            (None, Some((t, col))) => distinct_value(index, t, col, &first),
            (None, None) => match &first {
                Value::Int(n) => n.checked_add(1).map(Value::Int),
                Value::Bool(b) => Some(Value::Bool(!b)),
                Value::Str(s) => Some(Value::Str(format!("{s}-b"))),
                _ => None,
            },
        };
        let Some(other) = other else { continue };
        // A value the path's zod schema rejects is not a valid
        // input.
        if crate::domains::zod_accepts(a, path, &other) == Some(false) {
            continue;
        }
        out.push(WorldSpec {
            id: format!("actors-differ:{path}"),
            origin: WorldOrigin::Domain,
            relationships: vec![format!(
                "the actors share the key but pass different `{path}` ({} and {})",
                first.render(),
                other.render()
            )],
            args: vec![ArgOverride {
                actors: (1..base_args.len()).collect(),
                path: path.clone(),
                value: other,
                absent: false,
            }],
            cells: Vec::new(),
            feedback: None,
            priority: 26,
            paths: vec![path.clone()],
        });
    }
    out
}

/// Columns that identify a row (a primary key, a unique key, a foreign key):
/// changing them changes which row a statement addresses, which the
/// actors' key relation already varies.
pub(crate) fn identity_column(index: &ProjectIndex, table: &str, column: &str) -> bool {
    let Some(t) = index.source_table(table) else {
        return true;
    };
    let Some(c) = t.column(column) else {
        return true;
    };
    c.primary_key
        || c.references.is_some()
        || t.primary_key.contains(&c.sql_name)
        || t.foreign_keys
            .iter()
            .any(|f| f.columns.contains(&c.sql_name))
        || t.unique.iter().any(|u| u.columns.contains(&c.sql_name))
}

/// A valid value of `table.column` different from `v`, or `None` when the
/// column's type has no other value the model represents.
pub fn distinct_value(index: &ProjectIndex, table: &str, column: &str, v: &Value) -> Option<Value> {
    let c = index.source_table(table)?.column(column)?;
    if !c.enum_values.is_empty() {
        return c
            .enum_values
            .iter()
            .map(|e| Value::Str(e.clone()))
            .find(|e| e != v);
    }
    let out = match (c.builder.as_str(), v) {
        ("uuid", _) => uuid_literal(&format!("stored-{column}"), 7),
        ("timestamp", Value::Str(s)) if s.starts_with("2025-06-01") => {
            Value::str("2026-01-01T00:00:00.000Z")
        }
        ("timestamp", _) => Value::str("2025-06-01T00:00:00.000Z"),
        ("date", Value::Str(s)) if s == "2025-06-01" => Value::str("2026-01-01"),
        ("date", _) => Value::str("2025-06-01"),
        ("json" | "jsonb", _) => return None,
        (_, Value::Int(n)) => Value::Int(n.checked_add(1)?),
        (_, Value::Bool(b)) => Value::Bool(!b),
        (_, Value::Str(s)) => {
            // Short, within any declared varchar length.
            let limit = c
                .sql_type
                .strip_prefix("varchar(")
                .and_then(|r| r.strip_suffix(')'))
                .and_then(|n| n.parse::<usize>().ok());
            let candidate = format!("{column}-stored");
            Value::Str(match limit {
                Some(n) if candidate.chars().count() > n => {
                    if s == "x" { "y" } else { "x" }.to_owned()
                }
                _ if candidate == *s => format!("{column}-stored-2"),
                _ => candidate,
            })
        }
        _ => return None,
    };
    (out != *v).then_some(out)
}

/// Relationship worlds from a built base world: for every column an actor
/// writes from a parameter, the stored values of that table's synthesized
/// rows are set to the written value (when they differ) or to another valid
/// value (when they coincide). `reads` lists, per actor, the columns its
/// code reads back from tables it also writes (`table.column`): such a
/// world comes first.
pub fn relationship_worlds(
    c: &Candidate,
    analyzed: &[&Analyzed],
    index: &ProjectIndex,
    base: &Plan,
) -> Vec<WorldSpec> {
    // (table, column) → written values, with the actors writing them.
    let mut written: BTreeMap<(String, String), Vec<(usize, Value)>> = BTreeMap::new();
    let mut read_back: BTreeSet<(String, String)> = BTreeSet::new();
    for (k, &ai) in c.actor_entries.iter().enumerate() {
        let a = analyzed[ai];
        let Some(args) = base.actors.get(k).map(|p| &p.arguments) else {
            continue;
        };
        for e in &a.summary.db {
            if e.access.writes() {
                for (col, v) in &e.writes {
                    if let Val::Param { path } = v
                        && let Some(value) = get_path(args, &a.summary.params, path)
                        && !value.is_null()
                    {
                        written
                            .entry((e.table.clone(), col.clone()))
                            .or_default()
                            .push((k, value));
                    }
                }
            }
        }
        // Columns read back: a later read's result columns are not
        // summarized per column; a read of a written table counts for each
        // written column of it.
        for e in a.summary.db.iter().filter(|e| !e.access.writes()) {
            for ((t, col), _) in written.iter().filter(|((t, _), _)| *t == e.table) {
                read_back.insert((t.clone(), col.clone()));
            }
        }
    }
    let mut out = Vec::new();
    for ((table, column), values) in &written {
        if identity_column(index, table, column) {
            continue;
        }
        let Some(rows) = base.rows.get(table) else {
            continue;
        };
        let read = read_back.contains(&(table.clone(), column.clone()));
        let first = &values[0].1;
        // Stored == written → distinct world.
        let equal_rows: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| values.iter().any(|(_, v)| r.get(column) == Some(v)))
            .map(|(i, _)| i)
            .collect();
        if !equal_rows.is_empty() {
            let cells: Vec<CellOverride> = equal_rows
                .iter()
                .filter_map(|&i| {
                    let cur = rows[i].get(column)?;
                    let mut v = distinct_value(index, table, column, cur)?;
                    // Different from every written value too.
                    let mut guard = 0;
                    while values.iter().any(|(_, w)| *w == v) && guard < 4 {
                        v = distinct_value(index, table, column, &v)?;
                        guard += 1;
                    }
                    Some(CellOverride {
                        table: table.clone(),
                        row: i,
                        column: column.clone(),
                        value: v,
                    })
                })
                .collect();
            if !cells.is_empty() {
                out.push(WorldSpec {
                    id: format!("stored-distinct:{table}.{column}"),
                    origin: WorldOrigin::Relationship,
                    relationships: vec![format!(
                        "the stored `{table}.{column}` differs from the value an actor writes ({})",
                        first.render()
                    )],
                    args: Vec::new(),
                    cells,
                    feedback: Some(format!(
                        "in the base world the stored `{table}.{column}` equals the written value, so an execution that reads the old value cannot be told from one that reads the new one{}",
                        if read { "; an actor reads the column back" } else { "" }
                    )),
                    priority: if read { 10 } else { 40 },
                    paths: Vec::new(),
                });
            }
        }
        // Stored != written → equal world (same type only).
        let differing: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                r.get(column).is_some_and(|cur| {
                    !values.iter().any(|(_, v)| v == cur)
                        && std::mem::discriminant(cur) == std::mem::discriminant(first)
                })
            })
            .map(|(i, _)| i)
            .collect();
        if !differing.is_empty() {
            out.push(WorldSpec {
                id: format!("stored-equal:{table}.{column}"),
                origin: WorldOrigin::Relationship,
                relationships: vec![format!(
                    "the stored `{table}.{column}` equals the value an actor writes ({})",
                    first.render()
                )],
                args: Vec::new(),
                cells: differing
                    .iter()
                    .map(|&i| CellOverride {
                        table: table.clone(),
                        row: i,
                        column: column.clone(),
                        value: first.clone(),
                    })
                    .collect(),
                feedback: None,
                priority: 50,
                paths: Vec::new(),
            });
        }
    }
    out
}

/// Stored-value boundary worlds. A guard that compares a column
/// of a row the actor read with an integer (`quota.remaining <= 0`) has
/// three relevant stored values: one below, at and one above the operand.
/// Each becomes a world (the synthesized rows of that table get it), except
/// values a declared property over the column already excludes from valid
/// states (a negative value under `nonNegative`).
pub fn boundary_worlds(
    c: &Candidate,
    analyzed: &[&Analyzed],
    index: &ProjectIndex,
    base: &Plan,
) -> Vec<WorldSpec> {
    use midwrite_model::ir::Op;
    use midwrite_model::value::{BinOp, Expr};
    fn walk(e: &Expr, out: &mut Vec<(String, i64)>) {
        match e {
            Expr::Bin(op, a, b) => {
                if matches!(
                    op,
                    BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne
                ) && let (x, Expr::Lit(Value::Int(n))) | (Expr::Lit(Value::Int(n)), x) =
                    (a.as_ref(), b.as_ref())
                    && let Some((_, f)) = x.as_property()
                {
                    out.push((f.to_owned(), *n));
                }
                walk(a, out);
                walk(b, out);
            }
            Expr::Not(x) | Expr::Field(x, _) | Expr::IsNull(x) => walk(x, out),
            _ => {}
        }
    }
    let non_negative: BTreeSet<(String, String)> = c
        .properties
        .iter()
        .filter_map(|p| match &p.spec {
            crate::candidate::Spec::NonNegative { table, column } => {
                Some((table.clone(), column.clone()))
            }
            _ => None,
        })
        .collect();
    let mut found: BTreeMap<(String, String), BTreeSet<i64>> = BTreeMap::new();
    let mut entries: Vec<usize> = c.actor_entries.clone();
    entries.dedup();
    for ai in entries {
        let a = analyzed[ai];
        let s = &a.summary;
        for g in &s.guards {
            // The guard's condition and the locals it reads (one level).
            let mut tests = Vec::new();
            if let Some(Op::JumpIf { cond, .. }) = s
                .codes
                .get(g.at.0)
                .and_then(|c| c.get(g.at.1))
                .map(|st| &st.op)
            {
                walk(cond, &mut tests);
                let mut locals = Vec::new();
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
                locals_of(cond, &mut locals);
                for st in s.codes.iter().flatten() {
                    if let Op::Let { local, expr } = &st.op
                        && locals.contains(local)
                    {
                        walk(expr, &mut tests);
                    }
                }
            }
            for &ri in &g.reads {
                let Some(read) = s.db.get(ri) else { continue };
                for (f, n) in &tests {
                    if identity_column(index, &read.table, f)
                        || !base
                            .rows
                            .get(&read.table)
                            .is_some_and(|rs| rs.iter().any(|r| r.contains_key(f)))
                    {
                        continue;
                    }
                    found
                        .entry((read.table.clone(), f.clone()))
                        .or_default()
                        .insert(*n);
                }
            }
        }
    }
    let mut out = Vec::new();
    for ((table, column), operands) in found {
        let rows = &base.rows[&table];
        let mut values = BTreeSet::new();
        for n in operands {
            for v in [n.checked_sub(1), Some(n), n.checked_add(1)]
                .into_iter()
                .flatten()
            {
                values.insert(v);
            }
        }
        for v in values {
            if v < 0 && non_negative.contains(&(table.clone(), column.clone())) {
                continue;
            }
            let cells: Vec<CellOverride> = rows
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    r.get(&column)
                        .is_some_and(|cur| matches!(cur, Value::Int(_)) && *cur != Value::Int(v))
                })
                .map(|(i, _)| CellOverride {
                    table: table.clone(),
                    row: i,
                    column: column.clone(),
                    value: Value::Int(v),
                })
                .collect();
            if cells.is_empty() {
                continue;
            }
            out.push(WorldSpec {
                id: format!("stored-boundary:{table}.{column}={v}"),
                origin: WorldOrigin::Relationship,
                relationships: vec![format!(
                    "the stored `{table}.{column}` is {v}, at a boundary of a guard that tests it"
                )],
                args: Vec::new(),
                cells,
                feedback: Some(format!(
                    "a guard compares `{column}` of the row read from `{table}` with an integer"
                )),
                priority: 15,
                paths: Vec::new(),
            });
        }
    }
    out
}

/// Apply cell overrides to synthesized rows. Returns why one does not apply.
pub fn apply_cells(
    rows: &mut BTreeMap<String, Vec<Row>>,
    cells: &[CellOverride],
) -> Result<(), String> {
    for o in cells {
        let Some(r) = rows.get_mut(&o.table).and_then(|rs| rs.get_mut(o.row)) else {
            return Err(format!(
                "the world's rows have no `{}` row {} to change",
                o.table, o.row
            ));
        };
        if !r.contains_key(&o.column) {
            return Err(format!("`{}.{}` is not modeled", o.table, o.column));
        }
        r.insert(o.column.clone(), o.value.clone());
    }
    Ok(())
}

/// A fingerprint of everything the model checker is given for a world
/// (rows, arguments, invariants, isolation, bounds, features, schema).
pub fn fingerprint(plan: &Plan, index: &ProjectIndex) -> String {
    let mut doc = plan.to_json(index, ".");
    if let Some(o) = doc.as_object_mut() {
        o.remove("name");
        o.remove("description");
    }
    let text = doc.to_string();
    format!(
        "{:016x}",
        midwrite_core::diagnostic::stable_hash(&["midwrite-world-1", &text])
    )
}

/// Reorder pending worlds by feedback: a domain world whose path appears in
/// a branch that explored worlds took only one way comes first.
pub fn prioritize(pending: &mut [WorldSpec], unflipped_paths: &BTreeSet<String>) {
    for w in pending.iter_mut() {
        if w.origin == WorldOrigin::Domain
            && w.paths.iter().any(|p| unflipped_paths.contains(p))
            && w.priority >= 15
        {
            w.priority -= 10;
            w.feedback = Some(
                "a branch over this parameter took only one direction in the worlds explored so far"
                    .to_owned(),
            );
        }
    }
    pending.sort_by(|a, b| a.priority.cmp(&b.priority).then_with(|| a.id.cmp(&b.id)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_overrides_change_only_modeled_cells_of_existing_rows() {
        let mut row = Row::new();
        row.insert("name".to_owned(), Value::str("a"));
        let mut rows = BTreeMap::from([("t".to_owned(), vec![row])]);
        let set = |column: &str, row: usize| CellOverride {
            table: "t".to_owned(),
            row,
            column: column.to_owned(),
            value: Value::str("b"),
        };
        apply_cells(&mut rows, &[set("name", 0)]).unwrap();
        assert_eq!(rows["t"][0]["name"], Value::str("b"));
        assert!(apply_cells(&mut rows, &[set("name", 1)]).is_err());
        assert!(apply_cells(&mut rows, &[set("other", 0)]).is_err());
    }

    #[test]
    fn feedback_moves_worlds_over_unflipped_branches_first() {
        let world = |id: &str, priority: u8, path: &str| WorldSpec {
            id: id.to_owned(),
            origin: WorldOrigin::Domain,
            relationships: Vec::new(),
            args: Vec::new(),
            cells: Vec::new(),
            feedback: None,
            priority,
            paths: vec![path.to_owned()],
        };
        let mut pending = vec![world("a", 30, "x"), world("b", 32, "y")];
        prioritize(&mut pending, &BTreeSet::from(["y".to_owned()]));
        assert_eq!(pending[0].id, "b");
        assert!(pending[0].feedback.is_some());
    }
}

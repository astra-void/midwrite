//! Phase 15: typed input domains.
//!
//! Synthesis (Phase 11–14) picks one value per parameter path. This module
//! describes, for every parameter path an entry reads, the *domain* that
//! value comes from and the value classes that matter to the code: the
//! TypeScript annotation and optionality (does `null` or `undefined` belong
//! to the domain?), the column the path is compared with or written to (its
//! SQL type, nullability, enum values, foreign key), and the literals the
//! code compares the path with (a branch boundary).
//!
//! A domain never invents permission: `null` is in a domain only when the
//! parameter's annotation says so (`T | null`), `undefined` only when the
//! annotation says so or the parameter is optional. Without an annotation the
//! nullability is UNKNOWN and no `null` value is generated; that gap is
//! reported as unexplored input (`docs/phase15/INPUT-DOMAINS.md`).

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use commitrift_model::ir::Op;
use commitrift_model::value::{BinOp, Expr, Value};

use crate::detect::Analyzed;
use crate::effects::Val;
use crate::project::ProjectIndex;
use crate::synth::{Ty, exprs_of, param_paths, path_of};

/// The kind of value a parameter path holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DomainKind {
    Null,
    Undefined,
    Boolean,
    Integer,
    Decimal,
    String,
    Uuid,
    GeneratedIdentifier,
    Timestamp,
    Date,
    Json,
    Array,
    Object,
    Enum,
    DatabaseRowReference,
    Unknown,
}

/// Whether a domain contains a value (`null`, `undefined`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    /// The source contract (annotation, optionality) admits it.
    Permitted,
    /// The source contract excludes it.
    Excluded,
    /// No contract says: never generated, reported as unexplored.
    Unknown,
}

/// A value class worth exploring for a path: it can change a branch or an
/// observable outcome.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "class", rename_all = "kebab-case")]
pub enum ValueClass {
    /// `null` (the domain permits it).
    Null,
    /// `undefined` (the domain permits it).
    Undefined,
    /// Equal to a literal the code compares the path with.
    EqualToOperand {
        operand: Value,
    },
    /// One below / above an integer the code compares the path with.
    BelowOperand {
        operand: i64,
    },
    AboveOperand {
        operand: i64,
    },
    /// The opposite of a boolean the code tests.
    Negated,
}

impl ValueClass {
    /// The value of this class, given the synthesized `base` value.
    pub fn value(&self, base: &Value) -> Option<Value> {
        let v = match self {
            Self::Null => Value::Null,
            Self::Undefined => {
                Value::Undefined(commitrift_model::value::UndefinedMark { undefined: true })
            }
            Self::EqualToOperand { operand } => operand.clone(),
            Self::BelowOperand { operand } => Value::Int(operand.checked_sub(1)?),
            Self::AboveOperand { operand } => Value::Int(operand.checked_add(1)?),
            Self::Negated => match base {
                Value::Bool(b) => Value::Bool(!b),
                _ => return None,
            },
        };
        (v != *base).then_some(v)
    }

    pub fn describe(&self, path: &str) -> String {
        match self {
            Self::Null => format!("{path} = null"),
            Self::Undefined => format!("{path} = undefined"),
            Self::EqualToOperand { operand } => format!("{path} = {}", operand.render()),
            Self::BelowOperand { operand } => format!("{path} = {} (below {operand})", operand - 1),
            Self::AboveOperand { operand } => format!("{path} = {} (above {operand})", operand + 1),
            Self::Negated => format!("{path} negated"),
        }
    }
}

/// The domain of one parameter path of one entry.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathDomain {
    pub path: String,
    pub kind: DomainKind,
    /// The path carries a key (compared with or written to a key column).
    pub key: bool,
    pub null: Permission,
    pub undefined: Permission,
    /// Where the kind and permissions come from.
    pub provenance: Vec<String>,
    /// Value classes relevant to the code, besides the synthesized value.
    pub classes: Vec<ValueClass>,
}

/// Union members of a written annotation (`number | null` → `number`,
/// `null`), at the top level only.
fn members(annotation: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in annotation.chars() {
        match ch {
            '<' | '(' | '[' | '{' => depth += 1,
            '>' | ')' | ']' | '}' => depth -= 1,
            '|' if depth == 0 => {
                out.push(cur.trim().to_owned());
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(ch);
    }
    out.push(cur.trim().to_owned());
    out.retain(|m| !m.is_empty());
    out
}

/// Phase 15: the members of an annotation that is a union of string
/// literals (`"page" | "redirect"`, as the annotation reader resolves
/// aliases), else `None`.
pub fn literal_members(annotation: &str) -> Option<Vec<String>> {
    let m = members(annotation);
    let out: Option<Vec<String>> = m
        .iter()
        .map(|x| serde_json::from_str::<String>(x).ok())
        .collect();
    out.filter(|v| !v.is_empty())
}

/// `null`/`undefined` permissions of a path from its annotation and
/// optionality.
pub fn nullability(
    annotation: Option<&str>,
    optional: bool,
) -> (Permission, Permission, Option<String>) {
    let Some(a) = annotation else {
        return (
            Permission::Unknown,
            if optional {
                Permission::Permitted
            } else {
                Permission::Unknown
            },
            optional.then(|| "the parameter is optional".to_owned()),
        );
    };
    let m = members(a);
    let has = |x: &str| m.iter().any(|v| v == x);
    // `any`/`unknown` admit everything: a contract that does not exclude.
    if has("any") || has("unknown") {
        return (
            Permission::Unknown,
            Permission::Unknown,
            Some(format!("annotation `{a}` does not constrain the value")),
        );
    }
    let null = if has("null") {
        Permission::Permitted
    } else {
        Permission::Excluded
    };
    let undefined = if has("undefined") || has("void") || optional {
        Permission::Permitted
    } else {
        Permission::Excluded
    };
    (
        null,
        undefined,
        Some(format!(
            "annotation `{a}`{}",
            if optional { ", optional" } else { "" }
        )),
    )
}

/// Literals each parameter path is compared with anywhere in the code
/// (`path op literal`), with the comparison.
fn compared_literals(a: &Analyzed) -> BTreeMap<String, Vec<(BinOp, Value)>> {
    let params = &a.summary.params;
    // Locals defined once from a path stand for it.
    let mut defs: BTreeMap<&str, Vec<&Expr>> = BTreeMap::new();
    for op in a.summary.codes.iter().flatten().map(|s| &s.op) {
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
    let mut out: BTreeMap<String, Vec<(BinOp, Value)>> = BTreeMap::new();
    fn walk(
        e: &Expr,
        path: &dyn Fn(&Expr) -> Option<String>,
        out: &mut BTreeMap<String, Vec<(BinOp, Value)>>,
    ) {
        match e {
            Expr::Bin(op, l, r) => {
                let pair = match (l.as_ref(), r.as_ref()) {
                    (x, Expr::Lit(v)) => path(x).map(|p| (p, *op, v.clone())),
                    (Expr::Lit(v), x) => path(x).map(|p| (p, flip(*op), v.clone())),
                    _ => None,
                };
                if let Some((p, op, v)) = pair
                    && matches!(
                        op,
                        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                    )
                    && matches!(
                        v,
                        Value::Int(_) | Value::Str(_) | Value::Bool(_) | Value::Null
                    )
                {
                    let seen = out.entry(p).or_default();
                    if !seen.contains(&(op, v.clone())) {
                        seen.push((op, v));
                    }
                }
                walk(l, path, out);
                walk(r, path, out);
            }
            Expr::IsNull(x) => {
                if let Some(p) = path(x) {
                    let seen = out.entry(p).or_default();
                    if !seen.contains(&(BinOp::Eq, Value::Null)) {
                        seen.push((BinOp::Eq, Value::Null));
                    }
                }
                walk(x, path, out);
            }
            Expr::Field(b, _) | Expr::Not(b) => walk(b, path, out),
            Expr::Object(fields) => fields.iter().for_each(|(_, v)| walk(v, path, out)),
            Expr::Array(items) | Expr::Call { args: items, .. } => {
                items.iter().for_each(|v| walk(v, path, out));
            }
            Expr::Each { list, body, .. } => {
                walk(list, path, out);
                walk(body, path, out);
            }
            _ => {}
        }
    }
    for op in a.summary.codes.iter().flatten().map(|s| &s.op) {
        for e in exprs_of(op) {
            walk(e, &path, &mut out);
        }
    }
    out
}

/// Phase 15: the parameter paths each branch (`JumpIf`) of `a` depends
/// on, by source location (through locals defined once).
pub fn branch_paths(a: &Analyzed) -> BTreeMap<String, BTreeSet<String>> {
    let params = &a.summary.params;
    let mut defs: BTreeMap<&str, Vec<&Expr>> = BTreeMap::new();
    for op in a.summary.codes.iter().flatten().map(|s| &s.op) {
        if let Op::Let { local, expr } = op {
            defs.entry(local.as_str()).or_default().push(expr);
        }
    }
    fn paths(
        e: &Expr,
        params: &[String],
        defs: &BTreeMap<&str, Vec<&Expr>>,
        depth: u32,
        out: &mut BTreeSet<String>,
    ) {
        if let Some(p) = path_of(e, params) {
            out.insert(p);
            return;
        }
        match e {
            Expr::Local(l) if depth < 4 => {
                if let Some([one]) = defs.get(l.as_str()).map(Vec::as_slice) {
                    paths(one, params, defs, depth + 1, out);
                }
            }
            Expr::Field(b, _) | Expr::Not(b) | Expr::IsNull(b) => {
                paths(b, params, defs, depth, out);
            }
            Expr::Bin(_, l, r) => {
                paths(l, params, defs, depth, out);
                paths(r, params, defs, depth, out);
            }
            Expr::Object(fields) => fields
                .iter()
                .for_each(|(_, v)| paths(v, params, defs, depth, out)),
            Expr::Array(items) | Expr::Call { args: items, .. } => items
                .iter()
                .for_each(|v| paths(v, params, defs, depth, out)),
            Expr::Each { list, body, .. } => {
                paths(list, params, defs, depth, out);
                paths(body, params, defs, depth, out);
            }
            _ => {}
        }
    }
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for step in a.summary.codes.iter().flatten() {
        if let Op::JumpIf { cond, .. } = &step.op {
            let mut found = BTreeSet::new();
            paths(cond, params, &defs, 0, &mut found);
            out.entry(step.src.label()).or_default().extend(found);
        }
    }
    out
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

/// The domains of every parameter path `a` reads, given its synthesized
/// arguments. Declared arguments (`commitrift-contracts.json` `arguments`)
/// are a fixed, user-chosen input: their paths get no value classes.
pub fn entry_domains(a: &Analyzed, index: &ProjectIndex) -> Vec<PathDomain> {
    let declared = a.contract.as_ref().is_some_and(|c| c.arguments.is_some());
    let fixed: BTreeSet<&String> = a
        .contract
        .as_ref()
        .map(|c| c.values.keys().collect())
        .unwrap_or_default();
    let paths = param_paths(a, index);
    let (annotations, optional) = commitrift_analyzer::TsActorProvider::new(&index.root)
        .param_signature(&index.root, &a.fact.selector);
    let literals = compared_literals(a);
    // Columns each path is compared with or written to.
    let mut columns: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for e in &a.summary.db {
        for (c, v) in e.eq.iter().chain(e.writes.iter()) {
            if let Val::Param { path } = v {
                columns
                    .entry(path.clone())
                    .or_default()
                    .push((e.table.clone(), c.clone()));
            }
        }
    }
    let mut out = Vec::new();
    for (path, (ty, key)) in &paths {
        let mut provenance = Vec::new();
        let annotation = annotations.get(path).map(String::as_str);
        let (mut null, mut undefined, why) = nullability(
            annotation.filter(|a| literal_members(a).is_none()),
            optional.contains(path),
        );
        if let Some(w) = why {
            provenance.push(w);
        }
        let mut kind = match ty {
            Ty::Int => DomainKind::Integer,
            Ty::Bool => DomainKind::Boolean,
            Ty::Uuid => DomainKind::Uuid,
            Ty::Text => DomainKind::String,
            Ty::List { .. } => DomainKind::Array,
        };
        if let Some(lits) = annotation.and_then(literal_members) {
            kind = DomainKind::Enum;
            provenance.push(format!("string literal union: {}", lits.join(", ")));
        }
        for (table, column) in columns.get(path).into_iter().flatten() {
            let Some(c) = index.source_table(table).and_then(|t| t.column(column)) else {
                continue;
            };
            provenance.push(format!(
                "compared with or written to `{table}.{column}` ({}{})",
                c.sql_type,
                if c.not_null { ", NOT NULL" } else { "" }
            ));
            kind = match c.builder.as_str() {
                _ if kind == DomainKind::Enum => kind,
                "uuid" => DomainKind::Uuid,
                "timestamp" => DomainKind::Timestamp,
                "date" => DomainKind::Date,
                "json" | "jsonb" => DomainKind::Json,
                "numeric" | "decimal" | "real" | "doublePrecision" => DomainKind::Decimal,
                _ if !c.enum_values.is_empty() => DomainKind::Enum,
                _ if c.references.is_some() => DomainKind::DatabaseRowReference,
                _ => kind,
            };
            // A NOT NULL column written from the path: `null` would only
            // make the statement fail, which is a validation question, not a
            // concurrency one; it stays permitted if the annotation says so.
        }
        let mut classes = Vec::new();
        if declared || fixed.contains(path) {
            provenance.push("value declared in commitrift-contracts.json: not varied".to_owned());
            null = Permission::Excluded;
            undefined = Permission::Excluded;
        } else {
            if null == Permission::Permitted {
                classes.push(ValueClass::Null);
            }
            if undefined == Permission::Permitted && null != Permission::Permitted {
                classes.push(ValueClass::Undefined);
            }
            for (op, v) in literals.get(path).into_iter().flatten() {
                match (op, v) {
                    (_, Value::Null) => {
                        // A `== null` test: the null class covers it when
                        // permitted; otherwise it is reported unexplored.
                    }
                    (BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge, Value::Int(n)) => {
                        classes.push(ValueClass::BelowOperand { operand: *n });
                        classes.push(ValueClass::EqualToOperand {
                            operand: Value::Int(*n),
                        });
                        classes.push(ValueClass::AboveOperand { operand: *n });
                    }
                    (_, Value::Bool(_)) if *ty == Ty::Bool => classes.push(ValueClass::Negated),
                    (_, v @ (Value::Int(_) | Value::Str(_))) => {
                        let fits = matches!(
                            (ty, v),
                            (Ty::Int, Value::Int(_)) | (Ty::Text, Value::Str(_))
                        );
                        if fits {
                            classes.push(ValueClass::EqualToOperand { operand: v.clone() });
                        }
                    }
                    _ => {}
                }
            }
        }
        classes.sort();
        classes.dedup();
        out.push(PathDomain {
            path: path.clone(),
            kind,
            key: *key,
            null,
            undefined,
            provenance,
            classes,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_members_split_at_the_top_level_only() {
        assert_eq!(members("number | null"), vec!["number", "null"]);
        assert_eq!(
            members("Array<string | null> | undefined"),
            vec!["Array<string | null>", "undefined"]
        );
        assert_eq!(
            members("{ a: string | null }"),
            vec!["{ a: string | null }"]
        );
    }

    #[test]
    fn null_and_undefined_come_from_the_annotation_or_optionality_only() {
        let (n, u, _) = nullability(Some("number | null"), false);
        assert_eq!((n, u), (Permission::Permitted, Permission::Excluded));
        let (n, u, _) = nullability(Some("string"), true);
        assert_eq!((n, u), (Permission::Excluded, Permission::Permitted));
        let (n, u, _) = nullability(Some("string | undefined"), false);
        assert_eq!((n, u), (Permission::Excluded, Permission::Permitted));
        // No annotation: unknown, never generated.
        let (n, u, _) = nullability(None, false);
        assert_eq!((n, u), (Permission::Unknown, Permission::Unknown));
        let (n, _, _) = nullability(Some("any"), false);
        assert_eq!(n, Permission::Unknown);
    }

    #[test]
    fn classes_give_values_distinct_from_the_base() {
        let base = Value::Int(10);
        assert_eq!(
            ValueClass::BelowOperand { operand: 10 }.value(&base),
            Some(Value::Int(9))
        );
        assert_eq!(
            ValueClass::EqualToOperand {
                operand: Value::Int(10)
            }
            .value(&base),
            None,
            "the base value already belongs to the class"
        );
        assert_eq!(ValueClass::Null.value(&base), Some(Value::Null));
        assert_eq!(ValueClass::Negated.value(&base), None);
    }
}

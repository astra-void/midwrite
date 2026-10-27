//! Bounded values and expressions.
//!
//! Values are concrete (small integers, strings, booleans, `null`, rows):
//! the model checker explores *schedules* over concrete data, not symbolic
//! data. Expressions are evaluated in one of two modes ([`Mode`]): `Sql`
//! (comparisons with `null` are `null`, i.e. unknown, as in SQL) for
//! predicates, `SET` clauses and inserted values, and `Js` (strict equality,
//! JavaScript truthiness) for application-level conditions.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

pub type Row = BTreeMap<String, Value>;
pub type Locals = BTreeMap<String, Value>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    /// Phase 12: a value generated at run time by a column default (a
    /// random UUID from PostgreSQL, an id from an application generator).
    /// It is not a string the model knows: it equals only itself, and its
    /// order relative to other values is not determined.
    Gen(GenId),
    Row(Row),
}

/// The identity of one generation event (Phase 12): which generator
/// (`Model::generators` index), which actor, and that actor's generation
/// counter. The counter is per actor and never reset (not by a retry or a
/// rollback), so every event has its own identity and the identity does not
/// depend on how other actors are scheduled. Two events are assumed to
/// produce different values (the `generated-values-distinct` assumption).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenId {
    #[serde(rename = "$generated")]
    pub generator: u16,
    pub actor: u16,
    pub n: u32,
}

impl Value {
    pub fn str(text: impl Into<String>) -> Self {
        Self::Str(text.into())
    }

    /// JavaScript-style truthiness (`null`, `false`, `0` and `""` are falsy).
    pub fn truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(b) => *b,
            Self::Int(n) => *n != 0,
            Self::Str(s) => !s.is_empty(),
            Self::Gen(_) | Self::Row(_) => true,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Stable, single-line rendering used in witnesses and canonical state.
    pub fn render(&self) -> String {
        match self {
            Self::Null => "null".to_owned(),
            Self::Bool(b) => b.to_string(),
            Self::Int(n) => n.to_string(),
            Self::Str(s) => format!("{s:?}"),
            Self::Gen(g) => format!("gen#{}:{}.{}", g.generator, g.actor, g.n),
            Self::Row(row) => {
                let fields: Vec<String> = row
                    .iter()
                    .map(|(k, v)| format!("{k}={}", v.render()))
                    .collect();
                format!("{{{}}}", fields.join(", "))
            }
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

impl From<i64> for Value {
    fn from(n: i64) -> Self {
        Self::Int(n)
    }
}
impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Self::Bool(b)
    }
}
impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Self::Str(s.to_owned())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Eq => "=",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::And => "and",
            Self::Or => "or",
        }
    }
}

/// An expression. `Col` names a column of the row a predicate or `SET`
/// clause is being evaluated against; `Local` an actor-local variable.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Expr {
    Lit(Value),
    Local(String),
    Col(String),
    Field(Box<Expr>, String),
    Not(Box<Expr>),
    IsNull(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    /// An object literal (Phase 7): evaluates to a row of its properties,
    /// in JavaScript mode only.
    Object(Vec<(String, Expr)>),
    /// An expression the extractor did not understand. Evaluating it is an
    /// error: unknown behavior is never turned into a value.
    Unknown(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Sql,
    Js,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EvalError {
    #[error("unsupported expression: {0}")]
    Unknown(String),
    #[error("TypeError: {0}")]
    Type(String),
    #[error("arithmetic overflow")]
    Overflow,
    #[error("reference to undefined local `{0}`")]
    UndefinedLocal(String),
    #[error("unknown column `{0}`")]
    UnknownColumn(String),
}

impl Expr {
    pub fn lit(v: impl Into<Value>) -> Self {
        Self::Lit(v.into())
    }
    pub fn local(name: &str) -> Self {
        Self::Local(name.to_owned())
    }
    pub fn col(name: &str) -> Self {
        Self::Col(name.to_owned())
    }
    pub fn field(base: Expr, name: &str) -> Self {
        Self::Field(Box::new(base), name.to_owned())
    }
    pub fn bin(op: BinOp, a: Expr, b: Expr) -> Self {
        Self::Bin(op, Box::new(a), Box::new(b))
    }
    #[allow(clippy::should_implement_trait)]
    pub fn eq(a: Expr, b: Expr) -> Self {
        Self::bin(BinOp::Eq, a, b)
    }
    #[allow(clippy::should_implement_trait)]
    pub fn not(a: Expr) -> Self {
        Self::Not(Box::new(a))
    }

    /// Whether the expression contains `Col` references (so it depends on
    /// the row it is evaluated against).
    pub fn mentions_column(&self) -> bool {
        match self {
            Self::Col(_) => true,
            Self::Lit(_) | Self::Local(_) | Self::Unknown(_) => false,
            Self::Field(e, _) | Self::Not(e) | Self::IsNull(e) => e.mentions_column(),
            Self::Bin(_, a, b) => a.mentions_column() || b.mentions_column(),
            Self::Object(fields) => fields.iter().any(|(_, e)| e.mentions_column()),
        }
    }

    /// Whether the expression contains an `Unknown` node.
    pub fn unknown_detail(&self) -> Option<&str> {
        match self {
            Self::Unknown(d) => Some(d),
            Self::Lit(_) | Self::Local(_) | Self::Col(_) => None,
            Self::Field(e, _) | Self::Not(e) | Self::IsNull(e) => e.unknown_detail(),
            Self::Bin(_, a, b) => a.unknown_detail().or_else(|| b.unknown_detail()),
            Self::Object(fields) => fields.iter().find_map(|(_, e)| e.unknown_detail()),
        }
    }

    /// Names of locals read by the expression.
    pub fn locals(&self, out: &mut Vec<String>) {
        match self {
            Self::Local(n) => out.push(n.clone()),
            Self::Lit(_) | Self::Col(_) | Self::Unknown(_) => {}
            Self::Field(e, _) | Self::Not(e) | Self::IsNull(e) => e.locals(out),
            Self::Bin(_, a, b) => {
                a.locals(out);
                b.locals(out);
            }
            Self::Object(fields) => fields.iter().for_each(|(_, e)| e.locals(out)),
        }
    }

    /// Names of columns read by the expression.
    pub fn columns(&self, out: &mut Vec<String>) {
        match self {
            Self::Col(n) => out.push(n.clone()),
            Self::Lit(_) | Self::Local(_) | Self::Unknown(_) => {}
            Self::Field(e, _) | Self::Not(e) | Self::IsNull(e) => e.columns(out),
            Self::Bin(_, a, b) => {
                a.columns(out);
                b.columns(out);
            }
            Self::Object(fields) => fields.iter().for_each(|(_, e)| e.columns(out)),
        }
    }

    /// Evaluate against `locals` and an optional row (for `Col`).
    pub fn eval(&self, locals: &Locals, row: Option<&Row>, mode: Mode) -> Result<Value, EvalError> {
        match self {
            Self::Lit(v) => Ok(v.clone()),
            Self::Local(n) => locals
                .get(n)
                .cloned()
                .ok_or_else(|| EvalError::UndefinedLocal(n.clone())),
            Self::Col(c) => match row {
                Some(r) => r
                    .get(c)
                    .cloned()
                    .ok_or_else(|| EvalError::UnknownColumn(c.clone())),
                None => Err(EvalError::Type(format!(
                    "column `{c}` outside a row context"
                ))),
            },
            Self::Field(base, name) => match base.eval(locals, row, mode)? {
                Value::Row(r) => Ok(r.get(name).cloned().unwrap_or(Value::Null)),
                Value::Null => Err(EvalError::Type(format!(
                    "cannot read property `{name}` of null"
                ))),
                other => Err(EvalError::Type(format!(
                    "cannot read property `{name}` of {}",
                    other.render()
                ))),
            },
            Self::Not(e) => {
                let v = e.eval(locals, row, mode)?;
                match (mode, v) {
                    (Mode::Sql, Value::Null) => Ok(Value::Null),
                    (_, v) => Ok(Value::Bool(!v.truthy())),
                }
            }
            Self::IsNull(e) => Ok(Value::Bool(e.eval(locals, row, mode)?.is_null())),
            Self::Unknown(d) => Err(EvalError::Unknown(d.clone())),
            Self::Bin(op, a, b) => eval_bin(*op, a, b, locals, row, mode),
            Self::Object(fields) => {
                let mut out = Row::new();
                for (k, e) in fields {
                    out.insert(k.clone(), e.eval(locals, row, mode)?);
                }
                Ok(Value::Row(out))
            }
        }
    }

    /// Fold every subexpression that does not depend on a row into a
    /// literal. The result is *closed with respect to locals*, which is the
    /// form sent to a database backend. Errors in folded subexpressions are
    /// reported (a JavaScript exception before the statement is sent).
    pub fn close(&self, locals: &Locals, mode: Mode) -> Result<Expr, EvalError> {
        if !self.mentions_column() {
            return Ok(Expr::Lit(self.eval(locals, None, mode)?));
        }
        Ok(match self {
            Self::Not(e) => Self::Not(Box::new(e.close(locals, mode)?)),
            Self::IsNull(e) => Self::IsNull(Box::new(e.close(locals, mode)?)),
            Self::Field(e, n) => Self::Field(Box::new(e.close(locals, mode)?), n.clone()),
            Self::Bin(op, a, b) => Self::Bin(
                *op,
                Box::new(a.close(locals, mode)?),
                Box::new(b.close(locals, mode)?),
            ),
            other => other.clone(),
        })
    }

    /// A readable rendering for witnesses.
    pub fn render(&self) -> String {
        match self {
            Self::Lit(v) => v.render(),
            Self::Local(n) => n.clone(),
            Self::Col(c) => format!("@{c}"),
            Self::Field(e, n) => format!("{}.{n}", e.render()),
            Self::Not(e) => format!("!{}", e.render()),
            Self::IsNull(e) => format!("{} is null", e.render()),
            Self::Object(fields) => format!(
                "{{{}}}",
                fields
                    .iter()
                    .map(|(k, e)| format!("{k}: {}", e.render()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Unknown(d) => format!("<unknown: {d}>"),
            Self::Bin(op, a, b) => format!("({} {} {})", a.render(), op.symbol(), b.render()),
        }
    }
}

fn eval_bin(
    op: BinOp,
    a: &Expr,
    b: &Expr,
    locals: &Locals,
    row: Option<&Row>,
    mode: Mode,
) -> Result<Value, EvalError> {
    match op {
        BinOp::And | BinOp::Or => {
            let left = a.eval(locals, row, mode)?;
            // Short-circuit like both SQL (Kleene) and JavaScript do.
            match (op, mode) {
                (BinOp::And, Mode::Js) => {
                    if !left.truthy() {
                        return Ok(Value::Bool(false));
                    }
                    return Ok(Value::Bool(b.eval(locals, row, mode)?.truthy()));
                }
                (BinOp::Or, Mode::Js) => {
                    if left.truthy() {
                        return Ok(Value::Bool(true));
                    }
                    return Ok(Value::Bool(b.eval(locals, row, mode)?.truthy()));
                }
                _ => {}
            }
            let right = b.eval(locals, row, mode)?;
            let l = tri(&left);
            let r = tri(&right);
            let out = match op {
                BinOp::And => match (l, r) {
                    (Some(false), _) | (_, Some(false)) => Some(false),
                    (Some(true), Some(true)) => Some(true),
                    _ => None,
                },
                _ => match (l, r) {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (Some(false), Some(false)) => Some(false),
                    _ => None,
                },
            };
            Ok(out.map_or(Value::Null, Value::Bool))
        }
        _ => {
            let l = a.eval(locals, row, mode)?;
            let r = b.eval(locals, row, mode)?;
            match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul => match (&l, &r) {
                    (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                    (Value::Int(x), Value::Int(y)) => {
                        let out = match op {
                            BinOp::Add => x.checked_add(*y),
                            BinOp::Sub => x.checked_sub(*y),
                            _ => x.checked_mul(*y),
                        };
                        out.map(Value::Int).ok_or(EvalError::Overflow)
                    }
                    _ => Err(EvalError::Type(format!(
                        "arithmetic on {} and {}",
                        l.render(),
                        r.render()
                    ))),
                },
                _ => {
                    if mode == Mode::Sql && (l.is_null() || r.is_null()) {
                        return Ok(Value::Null);
                    }
                    if mode == Mode::Js && (l.is_null() || r.is_null()) {
                        return Ok(Value::Bool(match op {
                            BinOp::Eq => l == r,
                            BinOp::Ne => l != r,
                            _ => false,
                        }));
                    }
                    if matches!(l, Value::Gen(_)) || matches!(r, Value::Gen(_)) {
                        // Equality of a generated value is identity: it
                        // equals itself and nothing else (distinct events
                        // are assumed to produce distinct values).
                        match op {
                            BinOp::Eq => return Ok(Value::Bool(l == r)),
                            BinOp::Ne => return Ok(Value::Bool(l != r)),
                            _ => {}
                        }
                    }
                    let ord = compare(&l, &r)?;
                    Ok(Value::Bool(match op {
                        BinOp::Eq => ord == std::cmp::Ordering::Equal,
                        BinOp::Ne => ord != std::cmp::Ordering::Equal,
                        BinOp::Lt => ord == std::cmp::Ordering::Less,
                        BinOp::Le => ord != std::cmp::Ordering::Greater,
                        BinOp::Gt => ord == std::cmp::Ordering::Greater,
                        _ => ord != std::cmp::Ordering::Less,
                    }))
                }
            }
        }
    }
}

fn tri(v: &Value) -> Option<bool> {
    match v {
        Value::Null => None,
        other => Some(other.truthy()),
    }
}

fn compare(l: &Value, r: &Value) -> Result<std::cmp::Ordering, EvalError> {
    match (l, r) {
        (Value::Gen(_), _) | (_, Value::Gen(_)) => Err(EvalError::Unknown(format!(
            "the order of generated values is not determined ({} and {})",
            l.render(),
            r.render()
        ))),
        (Value::Int(x), Value::Int(y)) => Ok(x.cmp(y)),
        (Value::Str(x), Value::Str(y)) => Ok(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Ok(x.cmp(y)),
        _ => Err(EvalError::Type(format!(
            "cannot compare {} with {}",
            l.render(),
            r.render()
        ))),
    }
}

/// A predicate is true for a row only when it evaluates to `true`
/// (`null`/unknown does not match), as in SQL.
pub fn predicate_matches(pred: &Expr, row: &Row) -> Result<bool, EvalError> {
    let empty = Locals::new();
    Ok(matches!(
        pred.eval(&empty, Some(row), Mode::Sql)?,
        Value::Bool(true)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_comparison_with_null_is_unknown_and_does_not_match() {
        let mut row = Row::new();
        row.insert("stock".into(), Value::Null);
        let pred = Expr::bin(BinOp::Gt, Expr::col("stock"), Expr::lit(0));
        assert!(!predicate_matches(&pred, &row).unwrap());
        let not = Expr::not(pred);
        assert!(!predicate_matches(&not, &row).unwrap());
    }

    #[test]
    fn js_short_circuit_avoids_reading_a_property_of_null() {
        let mut locals = Locals::new();
        locals.insert("p".into(), Value::Null);
        let e = Expr::bin(
            BinOp::Or,
            Expr::not(Expr::local("p")),
            Expr::bin(
                BinOp::Le,
                Expr::field(Expr::local("p"), "stock"),
                Expr::lit(0),
            ),
        );
        assert_eq!(e.eval(&locals, None, Mode::Js), Ok(Value::Bool(true)));
        let unguarded = Expr::field(Expr::local("p"), "stock");
        assert!(matches!(
            unguarded.eval(&locals, None, Mode::Js),
            Err(EvalError::Type(_))
        ));
    }

    #[test]
    fn close_folds_locals_but_keeps_columns() {
        let mut locals = Locals::new();
        locals.insert("n".into(), Value::Int(4));
        let e = Expr::bin(
            BinOp::Sub,
            Expr::col("stock"),
            Expr::bin(BinOp::Add, Expr::local("n"), Expr::lit(1)),
        );
        let closed = e.close(&locals, Mode::Sql).unwrap();
        assert_eq!(
            closed,
            Expr::bin(BinOp::Sub, Expr::col("stock"), Expr::lit(5))
        );
    }

    #[test]
    fn overflow_is_an_error_not_a_wraparound() {
        let e = Expr::bin(BinOp::Add, Expr::lit(i64::MAX), Expr::lit(1));
        assert_eq!(
            e.eval(&Locals::new(), None, Mode::Sql),
            Err(EvalError::Overflow)
        );
    }
}

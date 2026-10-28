//! Phase 14: SQL built-ins of the value layer (Drizzle operators that are
//! not binary comparisons), with PostgreSQL's three-valued logic.
//!
//! `InList` is Drizzle's `inArray(column, values)`: `args[0]` is the tested
//! value, `args[1]` the list (a JavaScript array, [`Value::List`]). Drizzle
//! 0.45.4 (`sql/expressions/conditions.js`) sends `column in ($1, $2, ...)`
//! for a non-empty array and the literal `false` for an empty one (its
//! `notInArray` sends `not in (...)`, or `true` for an empty array, which is
//! `NOT` of this operator in three-valued logic: the extractor lowers it so).
//! PostgreSQL's `x IN (v1, ..., vn)` is `x = v1 OR ... OR x = vn`: `true`
//! when some comparison is true, else `NULL` when some comparison is
//! `NULL` (a `NULL` tested value or a `NULL` element), else `false`.

use crate::value::{Builtin, EvalError, Expr, Mode, Value};

/// Evaluate a SQL built-in on evaluated arguments.
pub fn eval(func: Builtin, args: &[Value], mode: Mode) -> Result<Value, EvalError> {
    match func {
        Builtin::InList => in_list(args, mode),
        other => Err(EvalError::Unknown(format!(
            "the SQL operator `{}` is not modeled",
            other.name()
        ))),
    }
}

fn in_list(args: &[Value], mode: Mode) -> Result<Value, EvalError> {
    if mode != Mode::Sql {
        return Err(EvalError::Unknown(
            "`inArray` outside a SQL predicate is not modeled".to_owned(),
        ));
    }
    let [needle, list] = args else {
        return Err(EvalError::Unknown(format!(
            "`inArray` with {} arguments",
            args.len()
        )));
    };
    let Value::List(items) = list else {
        return Err(EvalError::Unknown(format!(
            "`inArray` over {} (only an array of values is modeled)",
            list.render()
        )));
    };
    // Drizzle sends the literal `false` for an empty array, whatever the
    // tested value is (also NULL).
    if items.is_empty() {
        return Ok(Value::Bool(false));
    }
    let mut unknown = false;
    for item in items {
        match sql_eq(needle, item)? {
            Some(true) => return Ok(Value::Bool(true)),
            Some(false) => {}
            None => unknown = true,
        }
    }
    Ok(if unknown {
        Value::Null
    } else {
        Value::Bool(false)
    })
}

/// SQL `=` of two scalar values: `None` is `NULL` (unknown).
fn sql_eq(a: &Value, b: &Value) -> Result<Option<bool>, EvalError> {
    match (a, b) {
        (Value::Null, _) | (_, Value::Null) => Ok(None),
        // A generated value (or clock reading) equals itself only (the
        // `generated-values-distinct` / `clock-readings-distinct`
        // assumptions, as for `=` in `crate::value`).
        (Value::Gen(_), Value::Gen(_) | Value::Str(_) | Value::Int(_) | Value::Bool(_))
        | (Value::Str(_) | Value::Int(_) | Value::Bool(_), Value::Gen(_)) => Ok(Some(a == b)),
        (Value::Int(x), Value::Int(y)) => Ok(Some(x == y)),
        (Value::Str(x), Value::Str(y)) => Ok(Some(x == y)),
        (Value::Bool(x), Value::Bool(y)) => Ok(Some(x == y)),
        (
            Value::Int(_) | Value::Str(_) | Value::Bool(_),
            Value::Int(_) | Value::Str(_) | Value::Bool(_),
        ) => Err(EvalError::Type(format!(
            "cannot compare {} with {}",
            a.render(),
            b.render()
        ))),
        _ => Err(EvalError::Unknown(format!(
            "`inArray` comparing {} with {} is not modeled",
            a.render(),
            b.render()
        ))),
    }
}

/// The PostgreSQL text of a SQL built-in (the lockstep oracle).
pub fn render(func: Builtin, args: &[Expr]) -> Result<String, String> {
    match func {
        Builtin::InList => render_in_list(args),
        other => Err(format!(
            "the SQL operator `{}` is not rendered",
            other.name()
        )),
    }
}

fn render_in_list(args: &[Expr]) -> Result<String, String> {
    let [needle, list] = args else {
        return Err(format!("`inArray` with {} arguments", args.len()));
    };
    let items: Vec<Expr> = match list {
        Expr::Lit(Value::List(items)) => items.iter().cloned().map(Expr::Lit).collect(),
        Expr::Array(items) => items.clone(),
        other => return Err(format!("`inArray` over {}", other.render())),
    };
    if items.is_empty() {
        return Ok("FALSE".to_owned());
    }
    let needle = sub(needle)?;
    let items = items.iter().map(sub).collect::<Result<Vec<_>, _>>()?;
    Ok(format!("({needle} IN ({}))", items.join(", ")))
}

#[cfg(feature = "postgres")]
fn sub(e: &Expr) -> Result<String, String> {
    crate::oracle::expr_sql(e)
}

#[cfg(not(feature = "postgres"))]
fn sub(e: &Expr) -> Result<String, String> {
    Err(format!(
        "{} is rendered by the PostgreSQL oracle only",
        e.render()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[Value]) -> Value {
        Value::List(items.to_vec())
    }

    #[test]
    fn in_list_follows_three_valued_logic() {
        let e = |x: Value, l: Value| eval(Builtin::InList, &[x, l], Mode::Sql).unwrap();
        assert_eq!(
            e(Value::Int(1), list(&[Value::Int(1), Value::Int(2)])),
            Value::Bool(true)
        );
        assert_eq!(
            e(Value::Int(3), list(&[Value::Int(1), Value::Int(2)])),
            Value::Bool(false)
        );
        // `1 IN (2, NULL)` is NULL; `1 IN (1, NULL)` is true.
        assert_eq!(
            e(Value::Int(1), list(&[Value::Int(2), Value::Null])),
            Value::Null
        );
        assert_eq!(
            e(Value::Int(1), list(&[Value::Int(1), Value::Null])),
            Value::Bool(true)
        );
        // `NULL IN (1)` is NULL; Drizzle's empty `inArray` is `false`.
        assert_eq!(e(Value::Null, list(&[Value::Int(1)])), Value::Null);
        assert_eq!(e(Value::Null, list(&[])), Value::Bool(false));
        // NOT of it is `notInArray`: `NOT (1 IN (2, NULL))` stays NULL.
        let not = Expr::not(Expr::Call {
            func: Builtin::InList,
            args: vec![Expr::lit(1), Expr::Lit(list(&[Value::Int(2), Value::Null]))],
        });
        assert_eq!(
            not.eval(&Default::default(), None, Mode::Sql),
            Ok(Value::Null)
        );
    }

    #[test]
    fn in_list_refuses_what_it_does_not_model() {
        let r = eval(Builtin::InList, &[Value::Int(1), Value::Int(2)], Mode::Sql);
        assert!(matches!(r, Err(EvalError::Unknown(_))));
        let r = eval(
            Builtin::InList,
            &[Value::Int(1), list(&[Value::Int(1)])],
            Mode::Js,
        );
        assert!(matches!(r, Err(EvalError::Unknown(_))));
        let r = eval(
            Builtin::InList,
            &[Value::Int(1), list(&[Value::str("a")])],
            Mode::Sql,
        );
        assert!(matches!(r, Err(EvalError::Type(_))));
    }
}

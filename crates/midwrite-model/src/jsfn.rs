//! JavaScript built-ins of the value layer (strings, arrays,
//! `Map`, `Set`) and array methods with a pure callback. Each function
//! implements the ECMAScript behavior for the value types the model has and
//! refuses (an unsupported evaluation) every other receiver.
//!
//! Conventions:
//!
//! * a `null`/`undefined` receiver of a method is the `TypeError` JavaScript
//!   throws (`Cannot read properties of null`): [`EvalError::Type`];
//! * any other receiver or argument whose behavior is not modeled exactly
//!   (a number, a row, a generated value whose text the model does not know,
//!   an object compared by identity, a non-ASCII case mapping, ...) is
//!   [`EvalError::Unknown`]: never a guess;
//! * strings are Rust strings (valid UTF-16 without lone surrogates);
//!   lengths and indices are in UTF-16 code units, as in JavaScript;
//! * numbers are the model's integers; a number outside the exactly
//!   representable range ±(2^53 − 1) is refused where its text or its
//!   double value would matter.
//!
//! The functions do not depend on [`Mode`]: they are JavaScript functions
//! evaluated before a statement is sent (SQL-mode folding of locals calls
//! them with the same meaning); their arguments and callback bodies are
//! evaluated in JavaScript mode.

use crate::value::{Builtin, EachOp, EvalError, Expr, Locals, MapVal, Mode, Row, SetVal, Value};

/// The largest integer JavaScript represents exactly (`Number.MAX_SAFE_INTEGER`).
pub const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;

fn unknown<T>(text: impl Into<String>) -> Result<T, EvalError> {
    Err(EvalError::Unknown(text.into()))
}

/// `TypeError: Cannot read properties of null (reading 'm')`.
fn nullish_receiver<T>(receiver: &Value, what: &str) -> Result<T, EvalError> {
    Err(EvalError::Type(format!(
        "cannot read properties of {} (reading '{what}')",
        receiver.render()
    )))
}

/// A receiver whose method is not modeled for its type.
fn bad_receiver<T>(func: Builtin, receiver: &Value) -> Result<T, EvalError> {
    if receiver.is_nullish() {
        return nullish_receiver(receiver, func.name());
    }
    unknown(format!(
        "`{}` on {} is not modeled",
        func.name(),
        describe(receiver)
    ))
}

fn describe(v: &Value) -> String {
    match v {
        Value::Gen(_) => format!("the generated value {} (its text is not known)", v.render()),
        Value::Row(_) => "an object".to_owned(),
        Value::Int(_) => "a number".to_owned(),
        Value::Bool(_) => "a boolean".to_owned(),
        Value::Str(_) => "a string".to_owned(),
        Value::List(_) => "an array".to_owned(),
        Value::Map(_) => "a Map".to_owned(),
        Value::Set(_) => "a Set".to_owned(),
        Value::Null => "null".to_owned(),
        Value::Undefined(_) => "undefined".to_owned(),
    }
}

/// Whether the value is an object (compared by identity in JavaScript).
fn is_object(v: &Value) -> bool {
    matches!(
        v,
        Value::Row(_) | Value::List(_) | Value::Map(_) | Value::Set(_)
    )
}

/// ECMAScript `SameValueZero` on the model's values (also `IsStrictlyEqual`:
/// the two differ only on `NaN` and `-0`, which the model does not have).
/// Two generated values are equal when they are the same generation event;
/// a generated value differs from every concrete value (the
/// `generated-values-distinct` assumption). An object compared with an
/// object is refused (identity is not modeled); an object never equals a
/// primitive.
pub fn same_value_zero(a: &Value, b: &Value) -> Result<bool, EvalError> {
    Ok(match (a, b) {
        (x, y) if is_object(x) && is_object(y) => {
            return unknown("the identity of objects (arrays, rows, Map, Set) is not modeled");
        }
        (x, y) if is_object(x) || is_object(y) => false,
        (Value::Undefined(_), Value::Undefined(_)) => true,
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Gen(x), Value::Gen(y)) => x == y,
        _ => false,
    })
}

/// A key or item of a `Map`/`Set`: primitives only.
fn key(v: &Value, func: Builtin) -> Result<&Value, EvalError> {
    if is_object(v) {
        return unknown(format!(
            "`{}` with an object key: keys are compared by identity, which is not modeled",
            func.name()
        ));
    }
    Ok(v)
}

fn position(items: &[Value], x: &Value) -> Result<Option<usize>, EvalError> {
    for (i, item) in items.iter().enumerate() {
        if same_value_zero(item, x)? {
            return Ok(Some(i));
        }
    }
    Ok(None)
}

fn map_position(entries: &[(Value, Value)], k: &Value) -> Result<Option<usize>, EvalError> {
    for (i, (key, _)) in entries.iter().enumerate() {
        if same_value_zero(key, k)? {
            return Ok(Some(i));
        }
    }
    Ok(None)
}

/// ECMAScript `ToString` of a value (`String(x)`, template interpolation,
/// `+` with a string operand). Refuses generated values (their text is not
/// known), rows (`[object Object]` is never what the code means to build),
/// `Map`/`Set`, and integers outside ±(2^53 − 1).
pub fn to_string(v: &Value) -> Result<String, EvalError> {
    match v {
        Value::Null => Ok("null".to_owned()),
        Value::Undefined(_) => Ok("undefined".to_owned()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Int(n) => {
            if n.unsigned_abs() > MAX_SAFE_INTEGER as u64 {
                unknown(format!(
                    "the text of {n} (outside the integers JavaScript represents exactly)"
                ))
            } else {
                Ok(n.to_string())
            }
        }
        Value::Str(s) => Ok(s.clone()),
        // Array.prototype.toString is join(","): null and undefined
        // elements are empty.
        Value::List(items) => join(items, ","),
        Value::Gen(_) | Value::Row(_) | Value::Map(_) | Value::Set(_) => unknown(format!(
            "the string conversion of {} is not modeled",
            describe(v)
        )),
    }
}

fn join(items: &[Value], sep: &str) -> Result<String, EvalError> {
    let mut out = String::new();
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(sep);
        }
        if !item.is_nullish() {
            out.push_str(&to_string(item)?);
        }
    }
    Ok(out)
}

/// Length of a string in UTF-16 code units.
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// ECMAScript `WhiteSpace` and `LineTerminator` (what `String.prototype.trim`
/// removes): TAB, VT, FF, ZWNBSP (U+FEFF), the `Space_Separator` (Zs)
/// characters, LF, CR, LS and PS. Unlike Rust's `char::is_whitespace`, it
/// excludes U+0085 (NEL) and includes U+FEFF.
pub fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{FEFF}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{000A}'
                | '\u{000D}'
                | '\u{2028}'
                | '\u{2029}'
    )
}

fn arity(func: Builtin, args: &[Value], min: usize, max: usize) -> Result<(), EvalError> {
    if args.len() < min || args.len() > max {
        return unknown(format!(
            "`{}` with {} argument(s) is not modeled",
            func.name(),
            args.len().saturating_sub(1)
        ));
    }
    Ok(())
}

/// The search string of `includes`/`startsWith`/`endsWith`/`indexOf` on a
/// string: `ToString` of the argument (a regular expression would throw; the
/// model has none).
fn search_string(v: &Value) -> Result<String, EvalError> {
    to_string(v)
}

/// The reserved key of a caught error whose origin the model does
/// not know (a lost connection, a child task, an unmodeled `throw`): its
/// value is the failure's text, and no property of the error is known.
pub const UNMODELED_ERROR: &str = "$unmodeled";
/// The reserved key holding the failure text of a caught error
/// (what a re-throw fails with).
pub const ERROR_REASON: &str = "$reason";
/// The reserved key telling that a caught error is an `Error`
/// instance (`e instanceof Error`).
pub const ERROR_INSTANCE: &str = "$isError";

/// Whether a row is an error object built for a handler that
/// inspects a caught error (it carries one of the reserved keys).
pub fn is_error_object(row: &Row) -> bool {
    row.contains_key(ERROR_INSTANCE)
        || row.contains_key(ERROR_REASON)
        || row.contains_key(UNMODELED_ERROR)
}

/// `recv.key` where `recv` is an error object
/// ([`is_error_object`]). It lists exactly the properties the model knows
/// (an explicit `undefined` included); every other property is unknown
/// rather than `undefined`.
fn caught_prop(recv: &Value, key: &Value) -> Result<Value, EvalError> {
    let Value::Str(name) = key else {
        return unknown("a computed property of a caught error is not modeled");
    };
    match recv {
        r if r.is_nullish() => Err(EvalError::Type(format!(
            "cannot read properties of {} (reading '{name}')",
            r.render()
        ))),
        Value::Row(row) => {
            if let Some(reason) = row.get(UNMODELED_ERROR) {
                return unknown(format!(
                    "the caught error comes from a failure whose error object is not modeled ({}): its property `{name}` is unknown",
                    match reason {
                        Value::Str(s) => s.clone(),
                        other => other.render(),
                    }
                ));
            }
            if name.starts_with('$') && name != ERROR_INSTANCE {
                return unknown(format!(
                    "property `{name}` of a caught error is not modeled"
                ));
            }
            match row.get(name.as_str()) {
                Some(v) => Ok(v.clone()),
                None => unknown(format!(
                    "property `{name}` of a caught error is not modeled (only the SQLSTATE `code` of a database error and the `message`/`name` of a thrown `Error` are)"
                )),
            }
        }
        other => unknown(format!(
            "reading property `{name}` of {} (from a caught error) is not modeled",
            describe(other)
        )),
    }
}

/// `recv[key]` (property access): an array or string element by integer
/// index (`undefined` out of range, negative indices included), a property of
/// an object (`undefined` when absent).
fn at(recv: &Value, key: &Value) -> Result<Value, EvalError> {
    match (recv, key) {
        (r, k) if r.is_nullish() => Err(EvalError::Type(format!(
            "cannot read properties of {} (reading '{}')",
            r.render(),
            match k {
                Value::Str(s) => s.clone(),
                other => other.render(),
            }
        ))),
        (Value::List(items), Value::Int(i)) => Ok(usize::try_from(*i)
            .ok()
            .and_then(|i| items.get(i).cloned())
            .unwrap_or_else(Value::undefined)),
        (Value::Str(s), Value::Int(i)) => {
            // A character outside the BMP is two code units: indexing into
            // it gives a lone surrogate, which is not a model string.
            if s.chars().any(|c| c.len_utf16() > 1) {
                return unknown(
                    "indexing a string with characters outside the Basic Multilingual Plane is not modeled",
                );
            }
            Ok(usize::try_from(*i)
                .ok()
                .and_then(|i| s.chars().nth(i))
                .map_or_else(Value::undefined, |c| Value::Str(c.to_string())))
        }
        // A caught error object knows only its listed properties.
        (Value::Row(row), _) if is_error_object(row) => caught_prop(recv, key),
        (Value::Row(row), Value::Str(k)) => {
            Ok(row.get(k).cloned().unwrap_or_else(Value::undefined))
        }
        _ => unknown(format!(
            "reading {} of {} is not modeled",
            match key {
                Value::Str(s) => format!("property `{s}`"),
                other => format!("index {}", other.render()),
            },
            describe(recv)
        )),
    }
}

/// Evaluate a JavaScript built-in on evaluated arguments (`args[0]` is the
/// receiver of a method).
pub fn eval(func: Builtin, args: &[Value], mode: Mode) -> Result<Value, EvalError> {
    let _ = mode;
    use Builtin as B;
    match func {
        B::ToString => {
            arity(func, args, 1, 1)?;
            Ok(Value::Str(to_string(&args[0])?))
        }
        B::TypeOf => {
            arity(func, args, 1, 1)?;
            Ok(Value::str(match &args[0] {
                Value::Undefined(_) => "undefined",
                Value::Null | Value::Row(_) | Value::List(_) | Value::Map(_) | Value::Set(_) => {
                    "object"
                }
                Value::Bool(_) => "boolean",
                Value::Int(_) => "number",
                Value::Str(_) => "string",
                Value::Gen(_) => {
                    return unknown(
                        "`typeof` of a generated value (a string or a number depending on the column) is not modeled",
                    );
                }
            }))
        }
        B::Concat => {
            let mut out = String::new();
            for a in args {
                out.push_str(&to_string(a)?);
            }
            Ok(Value::Str(out))
        }
        B::Length => {
            arity(func, args, 1, 1)?;
            match &args[0] {
                Value::Str(s) => Ok(Value::Int(utf16_len(s) as i64)),
                Value::List(items) => Ok(Value::Int(items.len() as i64)),
                // A property like any other of an object; numbers,
                // booleans, `Map` and `Set` have none.
                Value::Row(row) if is_error_object(row) => {
                    caught_prop(&args[0], &Value::str("length"))
                }
                Value::Row(row) => Ok(row.get("length").cloned().unwrap_or_else(Value::undefined)),
                Value::Int(_) | Value::Bool(_) | Value::Map(_) | Value::Set(_) => {
                    Ok(Value::undefined())
                }
                other => bad_receiver(func, other),
            }
        }
        B::At => {
            arity(func, args, 2, 2)?;
            at(&args[0], &args[1])
        }
        B::Includes | B::IndexOf => {
            arity(func, args, 2, 2)?;
            let needle = &args[1];
            match &args[0] {
                Value::Str(s) => {
                    let n = search_string(needle)?;
                    if func == B::Includes {
                        Ok(Value::Bool(s.contains(n.as_str())))
                    } else {
                        Ok(Value::Int(match s.find(n.as_str()) {
                            Some(byte) => utf16_len(&s[..byte]) as i64,
                            None => -1,
                        }))
                    }
                }
                Value::List(items) => {
                    if is_object(needle) {
                        return unknown(format!(
                            "`{}` of an object: identity is not modeled",
                            func.name()
                        ));
                    }
                    let found = position(items, needle)?;
                    Ok(if func == B::Includes {
                        Value::Bool(found.is_some())
                    } else {
                        Value::Int(found.map_or(-1, |i| i as i64))
                    })
                }
                other => bad_receiver(func, other),
            }
        }
        B::StartsWith | B::EndsWith => {
            arity(func, args, 2, 2)?;
            match &args[0] {
                Value::Str(s) => {
                    let n = search_string(&args[1])?;
                    Ok(Value::Bool(if func == B::StartsWith {
                        s.starts_with(n.as_str())
                    } else {
                        s.ends_with(n.as_str())
                    }))
                }
                other => bad_receiver(func, other),
            }
        }
        B::Split => {
            arity(func, args, 2, 2)?;
            match (&args[0], &args[1]) {
                (Value::Str(s), Value::Str(sep)) if !sep.is_empty() => {
                    Ok(Value::List(s.split(sep.as_str()).map(Value::str).collect()))
                }
                (Value::Str(_), sep) => unknown(format!(
                    "`split` with the separator {} is not modeled (only a non-empty string)",
                    sep.render()
                )),
                (other, _) => bad_receiver(func, other),
            }
        }
        B::ToLowerCase | B::ToUpperCase => {
            arity(func, args, 1, 1)?;
            match &args[0] {
                Value::Str(s) if s.is_ascii() => Ok(Value::Str(if func == B::ToLowerCase {
                    s.to_ascii_lowercase()
                } else {
                    s.to_ascii_uppercase()
                })),
                Value::Str(_) => unknown(format!(
                    "`{}` of a string with non-ASCII characters is not modeled",
                    func.name()
                )),
                other => bad_receiver(func, other),
            }
        }
        B::Trim => {
            arity(func, args, 1, 1)?;
            match &args[0] {
                Value::Str(s) => Ok(Value::str(s.trim_matches(is_js_whitespace))),
                other => bad_receiver(func, other),
            }
        }
        B::Join => {
            arity(func, args, 1, 2)?;
            let sep = match args.get(1) {
                None | Some(Value::Undefined(_)) => ",".to_owned(),
                Some(v) => to_string(v)?,
            };
            match &args[0] {
                Value::List(items) => Ok(Value::Str(join(items, &sep)?)),
                other => bad_receiver(func, other),
            }
        }
        B::NewMap => {
            arity(func, args, 0, 1)?;
            let mut entries: Vec<(Value, Value)> = Vec::new();
            match args.first() {
                None | Some(Value::Null | Value::Undefined(_)) => {}
                Some(Value::List(items)) => {
                    for item in items {
                        let Value::List(pair) = item else {
                            if is_object(item) {
                                return unknown("a `Map` entry that is not an array");
                            }
                            return Err(EvalError::Type(format!(
                                "iterator value {} is not an entry object",
                                item.render()
                            )));
                        };
                        let k = pair.first().cloned().unwrap_or_else(Value::undefined);
                        let v = pair.get(1).cloned().unwrap_or_else(Value::undefined);
                        key(&k, func)?;
                        match map_position(&entries, &k)? {
                            Some(i) => entries[i].1 = v,
                            None => entries.push((k, v)),
                        }
                    }
                }
                Some(Value::Map(m)) => entries = m.entries.clone(),
                Some(other) => {
                    return unknown(format!("`new Map` of {} is not modeled", describe(other)));
                }
            }
            Ok(Value::Map(MapVal { entries }))
        }
        B::NewSet => {
            arity(func, args, 0, 1)?;
            let source: Vec<Value> = match args.first() {
                None | Some(Value::Null | Value::Undefined(_)) => Vec::new(),
                Some(Value::List(items)) => items.clone(),
                Some(Value::Set(s)) => s.items.clone(),
                // A string iterates by code point.
                Some(Value::Str(s)) => s.chars().map(|c| Value::Str(c.to_string())).collect(),
                Some(other) => {
                    return unknown(format!("`new Set` of {} is not modeled", describe(other)));
                }
            };
            let mut items: Vec<Value> = Vec::new();
            for item in source {
                key(&item, func)?;
                if position(&items, &item)?.is_none() {
                    items.push(item);
                }
            }
            Ok(Value::Set(SetVal { items }))
        }
        B::MapGet => {
            arity(func, args, 2, 2)?;
            match &args[0] {
                Value::Map(m) => {
                    let k = key(&args[1], func)?;
                    Ok(match map_position(&m.entries, k)? {
                        Some(i) => m.entries[i].1.clone(),
                        None => Value::undefined(),
                    })
                }
                other => bad_receiver(func, other),
            }
        }
        B::MapSet => {
            arity(func, args, 3, 3)?;
            match &args[0] {
                Value::Map(m) => {
                    let k = key(&args[1], func)?;
                    let mut entries = m.entries.clone();
                    match map_position(&entries, k)? {
                        Some(i) => entries[i].1 = args[2].clone(),
                        None => entries.push((k.clone(), args[2].clone())),
                    }
                    Ok(Value::Map(MapVal { entries }))
                }
                other => bad_receiver(func, other),
            }
        }
        B::Has => {
            arity(func, args, 2, 2)?;
            let k = &args[1];
            match &args[0] {
                Value::Map(m) => Ok(Value::Bool(
                    map_position(&m.entries, key(k, func)?)?.is_some(),
                )),
                Value::Set(s) => Ok(Value::Bool(position(&s.items, key(k, func)?)?.is_some())),
                other => bad_receiver(func, other),
            }
        }
        B::SetAdd => {
            arity(func, args, 2, 2)?;
            match &args[0] {
                Value::Set(s) => {
                    let x = key(&args[1], func)?;
                    let mut items = s.items.clone();
                    if position(&items, x)?.is_none() {
                        items.push(x.clone());
                    }
                    Ok(Value::Set(SetVal { items }))
                }
                other => bad_receiver(func, other),
            }
        }
        B::Delete => {
            arity(func, args, 2, 2)?;
            let k = &args[1];
            match &args[0] {
                Value::Map(m) => {
                    let mut entries = m.entries.clone();
                    if let Some(i) = map_position(&entries, key(k, func)?)? {
                        entries.remove(i);
                    }
                    Ok(Value::Map(MapVal { entries }))
                }
                Value::Set(s) => {
                    let mut items = s.items.clone();
                    if let Some(i) = position(&items, key(k, func)?)? {
                        items.remove(i);
                    }
                    Ok(Value::Set(SetVal { items }))
                }
                other => bad_receiver(func, other),
            }
        }
        B::Size => {
            arity(func, args, 1, 1)?;
            match &args[0] {
                Value::Map(m) => Ok(Value::Int(m.entries.len() as i64)),
                Value::Set(s) => Ok(Value::Int(s.items.len() as i64)),
                // `x.size` of another value is its property `size`.
                Value::Row(row) if is_error_object(row) => {
                    caught_prop(&args[0], &Value::str("size"))
                }
                Value::Row(row) => Ok(row.get("size").cloned().unwrap_or_else(Value::undefined)),
                Value::Int(_) | Value::Bool(_) | Value::Str(_) | Value::List(_) | Value::Gen(_) => {
                    Ok(Value::undefined())
                }
                other => bad_receiver(func, other),
            }
        }
        // Plain objects (rows).
        B::HasOwn => {
            arity(func, args, 2, 2)?;
            match (&args[0], &args[1]) {
                (Value::Row(row), Value::Str(k)) => Ok(Value::Bool(row.contains_key(k))),
                (r, _) if r.is_nullish() => Err(EvalError::Type(
                    "cannot convert undefined or null to object".to_owned(),
                )),
                (r, k) => unknown(format!(
                    "`Object.hasOwn` of {} with the key {} is not modeled",
                    describe(r),
                    k.render()
                )),
            }
        }
        B::OmitKeys => {
            arity(func, args, 2, 2)?;
            match (&args[0], &args[1]) {
                (Value::Row(row), Value::List(keys)) => {
                    let mut out = row.clone();
                    for k in keys {
                        match k {
                            Value::Str(k) => {
                                out.remove(k);
                            }
                            other => {
                                return unknown(format!(
                                    "an object rest without the key {}",
                                    other.render()
                                ));
                            }
                        }
                    }
                    Ok(Value::Row(out))
                }
                (r, _) if r.is_nullish() => Err(EvalError::Type(
                    "cannot destructure undefined or null".to_owned(),
                )),
                (r, _) => unknown(format!("the rest of {} is not modeled", describe(r))),
            }
        }
        B::ObjectAssign => {
            arity(func, args, 2, 2)?;
            let mut out = Row::new();
            for a in args {
                match a {
                    Value::Row(row) => out.extend(row.iter().map(|(k, v)| (k.clone(), v.clone()))),
                    v if v.is_nullish() => {}
                    other => {
                        return unknown(format!("spreading {} is not modeled", describe(other)));
                    }
                }
            }
            Ok(Value::Row(out))
        }
        B::DefinedEntries => {
            arity(func, args, 1, 1)?;
            match &args[0] {
                Value::Row(row) => Ok(Value::Row(
                    row.iter()
                        .filter(|(_, v)| !matches!(v, Value::Undefined(_)))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                )),
                other => bad_receiver(func, other),
            }
        }
        B::ZodData => {
            arity(func, args, 2, 2)?;
            let (version, schema) =
                crate::zodfn::from_value(&args[0]).map_err(EvalError::Unknown)?;
            match crate::zodfn::parse(version, &schema, &args[1]) {
                Ok(Some(out)) => Ok(out),
                Ok(None) => unknown("zod data of an input the schema rejects"),
                Err(why) => unknown(format!("zod validation: {why}")),
            }
        }
        B::ZodCheck => {
            arity(func, args, 2, 2)?;
            let (version, schema) =
                crate::zodfn::from_value(&args[0]).map_err(EvalError::Unknown)?;
            match crate::zodfn::parse(version, &schema, &args[1]) {
                Ok(out) => Ok(Value::Bool(out.is_some())),
                Err(why) => unknown(format!("zod validation: {why}")),
            }
        }
        other if other.is_sql() => unknown(format!(
            "the SQL operator `{}` evaluated as JavaScript",
            other.name()
        )),
        other => unknown(format!("the built-in `{}` is not modeled", other.name())),
    }
}

/// Evaluate `list.<op>((var) => body)`: the callback is applied to the
/// elements in index order, stopping where JavaScript stops (`find`,
/// `findIndex`, `some`, `every`).
pub fn each(
    op: EachOp,
    list: &Value,
    var: &str,
    body: &Expr,
    locals: &Locals,
    row: Option<&Row>,
    mode: Mode,
) -> Result<Value, EvalError> {
    let items = match list {
        Value::List(items) => items,
        other if other.is_nullish() => {
            return nullish_receiver(other, &format!("{op:?}").to_lowercase());
        }
        other => {
            return unknown(format!(
                "the array method `{op:?}` on {} is not modeled",
                describe(other)
            ));
        }
    };
    // The callback is JavaScript whatever the enclosing mode (SQL-mode
    // folding of a statement's values evaluates it before sending).
    let _ = mode;
    let mut scope = locals.clone();
    let mut apply = |item: &Value| -> Result<Value, EvalError> {
        scope.insert(var.to_owned(), item.clone());
        body.eval(&scope, row, Mode::Js)
    };
    match op {
        EachOp::Map => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(apply(item)?);
            }
            Ok(Value::List(out))
        }
        EachOp::Filter => {
            let mut out = Vec::new();
            for item in items {
                if apply(item)?.truthy() {
                    out.push(item.clone());
                }
            }
            Ok(Value::List(out))
        }
        EachOp::Find => {
            for item in items {
                if apply(item)?.truthy() {
                    return Ok(item.clone());
                }
            }
            Ok(Value::undefined())
        }
        EachOp::FindIndex => {
            for (i, item) in items.iter().enumerate() {
                if apply(item)?.truthy() {
                    return Ok(Value::Int(i as i64));
                }
            }
            Ok(Value::Int(-1))
        }
        EachOp::Some => {
            for item in items {
                if apply(item)?.truthy() {
                    return Ok(Value::Bool(true));
                }
            }
            Ok(Value::Bool(false))
        }
        EachOp::Every => {
            for item in items {
                if !apply(item)?.truthy() {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        }
    }
}

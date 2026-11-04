//! Phase 16: zod schemas as model values, and `schema.safeParse(x)` for
//! the documented subset (`docs/phase16/ZOD-VALIDATION.md`).
//!
//! A schema the extractor reads (`crate::value::Builtin::ZodCheck`'s first
//! argument) is a literal [`Value`] *descriptor* (see [`to_value`]) carrying
//! the zod API it was built with. [`parse`] is `safeParse` of that schema on
//! a model value: `Ok(Some(output))` on success, `Ok(None)` on failure,
//! `Err(why)` when the outcome is not modeled (never a guess).
//!
//! The semantics were read from the package sources and are checked against
//! the real runtimes by `crates/midwrite-analyzer/tests/phase16_zod.rs`:
//!
//! * zod 3 (`zod` 3.x, `zod/v3` of 3.25 and 4.x): `zod/v3/types.js` and
//!   `zod/v3/helpers/parseUtil.js` of 3.25.76;
//! * zod 4 (`zod` 4.x, `zod/v4` of 4.x): `zod/v4/core/schemas.js`,
//!   `checks.js` and `regexes.js` of 4.6.5.
//!
//! When the major version is unknown ([`Version::Either`]) both are
//! evaluated and the result is used only when they agree.

use crate::jsfn::{MAX_SAFE_INTEGER, is_js_whitespace, same_value_zero, utf16_len};
use crate::jsregex::Regex;
use crate::value::{Row, Value};

/// The zod API a schema was built with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Version {
    V3,
    V4,
    /// The project's zod major version is unknown: both are evaluated and
    /// must agree.
    Either,
}

impl Version {
    pub fn tag(self) -> &'static str {
        match self {
            Self::V3 => "3",
            Self::V4 => "4",
            Self::Either => "3|4",
        }
    }
    fn from_tag(t: &str) -> Option<Self> {
        Some(match t {
            "3" => Self::V3,
            "4" => Self::V4,
            "3|4" => Self::Either,
            _ => return None,
        })
    }
    fn apis(self) -> &'static [Api] {
        match self {
            Self::V3 => &[Api::V3],
            Self::V4 => &[Api::V4],
            Self::Either => &[Api::V3, Api::V4],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Api {
    V3,
    V4,
}

/// A modeled zod schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Schema {
    Str(Vec<StrCheck>),
    /// `z.number()`: the model's numbers are integers.
    Num(Vec<NumCheck>),
    Bool,
    /// `z.literal(v)`.
    Literal(Value),
    /// `z.enum([...])`.
    Enum(Vec<String>),
    /// `z.object({...})` (unknown keys stripped): keys in shape order.
    Object(Vec<(String, Schema)>),
    /// Phase 16: `z.strictObject`, `.strict()`: unknown keys rejected.
    StrictObject(Vec<(String, Schema)>),
    /// Phase 16: `.passthrough()`, zod 4 `z.looseObject`: unknown keys kept.
    LooseObject(Vec<(String, Schema)>),
    /// Phase 16: `z.array(item)` with its length checks.
    Array(Box<Schema>, Vec<ArrCheck>),
    /// Phase 16: `.optional()`.
    Optional(Box<Schema>),
    /// Phase 16: `.nullable()`.
    Nullable(Box<Schema>),
    /// Phase 16: `.default(literal)`.
    Default(Box<Schema>, Value),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StrCheck {
    Min(i64),
    Max(i64),
    Length(i64),
    Trim,
    Lower,
    Upper,
    StartsWith(String),
    EndsWith(String),
    Includes(String),
    /// Phase 16: `.uuid()` / zod 4 `z.uuid()` (each version's pattern).
    Uuid,
    /// Phase 16: `.email()` / zod 4 `z.email()` (each version's pattern).
    Email,
    /// Phase 16: `.regex(/pattern/flags)` (the subset of [`crate::jsregex`]).
    Regex {
        pattern: String,
        flags: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NumCheck {
    /// `min`/`gte`: value >= n.
    Ge(i64),
    /// `gt`/`positive` (n = 0).
    Gt(i64),
    /// `max`/`lte`.
    Le(i64),
    /// `lt`/`negative` (n = 0).
    Lt(i64),
    /// Phase 16: `.int()` (zod 3: `Number.isInteger`; zod 4: a safe
    /// integer). The model's numbers are integers within ±(2^53 − 1)
    /// ([`parse`] refuses others), where both hold.
    Int,
}

/// Phase 16: `z.array(...).min/max/length/nonempty`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArrCheck {
    Min(i64),
    Max(i64),
    Length(i64),
}

/// zod 3's `uuidRegex` (`zod/v3/types.js` of 3.25.76).
pub const V3_UUID: (&str, &str) = (
    r"^[0-9a-fA-F]{8}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{12}$",
    "i",
);
/// zod 3's `emailRegex` (`zod/v3/types.js` of 3.25.76).
pub const V3_EMAIL: (&str, &str) = (
    r"^(?!\.)(?!.*\.\.)([A-Z0-9_'+\-\.]*)[A-Z0-9_+-]@([A-Z0-9][A-Z0-9\-]*\.)+[A-Z]{2,}$",
    "i",
);
/// zod 4's `uuid()` without a version (`zod/v4/core/regexes.js` of 4.6.5:
/// RFC 9562 versions 1–8 and variant 10, plus the nil and max UUIDs).
pub const V4_UUID: (&str, &str) = (
    r"^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}|00000000-0000-0000-0000-000000000000|ffffffff-ffff-ffff-ffff-ffffffffffff)$",
    "",
);
/// zod 4's `email` (`zod/v4/core/regexes.js` of 4.6.5).
pub const V4_EMAIL: (&str, &str) = (
    r"^(?:[A-Za-z0-9_'+\-]+\.)*[A-Za-z0-9_'+\-]*[A-Za-z0-9_+-]@(?:[A-Za-z0-9][A-Za-z0-9\-]*\.)+[A-Za-z]{2,}$",
    "",
);

/// Property names of `Object.prototype`: `key in input` and `input[key]`
/// read them through the prototype, which the model's rows do not have.
pub const PROTOTYPE_KEYS: &[&str] = &[
    "__proto__",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
];

impl Schema {
    /// Whether the schema uses only the Phase 14 subset (lowered as
    /// explicit steps by the extractor, unchanged).
    pub fn is_phase14(&self) -> bool {
        match self {
            Self::Str(checks) => checks
                .iter()
                .all(|c| !matches!(c, StrCheck::Uuid | StrCheck::Email | StrCheck::Regex { .. })),
            Self::Num(checks) => !checks.contains(&NumCheck::Int),
            Self::Bool | Self::Literal(_) | Self::Enum(_) => true,
            Self::Object(fields) => fields.iter().all(|(_, s)| s.is_phase14()),
            _ => false,
        }
    }

    /// The fields and unknown-key mode of an object schema.
    pub fn object(&self) -> Option<(&[(String, Schema)], Unknown)> {
        match self {
            Self::Object(f) => Some((f, Unknown::Strip)),
            Self::StrictObject(f) => Some((f, Unknown::Strict)),
            Self::LooseObject(f) => Some((f, Unknown::Passthrough)),
            _ => None,
        }
    }

    /// Whether every valid input is its own output (no transform, no
    /// default, no object rebuilt).
    pub fn output_is_input(&self) -> bool {
        match self {
            Self::Str(checks) => !checks
                .iter()
                .any(|c| matches!(c, StrCheck::Trim | StrCheck::Lower | StrCheck::Upper)),
            Self::Num(_) | Self::Bool | Self::Literal(_) | Self::Enum(_) => true,
            Self::Array(item, _) => item.output_is_input(),
            Self::Optional(s) | Self::Nullable(s) => s.output_is_input(),
            Self::Default(..) | Self::Object(_) | Self::StrictObject(_) | Self::LooseObject(_) => {
                false
            }
        }
    }
}

/// What an object schema does with keys outside its shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unknown {
    Strip,
    Strict,
    Passthrough,
}

// ---------------------------------------------------------------------------
// Descriptors
// ---------------------------------------------------------------------------

fn row(entries: Vec<(&str, Value)>) -> Value {
    Value::Row(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    )
}

/// The descriptor of `schema` built with `version`: the literal first
/// argument of `ZodCheck`.
pub fn to_value(version: Version, schema: &Schema) -> Value {
    row(vec![
        ("zod", Value::str(version.tag())),
        ("schema", schema_value(schema)),
    ])
}

fn schema_value(s: &Schema) -> Value {
    let ty = |t: &str| ("type", Value::str(t));
    let shape = |fields: &[(String, Schema)]| {
        Value::List(
            fields
                .iter()
                .map(|(k, s)| Value::List(vec![Value::str(k.as_str()), schema_value(s)]))
                .collect(),
        )
    };
    match s {
        Schema::Str(checks) => row(vec![
            ty("string"),
            (
                "checks",
                Value::List(
                    checks
                        .iter()
                        .map(|c| match c {
                            StrCheck::Min(n) => check("min", Some(Value::Int(*n))),
                            StrCheck::Max(n) => check("max", Some(Value::Int(*n))),
                            StrCheck::Length(n) => check("length", Some(Value::Int(*n))),
                            StrCheck::Trim => check("trim", None),
                            StrCheck::Lower => check("toLowerCase", None),
                            StrCheck::Upper => check("toUpperCase", None),
                            StrCheck::StartsWith(t) => {
                                check("startsWith", Some(Value::str(t.as_str())))
                            }
                            StrCheck::EndsWith(t) => {
                                check("endsWith", Some(Value::str(t.as_str())))
                            }
                            StrCheck::Includes(t) => {
                                check("includes", Some(Value::str(t.as_str())))
                            }
                            StrCheck::Uuid => check("uuid", None),
                            StrCheck::Email => check("email", None),
                            StrCheck::Regex { pattern, flags } => row(vec![
                                ("check", Value::str("regex")),
                                ("pattern", Value::str(pattern.as_str())),
                                ("flags", Value::str(flags.as_str())),
                            ]),
                        })
                        .collect(),
                ),
            ),
        ]),
        Schema::Num(checks) => row(vec![
            ty("number"),
            (
                "checks",
                Value::List(
                    checks
                        .iter()
                        .map(|c| match c {
                            NumCheck::Ge(n) => check("gte", Some(Value::Int(*n))),
                            NumCheck::Gt(n) => check("gt", Some(Value::Int(*n))),
                            NumCheck::Le(n) => check("lte", Some(Value::Int(*n))),
                            NumCheck::Lt(n) => check("lt", Some(Value::Int(*n))),
                            NumCheck::Int => check("int", None),
                        })
                        .collect(),
                ),
            ),
        ]),
        Schema::Bool => row(vec![ty("boolean")]),
        Schema::Literal(v) => row(vec![ty("literal"), ("value", v.clone())]),
        Schema::Enum(items) => row(vec![
            ty("enum"),
            (
                "values",
                Value::List(items.iter().map(|i| Value::str(i.as_str())).collect()),
            ),
        ]),
        Schema::Object(f) => row(vec![
            ty("object"),
            ("unknownKeys", Value::str("strip")),
            ("shape", shape(f)),
        ]),
        Schema::StrictObject(f) => row(vec![
            ty("object"),
            ("unknownKeys", Value::str("strict")),
            ("shape", shape(f)),
        ]),
        Schema::LooseObject(f) => row(vec![
            ty("object"),
            ("unknownKeys", Value::str("passthrough")),
            ("shape", shape(f)),
        ]),
        Schema::Array(item, checks) => row(vec![
            ty("array"),
            ("element", schema_value(item)),
            (
                "checks",
                Value::List(
                    checks
                        .iter()
                        .map(|c| match c {
                            ArrCheck::Min(n) => check("min", Some(Value::Int(*n))),
                            ArrCheck::Max(n) => check("max", Some(Value::Int(*n))),
                            ArrCheck::Length(n) => check("length", Some(Value::Int(*n))),
                        })
                        .collect(),
                ),
            ),
        ]),
        Schema::Optional(s) => row(vec![ty("optional"), ("inner", schema_value(s))]),
        Schema::Nullable(s) => row(vec![ty("nullable"), ("inner", schema_value(s))]),
        Schema::Default(s, d) => row(vec![
            ty("default"),
            ("inner", schema_value(s)),
            ("value", d.clone()),
        ]),
    }
}

fn check(name: &str, value: Option<Value>) -> Value {
    let mut entries = vec![("check", Value::str(name))];
    if let Some(v) = value {
        entries.push(("value", v));
    }
    row(entries)
}

/// Read a descriptor back.
pub fn from_value(v: &Value) -> Result<(Version, Schema), String> {
    let bad = || "a malformed zod schema descriptor".to_owned();
    let Value::Row(r) = v else { return Err(bad()) };
    let version = match r.get("zod") {
        Some(Value::Str(t)) => Version::from_tag(t).ok_or_else(bad)?,
        _ => return Err(bad()),
    };
    Ok((version, schema_from(r.get("schema").ok_or_else(bad)?)?))
}

fn schema_from(v: &Value) -> Result<Schema, String> {
    let bad = || "a malformed zod schema descriptor".to_owned();
    let Value::Row(r) = v else { return Err(bad()) };
    let text = |k: &str| match r.get(k) {
        Some(Value::Str(s)) => Ok(s.clone()),
        _ => Err(bad()),
    };
    let list = |k: &str| match r.get(k) {
        Some(Value::List(items)) => Ok(items.clone()),
        _ => Err(bad()),
    };
    let int_of = |c: &Row| match c.get("value") {
        Some(Value::Int(n)) => Ok(*n),
        _ => Err(bad()),
    };
    let str_of = |c: &Row| match c.get("value") {
        Some(Value::Str(s)) => Ok(s.clone()),
        _ => Err(bad()),
    };
    let checks = || -> Result<Vec<Row>, String> {
        list("checks")?
            .into_iter()
            .map(|c| match c {
                Value::Row(c) => Ok(c),
                _ => Err(bad()),
            })
            .collect()
    };
    let name = |c: &Row| match c.get("check") {
        Some(Value::Str(s)) => Ok(s.clone()),
        _ => Err(bad()),
    };
    let inner = || schema_from(r.get("inner").ok_or_else(bad)?).map(Box::new);
    let shape = || -> Result<Vec<(String, Schema)>, String> {
        list("shape")?
            .iter()
            .map(|pair| match pair {
                Value::List(p) => match p.as_slice() {
                    [Value::Str(k), s] => Ok((k.clone(), schema_from(s)?)),
                    _ => Err(bad()),
                },
                _ => Err(bad()),
            })
            .collect()
    };
    Ok(match text("type")?.as_str() {
        "string" => Schema::Str(
            checks()?
                .iter()
                .map(|c| {
                    Ok(match name(c)?.as_str() {
                        "min" => StrCheck::Min(int_of(c)?),
                        "max" => StrCheck::Max(int_of(c)?),
                        "length" => StrCheck::Length(int_of(c)?),
                        "trim" => StrCheck::Trim,
                        "toLowerCase" => StrCheck::Lower,
                        "toUpperCase" => StrCheck::Upper,
                        "startsWith" => StrCheck::StartsWith(str_of(c)?),
                        "endsWith" => StrCheck::EndsWith(str_of(c)?),
                        "includes" => StrCheck::Includes(str_of(c)?),
                        "uuid" => StrCheck::Uuid,
                        "email" => StrCheck::Email,
                        "regex" => match (c.get("pattern"), c.get("flags")) {
                            (Some(Value::Str(p)), Some(Value::Str(f))) => StrCheck::Regex {
                                pattern: p.clone(),
                                flags: f.clone(),
                            },
                            _ => return Err(bad()),
                        },
                        _ => return Err(bad()),
                    })
                })
                .collect::<Result<_, String>>()?,
        ),
        "number" => Schema::Num(
            checks()?
                .iter()
                .map(|c| {
                    Ok(match name(c)?.as_str() {
                        "gte" => NumCheck::Ge(int_of(c)?),
                        "gt" => NumCheck::Gt(int_of(c)?),
                        "lte" => NumCheck::Le(int_of(c)?),
                        "lt" => NumCheck::Lt(int_of(c)?),
                        "int" => NumCheck::Int,
                        _ => return Err(bad()),
                    })
                })
                .collect::<Result<_, String>>()?,
        ),
        "boolean" => Schema::Bool,
        "literal" => Schema::Literal(r.get("value").cloned().ok_or_else(bad)?),
        "enum" => Schema::Enum(
            list("values")?
                .into_iter()
                .map(|v| match v {
                    Value::Str(s) => Ok(s),
                    _ => Err(bad()),
                })
                .collect::<Result<_, String>>()?,
        ),
        "object" => {
            let fields = shape()?;
            match text("unknownKeys")?.as_str() {
                "strip" => Schema::Object(fields),
                "strict" => Schema::StrictObject(fields),
                "passthrough" => Schema::LooseObject(fields),
                _ => return Err(bad()),
            }
        }
        "array" => Schema::Array(
            Box::new(schema_from(r.get("element").ok_or_else(bad)?)?),
            checks()?
                .iter()
                .map(|c| {
                    Ok(match name(c)?.as_str() {
                        "min" => ArrCheck::Min(int_of(c)?),
                        "max" => ArrCheck::Max(int_of(c)?),
                        "length" => ArrCheck::Length(int_of(c)?),
                        _ => return Err(bad()),
                    })
                })
                .collect::<Result<_, String>>()?,
        ),
        "optional" => Schema::Optional(inner()?),
        "nullable" => Schema::Nullable(inner()?),
        "default" => Schema::Default(inner()?, r.get("value").cloned().ok_or_else(bad)?),
        _ => return Err(bad()),
    })
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn js_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("{s:?}"))
}

/// The schema as the zod builder chain it stands for.
pub fn render(s: &Schema) -> String {
    let fields = |f: &[(String, Schema)]| {
        f.iter()
            .map(|(k, s)| format!("{k}: {}", render(s)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match s {
        Schema::Str(checks) => {
            let mut out = "z.string()".to_owned();
            for c in checks {
                out.push_str(&match c {
                    StrCheck::Min(n) => format!(".min({n})"),
                    StrCheck::Max(n) => format!(".max({n})"),
                    StrCheck::Length(n) => format!(".length({n})"),
                    StrCheck::Trim => ".trim()".to_owned(),
                    StrCheck::Lower => ".toLowerCase()".to_owned(),
                    StrCheck::Upper => ".toUpperCase()".to_owned(),
                    StrCheck::StartsWith(t) => format!(".startsWith({})", js_string(t)),
                    StrCheck::EndsWith(t) => format!(".endsWith({})", js_string(t)),
                    StrCheck::Includes(t) => format!(".includes({})", js_string(t)),
                    StrCheck::Uuid => ".uuid()".to_owned(),
                    StrCheck::Email => ".email()".to_owned(),
                    StrCheck::Regex { pattern, flags } => format!(".regex(/{pattern}/{flags})"),
                });
            }
            out
        }
        Schema::Num(checks) => {
            let mut out = "z.number()".to_owned();
            for c in checks {
                out.push_str(&match c {
                    NumCheck::Ge(n) => format!(".gte({n})"),
                    NumCheck::Gt(n) => format!(".gt({n})"),
                    NumCheck::Le(n) => format!(".lte({n})"),
                    NumCheck::Lt(n) => format!(".lt({n})"),
                    NumCheck::Int => ".int()".to_owned(),
                });
            }
            out
        }
        Schema::Bool => "z.boolean()".to_owned(),
        Schema::Literal(v) => format!("z.literal({})", v.render()),
        Schema::Enum(items) => format!(
            "z.enum([{}])",
            items
                .iter()
                .map(|i| js_string(i))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Schema::Object(f) => format!("z.object({{ {} }})", fields(f)),
        Schema::StrictObject(f) => format!("z.strictObject({{ {} }})", fields(f)),
        Schema::LooseObject(f) => format!("z.object({{ {} }}).passthrough()", fields(f)),
        Schema::Array(item, checks) => {
            let mut out = format!("z.array({})", render(item));
            for c in checks {
                out.push_str(&match c {
                    ArrCheck::Min(n) => format!(".min({n})"),
                    ArrCheck::Max(n) => format!(".max({n})"),
                    ArrCheck::Length(n) => format!(".length({n})"),
                });
            }
            out
        }
        Schema::Optional(s) => format!("{}.optional()", render(s)),
        Schema::Nullable(s) => format!("{}.nullable()", render(s)),
        Schema::Default(s, d) => format!("{}.default({})", render(s), d.render()),
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// The outcome of a parse: `Some(output)` on success, `None` on failure.
type Outcome = Result<Option<Value>, String>;

/// `schema.safeParse(input)`: `Ok(Some(data))`, `Ok(None)` (failure), or
/// `Err` when the outcome is not modeled. With [`Version::Either`], zod 3
/// and zod 4 must agree.
pub fn parse(version: Version, schema: &Schema, input: &Value) -> Outcome {
    let mut result: Option<Option<Value>> = None;
    for &api in version.apis() {
        let r = run(api, schema, input)?;
        match &result {
            None => result = Some(r),
            Some(prev) if *prev == r => {}
            Some(_) => {
                return Err(
                    "the project's zod major version is unknown and zod 3 and zod 4 parse this input differently"
                        .to_owned(),
                );
            }
        }
    }
    Ok(result.unwrap_or(None))
}

fn unsupported_input(v: &Value) -> Option<String> {
    match v {
        Value::Gen(_) => Some(format!(
            "validating the generated value {} (its type and text are not known)",
            v.render()
        )),
        Value::Int(n) if n.unsigned_abs() > MAX_SAFE_INTEGER as u64 => Some(format!(
            "validating the number {n} (outside the integers JavaScript represents exactly)"
        )),
        _ => None,
    }
}

/// zod 4 `optin`: whether (and how) a schema admits an absent value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OptIn {
    Required,
    Optional,
    Defaulted,
}

fn optin(s: &Schema) -> OptIn {
    match s {
        Schema::Optional(inner) => {
            if optin(inner) == OptIn::Defaulted {
                OptIn::Defaulted
            } else {
                OptIn::Optional
            }
        }
        Schema::Nullable(inner) => optin(inner),
        Schema::Default(..) => OptIn::Defaulted,
        _ => OptIn::Required,
    }
}

/// zod 4 `optout === "optional"`.
fn optout(s: &Schema) -> bool {
    match s {
        Schema::Optional(_) => true,
        Schema::Nullable(inner) => optout(inner),
        _ => false,
    }
}

fn run(api: Api, s: &Schema, input: &Value) -> Outcome {
    match s {
        Schema::Optional(inner) => {
            if matches!(input, Value::Undefined(_)) {
                // zod 3 returns `undefined`; zod 4 lets a defaulted inner
                // substitute its default (`handleOptionalResult`).
                if api == Api::V4 && optin(inner) == OptIn::Defaulted {
                    return Ok(Some(
                        run(api, inner, input)?.unwrap_or_else(Value::undefined),
                    ));
                }
                return Ok(Some(Value::undefined()));
            }
            run(api, inner, input)
        }
        Schema::Nullable(inner) => {
            if input.is_null() {
                return Ok(Some(Value::Null));
            }
            run(api, inner, input)
        }
        Schema::Default(inner, d) => {
            if matches!(input, Value::Undefined(_)) {
                // zod 3 parses the default through the inner schema; zod 4
                // returns it as is.
                return match api {
                    Api::V3 => run(api, inner, d),
                    Api::V4 => Ok(Some(d.clone())),
                };
            }
            let out = run(api, inner, input)?;
            Ok(match (api, out) {
                (Api::V4, Some(Value::Undefined(_))) => Some(d.clone()),
                (_, out) => out,
            })
        }
        _ => {
            if let Some(why) = unsupported_input(input) {
                return Err(why);
            }
            base(api, s, input)
        }
    }
}

fn base(api: Api, s: &Schema, input: &Value) -> Outcome {
    match s {
        Schema::Str(checks) => {
            let Value::Str(text) = input else {
                return Ok(None);
            };
            let mut cur = text.clone();
            for c in checks {
                let ok = match c {
                    StrCheck::Min(n) => utf16_len(&cur) as i64 >= *n,
                    StrCheck::Max(n) => utf16_len(&cur) as i64 <= *n,
                    StrCheck::Length(n) => utf16_len(&cur) as i64 == *n,
                    StrCheck::StartsWith(t) => cur.starts_with(t.as_str()),
                    StrCheck::EndsWith(t) => cur.ends_with(t.as_str()),
                    StrCheck::Includes(t) => cur.contains(t.as_str()),
                    StrCheck::Trim => {
                        cur = cur.trim_matches(is_js_whitespace).to_owned();
                        true
                    }
                    StrCheck::Lower | StrCheck::Upper => {
                        if !cur.is_ascii() {
                            return Err(
                                "zod `.toLowerCase()`/`.toUpperCase()` of a string with non-ASCII characters is not modeled"
                                    .to_owned(),
                            );
                        }
                        cur = if *c == StrCheck::Lower {
                            cur.to_ascii_lowercase()
                        } else {
                            cur.to_ascii_uppercase()
                        };
                        true
                    }
                    StrCheck::Uuid => {
                        let (p, f) = if api == Api::V3 { V3_UUID } else { V4_UUID };
                        Regex::new(p, f)?.test(&cur)?
                    }
                    StrCheck::Email => {
                        let (p, f) = if api == Api::V3 { V3_EMAIL } else { V4_EMAIL };
                        Regex::new(p, f)?.test(&cur)?
                    }
                    StrCheck::Regex { pattern, flags } => Regex::new(pattern, flags)?.test(&cur)?,
                };
                if !ok {
                    return Ok(None);
                }
            }
            Ok(Some(Value::Str(cur)))
        }
        Schema::Num(checks) => {
            let Value::Int(n) = input else {
                return Ok(None);
            };
            let ok = checks.iter().all(|c| match c {
                NumCheck::Ge(m) => n >= m,
                NumCheck::Gt(m) => n > m,
                NumCheck::Le(m) => n <= m,
                NumCheck::Lt(m) => n < m,
                NumCheck::Int => true,
            });
            Ok(ok.then(|| input.clone()))
        }
        Schema::Bool => Ok(matches!(input, Value::Bool(_)).then(|| input.clone())),
        Schema::Literal(v) => Ok(same_value_zero(input, v)
            .map_err(|e| e.to_string())?
            .then(|| input.clone())),
        Schema::Enum(items) => Ok(match input {
            Value::Str(s) if items.contains(s) => Some(input.clone()),
            _ => None,
        }),
        Schema::Array(item, checks) => {
            let Value::List(items) = input else {
                return Ok(None);
            };
            let len = items.len() as i64;
            // zod 3 keeps one slot per kind (the last call wins); zod 4
            // keeps every check.
            let effective: Vec<&ArrCheck> = match api {
                Api::V4 => checks.iter().collect(),
                Api::V3 => {
                    let last = |f: fn(&ArrCheck) -> bool| checks.iter().rev().find(|c| f(c));
                    [
                        last(|c| matches!(c, ArrCheck::Length(_))),
                        last(|c| matches!(c, ArrCheck::Min(_))),
                        last(|c| matches!(c, ArrCheck::Max(_))),
                    ]
                    .into_iter()
                    .flatten()
                    .collect()
                }
            };
            let mut failed = !effective.iter().all(|c| match c {
                ArrCheck::Min(n) => len >= *n,
                ArrCheck::Max(n) => len <= *n,
                ArrCheck::Length(n) => len == *n,
            });
            let mut out = Vec::with_capacity(items.len());
            let mut unknown = None;
            for v in items {
                match run(api, item, v) {
                    Ok(Some(o)) => out.push(o),
                    Ok(None) => failed = true,
                    Err(e) => unknown = unknown.or(Some(e)),
                }
            }
            if failed {
                return Ok(None);
            }
            if let Some(e) = unknown {
                return Err(e);
            }
            Ok(Some(Value::List(out)))
        }
        Schema::Object(_) | Schema::StrictObject(_) | Schema::LooseObject(_) => {
            object(api, s, input)
        }
        Schema::Optional(_) | Schema::Nullable(_) | Schema::Default(..) => run(api, s, input),
    }
}

fn object(api: Api, s: &Schema, input: &Value) -> Outcome {
    let Some((fields, unknown_keys)) = s.object() else {
        return Ok(None);
    };
    let data = match input {
        Value::Row(r) => r,
        // zod 3 parses a Map or Set as its own type; zod 4's `isObject`
        // accepts them as objects (with no own properties) — not modeled.
        Value::Map(_) | Value::Set(_) if api == Api::V4 => {
            return Err("a Map or Set as the input of a zod 4 object schema".to_owned());
        }
        _ => return Ok(None),
    };
    if data.contains_key("__proto__") {
        return Err("an object input with an own `__proto__` key".to_owned());
    }
    let mut out = Row::new();
    let mut failed = false;
    let mut unknown: Option<String> = None;
    for (key, field) in fields {
        if PROTOTYPE_KEYS.contains(&key.as_str()) {
            return Err(format!(
                "the zod object key `{key}` (a property of Object.prototype)"
            ));
        }
        let present = data.contains_key(key);
        let value = data.get(key).cloned().unwrap_or_else(Value::undefined);
        match api {
            Api::V3 => match run(api, field, &value) {
                // `mergeObjectSync`: set when the value is not undefined or
                // the key is in the input.
                Ok(Some(v)) => {
                    if !matches!(v, Value::Undefined(_)) || present {
                        out.insert(key.clone(), v);
                    }
                }
                Ok(None) => failed = true,
                Err(e) => unknown = unknown.or(Some(e)),
            },
            Api::V4 => {
                // `handlePropertyResult`.
                let (i, o) = (optin(field), optout(field));
                if !present && o && i == OptIn::Optional {
                    continue;
                }
                match run(api, field, &value) {
                    Ok(None) => {
                        if !(i != OptIn::Required && o && !present) {
                            failed = true;
                        }
                    }
                    Ok(Some(_)) if !present && i == OptIn::Required => failed = true,
                    Ok(Some(Value::Undefined(_))) => {
                        if present || (i == OptIn::Defaulted && !o) {
                            out.insert(key.clone(), Value::undefined());
                        }
                    }
                    Ok(Some(v)) => {
                        out.insert(key.clone(), v);
                    }
                    Err(e) => unknown = unknown.or(Some(e)),
                }
            }
        }
    }
    let extra: Vec<&String> = data
        .keys()
        .filter(|k| !fields.iter().any(|(f, _)| f == *k))
        .collect();
    match unknown_keys {
        Unknown::Strip => {}
        Unknown::Strict => {
            if !extra.is_empty() {
                failed = true;
            }
        }
        Unknown::Passthrough => {
            for k in extra {
                // zod 3 pushes the unknown keys without `alwaysSet`, so
                // `mergeObjectSync` drops those whose value is undefined;
                // zod 4 (`handleCatchall`) keeps every present key.
                if api == Api::V3 && matches!(data[k], Value::Undefined(_)) {
                    continue;
                }
                out.insert(k.clone(), data[k].clone());
            }
        }
    }
    if failed {
        return Ok(None);
    }
    if let Some(e) = unknown {
        return Err(e);
    }
    Ok(Some(Value::Row(out)))
}

/// What a valid object input without `key` yields for a field: the field is
/// omitted from the output, set to a value, or the parse fails.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Absent {
    Fail,
    Omit,
    Set(Value),
}

/// The output of field `field` when its key is absent from the input (it
/// depends on the schema only).
pub fn absent(version: Version, field: &Schema) -> Result<Absent, String> {
    let probe = Schema::Object(vec![("k".to_owned(), field.clone())]);
    let out = parse(version, &probe, &Value::Row(Row::new()))?;
    Ok(match out {
        None => Absent::Fail,
        Some(Value::Row(r)) => match r.get("k") {
            None => Absent::Omit,
            Some(v) => Absent::Set(v.clone()),
        },
        Some(_) => Absent::Fail,
    })
}

// ---------------------------------------------------------------------------
// Valid inputs (synthesis)
// ---------------------------------------------------------------------------

/// A valid input of `schema` (for input synthesis), as close to `current`
/// as the subset allows: a valid `current` is kept; an object keeps its
/// valid fields, gets its missing required fields, and loses keys a strict
/// schema rejects. `name` seeds readable placeholder text. `None` when no
/// valid value is found (a `.regex()` the placeholders do not satisfy).
/// Every result is checked with [`parse`].
pub fn conform(
    version: Version,
    schema: &Schema,
    current: Option<&Value>,
    name: &str,
) -> Option<Value> {
    if let Some(c) = current
        && matches!(parse(version, schema, c), Ok(Some(_)))
    {
        return Some(c.clone());
    }
    let candidate = match schema {
        Schema::Optional(inner) | Schema::Nullable(inner) | Schema::Default(inner, _) => {
            return conform(version, inner, current, name).or_else(|| match schema {
                Schema::Optional(_) | Schema::Default(..) => Some(Value::undefined()),
                _ => Some(Value::Null),
            });
        }
        Schema::Str(checks) => string_candidates(checks, current, name)
            .into_iter()
            .find(|v| matches!(parse(version, schema, v), Ok(Some(_))))?,
        Schema::Num(checks) => {
            let mut lo = i64::MIN / 4;
            let mut hi = i64::MAX / 4;
            for c in checks {
                match c {
                    NumCheck::Ge(n) => lo = lo.max(*n),
                    NumCheck::Gt(n) => lo = lo.max(n.saturating_add(1)),
                    NumCheck::Le(n) => hi = hi.min(*n),
                    NumCheck::Lt(n) => hi = hi.min(n.saturating_sub(1)),
                    NumCheck::Int => {}
                }
            }
            let preferred = match current {
                Some(Value::Int(n)) => *n,
                _ => 3,
            };
            Value::Int(preferred.clamp(lo, hi.max(lo)))
        }
        Schema::Bool => Value::Bool(true),
        Schema::Literal(v) => v.clone(),
        Schema::Enum(items) => Value::str(items.first()?.as_str()),
        Schema::Array(item, checks) => {
            let min = checks
                .iter()
                .filter_map(|c| match c {
                    ArrCheck::Min(n) | ArrCheck::Length(n) => Some(*n),
                    ArrCheck::Max(_) => None,
                })
                .max()
                .unwrap_or(0)
                .clamp(0, 8) as usize;
            let mut items: Vec<Value> = match current {
                Some(Value::List(items)) => items
                    .iter()
                    .filter_map(|v| conform(version, item, Some(v), name))
                    .collect(),
                _ => Vec::new(),
            };
            while items.len() < min {
                items.push(conform(version, item, None, name)?);
            }
            Value::List(items)
        }
        Schema::Object(_) | Schema::StrictObject(_) | Schema::LooseObject(_) => {
            let (fields, unknown_keys) = schema.object()?;
            let mut out = Row::new();
            if let Some(Value::Row(r)) = current
                && unknown_keys != Unknown::Strict
            {
                out = r.clone();
            }
            for (key, field) in fields {
                let cur = match current {
                    Some(Value::Row(r)) => r.get(key),
                    _ => None,
                };
                let required = matches!(absent(version, field), Ok(Absent::Fail));
                match cur {
                    Some(v) => {
                        out.insert(key.clone(), conform(version, field, Some(v), key)?);
                    }
                    None if required => {
                        out.insert(key.clone(), conform(version, field, None, key)?);
                    }
                    None => {}
                }
            }
            Value::Row(out)
        }
    };
    matches!(parse(version, schema, &candidate), Ok(Some(_))).then_some(candidate)
}

fn string_candidates(checks: &[StrCheck], current: Option<&Value>, name: &str) -> Vec<Value> {
    let mut prefix = String::new();
    let mut suffix = String::new();
    let mut middle = String::new();
    let (mut min, mut max) = (0i64, i64::MAX);
    for c in checks {
        match c {
            StrCheck::StartsWith(t) => prefix = t.clone(),
            StrCheck::EndsWith(t) => suffix = t.clone(),
            StrCheck::Includes(t) => middle = t.clone(),
            StrCheck::Min(n) => min = min.max(*n),
            StrCheck::Max(n) => max = max.min(*n),
            StrCheck::Length(n) => {
                min = min.max(*n);
                max = max.min(*n);
            }
            _ => {}
        }
    }
    let base = match current {
        Some(Value::Str(s)) => s.clone(),
        _ => format!("{name}-1"),
    };
    let fit = |s: String| -> String {
        let mut s = s;
        while (utf16_len(&s) as i64) < min {
            s.push('x');
        }
        while (utf16_len(&s) as i64) > max && !s.is_empty() {
            s.pop();
        }
        s
    };
    let mut out = vec![
        Value::Str(fit(format!("{prefix}{base}{middle}{suffix}"))),
        Value::Str(fit(format!("{prefix}{middle}{suffix}"))),
        Value::Str(fit(format!("{prefix}x{middle}{suffix}"))),
    ];
    for c in checks {
        if let StrCheck::Regex { pattern, flags } = c
            && let Some(example) = Regex::new(pattern, flags).ok().and_then(|r| r.example())
        {
            out.insert(0, Value::Str(example));
        }
    }
    if checks.iter().any(|c| matches!(c, StrCheck::Uuid)) {
        // A version 4, variant 10 UUID (valid for both versions' patterns)
        // whose first group is a stable hash of the name, so that two
        // fields do not share a value by accident.
        let h = name.bytes().fold(0x811c_9dc5u32, |h, b| {
            (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
        });
        out.push(Value::Str(format!("{h:08x}-0000-4000-a000-000000000001")));
    }
    if checks.iter().any(|c| matches!(c, StrCheck::Email)) {
        let local: String = name
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        let local = if local.is_empty() {
            "alice".to_owned()
        } else {
            local
        };
        out.insert(0, Value::Str(format!("{local}@example.test")));
        out.push(Value::str("alice@example.test"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(entries: &[(&str, Value)]) -> Value {
        Value::Row(
            entries
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn descriptors_round_trip() {
        let s = Schema::StrictObject(vec![
            ("id".into(), Schema::Str(vec![StrCheck::Uuid])),
            (
                "tags".into(),
                Schema::Array(
                    Box::new(Schema::Enum(vec!["a".into()])),
                    vec![ArrCheck::Min(1)],
                ),
            ),
            (
                "n".into(),
                Schema::Default(Box::new(Schema::Num(vec![NumCheck::Int])), Value::Int(2)),
            ),
        ]);
        let v = to_value(Version::V4, &s);
        assert_eq!(from_value(&v), Ok((Version::V4, s)));
    }

    #[test]
    fn optional_of_default_differs_between_versions() {
        let s = Schema::Optional(Box::new(Schema::Default(
            Box::new(Schema::Str(Vec::new())),
            Value::str("d"),
        )));
        assert_eq!(absent(Version::V3, &s), Ok(Absent::Omit));
        assert_eq!(absent(Version::V4, &s), Ok(Absent::Set(Value::str("d"))));
        assert!(absent(Version::Either, &s).is_err());
    }

    #[test]
    fn strict_objects_reject_unknown_keys_even_when_undefined() {
        let s = Schema::StrictObject(vec![("a".into(), Schema::Str(Vec::new()))]);
        let input = obj(&[("a", Value::str("x")), ("b", Value::undefined())]);
        assert_eq!(parse(Version::Either, &s, &input), Ok(None));
    }

    #[test]
    fn uuid_patterns_differ_per_version() {
        let s = Schema::Str(vec![StrCheck::Uuid]);
        let v0 = Value::str("123e4567-e89b-02d3-a456-426614174000");
        assert!(matches!(parse(Version::V3, &s, &v0), Ok(Some(_))));
        assert_eq!(parse(Version::V4, &s, &v0), Ok(None));
        assert!(parse(Version::Either, &s, &v0).is_err());
    }

    #[test]
    fn conform_builds_valid_inputs() {
        let s = Schema::StrictObject(vec![
            ("id".into(), Schema::Str(vec![StrCheck::Uuid])),
            ("email".into(), Schema::Str(vec![StrCheck::Email])),
            (
                "role".into(),
                Schema::Enum(vec!["admin".into(), "member".into()]),
            ),
            (
                "note".into(),
                Schema::Optional(Box::new(Schema::Str(vec![StrCheck::Min(1)]))),
            ),
        ]);
        let current = obj(&[("id", Value::str("id-1")), ("extra", Value::Int(1))]);
        let v = conform(Version::V4, &s, Some(&current), "input").unwrap();
        assert!(matches!(parse(Version::V4, &s, &v), Ok(Some(_))));
        let Value::Row(r) = v else { panic!() };
        assert!(!r.contains_key("extra") && !r.contains_key("note"));
        assert_eq!(r["role"], Value::str("admin"));
    }
}

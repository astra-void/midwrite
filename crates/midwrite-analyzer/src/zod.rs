//! Phase 14: the zod schemas whose `parse`/`safeParse` the extractor
//! models, read statically from their builder chains (`z.object({ name:
//! z.string().trim().min(1) })`).
//!
//! Only what zod 3.x does is modeled (read from `zod/v3/types.js` of zod
//! 3.25.76: `ZodString._parse` applies its checks in declaration order,
//! `trim`/`toLowerCase`/`toUpperCase` rewrite the value for the checks that
//! follow and for the output, `min`/`max`/`length` compare `.length`;
//! `ZodObject._parse` requires a non-null, non-array object and outputs
//! only the shape's keys (unknown keys are stripped)). Every other schema
//! or check is refused with its name: a validation that is not modeled is
//! never treated as the identity.
//!
//! Phase 16: the zod major version is read statically per import
//! ([`resolve_version`]: the installed `node_modules/zod/package.json`, else
//! the nearest `package.json` that declares zod, catalogs included; nothing
//! is executed), and the subset grows to strict/loose objects, arrays,
//! `optional`/`nullable`/`nullish`/`default`, the `uuid`/`email` formats and
//! `.regex(/literal/)`, each with the semantics of the version it was built
//! with (`midwrite_model::zodfn`, `docs/phase16/ZOD-VALIDATION.md`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use oxc_ast::ast::{Argument, Expression, ObjectPropertyKind};

pub(crate) use midwrite_model::zodfn::{ArrCheck, NumCheck, Schema, StrCheck, Version};

/// Phase 16: a schema and the zod API it was built with (`Err`: the
/// version could not be determined, and why).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Validated {
    pub version: Result<Version, String>,
    pub schema: Schema,
}

/// How the interpreter resolves names it meets in a chain.
pub(crate) trait Names<'a> {
    /// Phase 16: the module specifier `name` is zod's namespace imported
    /// from (`import { z } from "zod"`, `import * as z from "zod/v4"`,
    /// `import z from "zod"`), or `None`.
    fn zod_import(&self, name: &str) -> Option<String>;
    /// Phase 16: the zod API a specifier gives in this module's package.
    fn version(&self, specifier: &str) -> Result<Version, String>;
    /// The initializer of a module-level `const` (another schema).
    fn constant(&self, name: &str) -> Option<&'a Expression<'a>>;
    /// Phase 16: the strings of a `const` array of string literals (`const
    /// KINDS = ["a", "b"] as const`, in this module or imported; spreads of
    /// such arrays included), for `z.enum(KINDS)`.
    fn string_array(&self, name: &str) -> Option<Vec<String>>;
}

/// Read the schema built by `e`.
pub(crate) fn interpret<'a>(
    e: &'a Expression<'a>,
    names: &dyn Names<'a>,
) -> Result<Validated, String> {
    let mut cx = Interp {
        names,
        specifier: None,
    };
    let schema = cx.schema(e, 0)?;
    let specifier = cx
        .specifier
        .ok_or_else(|| "a zod schema without a `z` root".to_owned())?;
    Ok(Validated {
        version: names.version(&specifier),
        schema,
    })
}

struct Interp<'n, 'a> {
    names: &'n dyn Names<'a>,
    specifier: Option<String>,
}

impl<'a> Interp<'_, 'a> {
    /// The version, for a construct whose semantics or existence depends on
    /// it.
    fn version(&self, what: &str) -> Result<Version, String> {
        let spec = self.specifier.as_deref().unwrap_or("zod");
        self.names.version(spec).map_err(|why| {
            format!("{what} depends on the zod major version, which is not known: {why}")
        })
    }

    /// A construct whose semantics differ between zod 3 and zod 4: modeled
    /// with the project's version, refused when it is not known.
    fn version_dependent(&self, what: &str, how: &str) -> Result<(), String> {
        match self.version(what)? {
            Version::V3 | Version::V4 => Ok(()),
            Version::Either => Err(format!(
                "{what} differs between zod 3 and zod 4 ({how}), and the project's zod major version is not known"
            )),
        }
    }

    /// A construct that exists only in zod 4.
    fn zod4_only(&self, what: &str) -> Result<(), String> {
        match self.version(what)? {
            Version::V4 => Ok(()),
            Version::V3 => Err(format!("{what} does not exist in zod 3")),
            Version::Either => Err(format!(
                "{what} exists only in zod 4, and the project's zod major version is not known"
            )),
        }
    }

    fn schema(&mut self, e: &'a Expression<'a>, depth: u32) -> Result<Schema, String> {
        if depth > 16 {
            return Err("a zod schema nested too deeply".to_owned());
        }
        match e.get_inner_expression() {
            Expression::Identifier(id) => match self.names.constant(id.name.as_str()) {
                Some(init) => self.schema(init, depth + 1),
                None => Err(format!("`{}` is not a module-level zod schema", id.name)),
            },
            Expression::CallExpression(call) => {
                let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression()
                else {
                    return Err("a zod schema built by a call that is not a method".to_owned());
                };
                let method = member.property.name.as_str();
                // The root: `z.<type>(...)`.
                if let Expression::Identifier(root) = member.object.get_inner_expression()
                    && let Some(spec) = self.names.zod_import(root.name.as_str())
                {
                    match &self.specifier {
                        Some(s) if *s != spec => {
                            return Err(format!(
                                "a zod schema that mixes `{s}` and `{spec}` imports"
                            ));
                        }
                        _ => self.specifier = Some(spec),
                    }
                    return self.root_schema(method, &call.arguments, depth);
                }
                let base = self.schema(&member.object, depth + 1)?;
                self.refine(base, method, &call.arguments, depth)
            }
            _ => {
                Err("a zod schema written as an expression that is not a builder chain".to_owned())
            }
        }
    }

    fn shape(
        &mut self,
        what: &str,
        args: &'a [Argument<'a>],
        depth: u32,
    ) -> Result<Vec<(String, Schema)>, String> {
        let Some(Expression::ObjectExpression(shape)) = args
            .first()
            .and_then(Argument::as_expression)
            .map(|e| e.get_inner_expression())
        else {
            return Err(format!(
                "`z.{what}` of something other than an object literal"
            ));
        };
        if args.len() != 1 {
            return Err(format!("`z.{what}` with options"));
        }
        let mut fields = Vec::new();
        for property in &shape.properties {
            let ObjectPropertyKind::ObjectProperty(p) = property else {
                return Err(format!("a spread in a `z.{what}` shape"));
            };
            let key = match &p.key {
                oxc_ast::ast::PropertyKey::StaticIdentifier(id) => id.name.to_string(),
                oxc_ast::ast::PropertyKey::StringLiteral(s) => match s.value.as_str() {
                    Some(k) => k.to_owned(),
                    None => return Err(format!("a `z.{what}` key with a lone surrogate")),
                },
                _ => return Err(format!("a computed key in a `z.{what}` shape")),
            };
            fields.push((key, self.schema(&p.value, depth + 1)?));
        }
        Ok(fields)
    }

    fn root_schema(
        &mut self,
        method: &str,
        args: &'a [Argument<'a>],
        depth: u32,
    ) -> Result<Schema, String> {
        match method {
            // Phase 16: an error-message parameter changes no outcome.
            "string" | "number" | "boolean" => {
                message_only(&format!("z.{method}"), args, 0)?;
                Ok(match method {
                    "string" => Schema::Str(Vec::new()),
                    "number" => Schema::Num(Vec::new()),
                    _ => Schema::Bool,
                })
            }
            "literal" => match args.first().and_then(Argument::as_expression).map(literal) {
                Some(Some(v)) if args.len() == 1 => Ok(Schema::Literal(v)),
                _ => Err("`z.literal` of a value that is not a literal".to_owned()),
            },
            "enum" => {
                // Phase 16: a message (second argument) does not change what
                // is accepted; the members may be a `const` array.
                message_only("z.enum", args, 1)?;
                let items = match args
                    .first()
                    .and_then(Argument::as_expression)
                    .map(|e| e.get_inner_expression())
                {
                    Some(Expression::ArrayExpression(items)) => items,
                    Some(Expression::Identifier(id)) => {
                        return match self.names.string_array(id.name.as_str()) {
                            Some(members) if !members.is_empty() => Ok(Schema::Enum(members)),
                            _ => Err(format!(
                                "`z.enum({})` of something other than a `const` array of string literals",
                                id.name
                            )),
                        };
                    }
                    _ => {
                        return Err("`z.enum` of something other than an array literal".to_owned());
                    }
                };
                let mut out = Vec::new();
                for item in &items.elements {
                    match item {
                        oxc_ast::ast::ArrayExpressionElement::SpreadElement(spread) => {
                            match spread.argument.get_inner_expression() {
                                Expression::Identifier(id) => {
                                    match self.names.string_array(id.name.as_str()) {
                                        Some(members) => out.extend(members),
                                        None => {
                                            return Err(format!(
                                                "`z.enum` with a spread of `{}`, which is not a `const` array of string literals",
                                                id.name
                                            ));
                                        }
                                    }
                                }
                                _ => return Err("`z.enum` with a computed spread".to_owned()),
                            }
                        }
                        _ => match item.as_expression().map(|e| e.get_inner_expression()) {
                            Some(Expression::StringLiteral(s)) if s.value.as_str().is_some() => {
                                out.push(s.value.as_str().unwrap_or_default().to_owned());
                            }
                            _ => {
                                return Err("`z.enum` with a member that is not a string literal"
                                    .to_owned());
                            }
                        },
                    }
                }
                Ok(Schema::Enum(out))
            }
            "object" => Ok(Schema::Object(self.shape("object", args, depth)?)),
            // Phase 16.
            "strictObject" => Ok(Schema::StrictObject(self.shape(
                "strictObject",
                args,
                depth,
            )?)),
            "looseObject" => {
                self.zod4_only("`z.looseObject`")?;
                Ok(Schema::LooseObject(self.shape(
                    "looseObject",
                    args,
                    depth,
                )?))
            }
            "array" => {
                let item = match args.first().and_then(Argument::as_expression) {
                    Some(e) => self.schema(e, depth + 1)?,
                    None => return Err("`z.array` without an element schema".to_owned()),
                };
                message_only("z.array", args, 1)?;
                array_of(item)
            }
            "optional" | "nullable" if args.len() == 1 => {
                let inner = match args[0].as_expression() {
                    Some(e) => self.schema(e, depth + 1)?,
                    None => return Err(format!("`z.{method}` of a spread")),
                };
                Ok(if method == "optional" {
                    Schema::Optional(Box::new(inner))
                } else {
                    Schema::Nullable(Box::new(inner))
                })
            }
            "uuid" | "email" => {
                self.zod4_only(&format!("`z.{method}()`"))?;
                message_only(&format!("z.{method}"), args, 0)?;
                Ok(Schema::Str(vec![if method == "uuid" {
                    StrCheck::Uuid
                } else {
                    StrCheck::Email
                }]))
            }
            "int" => {
                self.zod4_only("`z.int()`")?;
                message_only("z.int", args, 0)?;
                Ok(Schema::Num(vec![NumCheck::Int]))
            }
            other => Err(format!("zod `z.{other}(...)` is not modeled")),
        }
    }

    fn refine(
        &mut self,
        base: Schema,
        method: &str,
        args: &'a [Argument<'a>],
        depth: u32,
    ) -> Result<Schema, String> {
        let _ = depth;
        let names = self.names;
        let int = |i: usize| -> Result<i64, String> {
            let arg = args.get(i).and_then(Argument::as_expression);
            // Phase 16: a module-level `const` initialized with an integer
            // literal stands for it.
            let value = match arg.map(|e| e.get_inner_expression()) {
                Some(Expression::Identifier(id)) => {
                    names.constant(id.name.as_str()).and_then(literal)
                }
                _ => arg.and_then(literal),
            };
            match value {
                Some(midwrite_model::value::Value::Int(n)) => Ok(n),
                _ => Err(format!(
                    "zod `.{method}` with an argument that is not an integer literal or constant"
                )),
            }
        };
        let text = |i: usize| -> Result<String, String> {
            match args.get(i).and_then(Argument::as_expression).map(literal) {
                Some(Some(midwrite_model::value::Value::Str(s))) => Ok(s),
                _ => Err(format!(
                    "zod `.{method}` with an argument that is not a string literal"
                )),
            }
        };
        let message_only = |n: usize| message_only(&format!(".{method}"), args, n);
        // Phase 16: wrappers apply to every schema.
        match method {
            "optional" if args.is_empty() => return Ok(Schema::Optional(Box::new(base))),
            "nullable" if args.is_empty() => return Ok(Schema::Nullable(Box::new(base))),
            "nullish" if args.is_empty() => {
                return Ok(Schema::Optional(Box::new(Schema::Nullable(Box::new(base)))));
            }
            "default" => {
                let value = match args.first().and_then(Argument::as_expression) {
                    Some(e) if args.len() == 1 => match e.get_inner_expression() {
                        Expression::NullLiteral(_) => Some(midwrite_model::value::Value::Null),
                        other => literal(other),
                    },
                    _ => None,
                };
                let Some(value) = value else {
                    return Err(
                        "zod `.default()` of a value that is not a string, number, boolean or null literal"
                            .to_owned(),
                    );
                };
                return Ok(Schema::Default(Box::new(base), value));
            }
            "array" if args.is_empty() => return array_of(base),
            _ => {}
        }
        match base {
            Schema::Str(mut checks) => {
                let check = match method {
                    "nonempty" => {
                        message_only(0)?;
                        StrCheck::Min(1)
                    }
                    "min" => {
                        message_only(1)?;
                        StrCheck::Min(int(0)?)
                    }
                    "max" => {
                        message_only(1)?;
                        StrCheck::Max(int(0)?)
                    }
                    "length" => {
                        message_only(1)?;
                        StrCheck::Length(int(0)?)
                    }
                    "trim" if args.is_empty() => StrCheck::Trim,
                    "toLowerCase" if args.is_empty() => StrCheck::Lower,
                    "toUpperCase" if args.is_empty() => StrCheck::Upper,
                    "startsWith" => {
                        message_only(1)?;
                        StrCheck::StartsWith(text(0)?)
                    }
                    "endsWith" => {
                        message_only(1)?;
                        StrCheck::EndsWith(text(0)?)
                    }
                    "includes" => {
                        message_only(1)?;
                        StrCheck::Includes(text(0)?)
                    }
                    // Phase 16: formats (each version's pattern) and
                    // regular-expression literals.
                    "uuid" => {
                        message_only(0)?;
                        self.version_dependent(
                            "zod `.uuid()`",
                            "zod 4 accepts RFC 9562 versions and variants only",
                        )?;
                        StrCheck::Uuid
                    }
                    "email" => {
                        message_only(0)?;
                        self.version_dependent("zod `.email()`", "the two use different patterns")?;
                        StrCheck::Email
                    }
                    "regex" => {
                        message_only(1)?;
                        let Some(Expression::RegExpLiteral(r)) = args
                            .first()
                            .and_then(Argument::as_expression)
                            .map(|e| e.get_inner_expression())
                        else {
                            return Err(
                                "zod `.regex()` of something other than a regular-expression literal"
                                    .to_owned(),
                            );
                        };
                        let pattern = r.regex.pattern.text.as_str().to_owned();
                        let flags = r.regex.flags.to_string();
                        midwrite_model::jsregex::Regex::new(&pattern, &flags).map_err(|why| {
                            format!("zod `.regex(/{pattern}/{flags})`: the pattern uses {why}, which is not modeled")
                        })?;
                        StrCheck::Regex { pattern, flags }
                    }
                    other => return Err(format!("zod string check `.{other}()` is not modeled")),
                };
                checks.push(check);
                Ok(Schema::Str(checks))
            }
            Schema::Num(mut checks) => {
                let check = match method {
                    // Model numbers are integers: `finite` holds for every
                    // model value; `int` too in zod 3 (Phase 14). Phase 16:
                    // zod 4's `int` is a safe-integer check, kept in the
                    // schema (the model refuses numbers outside the safe
                    // range, where the versions differ).
                    "int" => {
                        message_only(0)?;
                        if self
                            .names
                            .version(self.specifier.as_deref().unwrap_or("zod"))
                            == Ok(Version::V3)
                        {
                            return Ok(Schema::Num(checks));
                        }
                        NumCheck::Int
                    }
                    "finite" => {
                        message_only(0)?;
                        return Ok(Schema::Num(checks));
                    }
                    "min" | "gte" => {
                        message_only(1)?;
                        NumCheck::Ge(int(0)?)
                    }
                    "gt" => {
                        message_only(1)?;
                        NumCheck::Gt(int(0)?)
                    }
                    "max" | "lte" => {
                        message_only(1)?;
                        NumCheck::Le(int(0)?)
                    }
                    "lt" => {
                        message_only(1)?;
                        NumCheck::Lt(int(0)?)
                    }
                    "positive" => {
                        message_only(0)?;
                        NumCheck::Gt(0)
                    }
                    "nonnegative" => {
                        message_only(0)?;
                        NumCheck::Ge(0)
                    }
                    "negative" => {
                        message_only(0)?;
                        NumCheck::Lt(0)
                    }
                    "nonpositive" => {
                        message_only(0)?;
                        NumCheck::Le(0)
                    }
                    other => return Err(format!("zod number check `.{other}()` is not modeled")),
                };
                checks.push(check);
                Ok(Schema::Num(checks))
            }
            Schema::Object(fields) | Schema::StrictObject(fields) | Schema::LooseObject(fields)
                if args.is_empty() || method == "strict" =>
            {
                match method {
                    // `strip` is the default; Phase 16: `strict` rejects and
                    // `passthrough` keeps unknown keys (the last call wins,
                    // verified for both versions).
                    "strip" => Ok(Schema::Object(fields)),
                    "strict" => {
                        message_only(0)?;
                        Ok(Schema::StrictObject(fields))
                    }
                    "passthrough" => {
                        self.version_dependent(
                            "zod `.passthrough()`",
                            "zod 3 drops unknown keys whose value is undefined",
                        )?;
                        Ok(Schema::LooseObject(fields))
                    }
                    other => Err(format!("zod object method `.{other}()` is not modeled")),
                }
            }
            Schema::Array(item, mut checks) => {
                let check = match method {
                    "nonempty" => {
                        message_only(0)?;
                        ArrCheck::Min(1)
                    }
                    "min" => {
                        message_only(1)?;
                        ArrCheck::Min(int(0)?)
                    }
                    "max" => {
                        message_only(1)?;
                        ArrCheck::Max(int(0)?)
                    }
                    "length" => {
                        message_only(1)?;
                        ArrCheck::Length(int(0)?)
                    }
                    other => return Err(format!("zod array method `.{other}()` is not modeled")),
                };
                if checks
                    .iter()
                    .any(|c| std::mem::discriminant(c) == std::mem::discriminant(&check))
                {
                    self.version_dependent(
                        &format!("a repeated zod array `.{method}()`"),
                        "zod 3 keeps the last, zod 4 every one",
                    )?;
                }
                checks.push(check);
                Ok(Schema::Array(item, checks))
            }
            Schema::Optional(_) | Schema::Nullable(_) | Schema::Default(..) => Err(format!(
                "zod `.{method}()` on an optional, nullable or defaulted schema is not modeled"
            )),
            _ => Err(format!("zod `.{method}()` on this schema is not modeled")),
        }
    }
}

/// Phase 16: `z.array(item)`. An array whose elements are their own output
/// is its own output; otherwise (objects, transforms, defaults) its parsed
/// value is computed whole (`ZodData`).
fn array_of(item: Schema) -> Result<Schema, String> {
    Ok(Schema::Array(Box::new(item), Vec::new()))
}

/// A custom message (argument `n`) does not change what is accepted; other
/// options (`{ position }`, `{ version }`, ...) might.
fn message_only(what: &str, args: &[Argument<'_>], n: usize) -> Result<(), String> {
    if args.len() > n + 1 {
        return Err(format!("zod `{what}` with more arguments than modeled"));
    }
    match args.get(n).and_then(Argument::as_expression).map(|e| e.get_inner_expression()) {
        None | Some(Expression::StringLiteral(_)) => Ok(()),
        // Phase 16: a template-literal message whose interpolations are
        // names or literals (evaluated once, when the schema is built).
        Some(Expression::TemplateLiteral(t))
            if t.expressions.iter().all(|e| {
                matches!(
                    e.get_inner_expression(),
                    Expression::Identifier(_)
                        | Expression::StringLiteral(_)
                        | Expression::NumericLiteral(_)
                )
            }) =>
        {
            Ok(())
        }
        Some(Expression::ObjectExpression(o))
            if o.properties.iter().all(|p| {
                matches!(p, ObjectPropertyKind::ObjectProperty(p)
                    if matches!(&p.key, oxc_ast::ast::PropertyKey::StaticIdentifier(id)
                        if matches!(id.name.as_str(),
                            "message" | "error" | "required_error" | "invalid_type_error" | "description")))
            }) =>
        {
            Ok(())
        }
        _ => Err(format!("zod `{what}` with options other than a message")),
    }
}

fn literal(e: &Expression<'_>) -> Option<midwrite_model::value::Value> {
    use midwrite_model::value::Value;
    match e.get_inner_expression() {
        Expression::StringLiteral(s) => s.value.as_str().map(|t| Value::Str(t.to_owned())),
        Expression::BooleanLiteral(b) => Some(Value::Bool(b.value)),
        Expression::NumericLiteral(n)
            if n.value.fract() == 0.0 && n.value.abs() < 9.007_199_254_740_992e15 =>
        {
            Some(Value::Int(n.value as i64))
        }
        Expression::UnaryExpression(u)
            if u.operator == oxc_syntax::operator::UnaryOperator::UnaryNegation =>
        {
            match literal(&u.argument)? {
                Value::Int(n) => Some(Value::Int(-n)),
                _ => None,
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Phase 16: the zod version of a module
// ---------------------------------------------------------------------------

/// The zod API that `specifier` (`zod`, `zod/v3`, `zod/v4`) gives code in
/// `file` of the project at `root`: `zod/v3` is zod 3 in every package that
/// ships it; `zod` and `zod/v4` depend on the package's major version, read
/// from the installed `node_modules/zod/package.json` (Node.js resolution
/// from the file's directory up to the root) or else the nearest
/// `package.json` that declares a `zod` dependency (a `catalog:` reference
/// is followed to the workspace catalog). `zod/v4` of a zod 3.x package is
/// the early zod 4 implementation shipped with 3.25, which differs from zod
/// 4.x (its UUID pattern rejects the max UUID): not modeled.
pub(crate) fn resolve_version(
    file: &Path,
    root: &Path,
    specifier: &str,
) -> Result<Version, String> {
    if specifier == "zod/v3" {
        return Ok(Version::V3);
    }
    let major = package_major(file, root);
    match (specifier, major) {
        ("zod", Ok(3)) => Ok(Version::V3),
        ("zod", Ok(4)) | ("zod/v4", Ok(4)) => Ok(Version::V4),
        ("zod", Err(_)) => Ok(Version::Either),
        ("zod/v4", Ok(3)) => Err(
            "`zod/v4` of a zod 3.x package (the early zod 4 implementation of zod 3.25, whose semantics differ from zod 4.x)"
                .to_owned(),
        ),
        ("zod/v4", Err(why)) => Err(format!("`zod/v4` of an unknown zod package: {why}")),
        (_, Ok(m)) => Err(format!("zod major version {m}")),
        (s, Err(why)) => Err(format!("`{s}`: {why}")),
    }
}

/// The major versions found, by (directory of the importing file, root).
type MajorCache = Mutex<HashMap<(PathBuf, PathBuf), Result<u64, String>>>;

fn cache() -> &'static MajorCache {
    static CACHE: OnceLock<MajorCache> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

/// The directories from `file`'s up to `root` (inclusive).
fn dirs_up(file: &Path, root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut d = file.parent();
    while let Some(dir) = d {
        out.push(dir.to_path_buf());
        if dir == root || !dir.starts_with(root) {
            break;
        }
        d = dir.parent();
    }
    out
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The major version of the zod package code in `file` uses.
pub(crate) fn package_major(file: &Path, root: &Path) -> Result<u64, String> {
    let key = (
        file.parent().map(Path::to_path_buf).unwrap_or_default(),
        root.to_path_buf(),
    );
    if let Some(hit) = cache().lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit;
    }
    let found = find_major(file, root);
    if let Ok(mut c) = cache().lock() {
        c.insert(key, found.clone());
    }
    found
}

fn find_major(file: &Path, root: &Path) -> Result<u64, String> {
    let dirs = dirs_up(file, root);
    for d in &dirs {
        let installed = d.join("node_modules").join("zod").join("package.json");
        if let Some(json) = read_json(&installed) {
            return match json.get("version").and_then(|v| v.as_str()) {
                Some(v) => major_of(v)
                    .ok_or_else(|| format!("the installed zod has the unreadable version `{v}`")),
                None => Err("the installed zod's package.json has no version".to_owned()),
            };
        }
    }
    for (i, d) in dirs.iter().enumerate() {
        let Some(json) = read_json(&d.join("package.json")) else {
            continue;
        };
        let spec = [
            "dependencies",
            "devDependencies",
            "peerDependencies",
            "optionalDependencies",
        ]
        .iter()
        .find_map(|k| json.get(*k)?.get("zod")?.as_str().map(str::to_owned));
        let Some(spec) = spec else { continue };
        let spec = match spec.strip_prefix("catalog:") {
            Some(name) => catalog_spec(&dirs[i..], name.trim()).ok_or_else(|| {
                format!("the zod dependency `{spec}` names a catalog entry that was not found")
            })?,
            None => spec,
        };
        return major_of(&spec).ok_or_else(|| {
            format!("the zod dependency range `{spec}` does not fix a major version")
        });
    }
    Err("no installed zod package and no package.json declaring zod".to_owned())
}

/// The `zod` entry of a workspace catalog (`catalog`/`catalogs` of a
/// `package.json`, also under `workspaces`; `catalog:` of
/// `pnpm-workspace.yaml`).
fn catalog_spec(dirs: &[PathBuf], name: &str) -> Option<String> {
    let named = !name.is_empty() && name != "default";
    for d in dirs {
        if let Some(json) = read_json(&d.join("package.json")) {
            for base in [Some(&json), json.get("workspaces")].into_iter().flatten() {
                let entry = if named {
                    base.get("catalogs")
                        .and_then(|c| c.get(name))
                        .and_then(|c| c.get("zod"))
                } else {
                    base.get("catalog").and_then(|c| c.get("zod"))
                };
                if let Some(s) = entry.and_then(|v| v.as_str()) {
                    return Some(s.to_owned());
                }
            }
        }
        if !named && let Ok(text) = std::fs::read_to_string(d.join("pnpm-workspace.yaml")) {
            let mut in_catalog = false;
            for line in text.lines() {
                if !line.starts_with(' ') && !line.starts_with('\t') {
                    in_catalog = line.trim_end() == "catalog:";
                    continue;
                }
                if in_catalog {
                    let t = line.trim();
                    let (k, v) = t.split_once(':')?;
                    if k.trim().trim_matches(|c| c == '\'' || c == '"') == "zod" {
                        return Some(v.trim().trim_matches(|c| c == '\'' || c == '"').to_owned());
                    }
                }
            }
        }
    }
    None
}

/// The major version an exact version or a range within one major fixes
/// (`4.6.5`, `^4.3.6`, `~3.22.0`, `3.x`, `npm:zod@^4`); `None` otherwise.
fn major_of(spec: &str) -> Option<u64> {
    let s = spec.trim();
    let s = s.strip_prefix("npm:zod@").unwrap_or(s);
    let s = s
        .strip_prefix('^')
        .or_else(|| s.strip_prefix('~'))
        .or_else(|| s.strip_prefix('='))
        .unwrap_or(s);
    let s = s.strip_prefix('v').unwrap_or(s);
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    let rest = &s[digits.len()..];
    let tail_ok = rest.is_empty()
        || (rest.starts_with(['.', '-', '+'])
            && rest[1..]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '*')));
    if digits.is_empty() || !tail_ok {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxc_allocator::Allocator;
    use oxc_ast::ast::Statement;
    use oxc_parser::Parser;
    use oxc_span::SourceType;

    struct Z(Result<Version, String>);
    impl<'a> Names<'a> for Z {
        fn zod_import(&self, name: &str) -> Option<String> {
            (name == "z").then(|| "zod".to_owned())
        }
        fn version(&self, _: &str) -> Result<Version, String> {
            self.0.clone()
        }
        fn constant(&self, _: &str) -> Option<&'a Expression<'a>> {
            None
        }
        fn string_array(&self, name: &str) -> Option<Vec<String>> {
            (name == "KINDS").then(|| vec!["a".to_owned(), "b".to_owned()])
        }
    }

    fn schema_in(src: &str, version: Result<Version, String>) -> Result<Schema, String> {
        let allocator = Allocator::default();
        let text = format!("const s = {src};");
        let parsed = Parser::new(&allocator, &text, SourceType::ts()).parse();
        let Statement::VariableDeclaration(v) = &parsed.program.body[0] else {
            panic!()
        };
        let init = v.declarations[0].init.as_ref().unwrap();
        interpret(init, &Z(version)).map(|v| v.schema)
    }

    fn schema(src: &str) -> Result<Schema, String> {
        schema_in(src, Ok(Version::V3))
    }

    #[test]
    fn checks_keep_their_order() {
        assert_eq!(
            schema("z.object({ name: z.string().min(1).trim().max(80, 'too long') })"),
            Ok(Schema::Object(vec![(
                "name".into(),
                Schema::Str(vec![StrCheck::Min(1), StrCheck::Trim, StrCheck::Max(80)])
            )]))
        );
    }

    #[test]
    fn unmodeled_checks_are_refused_by_name() {
        assert_eq!(
            schema("z.string().url()"),
            Err("zod string check `.url()` is not modeled".into())
        );
        assert!(schema("z.string().optional().min(1)").is_err());
        assert!(schema("z.object({ a: z.string() }).extend({})").is_err());
        assert!(schema("z.string().transform((s) => s.length)").is_err());
        assert!(schema("z.union([z.string(), z.number()])").is_err());
        assert!(schema("z.string().min(n)").is_err());
        assert!(schema("z.string().refine((s) => s.length > 1)").is_err());
        assert!(schema("z.coerce.number()").is_err());
        assert!(schema("z.discriminatedUnion('k', [])").is_err());
        assert!(schema("z.preprocess((x) => x, z.string())").is_err());
        assert!(schema("z.string().regex(/(a)\\1/)").is_err());
    }

    #[test]
    fn phase16_constructs_are_read_with_their_version() {
        assert_eq!(
            schema_in(
                "z.strictObject({ id: z.uuid(), role: z.enum(['a', 'b']).default('a'), note: z.string().nullish() })",
                Ok(Version::V4)
            ),
            Ok(Schema::StrictObject(vec![
                ("id".into(), Schema::Str(vec![StrCheck::Uuid])),
                (
                    "role".into(),
                    Schema::Default(
                        Box::new(Schema::Enum(vec!["a".into(), "b".into()])),
                        midwrite_model::value::Value::str("a")
                    )
                ),
                (
                    "note".into(),
                    Schema::Optional(Box::new(Schema::Nullable(Box::new(
                        Schema::Str(Vec::new())
                    ))))
                ),
            ]))
        );
        // zod 4 only.
        assert!(schema("z.uuid()").is_err());
        assert!(schema_in("z.looseObject({})", Ok(Version::Either)).is_err());
        assert!(schema_in("z.looseObject({})", Ok(Version::V4)).is_ok());
        // `.int()`: dropped for zod 3 (Phase 14), kept otherwise.
        assert_eq!(schema("z.number().int()"), Ok(Schema::Num(Vec::new())));
        assert_eq!(
            schema_in("z.number().int()", Ok(Version::V4)),
            Ok(Schema::Num(vec![NumCheck::Int]))
        );
        // `const` arrays as enum members, a message second argument.
        assert_eq!(
            schema("z.enum(KINDS, 'pick one')"),
            Ok(Schema::Enum(vec!["a".into(), "b".into()]))
        );
        assert_eq!(
            schema("z.enum(['all', ...KINDS])"),
            Ok(Schema::Enum(vec!["all".into(), "a".into(), "b".into()]))
        );
        assert!(schema("z.enum(OTHER)").is_err());
        // Arrays, of rebuilt elements too.
        assert!(schema("z.array(z.string().trim())").is_ok());
        assert!(schema("z.array(z.strictObject({ id: z.uuid() })).min(1)").is_err());
        assert!(
            schema_in(
                "z.array(z.strictObject({ id: z.uuid() })).min(1)",
                Ok(Version::V4)
            )
            .is_ok()
        );
    }

    #[test]
    fn version_ranges_fix_a_major_only_when_they_say_so() {
        assert_eq!(major_of("4.6.5"), Some(4));
        assert_eq!(major_of("^4.3.6"), Some(4));
        assert_eq!(major_of("~3.22.0"), Some(3));
        assert_eq!(major_of("3.x"), Some(3));
        assert_eq!(major_of("npm:zod@^4"), Some(4));
        assert_eq!(major_of(">=3.0.0"), None);
        assert_eq!(major_of("latest"), None);
        assert_eq!(major_of("*"), None);
        assert_eq!(major_of("^3 || ^4"), None);
        assert_eq!(major_of("workspace:*"), None);
    }
}

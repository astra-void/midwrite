//! The zod schemas whose `parse`/`safeParse` the extractor
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
//! The zod major version is read statically per import
//! ([`resolve_version`]: the installed `node_modules/zod/package.json`, else
//! the nearest `package.json` that declares zod, catalogs included; nothing
//! is executed), and the subset grows to strict/loose objects, arrays,
//! `optional`/`nullable`/`nullish`/`default`, the `uuid`/`email` formats and
//! `.regex(/literal/)`, each with the semantics of the version it was built
//! with (`midwrite_model::zodfn`, `docs/reference/zod.md`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use oxc_ast::ast::{Argument, Expression, ObjectPropertyKind};

pub(crate) use midwrite_model::zodfn::{ArrCheck, NumCheck, Schema, StrCheck, Version};

/// A schema and the zod API it was built with (`Err`: the
/// version could not be determined, and why).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Validated {
    pub version: Result<Version, String>,
    pub schema: Schema,
}

/// How the interpreter resolves names it meets in a chain.
pub(crate) trait Names<'a> {
    /// The module specifier `name` is zod's namespace imported
    /// from (`import { z } from "zod"`, `import * as z from "zod/v4"`,
    /// `import z from "zod"`), or `None`.
    fn zod_import(&self, name: &str) -> Option<String>;
    /// The zod API a specifier gives in this module's package.
    fn version(&self, specifier: &str) -> Result<Version, String>;
    /// The initializer of a module-level `const` (another schema).
    fn constant(&self, name: &str) -> Option<&'a Expression<'a>>;
    /// The strings of a `const` array of string literals (`const
    /// KINDS = ["a", "b"] as const`, in this module or imported; spreads of
    /// such arrays included), for `z.enum(KINDS)`.
    fn string_array(&self, name: &str) -> Option<Vec<String>>;
    /// The source text of `span` in this module, for explanations.
    fn source(&self, span: oxc_span::Span) -> Option<String> {
        let _ = span;
        None
    }
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
            // An error-message parameter changes no outcome.
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
                // A message (second argument) does not change what
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
            // A module-level `const` initialized with an integer
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
        // Wrappers apply to every schema.
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
            // Metadata for documentation and JSON Schema generation: zod 3
            // and 4 validate exactly as without it.
            "describe" if args.len() == 1 => return Ok(base),
            // `@hono/zod-openapi` metadata (an example, a description, a
            // component name): registered, not validated.
            "openapi" if self.specifier.as_deref() == Some(HONO_ZOD_OPENAPI) => {
                return Ok(base);
            }
            "refine" => return self.refinement(base, args, false),
            "superRefine" => return self.refinement(base, args, true),
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
                    // Formats (each version's pattern) and
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
                    // model value; `int` too in zod 3. Phase 16:
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

/// `z.array(item)`. An array whose elements are their own output
/// is its own output; otherwise (objects, transforms, defaults) its parsed
/// value is computed whole (`ZodData`).
fn array_of(item: Schema) -> Result<Schema, String> {
    Ok(Schema::Array(Box::new(item), Vec::new()))
}

// ---------------------------------------------------------------------------
// Refinements
// ---------------------------------------------------------------------------

impl<'a> Interp<'_, 'a> {
    /// `.refine(callback, message?)` (`sup`: `.superRefine(callback)`): the
    /// callback lowered to a pure predicate of the value
    /// ([`midwrite_model::zodfn::Refinement`]), or the reason it is not.
    fn refinement(
        &self,
        base: Schema,
        args: &'a [Argument<'a>],
        sup: bool,
    ) -> Result<Schema, String> {
        use oxc_ast::ast::{ArrowFunctionBody, BindingPattern, Statement};
        let what = if sup { ".superRefine()" } else { ".refine()" };
        let refused = |why: &str| Err(format!("zod `{what}` with {why}"));
        let Some(callback) = args.first().and_then(Argument::as_expression) else {
            return refused("no callback");
        };
        if sup {
            if args.len() > 1 {
                return refused("more arguments than the callback");
            }
        } else {
            refine_options(args.get(1)).map_err(|why| format!("zod `{what}` with {why}"))?;
            if args.len() > 2 {
                return refused("more arguments than modeled");
            }
        }
        let callback = callback.get_inner_expression();
        let (params, body, is_async) = match callback {
            Expression::ArrowFunctionExpression(f) => (
                &f.params,
                match &f.body {
                    ArrowFunctionBody::FunctionBody(b) => Body::Block(&b.statements),
                    expr => match expr.as_expression() {
                        Some(e) => Body::Expr(e),
                        None => return refused("an unexpected callback body"),
                    },
                },
                f.r#async,
            ),
            Expression::FunctionExpression(f) if !f.generator => match &f.body {
                Some(b) => (&f.params, Body::Block(&b.statements), f.r#async),
                None => return refused("a callback without a body"),
            },
            _ => return refused("a callback that is not an inline function"),
        };
        if is_async {
            // zod throws on an async refinement in a synchronous parse.
            return refused("an async callback");
        }
        let names: Vec<&str> = params
            .items
            .iter()
            .map(|p| match &p.pattern {
                BindingPattern::BindingIdentifier(id) if p.initializer.is_none() => {
                    Ok(id.name.as_str())
                }
                _ => Err(()),
            })
            .collect::<Result<_, ()>>()
            .map_err(|()| {
                format!("zod `{what}` with a callback parameter that is not a plain name")
            })?;
        if params.rest.is_some() || names.is_empty() || names.len() > if sup { 2 } else { 1 } {
            return refused("a callback with other than its documented parameters");
        }
        let lower = Lower {
            interp: self,
            param: names[0],
        };
        let ok = if sup {
            let Body::Block(statements) = body else {
                return refused("a callback whose body is not a block of `if (…) ctx.addIssue(…)`");
            };
            let ctx = names.get(1).copied();
            let mut issues = Vec::new();
            for statement in statements.iter() {
                let Statement::IfStatement(test) = statement else {
                    return refused("a statement other than `if (…) ctx.addIssue(…)`");
                };
                if test.alternate.is_some() || !adds_issue(&test.consequent, ctx) {
                    return refused("a statement other than `if (…) ctx.addIssue(…)`");
                }
                // `!!cond`: every condition is evaluated, in order (a later
                // one may throw after an issue was added).
                issues.push(midwrite_model::value::Expr::not(
                    midwrite_model::value::Expr::not(lower.expr(&test.test, true)?),
                ));
            }
            midwrite_model::value::Expr::not(midwrite_model::value::Expr::call(
                midwrite_model::value::Builtin::Includes,
                vec![
                    midwrite_model::value::Expr::Array(issues),
                    midwrite_model::value::Expr::lit(true),
                ],
            ))
        } else {
            let e = match body {
                Body::Expr(e) => e,
                Body::Block(statements) => match statements.as_slice() {
                    [Statement::ReturnStatement(r)] => match &r.argument {
                        Some(e) => e,
                        None => return refused("a callback returning nothing"),
                    },
                    _ => return refused("a callback whose body is not one expression"),
                },
            };
            lower.expr(e, true)?
        };
        let source = self
            .names
            .source(oxc_span::GetSpan::span(callback))
            .unwrap_or_else(|| format!("({}) => …", names.join(", ")));
        Ok(Schema::Refine(
            Box::new(base),
            midwrite_model::zodfn::Refinement {
                param: names[0].to_owned(),
                ok,
                source,
            },
        ))
    }
}

enum Body<'b, 'a> {
    Expr(&'b Expression<'a>),
    Block(&'b oxc_allocator::Vec<'a, oxc_ast::ast::Statement<'a>>),
}

/// The second argument of `.refine()`: a message, or `{ message, path }`.
/// Others (zod 4 `abort`, `when`, an `error` function, zod 3 a function of
/// the value) change when the callback or other code runs.
fn refine_options(arg: Option<&Argument<'_>>) -> Result<(), String> {
    let static_text = |e: &Expression<'_>| match e.get_inner_expression() {
        Expression::StringLiteral(_) => true,
        Expression::TemplateLiteral(t) => t.expressions.is_empty(),
        _ => false,
    };
    let Some(e) = arg
        .and_then(Argument::as_expression)
        .map(|e| e.get_inner_expression())
    else {
        return match arg {
            None => Ok(()),
            Some(_) => Err("a spread argument".to_owned()),
        };
    };
    match e {
        e if static_text(e) => Ok(()),
        Expression::ObjectExpression(o) => {
            for p in &o.properties {
                let ObjectPropertyKind::ObjectProperty(p) = p else {
                    return Err("a spread in its options".to_owned());
                };
                let key = p
                    .key
                    .static_name()
                    .filter(|_| !p.computed)
                    .map(|n| n.display().to_string());
                let fine = match key.as_deref() {
                    Some("message") => static_text(&p.value),
                    Some("path") => match p.value.get_inner_expression() {
                        Expression::ArrayExpression(a) => a.elements.iter().all(|x| {
                            x.as_expression().is_some_and(|x| {
                                static_text(x) || matches!(x, Expression::NumericLiteral(_))
                            })
                        }),
                        _ => false,
                    },
                    _ => false,
                };
                if !fine {
                    return Err("options other than a static `message` and `path`".to_owned());
                }
            }
            Ok(())
        }
        _ => Err("options other than a static message".to_owned()),
    }
}

/// `ctx.addIssue({ … })` with static properties (a block holding only
/// that, too), without `fatal` (zod 3: aborts) or `continue` (zod 4).
fn adds_issue(statement: &oxc_ast::ast::Statement<'_>, ctx: Option<&str>) -> bool {
    use oxc_ast::ast::Statement;
    let call = match statement {
        Statement::ExpressionStatement(s) => &s.expression,
        Statement::BlockStatement(b) => match b.body.as_slice() {
            [Statement::ExpressionStatement(s)] => &s.expression,
            _ => return false,
        },
        _ => return false,
    };
    let Expression::CallExpression(call) = call.get_inner_expression() else {
        return false;
    };
    let Expression::StaticMemberExpression(m) = call.callee.get_inner_expression() else {
        return false;
    };
    let receiver_is_ctx = matches!(m.object.get_inner_expression(),
        Expression::Identifier(id) if Some(id.name.as_str()) == ctx);
    let [arg] = call.arguments.as_slice() else {
        return false;
    };
    let Some(Expression::ObjectExpression(o)) =
        arg.as_expression().map(|e| e.get_inner_expression())
    else {
        return false;
    };
    let static_value = |e: &Expression<'_>| -> bool {
        match e.get_inner_expression() {
            Expression::StringLiteral(_) | Expression::NumericLiteral(_) => true,
            Expression::TemplateLiteral(t) => t.expressions.is_empty(),
            Expression::ArrayExpression(a) => a.elements.iter().all(|x| {
                x.as_expression().is_some_and(|x| {
                    matches!(
                        x.get_inner_expression(),
                        Expression::StringLiteral(_) | Expression::NumericLiteral(_)
                    )
                })
            }),
            // `z.ZodIssueCode.custom`
            Expression::StaticMemberExpression(m) => matches!(
                m.object.get_inner_expression(),
                Expression::StaticMemberExpression(inner) if inner.property.name == "ZodIssueCode"
            ),
            _ => false,
        }
    };
    receiver_is_ctx
        && m.property.name == "addIssue"
        && !call.optional
        && o.properties.iter().all(|p| match p {
            ObjectPropertyKind::ObjectProperty(p) => {
                matches!(
                    p.key
                        .static_name()
                        .filter(|_| !p.computed)
                        .map(|n| n.display().to_string())
                        .as_deref(),
                    Some("code" | "message" | "path")
                ) && static_value(&p.value)
            }
            ObjectPropertyKind::SpreadProperty(_) => false,
        })
}

/// Lowering of a refinement callback body to a model expression of its
/// parameter (the only local). Each construct keeps JavaScript's result,
/// including when JavaScript throws (a property of `null`/`undefined`);
/// what the model cannot evaluate exactly is refused, or evaluates to an
/// explicit unknown.
struct Lower<'i, 'n, 'a> {
    interp: &'i Interp<'n, 'a>,
    param: &'i str,
}

impl<'a> Lower<'_, '_, 'a> {
    /// `cond`: only the truthiness of the result is used.
    fn expr(&self, e: &Expression<'_>, cond: bool) -> Result<midwrite_model::value::Expr, String> {
        use midwrite_model::value::{BinOp, Builtin as B, Expr, Value};
        use oxc_syntax::operator::{BinaryOperator, LogicalOperator, UnaryOperator};
        let refused = |what: &str| Err(format!("a refinement callback using {what}"));
        let typeof_is =
            |x: &Expr, t: &str| Expr::eq(Expr::call(B::TypeOf, vec![x.clone()]), Expr::lit(t));
        match e.get_inner_expression() {
            Expression::Identifier(id) => {
                let name = id.name.as_str();
                if name == self.param {
                    return Ok(Expr::Local(self.param.to_owned()));
                }
                if let Some(init) = self.interp.names.constant(name) {
                    return match literal(init) {
                        Some(v) => Ok(Expr::Lit(v)),
                        None => refused(&format!("`{name}`, which is not a literal constant")),
                    };
                }
                if name == "undefined" {
                    return Ok(Expr::undefined());
                }
                refused(&format!("`{name}`"))
            }
            Expression::NullLiteral(_) => Ok(Expr::Lit(Value::Null)),
            Expression::TemplateLiteral(t) if t.expressions.is_empty() => {
                match t
                    .quasis
                    .first()
                    .and_then(|q| q.value.cooked.as_ref())
                    .and_then(|c| c.as_str())
                {
                    Some(text) => Ok(Expr::Lit(Value::str(text))),
                    None => refused("a template literal with an invalid escape"),
                }
            }
            other if literal(other).is_some() => {
                Ok(Expr::Lit(literal(other).unwrap_or(Value::Null)))
            }
            Expression::ArrayExpression(a) => {
                let mut items = Vec::new();
                for x in &a.elements {
                    match x.as_expression() {
                        Some(x) => items.push(self.expr(x, false)?),
                        None => return refused("a spread or a hole in an array literal"),
                    }
                }
                Ok(Expr::Array(items))
            }
            Expression::StaticMemberExpression(m) if !m.optional => {
                let object = self.expr(&m.object, false)?;
                Ok(if m.property.name == "length" {
                    Expr::call(B::Length, vec![object])
                } else {
                    Expr::call(B::At, vec![object, Expr::lit(m.property.name.as_str())])
                })
            }
            Expression::UnaryExpression(u) => match u.operator {
                UnaryOperator::LogicalNot => Ok(Expr::not(self.expr(&u.argument, true)?)),
                UnaryOperator::Typeof => {
                    Ok(Expr::call(B::TypeOf, vec![self.expr(&u.argument, false)?]))
                }
                _ => refused(&format!("the operator `{}`", u.operator.as_str())),
            },
            Expression::LogicalExpression(l) if cond => {
                let op = match l.operator {
                    LogicalOperator::And => BinOp::And,
                    LogicalOperator::Or => BinOp::Or,
                    LogicalOperator::Coalesce => return refused("`??`"),
                };
                Ok(Expr::bin(
                    op,
                    self.expr(&l.left, true)?,
                    self.expr(&l.right, true)?,
                ))
            }
            Expression::LogicalExpression(_) => refused("`&&`/`||` for their value"),
            Expression::BinaryExpression(b) => {
                let (a, c) = (self.expr(&b.left, false)?, self.expr(&b.right, false)?);
                match b.operator {
                    // IsStrictlyEqual is SameValueZero on the model's values
                    // (it has no NaN or -0): `[c].includes(a)`, which refuses
                    // comparing two objects (identity).
                    BinaryOperator::StrictEquality | BinaryOperator::StrictInequality => {
                        let same = Expr::call(B::Includes, vec![Expr::Array(vec![c]), a]);
                        Ok(if b.operator == BinaryOperator::StrictEquality {
                            same
                        } else {
                            Expr::not(same)
                        })
                    }
                    // Numbers only: strings order by UTF-16 code units and
                    // mixed operands convert, neither of which is modeled.
                    BinaryOperator::LessThan
                    | BinaryOperator::LessEqualThan
                    | BinaryOperator::GreaterThan
                    | BinaryOperator::GreaterEqualThan => {
                        let op = match b.operator {
                            BinaryOperator::LessThan => BinOp::Lt,
                            BinaryOperator::LessEqualThan => BinOp::Le,
                            BinaryOperator::GreaterThan => BinOp::Gt,
                            _ => BinOp::Ge,
                        };
                        let numbers =
                            Expr::bin(BinOp::And, typeof_is(&a, "number"), typeof_is(&c, "number"));
                        Ok(Expr::bin(
                            BinOp::Or,
                            Expr::bin(BinOp::And, numbers.clone(), Expr::bin(op, a, c)),
                            Expr::bin(
                                BinOp::And,
                                Expr::not(numbers),
                                Expr::Unknown(
                                    "a refinement ordering values that are not two numbers"
                                        .to_owned(),
                                ),
                            ),
                        ))
                    }
                    other => refused(&format!("the operator `{}`", other.as_str())),
                }
            }
            Expression::CallExpression(call) if !call.optional => {
                let Expression::StaticMemberExpression(m) = call.callee.get_inner_expression()
                else {
                    return refused("a call of a function");
                };
                if m.optional {
                    return refused("optional chaining");
                }
                let method = m.property.name.as_str();
                let args = &call.arguments;
                // `xs.some((x) => …)` / `xs.every(…)`, with a callback of
                // the element only.
                if matches!(method, "some" | "every") {
                    let [arg] = args.as_slice() else {
                        return refused(&format!("`.{method}` with other than one callback"));
                    };
                    let Some(Expression::ArrowFunctionExpression(f)) =
                        arg.as_expression().map(|e| e.get_inner_expression())
                    else {
                        return refused(&format!("`.{method}` of something other than an arrow"));
                    };
                    let (var, body) = element_callback(f).ok_or_else(|| {
                        format!(
                            "a refinement callback using `.{method}` with an unsupported callback"
                        )
                    })?;
                    if var == self.param {
                        return refused("a callback parameter shadowing the value");
                    }
                    let list = self.expr(&m.object, false)?;
                    let inner = Lower {
                        interp: self.interp,
                        param: var,
                    };
                    // The element callback may read its element only.
                    let body = inner.expr(body, true)?;
                    return Ok(Expr::Each {
                        op: if method == "some" {
                            midwrite_model::value::EachOp::Some
                        } else {
                            midwrite_model::value::EachOp::Every
                        },
                        list: Box::new(list),
                        var: var.to_owned(),
                        body: Box::new(body),
                    });
                }
                let func = match (method, args.len()) {
                    ("trim", 0) => B::Trim,
                    ("toLowerCase", 0) => B::ToLowerCase,
                    ("toUpperCase", 0) => B::ToUpperCase,
                    ("startsWith", 1) => B::StartsWith,
                    ("endsWith", 1) => B::EndsWith,
                    ("includes", 1) => B::Includes,
                    _ => return refused(&format!("the method `.{method}()`")),
                };
                let mut lowered = vec![self.expr(&m.object, false)?];
                for a in args {
                    match a.as_expression() {
                        Some(a) => lowered.push(self.expr(a, false)?),
                        None => return refused("a spread argument"),
                    }
                }
                Ok(Expr::call(func, lowered))
            }
            _ => refused("an expression outside the modeled subset"),
        }
    }
}

/// `(x) => expr` with one plain, non-async parameter.
fn element_callback<'b, 'a>(
    f: &'b oxc_ast::ast::ArrowFunctionExpression<'a>,
) -> Option<(&'b str, &'b Expression<'a>)> {
    use oxc_ast::ast::{ArrowFunctionBody, BindingPattern};
    if f.r#async || f.params.rest.is_some() {
        return None;
    }
    let [p] = f.params.items.as_slice() else {
        return None;
    };
    let BindingPattern::BindingIdentifier(id) = &p.pattern else {
        return None;
    };
    if p.initializer.is_some() {
        return None;
    }
    let body = match &f.body {
        ArrowFunctionBody::FunctionBody(_) => return None,
        e => e.as_expression()?,
    };
    Some((id.name.as_str(), body))
}

/// A custom message (argument `n`) does not change what is accepted; other
/// options (`{ position }`, `{ version }`, ...) might.
fn message_only(what: &str, args: &[Argument<'_>], n: usize) -> Result<(), String> {
    if args.len() > n + 1 {
        return Err(format!("zod `{what}` with more arguments than modeled"));
    }
    match args.get(n).and_then(Argument::as_expression).map(|e| e.get_inner_expression()) {
        None | Some(Expression::StringLiteral(_)) => Ok(()),
        // A template-literal message whose interpolations are
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
// The zod version of a module
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
/// A package that re-exports zod's `z` with OpenAPI metadata methods.
pub(crate) const HONO_ZOD_OPENAPI: &str = "@hono/zod-openapi";

pub(crate) fn resolve_version(
    file: &Path,
    root: &Path,
    specifier: &str,
) -> Result<Version, String> {
    if specifier == "zod/v3" {
        return Ok(Version::V3);
    }
    let major = package_major(file, root);
    // `@hono/zod-openapi` re-exports zod's `z` (extended with `.openapi()`):
    // 0.x is built on zod 3, 1.x on zod 4. Its major and the project's zod
    // must agree.
    if specifier == HONO_ZOD_OPENAPI {
        let hono = find_major(file, root, HONO_ZOD_OPENAPI);
        return match (hono, major) {
            (Ok(0), Ok(3) | Err(_)) => Ok(Version::V3),
            (Ok(h), Ok(4) | Err(_)) if h >= 1 => Ok(Version::V4),
            (Ok(h), Ok(z)) => Err(format!(
                "`{HONO_ZOD_OPENAPI}` {h}.x with zod {z}.x: the zod it is built on is not known"
            )),
            (Ok(h), Err(why)) => Err(format!(
                "`{HONO_ZOD_OPENAPI}` {h}.x with an unknown zod: {why}"
            )),
            (Err(why), _) => Err(format!("`{HONO_ZOD_OPENAPI}`: {why}")),
        };
    }
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
    let found = find_major(file, root, "zod");
    if let Ok(mut c) = cache().lock() {
        c.insert(key, found.clone());
    }
    found
}

fn find_major(file: &Path, root: &Path, package: &str) -> Result<u64, String> {
    let dirs = dirs_up(file, root);
    for d in &dirs {
        let installed = d.join("node_modules").join(package).join("package.json");
        if let Some(json) = read_json(&installed) {
            return match json.get("version").and_then(|v| v.as_str()) {
                Some(v) => major_of(v).ok_or_else(|| {
                    format!("the installed {package} has the unreadable version `{v}`")
                }),
                None => Err(format!(
                    "the installed {package}'s package.json has no version"
                )),
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
        .find_map(|k| json.get(*k)?.get(package)?.as_str().map(str::to_owned));
        let Some(spec) = spec else { continue };
        let spec = match spec.strip_prefix("catalog:") {
            Some(name) => catalog_spec(&dirs[i..], name.trim(), package).ok_or_else(|| {
                format!(
                    "the {package} dependency `{spec}` names a catalog entry that was not found"
                )
            })?,
            None => spec,
        };
        return major_of(&spec).ok_or_else(|| {
            format!("the {package} dependency range `{spec}` does not fix a major version")
        });
    }
    Err(format!(
        "no installed {package} package and no package.json declaring {package}"
    ))
}

/// The `zod` entry of a workspace catalog (`catalog`/`catalogs` of a
/// `package.json`, also under `workspaces`; `catalog:` of
/// `pnpm-workspace.yaml`).
fn catalog_spec(dirs: &[PathBuf], name: &str, package: &str) -> Option<String> {
    let named = !name.is_empty() && name != "default";
    for d in dirs {
        if let Some(json) = read_json(&d.join("package.json")) {
            for base in [Some(&json), json.get("workspaces")].into_iter().flatten() {
                let entry = if named {
                    base.get("catalogs")
                        .and_then(|c| c.get(name))
                        .and_then(|c| c.get(package))
                } else {
                    base.get("catalog").and_then(|c| c.get(package))
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
                    if k.trim().trim_matches(|c| c == '\'' || c == '"') == package {
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

    /// Names whose `z` comes from `@hono/zod-openapi`.
    struct Hono;
    impl<'a> Names<'a> for Hono {
        fn zod_import(&self, name: &str) -> Option<String> {
            (name == "z").then(|| HONO_ZOD_OPENAPI.to_owned())
        }
        fn version(&self, _: &str) -> Result<Version, String> {
            Ok(Version::V4)
        }
        fn constant(&self, _: &str) -> Option<&'a Expression<'a>> {
            None
        }
        fn string_array(&self, _: &str) -> Option<Vec<String>> {
            None
        }
    }

    #[test]
    fn openapi_metadata_is_read_only_from_hono_zod_openapi() {
        let src =
            "z.object({ id: z.string().uuid().openapi({ example: 'x' }) }).openapi('Customer')";
        let allocator = Allocator::default();
        let text = format!("const s = {src};");
        let parsed = Parser::new(&allocator, &text, SourceType::ts()).parse();
        let Statement::VariableDeclaration(v) = &parsed.program.body[0] else {
            panic!()
        };
        let init = v.declarations[0].init.as_ref().unwrap();
        assert_eq!(
            interpret(init, &Hono).map(|v| v.schema),
            Ok(Schema::Object(vec![(
                "id".into(),
                Schema::Str(vec![StrCheck::Uuid])
            )]))
        );
        // Plain zod has no `.openapi()`.
        assert!(schema(src).is_err());
    }

    #[test]
    fn hono_zod_openapi_takes_the_zod_its_major_is_built_on() {
        let dir = std::env::temp_dir().join(format!("midwrite-hono-zod-{}", std::process::id()));
        let file = dir.join("src").join("a.ts");
        let check = |deps: &str| {
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("src")).unwrap();
            std::fs::write(
                dir.join("package.json"),
                format!(r#"{{ "name": "p", "dependencies": {{ {deps} }} }}"#),
            )
            .unwrap();
            cache().lock().unwrap().clear();
            resolve_version(&file, &dir, HONO_ZOD_OPENAPI)
        };
        assert_eq!(
            check(r#""@hono/zod-openapi": "1.2.3", "zod": "^4.3.6""#),
            Ok(Version::V4)
        );
        assert_eq!(
            check(r#""@hono/zod-openapi": "0.18.4", "zod": "^3.23.8""#),
            Ok(Version::V3)
        );
        assert_eq!(check(r#""@hono/zod-openapi": "1.2.3""#), Ok(Version::V4));
        assert!(check(r#""@hono/zod-openapi": "1.2.3", "zod": "^3.23.8""#).is_err());
        assert!(check(r#""zod": "^4.3.6""#).is_err());
        let _ = std::fs::remove_dir_all(&dir);
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
        assert!(schema("z.coerce.number()").is_err());
        assert!(schema("z.discriminatedUnion('k', [])").is_err());
        assert!(schema("z.preprocess((x) => x, z.string())").is_err());
        assert!(schema("z.string().regex(/(a)\\1/)").is_err());
    }

    #[test]
    fn describe_is_metadata_only() {
        assert_eq!(
            schema("z.string().min(1).describe('a name').max(3)"),
            schema("z.string().min(1).max(3)")
        );
        assert_eq!(
            schema("z.object({ a: z.number().describe('n').optional() }).describe('p').strict()"),
            schema("z.object({ a: z.number().optional() }).strict()")
        );
        assert!(schema("z.string().describe('x').refine((s) => s !== '')").is_ok());
    }

    #[test]
    fn refinements_in_the_modeled_subset_are_read() {
        for src in [
            "z.number().refine((port) => port !== 25, { message: 'no' })",
            "z.object({ a: z.string(), b: z.string() }).refine((d) => d.a === d.b, { path: ['b'] })",
            "z.string().refine(function (s) { return s.trim().length > 0; }, 'empty')",
            "z.array(z.number()).refine((xs) => xs.every((x) => x >= 0) && xs.length > 0)",
            "z.object({ k: z.string() }).superRefine((v, ctx) => { if (v.k === 'x') ctx.addIssue({ code: 'custom', message: 'no' }); })",
            "z.string().refine((s) => !s.startsWith('re:') || typeof s === 'string').optional()",
        ] {
            let read = schema(src).unwrap_or_else(|e| panic!("{src}: {e}"));
            assert!(
                matches!(&read, Schema::Refine(..) | Schema::Optional(_)),
                "{src}: {read:?}"
            );
        }
    }

    #[test]
    fn refinements_outside_the_subset_are_refused_by_name() {
        for (src, why) in [
            (
                "z.string().refine(async (s) => s !== '')",
                "an async callback",
            ),
            ("z.string().refine((s) => s ?? 'x')", "`??`"),
            (
                "z.string().refine((s) => s ? true : false)",
                "an expression outside",
            ),
            (
                "z.string().refine((s) => isValid(s))",
                "a call of a function",
            ),
            ("z.string().refine((s) => s === other)", "`other`"),
            (
                "z.string().refine((s) => s.length + 1 > 2)",
                "the operator `+`",
            ),
            (
                "z.string().refine((s) => s !== '', { when: () => true })",
                "options other than",
            ),
            (
                "z.string().refine((s) => s !== '', { abort: true })",
                "options other than",
            ),
            ("z.string().refine(check)", "not an inline function"),
            (
                "z.string().refine((s) => s?.length > 1)",
                "outside the modeled subset",
            ),
            (
                "z.string().superRefine((v, ctx) => { if (v === '') ctx.addIssue({ code: 'custom', fatal: true }); })",
                "`if (…) ctx.addIssue(…)`",
            ),
            (
                "z.string().superRefine((v, ctx) => { const n = v.length; if (n === 0) ctx.addIssue({ code: 'custom' }); })",
                "`if (…) ctx.addIssue(…)`",
            ),
            // Methods after a refinement differ between zod 3 and 4.
            ("z.string().refine((s) => s !== '').min(2)", ""),
        ] {
            let err = schema(src).expect_err(src);
            assert!(err.contains(why), "{src}: {err}");
        }
    }

    #[test]
    fn extended_schemas_are_read_with_their_version() {
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
        // `.int()`: dropped for zod 3, kept otherwise.
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

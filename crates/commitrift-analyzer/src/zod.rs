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

use oxc_ast::ast::{Argument, Expression, ObjectPropertyKind};

/// A modeled zod schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Schema {
    Str(Vec<StrCheck>),
    /// `z.number()`: the model's numbers are integers, so `.int()` holds
    /// for every model value.
    Num(Vec<NumCheck>),
    Bool,
    /// `z.literal(v)`.
    Literal(commitrift_model::value::Value),
    /// `z.enum([...])`.
    Enum(Vec<String>),
    /// `z.object({...})` (strip mode): keys in shape order.
    Object(Vec<(String, Schema)>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StrCheck {
    Min(i64),
    Max(i64),
    Length(i64),
    Trim,
    Lower,
    Upper,
    StartsWith(String),
    EndsWith(String),
    Includes(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NumCheck {
    /// `min`/`gte`: value >= n.
    Ge(i64),
    /// `gt`/`positive` (n = 0).
    Gt(i64),
    /// `max`/`lte`.
    Le(i64),
    /// `lt`/`negative` (n = 0).
    Lt(i64),
}

/// How the interpreter resolves names it meets in a chain.
pub(crate) trait Names<'a> {
    /// Whether `name` is zod's namespace (`import { z } from "zod"`,
    /// `import * as z from "zod"`).
    fn is_zod(&self, name: &str) -> bool;
    /// The initializer of a module-level `const` (another schema).
    fn constant(&self, name: &str) -> Option<&'a Expression<'a>>;
}

/// Read the schema built by `e`.
pub(crate) fn interpret<'a>(
    e: &'a Expression<'a>,
    names: &dyn Names<'a>,
) -> Result<Schema, String> {
    interpret_depth(e, names, 0)
}

fn interpret_depth<'a>(
    e: &'a Expression<'a>,
    names: &dyn Names<'a>,
    depth: u32,
) -> Result<Schema, String> {
    if depth > 16 {
        return Err("a zod schema nested too deeply".to_owned());
    }
    match e.get_inner_expression() {
        Expression::Identifier(id) => match names.constant(id.name.as_str()) {
            Some(init) => interpret_depth(init, names, depth + 1),
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
                && names.is_zod(root.name.as_str())
            {
                return root_schema(method, &call.arguments, names, depth);
            }
            let base = interpret_depth(&member.object, names, depth + 1)?;
            refine(base, method, &call.arguments)
        }
        _ => Err("a zod schema written as an expression that is not a builder chain".to_owned()),
    }
}

fn root_schema<'a>(
    method: &str,
    args: &'a [Argument<'a>],
    names: &dyn Names<'a>,
    depth: u32,
) -> Result<Schema, String> {
    match method {
        "string" if args.is_empty() => Ok(Schema::Str(Vec::new())),
        "number" if args.is_empty() => Ok(Schema::Num(Vec::new())),
        "boolean" if args.is_empty() => Ok(Schema::Bool),
        "literal" => match args.first().and_then(Argument::as_expression).map(literal) {
            Some(Some(v)) if args.len() == 1 => Ok(Schema::Literal(v)),
            _ => Err("`z.literal` of a value that is not a literal".to_owned()),
        },
        "enum" => {
            let Some(Expression::ArrayExpression(items)) = args
                .first()
                .and_then(Argument::as_expression)
                .map(|e| e.get_inner_expression())
            else {
                return Err("`z.enum` of something other than an array literal".to_owned());
            };
            let mut out = Vec::new();
            for item in &items.elements {
                match item.as_expression().map(|e| e.get_inner_expression()) {
                    Some(Expression::StringLiteral(s)) if s.value.as_str().is_some() => {
                        out.push(s.value.as_str().unwrap_or_default().to_owned());
                    }
                    _ => {
                        return Err(
                            "`z.enum` with a member that is not a string literal".to_owned()
                        );
                    }
                }
            }
            Ok(Schema::Enum(out))
        }
        "object" => {
            let Some(Expression::ObjectExpression(shape)) = args
                .first()
                .and_then(Argument::as_expression)
                .map(|e| e.get_inner_expression())
            else {
                return Err("`z.object` of something other than an object literal".to_owned());
            };
            if args.len() != 1 {
                return Err("`z.object` with options".to_owned());
            }
            let mut fields = Vec::new();
            for property in &shape.properties {
                let ObjectPropertyKind::ObjectProperty(p) = property else {
                    return Err("a spread in a `z.object` shape".to_owned());
                };
                let key = match &p.key {
                    oxc_ast::ast::PropertyKey::StaticIdentifier(id) => id.name.to_string(),
                    oxc_ast::ast::PropertyKey::StringLiteral(s) => match s.value.as_str() {
                        Some(k) => k.to_owned(),
                        None => return Err("a `z.object` key with a lone surrogate".to_owned()),
                    },
                    _ => return Err("a computed key in a `z.object` shape".to_owned()),
                };
                fields.push((key, interpret_depth(&p.value, names, depth + 1)?));
            }
            Ok(Schema::Object(fields))
        }
        other => Err(format!("zod `z.{other}(...)` is not modeled")),
    }
}

fn refine<'a>(base: Schema, method: &str, args: &'a [Argument<'a>]) -> Result<Schema, String> {
    let int = |i: usize| -> Result<i64, String> {
        match args.get(i).and_then(Argument::as_expression).map(literal) {
            Some(Some(commitrift_model::value::Value::Int(n))) => Ok(n),
            _ => Err(format!(
                "zod `.{method}` with an argument that is not an integer literal"
            )),
        }
    };
    let text = |i: usize| -> Result<String, String> {
        match args.get(i).and_then(Argument::as_expression).map(literal) {
            Some(Some(commitrift_model::value::Value::Str(s))) => Ok(s),
            _ => Err(format!(
                "zod `.{method}` with an argument that is not a string literal"
            )),
        }
    };
    // A custom message (second argument) does not change what is accepted;
    // other options (`{ position }`, ...) might.
    let message_only = |n: usize| -> Result<(), String> {
        match args.get(n).and_then(Argument::as_expression).map(|e| e.get_inner_expression()) {
            None | Some(Expression::StringLiteral(_)) => Ok(()),
            Some(Expression::ObjectExpression(o))
                if o.properties.iter().all(|p| {
                    matches!(p, ObjectPropertyKind::ObjectProperty(p)
                        if matches!(&p.key, oxc_ast::ast::PropertyKey::StaticIdentifier(id) if id.name == "message"))
                }) =>
            {
                Ok(())
            }
            _ => Err(format!("zod `.{method}` with options other than a message")),
        }
    };
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
                other => return Err(format!("zod string check `.{other}()` is not modeled")),
            };
            checks.push(check);
            Ok(Schema::Str(checks))
        }
        Schema::Num(mut checks) => {
            let check = match method {
                // Model numbers are integers: both hold for every model
                // value.
                "int" | "finite" => {
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
        Schema::Object(fields) => match method {
            // `strip` is the default; `strict` and `passthrough` change the
            // output or the accepted inputs.
            "strip" if args.is_empty() => Ok(Schema::Object(fields)),
            other => Err(format!("zod object method `.{other}()` is not modeled")),
        },
        _ => Err(format!("zod `.{method}()` on this schema is not modeled")),
    }
}

fn literal(e: &Expression<'_>) -> Option<commitrift_model::value::Value> {
    use commitrift_model::value::Value;
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

#[cfg(test)]
mod tests {
    use super::*;
    use oxc_allocator::Allocator;
    use oxc_ast::ast::Statement;
    use oxc_parser::Parser;
    use oxc_span::SourceType;

    struct Z;
    impl<'a> Names<'a> for Z {
        fn is_zod(&self, name: &str) -> bool {
            name == "z"
        }
        fn constant(&self, _: &str) -> Option<&'a Expression<'a>> {
            None
        }
    }

    fn schema(src: &str) -> Result<Schema, String> {
        let allocator = Allocator::default();
        let text = format!("const s = {src};");
        let parsed = Parser::new(&allocator, &text, SourceType::ts()).parse();
        let Statement::VariableDeclaration(v) = &parsed.program.body[0] else {
            panic!()
        };
        let init = v.declarations[0].init.as_ref().unwrap();
        interpret(init, &Z)
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
            schema("z.string().email()"),
            Err("zod string check `.email()` is not modeled".into())
        );
        assert!(schema("z.string().optional()").is_err());
        assert!(schema("z.object({ a: z.string() }).passthrough()").is_err());
        assert!(schema("z.string().transform((s) => s.length)").is_err());
        assert!(schema("z.union([z.string(), z.number()])").is_err());
        assert!(schema("z.string().min(n)").is_err());
    }
}

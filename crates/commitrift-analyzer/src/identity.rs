//! Source-level function identity and selectors (Phase 7).
//!
//! A function is identified by its module and its *anchor*: the line and
//! column where the function node starts. That is the anchor the static
//! analyzer already uses for anonymous functions (`<anonymous@43:3>`), so a
//! function selected for model checking links to the Effect IR function of
//! `commitrift check` by position, never by name. Two anonymous functions
//! with the same text are different functions.
//!
//! A *selector* says how to reach a function from a top-level binding, so a
//! scenario can name a callback without editing the application:
//!
//! ```text
//! src/app/actions.ts#inviteTeamMember/arg[1]   callback argument 1 of the call
//!                                              that initializes `inviteTeamMember`
//! src/factory.ts#makeHandler/return            the function `makeHandler` returns
//! src/service.ts#OrderService/method[place]    an instance method
//! src/service.ts#OrderService/static[create]   a static method
//! src/handlers.ts#handlers/property[create]    a function-valued object property
//! src/app/actions.ts@43:3                      the function starting at 43:3
//! ```
//!
//! Steps can be chained (`#router/arg[0]/property[create]/arg[1]`). Every
//! step must resolve to exactly one syntax node; anything else is an error,
//! never a guess.

use std::fmt;

use oxc_ast::ast::{
    Argument, ArrowFunctionBody, BindingPattern, Class, ClassElement, Declaration,
    ExportDefaultDeclarationKind, Expression, FormalParameters, Function, FunctionBody,
    MethodDefinitionKind, ObjectPropertyKind, Program, PropertyKey, Statement,
};
use oxc_span::Span;

use commitrift_core::location::LineIndex;

/// One step of a selector.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    /// Argument `n` of the call expression reached so far.
    Arg(usize),
    /// The function returned by the function reached so far.
    Return,
    /// An instance method of a class.
    Method(String),
    /// A static method of a class.
    Static(String),
    /// A property of an object literal.
    Property(String),
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arg(n) => write!(f, "arg[{n}]"),
            Self::Return => f.write_str("return"),
            Self::Method(m) => write!(f, "method[{m}]"),
            Self::Static(m) => write!(f, "static[{m}]"),
            Self::Property(p) => write!(f, "property[{p}]"),
        }
    }
}

/// A parsed selector: a module plus a binding path or an anchor.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Selector {
    pub module: String,
    pub target: Target,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    /// `#binding/step/...`
    Path { binding: String, steps: Vec<Step> },
    /// `@line:column` (1-based line, 1-based column, as reported).
    Anchor { line: u32, column: u32 },
}

impl fmt::Display for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.target {
            Target::Path { binding, steps } => {
                write!(f, "{}#{binding}", self.module)?;
                for s in steps {
                    write!(f, "/{s}")?;
                }
                Ok(())
            }
            Target::Anchor { line, column } => write!(f, "{}@{line}:{column}", self.module),
        }
    }
}

impl Selector {
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if let Some((module, path)) = text.split_once('#') {
            if module.is_empty() || path.is_empty() {
                return Err(format!(
                    "selector `{text}`: expected `file#binding[/step...]`"
                ));
            }
            let mut parts = path.split('/');
            let binding = parts.next().unwrap_or_default().to_owned();
            if !is_identifier(&binding) && binding != "default" {
                return Err(format!(
                    "selector `{text}`: `{binding}` is not a binding name"
                ));
            }
            let mut steps = Vec::new();
            for part in parts {
                steps.push(parse_step(part).ok_or_else(|| {
                    format!(
                        "selector `{text}`: unknown step `{part}` (arg[N], return, method[name], static[name], property[name])"
                    )
                })?);
            }
            return Ok(Self {
                module: module.to_owned(),
                target: Target::Path { binding, steps },
            });
        }
        if let Some((module, anchor)) = text.rsplit_once('@')
            && let Some((line, column)) = anchor.split_once(':')
            && let (Ok(line), Ok(column)) = (line.parse(), column.parse())
            && !module.is_empty()
        {
            return Ok(Self {
                module: module.to_owned(),
                target: Target::Anchor { line, column },
            });
        }
        Err(format!(
            "selector `{text}`: expected `file#binding[/step...]` or `file@line:column`"
        ))
    }
}

fn parse_step(part: &str) -> Option<Step> {
    if part == "return" {
        return Some(Step::Return);
    }
    let (kind, rest) = part.split_once('[')?;
    let inner = rest.strip_suffix(']')?;
    match kind {
        "arg" => inner.parse().ok().map(Step::Arg),
        "method" if is_identifier(inner) => Some(Step::Method(inner.to_owned())),
        "static" if is_identifier(inner) => Some(Step::Static(inner.to_owned())),
        "property" if !inner.is_empty() => Some(Step::Property(inner.to_owned())),
        _ => None,
    }
}

fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// A resolved function: its syntax and its identity.
pub struct Found<'a> {
    pub node: FnNode<'a>,
    /// Anchor of the function node (1-based line and column).
    pub line: u32,
    pub column: u32,
    pub span: Span,
    /// How the function was reached, for reports (`inviteTeamMember/arg[1]`).
    pub path: String,
    /// The innermost call through which an `arg[N]` step reached the
    /// function: `(callee expression, argument index)`.
    pub via_call: Option<(&'a Expression<'a>, usize)>,
}

/// A function-like syntax node.
#[derive(Clone, Copy)]
pub enum FnNode<'a> {
    Function(&'a Function<'a>),
    Arrow(&'a oxc_ast::ast::ArrowFunctionExpression<'a>),
}

impl<'a> FnNode<'a> {
    pub fn params(&self) -> &'a FormalParameters<'a> {
        match self {
            Self::Function(f) => &f.params,
            Self::Arrow(a) => &a.params,
        }
    }
    pub fn is_async(&self) -> bool {
        match self {
            Self::Function(f) => f.r#async,
            Self::Arrow(a) => a.r#async,
        }
    }
    /// The block body, or the expression of a concise arrow body.
    pub fn body(&self) -> Result<&'a FunctionBody<'a>, &'a Expression<'a>> {
        match self {
            Self::Function(f) => Ok(f
                .body
                .as_deref()
                .expect("functions without a body are never selected")),
            Self::Arrow(a) => match &a.body {
                ArrowFunctionBody::FunctionBody(b) => Ok(b),
                other => Err(other
                    .as_expression()
                    .expect("a concise arrow body is an expression")),
            },
        }
    }
    pub fn span(&self) -> Span {
        match self {
            Self::Function(f) => f.span,
            Self::Arrow(a) => a.span,
        }
    }
}

fn as_function<'a>(e: &'a Expression<'a>) -> Option<FnNode<'a>> {
    match e.get_inner_expression() {
        Expression::ArrowFunctionExpression(a) => Some(FnNode::Arrow(a)),
        Expression::FunctionExpression(f) if f.body.is_some() => Some(FnNode::Function(f)),
        _ => None,
    }
}

/// What a selector step currently points at.
#[derive(Clone, Copy)]
enum Node<'a> {
    Expr(&'a Expression<'a>),
    Fn(FnNode<'a>),
    Class(&'a Class<'a>),
}

/// Resolve `selector` (its module must be `program`).
pub fn resolve<'a>(
    program: &'a Program<'a>,
    lines: &LineIndex<'_>,
    selector: &Selector,
) -> Result<Found<'a>, String> {
    match &selector.target {
        Target::Anchor { line, column } => {
            match at_anchor(program, lines, *line, *column).as_slice() {
                [one] => Ok(found(*one, lines, format!("@{line}:{column}"), None)),
                [] => Err(format!(
                    "no function starts at {line}:{column} in `{}`",
                    selector.module
                )),
                _ => Err(format!("several functions start at {line}:{column}")),
            }
        }
        Target::Path { binding, steps } => {
            let mut node = top_level(program, binding).ok_or_else(|| {
                format!("no top-level binding `{binding}` in `{}`", selector.module)
            })?;
            let mut path = binding.clone();
            let mut via_call = None;
            for step in steps {
                path.push('/');
                path.push_str(&step.to_string());
                let (next, call) = apply(node, step).map_err(|why| format!("`{path}`: {why}"))?;
                node = next;
                if call.is_some() {
                    via_call = call;
                }
            }
            match node {
                Node::Fn(f) => Ok(found(f, lines, path, via_call)),
                Node::Expr(e) => match as_function(e) {
                    Some(f) => Ok(found(f, lines, path, via_call)),
                    None => Err(format!("`{path}` is not a function")),
                },
                Node::Class(_) => Err(format!("`{path}` is a class, not a function")),
            }
        }
    }
}

fn found<'a>(
    f: FnNode<'a>,
    lines: &LineIndex<'_>,
    path: String,
    via_call: Option<(&'a Expression<'a>, usize)>,
) -> Found<'a> {
    let span = f.span();
    let (line, column) = lines.position(span.start);
    Found {
        node: f,
        line,
        column,
        span,
        path,
        via_call,
    }
}

fn top_level<'a>(program: &'a Program<'a>, name: &str) -> Option<Node<'a>> {
    fn declaration<'a>(d: &'a Declaration<'a>, name: &str) -> Option<Node<'a>> {
        match d {
            Declaration::FunctionDeclaration(f) => {
                f.id.as_ref()
                    .filter(|id| id.name.as_str() == name && f.body.is_some())
                    .map(|_| Node::Fn(FnNode::Function(f)))
            }
            Declaration::ClassDeclaration(c) => {
                c.id.as_ref()
                    .filter(|id| id.name.as_str() == name)
                    .map(|_| Node::Class(c))
            }
            Declaration::VariableDeclaration(v) => {
                v.declarations.iter().find_map(|d| match (&d.id, &d.init) {
                    (BindingPattern::BindingIdentifier(id), Some(init))
                        if id.name.as_str() == name =>
                    {
                        Some(Node::Expr(init))
                    }
                    _ => None,
                })
            }
            _ => None,
        }
    }
    for statement in &program.body {
        let hit = match statement {
            Statement::ExportDeclaration(e) => declaration(&e.declaration, name),
            Statement::ExportDefaultDeclaration(e) if name == "default" => match &e.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(f) if f.body.is_some() => {
                    Some(Node::Fn(FnNode::Function(f)))
                }
                ExportDefaultDeclarationKind::ClassDeclaration(c) => Some(Node::Class(c)),
                other => other.as_expression().map(Node::Expr),
            },
            other => other.as_declaration().and_then(|d| declaration(d, name)),
        };
        if hit.is_some() {
            return hit;
        }
    }
    None
}

fn key_name(key: &PropertyKey<'_>) -> Option<String> {
    match key {
        PropertyKey::StaticIdentifier(id) => Some(id.name.as_str().to_owned()),
        PropertyKey::StringLiteral(s) => s.value.as_str().map(str::to_owned),
        _ => None,
    }
}

type Reached<'a> = (Node<'a>, Option<(&'a Expression<'a>, usize)>);

fn apply<'a>(node: Node<'a>, step: &Step) -> Result<Reached<'a>, String> {
    match (step, node) {
        (Step::Arg(n), Node::Expr(e)) => {
            let e = e.get_inner_expression();
            let (callee, args) = match e {
                Expression::CallExpression(c) => (&c.callee, &c.arguments),
                Expression::NewExpression(c) => (&c.callee, &c.arguments),
                _ => return Err("not a call expression".to_owned()),
            };
            match args.get(*n) {
                Some(Argument::SpreadElement(_)) => Err(format!("argument {n} is a spread")),
                Some(a) => Ok((
                    Node::Expr(
                        a.as_expression()
                            .expect("non-spread arguments are expressions"),
                    ),
                    Some((callee, *n)),
                )),
                None => Err(format!("the call has no argument {n}")),
            }
        }
        (Step::Return, node) => {
            let f = match node {
                Node::Fn(f) => f,
                Node::Expr(e) => as_function(e).ok_or("not a function")?,
                Node::Class(_) => return Err("a class does not return".to_owned()),
            };
            let returned = match f.body() {
                Err(e) => vec![e],
                Ok(body) => {
                    let mut found = Vec::new();
                    for s in &body.statements {
                        collect_returns(s, &mut found);
                    }
                    found
                }
            };
            match returned.as_slice() {
                [one] => Ok((Node::Expr(one), None)),
                [] => Err("the function has no `return <expression>`".to_owned()),
                more => Err(format!(
                    "the function has {} `return` statements: select the function by its anchor",
                    more.len()
                )),
            }
        }
        (Step::Method(m) | Step::Static(m), Node::Class(c)) => {
            let want_static = matches!(step, Step::Static(_));
            for element in &c.body.body {
                if let ClassElement::MethodDefinition(def) = element
                    && def.kind == MethodDefinitionKind::Method
                    && def.r#static == want_static
                    && key_name(&def.key).as_deref() == Some(m.as_str())
                    && def.value.body.is_some()
                {
                    return Ok((Node::Fn(FnNode::Function(&def.value)), None));
                }
            }
            Err(format!(
                "no {} method `{m}`",
                if want_static { "static" } else { "instance" }
            ))
        }
        (Step::Method(m) | Step::Static(m), Node::Expr(e)) => match e.get_inner_expression() {
            Expression::ClassExpression(c) => apply(Node::Class(c), step),
            _ => Err(format!("`{m}`: not a class")),
        },
        (Step::Property(p), Node::Expr(e)) => match e.get_inner_expression() {
            Expression::ObjectExpression(o) => {
                let mut hits = o.properties.iter().filter_map(|prop| match prop {
                    ObjectPropertyKind::ObjectProperty(op)
                        if !op.computed && key_name(&op.key).as_deref() == Some(p.as_str()) =>
                    {
                        Some(&op.value)
                    }
                    _ => None,
                });
                match (hits.next(), hits.next()) {
                    (Some(v), None) => Ok((Node::Expr(v), None)),
                    (None, _) => Err(format!("no property `{p}`")),
                    _ => Err(format!("property `{p}` is defined more than once")),
                }
            }
            _ => Err("not an object literal".to_owned()),
        },
        (step, _) => Err(format!("step `{step}` does not apply here")),
    }
}

/// `return <expression>` statements of one function body (not of nested
/// functions).
fn collect_returns<'a>(statement: &'a Statement<'a>, out: &mut Vec<&'a Expression<'a>>) {
    match statement {
        Statement::ReturnStatement(r) => {
            if let Some(argument) = &r.argument {
                out.push(argument);
            }
        }
        Statement::BlockStatement(b) => b.body.iter().for_each(|s| collect_returns(s, out)),
        Statement::IfStatement(i) => {
            collect_returns(&i.consequent, out);
            if let Some(alternate) = &i.alternate {
                collect_returns(alternate, out);
            }
        }
        Statement::TryStatement(t) => {
            t.block.body.iter().for_each(|s| collect_returns(s, out));
            if let Some(handler) = &t.handler {
                handler
                    .body
                    .body
                    .iter()
                    .for_each(|s| collect_returns(s, out));
            }
            if let Some(finalizer) = &t.finalizer {
                finalizer.body.iter().for_each(|s| collect_returns(s, out));
            }
        }
        Statement::SwitchStatement(sw) => sw
            .cases
            .iter()
            .flat_map(|c| c.consequent.iter())
            .for_each(|s| collect_returns(s, out)),
        Statement::ForStatement(f) => collect_returns(&f.body, out),
        Statement::ForOfStatement(f) => collect_returns(&f.body, out),
        Statement::ForInStatement(f) => collect_returns(&f.body, out),
        Statement::WhileStatement(w) => collect_returns(&w.body, out),
        Statement::DoWhileStatement(w) => collect_returns(&w.body, out),
        Statement::LabeledStatement(l) => collect_returns(&l.body, out),
        _ => {}
    }
}

/// Functions whose node starts at `line:column`.
fn at_anchor<'a>(
    program: &'a Program<'a>,
    lines: &LineIndex<'_>,
    line: u32,
    column: u32,
) -> Vec<FnNode<'a>> {
    let semantic = oxc_semantic::SemanticBuilder::new()
        .with_build_nodes(true)
        .build(program)
        .semantic;
    let mut out = Vec::new();
    for node in semantic.nodes().iter() {
        let candidate = match node.kind() {
            oxc_ast::AstKind::Function(f) if f.body.is_some() => FnNode::Function(f),
            oxc_ast::AstKind::ArrowFunctionExpression(a) => FnNode::Arrow(a),
            _ => continue,
        };
        if lines.position(candidate.span().start) == (line, column) {
            out.push(candidate);
        }
    }
    out
}

/// The name the static analyzer gives the function at this anchor when it
/// has no name of its own: `<anonymous@line:column>`. Used to link the two
/// frontends by position.
pub fn anonymous_name(line: u32, column: u32) -> String {
    format!("<anonymous@{line}:{column}>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxc_allocator::Allocator;
    use oxc_parser::Parser;
    use oxc_span::SourceType;

    const SOURCE: &str = r#"
import { wrap } from "./wrap";
export const action = wrap(schema, async (data, _, user) => {
  return data;
});
export const other = wrap(schema, async (data, _, user) => {
  return data;
});
export function makeHandler(db) {
  return async (id) => db.find(id);
}
export class OrderService {
  async place(order) { return order; }
  static async create() { return new OrderService(); }
}
export const handlers = { create: async () => 1, "update": function () { return 2; } };
"#;

    fn with<R>(f: impl FnOnce(&Program<'_>, &LineIndex<'_>) -> R) -> R {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, SOURCE, SourceType::ts()).parse();
        let lines = LineIndex::new(SOURCE);
        f(&parsed.program, &lines)
    }

    fn anchor(selector: &str) -> Result<(u32, u32), String> {
        with(|program, lines| {
            let selector = Selector::parse(selector)?;
            resolve(program, lines, &selector).map(|f| (f.line, f.column))
        })
    }

    #[test]
    fn selectors_round_trip_and_reject_garbage() {
        for text in [
            "src/a.ts#action/arg[1]",
            "src/a.ts#makeHandler/return",
            "src/a.ts#OrderService/method[place]",
            "src/a.ts#OrderService/static[create]",
            "src/a.ts#handlers/property[create]",
            "src/a.ts@3:36",
        ] {
            assert_eq!(Selector::parse(text).unwrap().to_string(), text);
        }
        for bad in [
            "src/a.ts",
            "#x",
            "a.ts#x/arg[x]",
            "a.ts#x/frob",
            "a.ts#1x",
            "a.ts@x:1",
        ] {
            assert!(Selector::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn callbacks_with_identical_text_are_different_functions() {
        let a = anchor("m.ts#action/arg[1]").unwrap();
        let b = anchor("m.ts#other/arg[1]").unwrap();
        assert_ne!(a, b);
        assert_eq!(a, (3, 36));
        // The anchor selects the same function.
        assert_eq!(anchor("m.ts@3:36").unwrap(), a);
    }

    #[test]
    fn methods_returns_and_properties_resolve() {
        assert_eq!(anchor("m.ts#makeHandler/return").unwrap(), (10, 10));
        assert_eq!(anchor("m.ts#OrderService/method[place]").unwrap().0, 13);
        assert_eq!(anchor("m.ts#OrderService/static[create]").unwrap().0, 14);
        assert_eq!(anchor("m.ts#handlers/property[create]").unwrap().0, 16);
        assert_eq!(anchor("m.ts#handlers/property[update]").unwrap().0, 16);
    }

    #[test]
    fn unresolvable_steps_are_errors_not_guesses() {
        assert!(
            anchor("m.ts#action/arg[2]")
                .unwrap_err()
                .contains("no argument 2")
        );
        assert!(
            anchor("m.ts#action/arg[0]")
                .unwrap_err()
                .contains("not a function")
        );
        assert!(anchor("m.ts#OrderService/method[create]").is_err());
        assert!(anchor("m.ts#missing").is_err());
        assert!(anchor("m.ts@99:1").is_err());
    }
}

//! How a higher-order wrapper uses a callback.
//!
//! `validatedActionWithUser(schema, async (data, _, user) => ...)` passes a
//! callback to a wrapper. Passing a callback does not mean it runs: the
//! wrapper may call it right away, return a function that calls it (maybe
//! only after some checks), store it, or never call it. This module reads
//! the wrapper's source (when it is analyzed) and reports, without
//! guessing, which of these it is. It is deliberately bounded: one level of
//! returned function, direct calls of the parameter only; every other use
//! of the parameter makes the result `unknown`.

use oxc_ast::ast::{
    Argument, ArrowFunctionBody, BindingPattern, Expression, FormalParameters, FunctionBody,
    Statement,
};
use oxc_ast_visit::Visit;
use oxc_span::{GetSpan, Span};

use midwrite_core::location::LineIndex;
use midwrite_model::provider::WrapperFacts;

/// Where the analyzed wrapper's source is.
pub(crate) struct Source<'s> {
    pub rel: &'s str,
    pub text: &'s str,
    pub lines: &'s LineIndex<'s>,
}

impl Source<'_> {
    fn at(&self, span: Span) -> String {
        let (line, column) = self.lines.position(span.start);
        format!("{}:{line}:{column}", self.rel)
    }
    fn excerpt(&self, span: Span) -> String {
        let end = (span.end as usize).min(self.text.len());
        let start = (span.start as usize).min(end);
        let mut text: String = self.text[start..end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if text.chars().count() > 70 {
            text = text.chars().take(67).collect::<String>() + "...";
        }
        text
    }
}

pub(crate) fn unknown(wrapper: &str, detail: impl Into<String>) -> WrapperFacts {
    WrapperFacts {
        wrapper: wrapper.to_owned(),
        defined_at: None,
        invocation: "unknown".to_owned(),
        conditional: true,
        guards: Vec::new(),
        arguments: Vec::new(),
        detail: detail.into(),
    }
}

/// A call of the callback parameter.
struct Call {
    span: Span,
    args: Vec<String>,
    /// Inside a function nested in the wrapper (0 = the wrapper's body).
    depth: u32,
    awaited: bool,
    returned: bool,
}

struct Uses<'n> {
    name: &'n str,
    depth: u32,
    references: Vec<Span>,
    calls: Vec<Call>,
    callee_spans: Vec<Span>,
    shadowed: bool,
    // Span of the call that is the operand of the enclosing await/return.
    awaited_call: Vec<Span>,
    returned_call: Vec<Span>,
    source: &'n str,
}

impl<'a> Visit<'a> for Uses<'_> {
    fn visit_identifier_reference(&mut self, it: &oxc_ast::ast::IdentifierReference<'a>) {
        if it.name.as_str() == self.name {
            self.references.push(it.span);
        }
    }
    fn visit_binding_identifier(&mut self, it: &oxc_ast::ast::BindingIdentifier<'a>) {
        if it.name.as_str() == self.name {
            self.shadowed = true;
        }
    }
    fn visit_await_expression(&mut self, it: &oxc_ast::ast::AwaitExpression<'a>) {
        self.awaited_call
            .push(it.argument.get_inner_expression().span());
        oxc_ast_visit::walk::walk_await_expression(self, it);
    }
    fn visit_return_statement(&mut self, it: &oxc_ast::ast::ReturnStatement<'a>) {
        if let Some(argument) = &it.argument {
            self.returned_call
                .push(argument.get_inner_expression().span());
        }
        oxc_ast_visit::walk::walk_return_statement(self, it);
    }
    fn visit_call_expression(&mut self, it: &oxc_ast::ast::CallExpression<'a>) {
        if let Expression::Identifier(id) = it.callee.get_inner_expression()
            && id.name.as_str() == self.name
        {
            let args = it
                .arguments
                .iter()
                .map(|a| match a {
                    Argument::SpreadElement(s) => {
                        format!("...{}", text(self.source, s.argument.span()))
                    }
                    other => text(
                        self.source,
                        other.as_expression().map_or(other.span(), GetSpan::span),
                    ),
                })
                .collect();
            self.callee_spans.push(id.span);
            self.calls.push(Call {
                span: it.span,
                args,
                depth: self.depth,
                awaited: self.awaited_call.contains(&it.span),
                returned: self.returned_call.contains(&it.span),
            });
        }
        oxc_ast_visit::walk::walk_call_expression(self, it);
    }
    fn visit_function(
        &mut self,
        it: &oxc_ast::ast::Function<'a>,
        flags: oxc_syntax::scope::ScopeFlags,
    ) {
        self.depth += 1;
        oxc_ast_visit::walk::walk_function(self, it, flags);
        self.depth -= 1;
    }
    fn visit_arrow_function_expression(&mut self, it: &oxc_ast::ast::ArrowFunctionExpression<'a>) {
        self.depth += 1;
        // A concise body returns its expression.
        if let Some(e) = it.body.as_expression() {
            self.returned_call.push(e.get_inner_expression().span());
        }
        oxc_ast_visit::walk::walk_arrow_function_expression(self, it);
        self.depth -= 1;
    }
}

fn text(source: &str, span: Span) -> String {
    let end = (span.end as usize).min(source.len());
    let start = (span.start as usize).min(end);
    source[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Exits (`return`/`throw`) anywhere inside a statement.
fn exits(statement: &Statement<'_>) -> bool {
    match statement {
        Statement::ReturnStatement(_) | Statement::ThrowStatement(_) => true,
        Statement::BlockStatement(b) => b.body.iter().any(exits),
        Statement::IfStatement(i) => {
            exits(&i.consequent) || i.alternate.as_ref().is_some_and(exits)
        }
        Statement::TryStatement(t) => {
            t.block.body.iter().any(exits)
                || t.handler
                    .as_ref()
                    .is_some_and(|h| h.body.body.iter().any(exits))
                || t.finalizer
                    .as_ref()
                    .is_some_and(|f| f.body.iter().any(exits))
        }
        _ => false,
    }
}

fn exit_kind(statement: &Statement<'_>) -> &'static str {
    fn find(s: &Statement<'_>) -> Option<&'static str> {
        match s {
            Statement::ReturnStatement(_) => Some("return"),
            Statement::ThrowStatement(_) => Some("throw"),
            Statement::BlockStatement(b) => b.body.iter().find_map(find),
            Statement::IfStatement(i) => {
                find(&i.consequent).or_else(|| i.alternate.as_ref().and_then(find))
            }
            _ => None,
        }
    }
    find(statement).unwrap_or("exit")
}

/// The statements of the function the call is in, when that function is
/// the one `return`ed by the wrapper (or the wrapper itself for depth 0).
fn invoking_body<'a>(body: &'a FunctionBody<'a>, depth: u32) -> Option<&'a FunctionBody<'a>> {
    if depth == 0 {
        return Some(body);
    }
    if depth != 1 {
        return None;
    }
    // `return async (...) => { ... }` / `return function (...) { ... }`.
    for statement in &body.statements {
        if let Statement::ReturnStatement(r) = statement
            && let Some(argument) = &r.argument
        {
            match argument.get_inner_expression() {
                Expression::ArrowFunctionExpression(a) => {
                    return match &a.body {
                        ArrowFunctionBody::FunctionBody(b) => Some(b),
                        _ => None,
                    };
                }
                Expression::FunctionExpression(f) => return f.body.as_deref(),
                _ => {}
            }
        }
    }
    None
}

/// Analyze how the wrapper (parameters `params`, body `body`) uses its
/// parameter `index`.
pub(crate) fn analyze(
    wrapper: &str,
    params: &FormalParameters<'_>,
    body: &FunctionBody<'_>,
    index: usize,
    source: &Source<'_>,
    defined: Span,
) -> WrapperFacts {
    let mut facts = unknown(wrapper, "");
    facts.defined_at = Some(source.at(defined));
    let Some(item) = params.items.get(index) else {
        facts.detail = format!("the wrapper has no parameter {index}");
        return facts;
    };
    let BindingPattern::BindingIdentifier(id) = &item.pattern else {
        facts.detail = format!("parameter {index} of the wrapper is destructured");
        return facts;
    };
    let name = id.name.as_str();
    let mut uses = Uses {
        name,
        depth: 0,
        references: Vec::new(),
        calls: Vec::new(),
        callee_spans: Vec::new(),
        shadowed: false,
        awaited_call: Vec::new(),
        returned_call: Vec::new(),
        source: source.text,
    };
    for statement in &body.statements {
        uses.visit_statement(statement);
    }
    if uses.shadowed {
        facts.detail = format!("`{name}` is redeclared inside the wrapper");
        return facts;
    }
    if uses.references.is_empty() {
        facts.invocation = "not-invoked".to_owned();
        facts.conditional = false;
        facts.detail = format!("the wrapper never references `{name}`");
        return facts;
    }
    let other: Vec<&Span> = uses
        .references
        .iter()
        .filter(|r| !uses.callee_spans.contains(r))
        .collect();
    if let Some(first) = other.first() {
        facts.invocation = "stored".to_owned();
        facts.detail = format!(
            "`{name}` is used other than by calling it at {} (`{}`): it may be stored or passed on",
            source.at(**first),
            source.excerpt(**first)
        );
        return facts;
    }
    let [call] = uses.calls.as_slice() else {
        facts.detail = format!("`{name}` is called {} times", uses.calls.len());
        return facts;
    };
    facts.arguments.clone_from(&call.args);
    let Some(invoking) = invoking_body(body, call.depth) else {
        facts.detail = format!(
            "`{name}` is called at {} inside a function that is not the one the wrapper returns",
            source.at(call.span)
        );
        return facts;
    };
    facts.invocation = match (call.depth, call.awaited, call.returned) {
        (0, true, _) | (0, _, true) => "invoked-during-wrap",
        (0, false, false) => "invoked-during-wrap-unawaited",
        (_, true, _) => "returned-function-awaits",
        (_, false, true) => "returned-function-returns-promise",
        (_, false, false) => "returned-function-fire-and-forget",
    }
    .to_owned();
    // Guards: earlier top-level statements of the invoking function that
    // can exit, and whether the call is nested in a conditional.
    let mut conditional = false;
    for statement in &invoking.statements {
        let span = statement.span();
        if span.start <= call.span.start && call.span.end <= span.end {
            let direct = matches!(
                statement,
                Statement::ReturnStatement(_)
                    | Statement::ExpressionStatement(_)
                    | Statement::VariableDeclaration(_)
            );
            if !direct {
                conditional = true;
                facts.guards.push(format!(
                    "the call is nested in `{}` ({})",
                    source.excerpt(span),
                    source.at(span)
                ));
            }
            break;
        }
        if exits(statement) {
            conditional = true;
            let shown = match statement {
                Statement::IfStatement(i) => format!("if ({})", source.excerpt(i.test.span())),
                other => source.excerpt(other.span()),
            };
            facts.guards.push(format!(
                "`{shown}` → {} ({})",
                exit_kind(statement),
                source.at(span)
            ));
        } else if contains_await(statement) {
            // Awaited work before the call can throw (a rejected promise).
            facts.guards.push(format!(
                "`{}` may reject before the call ({})",
                source.excerpt(span),
                source.at(span)
            ));
            conditional = true;
        }
    }
    facts.conditional = conditional;
    facts
}

fn contains_await(statement: &Statement<'_>) -> bool {
    struct Finder(bool);
    impl<'a> Visit<'a> for Finder {
        fn visit_await_expression(&mut self, _: &oxc_ast::ast::AwaitExpression<'a>) {
            self.0 = true;
        }
        fn visit_function(
            &mut self,
            _: &oxc_ast::ast::Function<'a>,
            _: oxc_syntax::scope::ScopeFlags,
        ) {
        }
        fn visit_arrow_function_expression(
            &mut self,
            _: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
        }
    }
    let mut finder = Finder(false);
    finder.visit_statement(statement);
    finder.0
}

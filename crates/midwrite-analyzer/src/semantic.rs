//! Shared semantic facts of TypeScript source (Milestone 1).
//!
//! Midwrite reads TypeScript in three places: the static analyzer's
//! lowering to Effect IR (`lower.rs`, `recognize.rs`), the concurrency
//! extractor's lowering to the model's IR (`concurrency.rs`), and the
//! Drizzle schema extractor (`schema.rs`). Before Phase 8 each decomposed
//! builder chains and read property keys and member paths on its own. This
//! module holds the facts they now share — *recognition*, not execution
//! semantics: each consumer still decides what a fact means for its own IR
//! (a sequential failure path or a concurrent state transition).
//!
//! Facts:
//!
//! * [`unwind`]: a call/member chain (`db.update(t).set({...}).where(...)`)
//!   as its root (identifier or `this`) and the property/call segments,
//!   looking through parentheses, TypeScript assertions and optional
//!   chaining (recorded in [`Chain::optional`] so that a consumer that does
//!   not model optional chaining can refuse it);
//! * [`Chain::method_calls`]: the same chain grouped into method calls,
//!   each with the member path before it (`db.query.users.findFirst(...)`
//!   → path `query.users`, method `findFirst`);
//! * [`property_key`]: the static name of an object property key;
//! * [`member_path`]: the dotted text of an identifier/static-member chain.
//!
//! Migration status and the differential evidence: `docs/architecture/source-extraction.md`.

use oxc_ast::ast::{
    Argument, CallExpression, ChainElement, Expression, IdentifierReference, PropertyKey,
    ThisExpression,
};

/// One step of a call chain, from the root outward.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Segment<'e, 'a> {
    Property(&'a str),
    Call(&'e CallExpression<'a>),
}

/// The expression a call chain is built on.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Root<'e, 'a> {
    Identifier(&'e IdentifierReference<'a>),
    This(&'e ThisExpression),
}

impl Root<'_, '_> {
    pub(crate) fn start(&self) -> u32 {
        match self {
            Self::Identifier(ident) => ident.span.start,
            Self::This(this) => this.span.start,
        }
    }

    /// The root as written: an identifier or `this`.
    pub(crate) fn written(&self) -> &str {
        match self {
            Self::Identifier(ident) => ident.name.as_str(),
            Self::This(_) => "this",
        }
    }
}

#[derive(Debug)]
pub(crate) struct Chain<'e, 'a> {
    pub root: Option<Root<'e, 'a>>,
    pub segments: Vec<Segment<'e, 'a>>,
    /// An optional chain (`a?.b()`) was looked through.
    pub optional: bool,
}

/// A method call of a chain with the member path before it.
#[derive(Clone, Debug)]
pub(crate) struct MethodCall<'e, 'a> {
    pub path: Vec<&'a str>,
    pub method: &'a str,
    pub arguments: &'e [Argument<'a>],
}

impl<'e, 'a> Chain<'e, 'a> {
    /// The root identifier's name (not `this`).
    pub(crate) fn root_name(&self) -> Option<&'a str> {
        match self.root? {
            Root::Identifier(ident) => Some(ident.name.as_str()),
            Root::This(_) => None,
        }
    }

    /// The call of the root itself (`pgTable(...)` in `pgTable(...).x()`).
    pub(crate) fn root_call(&self) -> Option<&'e CallExpression<'a>> {
        match self.segments.first()? {
            Segment::Call(call) => Some(call),
            Segment::Property(_) => None,
        }
    }

    /// The method calls after the root (and after the root's own call, if
    /// the root is called directly), each with the member path before it.
    /// `None` when the chain ends in a member access, not a call.
    pub(crate) fn method_calls(&self) -> Option<Vec<MethodCall<'e, 'a>>> {
        let skip = usize::from(self.root_call().is_some());
        let mut out = Vec::new();
        let mut pending: Vec<&'a str> = Vec::new();
        for segment in &self.segments[skip..] {
            match segment {
                Segment::Property(name) => pending.push(name),
                Segment::Call(call) => {
                    let method = pending.pop()?;
                    out.push(MethodCall {
                        path: std::mem::take(&mut pending),
                        method,
                        arguments: &call.arguments,
                    });
                }
            }
        }
        pending.is_empty().then_some(out)
    }
}

/// Unwind `expr` into root and segments. Parentheses, TypeScript
/// assertions (`as`, `!`, `satisfies`) and optional chaining are looked
/// through.
pub(crate) fn unwind<'e, 'a>(expr: &'e Expression<'a>) -> Chain<'e, 'a> {
    let mut segments = Vec::new();
    let mut optional = false;
    let mut current = expr;
    let root = loop {
        current = current.get_inner_expression();
        match current {
            Expression::CallExpression(call) => {
                segments.push(Segment::Call(call));
                current = &call.callee;
            }
            Expression::StaticMemberExpression(member) => {
                segments.push(Segment::Property(member.property.name.as_str()));
                current = &member.object;
            }
            Expression::ChainExpression(chain) => {
                optional = true;
                match &chain.expression {
                    ChainElement::CallExpression(call) => {
                        segments.push(Segment::Call(call));
                        current = &call.callee;
                    }
                    ChainElement::StaticMemberExpression(member) => {
                        segments.push(Segment::Property(member.property.name.as_str()));
                        current = &member.object;
                    }
                    ChainElement::TSNonNullExpression(inner) => current = &inner.expression,
                    _ => break None,
                }
            }
            Expression::Identifier(ident) => break Some(Root::Identifier(ident)),
            Expression::ThisExpression(this) => break Some(Root::This(this)),
            _ => break None,
        }
    };
    segments.reverse();
    Chain {
        root,
        segments,
        optional,
    }
}

/// The static name of a property key (`a`, `"a"`); `None` for computed
/// keys and for string keys that are not valid UTF-8 text (an unpaired
/// surrogate).
pub(crate) fn property_key(key: &PropertyKey<'_>) -> Option<String> {
    match key {
        PropertyKey::StaticIdentifier(id) => Some(id.name.as_str().to_owned()),
        PropertyKey::StringLiteral(s) => s.value.as_str().map(str::to_owned),
        _ => None,
    }
}

/// `a`, `a.b.c` for identifier/static-member chains (parentheses and
/// TypeScript assertions looked through).
pub(crate) fn member_path(e: &Expression<'_>) -> Option<String> {
    match e.get_inner_expression() {
        Expression::Identifier(id) => Some(id.name.as_str().to_owned()),
        Expression::StaticMemberExpression(member) => {
            let base = member_path(&member.object)?;
            Some(format!("{base}.{}", member.property.name))
        }
        _ => None,
    }
}

/// The text of a string literal (`None` for an unpaired surrogate).
pub(crate) fn string_value(e: &Expression<'_>) -> Option<String> {
    match e.get_inner_expression() {
        Expression::StringLiteral(s) => s.value.as_str().map(str::to_owned),
        Expression::TemplateLiteral(t) if t.expressions.is_empty() => t
            .quasis
            .first()
            .and_then(|q| q.value.cooked.as_ref())
            .and_then(|c| c.as_str().map(str::to_owned)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use oxc_allocator::Allocator;
    use oxc_ast::ast::{Expression, Statement};
    use oxc_parser::Parser;
    use oxc_span::SourceType;

    use super::*;

    fn with_expression(source: &str, f: impl FnOnce(&Expression<'_>)) {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
        let Some(Statement::ExpressionStatement(s)) = parsed.program.body.first() else {
            panic!("not an expression statement: {source}");
        };
        f(&s.expression);
    }

    fn calls(source: &str) -> Option<Vec<(String, String, usize)>> {
        let mut out = None;
        with_expression(source, |e| {
            out = unwind(e).method_calls().map(|calls| {
                calls
                    .iter()
                    .map(|c| (c.path.join("."), c.method.to_owned(), c.arguments.len()))
                    .collect()
            });
        });
        out
    }

    #[test]
    fn chains_are_grouped_into_method_calls_with_their_member_path() {
        assert_eq!(
            calls("db.query.users.findFirst({ where: x })"),
            Some(vec![("query.users".into(), "findFirst".into(), 1)])
        );
        assert_eq!(
            calls("db.update(t).set({ a: 1 }).where(eq(t.id, 1))"),
            Some(vec![
                (String::new(), "update".into(), 1),
                (String::new(), "set".into(), 1),
                (String::new(), "where".into(), 1),
            ])
        );
        // Ends in a member access: not a call chain.
        assert_eq!(calls("db.select().from"), None);
        // A directly called root is the root call, not a method.
        with_expression("pgTable('t', {}).enableRLS()", |e| {
            let chain = unwind(e);
            assert_eq!(chain.root_name(), Some("pgTable"));
            assert_eq!(chain.root_call().unwrap().arguments.len(), 2);
            assert_eq!(chain.method_calls().unwrap()[0].method, "enableRLS");
        });
    }

    #[test]
    fn parentheses_assertions_and_optional_chains_are_seen_through_and_recorded() {
        with_expression("((db as any)!).insert(t).values(v)", |e| {
            let chain = unwind(e);
            assert_eq!(chain.root_name(), Some("db"));
            assert!(!chain.optional);
            assert_eq!(chain.method_calls().unwrap().len(), 2);
        });
        with_expression("db?.insert(t).values(v)", |e| {
            let chain = unwind(e);
            assert_eq!(chain.root_name(), Some("db"));
            // Recorded, so that a consumer that does not model optional
            // chaining (the concurrency extractor) can refuse it.
            assert!(chain.optional);
        });
        with_expression("this.db.insert(t)", |e| {
            let chain = unwind(e);
            assert_eq!(chain.root_name(), None);
            assert!(matches!(chain.root, Some(Root::This(_))));
        });
    }

    #[test]
    fn keys_and_paths() {
        with_expression("({ a: 1, 'b-c': 2, [k]: 3 })", |e| {
            let Expression::ObjectExpression(o) = e.get_inner_expression() else {
                panic!()
            };
            let keys: Vec<Option<String>> = o
                .properties
                .iter()
                .map(|p| match p {
                    oxc_ast::ast::ObjectPropertyKind::ObjectProperty(p) => property_key(&p.key),
                    _ => None,
                })
                .collect();
            assert_eq!(keys, [Some("a".into()), Some("b-c".into()), None]);
        });
        with_expression("(a.b as X).c", |e| {
            assert_eq!(member_path(e).as_deref(), Some("a.b.c"))
        });
        with_expression("a[b].c", |e| assert_eq!(member_path(e), None));
        with_expression("`plain`", |e| {
            assert_eq!(string_value(e).as_deref(), Some("plain"))
        });
        with_expression("`x${y}`", |e| assert_eq!(string_value(e), None));
    }
}

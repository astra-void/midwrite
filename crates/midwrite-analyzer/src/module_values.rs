//! Stable module initializers are value providers, never executed entry points.
use oxc_ast::ast::{BindingPattern, Declaration, Expression, Program, Statement};
use oxc_semantic::Scoping;
use oxc_span::GetSpan;
use oxc_syntax::symbol::SymbolId;
use std::collections::HashMap;

pub(crate) const INITIALIZER_BIT: u32 = 0x8000_0000;
pub(crate) fn initializer_id(expression: &Expression<'_>) -> u32 {
    expression.span().start | INITIALIZER_BIT
}

pub(crate) fn eligible(expression: &Expression<'_>) -> bool {
    match expression.get_inner_expression() {
        Expression::CallExpression(_) | Expression::Identifier(_) => true,
        Expression::AwaitExpression(a) => eligible(&a.argument),
        _ => false,
    }
}

pub(crate) fn collect(program: &Program<'_>, scoping: &Scoping) -> HashMap<SymbolId, u32> {
    let mut values = HashMap::new();
    for statement in &program.body {
        let declaration = match statement {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            other => other.as_declaration(),
        };
        if let Some(Declaration::VariableDeclaration(d)) = declaration
            .filter(|d| matches!(d, Declaration::VariableDeclaration(d) if d.kind.is_const()))
        {
            for declarator in &d.declarations {
                if let (BindingPattern::BindingIdentifier(id), Some(init)) =
                    (&declarator.id, &declarator.init)
                    && let Some(symbol) = id.symbol_id.get()
                    && !scoping.symbol_is_mutated(symbol)
                    && eligible(init)
                {
                    values.insert(symbol, initializer_id(init));
                }
            }
        }
    }
    values
}

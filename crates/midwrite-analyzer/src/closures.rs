//! Lexical capture layouts, independent of the values at a creation site.

use crate::bindings::resolve_reference;
use oxc_ast::ast::{ArrowFunctionExpression, Function, IdentifierReference, Program};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::Scoping;
use oxc_span::Span;
use oxc_syntax::{scope::ScopeFlags, symbol::SymbolId};
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Debug, Default)]
pub(crate) struct Layout {
    pub parameters: u32,
    pub symbols: Vec<SymbolId>,
}

pub(crate) fn collect(program: &Program<'_>, scoping: &Scoping) -> HashMap<u32, Layout> {
    struct Finder<'s> {
        scoping: &'s Scoping,
        stack: Vec<(Span, u32, BTreeSet<SymbolId>)>,
        layouts: HashMap<u32, Layout>,
    }
    impl Finder<'_> {
        fn finish(&mut self) {
            let (span, parameters, symbols) = self.stack.pop().unwrap();
            self.layouts.insert(
                span.start,
                Layout {
                    parameters,
                    symbols: symbols.into_iter().collect(),
                },
            );
        }
    }
    impl<'a> Visit<'a> for Finder<'_> {
        fn visit_function(&mut self, f: &Function<'a>, flags: ScopeFlags) {
            self.stack
                .push((f.span, f.params.items.len() as u32, BTreeSet::new()));
            walk::walk_function(self, f, flags);
            self.finish();
        }
        fn visit_arrow_function_expression(&mut self, f: &ArrowFunctionExpression<'a>) {
            self.stack
                .push((f.span, f.params.items.len() as u32, BTreeSet::new()));
            walk::walk_arrow_function_expression(self, f);
            self.finish();
        }
        fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
            let Some(symbol) = resolve_reference(self.scoping, ident) else {
                return;
            };
            if self
                .scoping
                .scope_parent_id(self.scoping.symbol_scope_id(symbol))
                .is_none()
                || self.scoping.symbol_is_mutated(symbol)
            {
                return;
            }
            let declared = self.scoping.symbol_span(symbol);
            for (span, _, captures) in &mut self.stack {
                if declared.start < span.start || declared.end > span.end {
                    captures.insert(symbol);
                }
            }
        }
    }
    let mut finder = Finder {
        scoping,
        stack: Vec::new(),
        layouts: HashMap::new(),
    };
    finder.visit_program(program);
    finder.layouts
}

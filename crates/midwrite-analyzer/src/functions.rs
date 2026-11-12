//! Function extraction: finds every function-like node with a body, names it,
//! and lowers it to IR.

use std::collections::HashSet;

use oxc_ast::ast::{
    ArrowFunctionExpression, BindingPattern, Class, ExportDefaultDeclaration,
    ExportDefaultDeclarationKind, Expression, FormalParameters, Function, MethodDefinition,
    ObjectProperty, PropertyDefinition, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::Span;
use oxc_syntax::scope::ScopeFlags;

use std::collections::HashMap;

use midwrite_core::ir::FunctionIr;
use midwrite_core::location::LineIndex;

use crate::lower::{Body, CompactFacts, FactTable, LocalCallSite, ThisClass, lower_function};
use crate::recognize::Recognizer;
use crate::values::ValueStats;

/// What [`FunctionFinder::finish`] returns: the functions, their calls,
/// their summary facts and fact table, and each function's index by start
/// offset.
pub(crate) type Finished = (
    Vec<FunctionIr>,
    Vec<Vec<LocalCallSite>>,
    Vec<CompactFacts>,
    FactTable,
    HashMap<u32, usize>,
);

pub(crate) struct FunctionFinder<'c> {
    recognizer: &'c Recognizer<'c>,
    lines: &'c LineIndex<'c>,
    functions: Vec<FunctionIr>,
    /// Start offset of each function in `functions`.
    starts: Vec<u32>,
    /// Calls of same-file functions made by each function in `functions`.
    local_calls: Vec<Vec<LocalCallSite>>,
    /// Callbacks already lowered inline into an enclosing function.
    consumed: HashSet<u32>,
    /// Names inferred from the syntactic context, keyed by function start.
    names: Vec<(u32, String)>,
    classes: Vec<Option<String>>,
    /// Names of the functions enclosing the current position.
    enclosing: Vec<String>,
    /// Summary facts of each function in `functions`.
    facts: Vec<CompactFacts>,
    /// Targets and labels of the facts.
    table: FactTable,
    pub stats: ValueStats,
    /// Start offsets of the classes enclosing the current position.
    class_starts: Vec<u32>,
    /// `this` of methods and arrow-valued fields, by function start.
    pending_this: HashMap<u32, ThisClass>,
    /// `this` of the functions enclosing the current position (arrows
    /// inherit it).
    this_stack: Vec<Option<ThisClass>>,
}

impl<'c> FunctionFinder<'c> {
    pub fn new(recognizer: &'c Recognizer<'c>, lines: &'c LineIndex<'c>) -> Self {
        Self {
            recognizer,
            lines,
            functions: Vec::new(),
            starts: Vec::new(),
            local_calls: Vec::new(),
            consumed: HashSet::new(),
            names: Vec::new(),
            classes: Vec::new(),
            enclosing: Vec::new(),
            facts: Vec::new(),
            table: FactTable::default(),
            stats: ValueStats::default(),
            class_starts: Vec::new(),
            pending_this: HashMap::new(),
            this_stack: Vec::new(),
        }
    }

    /// Remember `name` for `value` if it is a function expression.
    fn name_value(&mut self, value: &Expression<'_>, name: String) {
        match value.get_inner_expression() {
            Expression::ArrowFunctionExpression(function) => {
                self.names.push((function.span.start, name));
            }
            Expression::FunctionExpression(function) if function.id.is_none() => {
                self.names.push((function.span.start, name));
            }
            _ => {}
        }
    }

    fn qualified(&self, name: &str) -> String {
        match self.classes.last() {
            Some(Some(class)) => format!("{class}.{name}"),
            _ => name.to_owned(),
        }
    }

    fn name_for(&mut self, span: Span, own: Option<&str>) -> String {
        if let Some(own) = own {
            return own.to_owned();
        }
        if let Some(index) = self
            .names
            .iter()
            .position(|(start, _)| *start == span.start)
        {
            return self.names.swap_remove(index).1;
        }
        let (line, column) = self.lines.position(span.start);
        match self.enclosing.last() {
            Some(parent) => format!("{parent}::<anonymous@{line}:{column}>"),
            None => format!("<anonymous@{line}:{column}>"),
        }
    }

    /// Lower a function unless it was consumed; returns whether a new
    /// enclosing name was pushed.
    #[allow(clippy::too_many_arguments)]
    fn lower(
        &mut self,
        name: String,
        span: Span,
        is_async: bool,
        is_generator: bool,
        arrow: bool,
        params: &FormalParameters<'_>,
        body: Body<'_, '_>,
    ) -> bool {
        if self.consumed.contains(&span.start) {
            return false;
        }
        let this_class = match self.pending_this.remove(&span.start) {
            Some(this) => Some(this),
            // Arrows inherit `this`; other functions have their own.
            None if arrow => self.this_stack.last().copied().flatten(),
            None => None,
        };
        let lowered = lower_function(
            self.recognizer,
            self.lines,
            name.clone(),
            span,
            is_async,
            is_generator,
            this_class,
            Some(params),
            body,
        );
        self.consumed.extend(lowered.consumed);
        self.functions.push(lowered.ir);
        self.starts.push(span.start);
        let mut local_calls = lowered.local_calls;
        for site in &mut local_calls {
            site.values.shrink_to_fit();
            site.arguments.shrink_to_fit();
        }
        local_calls.shrink_to_fit();
        self.local_calls.push(local_calls);
        let facts = self
            .table
            .compact(lowered.facts, self.recognizer.config.closed_world);
        self.facts.push(facts);
        self.stats.add(&lowered.stats);
        self.enclosing.push(name);
        self.this_stack.push(this_class);
        true
    }

    /// The lowered functions, in source order, the calls each makes of
    /// same-file or imported functions, and the index of each function by
    /// its start offset.
    pub fn finish(mut self) -> Finished {
        let index: HashMap<u32, usize> = self
            .starts
            .iter()
            .enumerate()
            .map(|(position, start)| (*start, position))
            .collect();
        self.table.finish();
        (
            self.functions,
            self.local_calls,
            self.facts,
            self.table,
            index,
        )
    }
}

impl<'a> Visit<'a> for FunctionFinder<'_> {
    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let (BindingPattern::BindingIdentifier(id), Some(init)) = (&it.id, &it.init) {
            self.name_value(init, id.name.as_str().to_owned());
        }
        walk::walk_variable_declarator(self, it);
    }

    fn visit_object_property(&mut self, it: &ObjectProperty<'a>) {
        if let Some(key) = it.key.static_name() {
            self.name_value(&it.value, key.display().to_string());
        }
        walk::walk_object_property(self, it);
    }

    fn visit_property_definition(&mut self, it: &PropertyDefinition<'a>) {
        if let (Some(value), Some(key)) = (&it.value, it.key.static_name()) {
            let name = self.qualified(&key.display().to_string());
            self.name_value(value, name);
        }
        if let (Some(value), Some(class)) = (&it.value, self.class_starts.last()) {
            let start = match value.get_inner_expression() {
                Expression::ArrowFunctionExpression(function) => Some(function.span.start),
                Expression::FunctionExpression(function) => Some(function.span.start),
                _ => None,
            };
            if let Some(start) = start {
                self.pending_this.insert(
                    start,
                    ThisClass {
                        class: *class,
                        is_static: it.r#static,
                    },
                );
            }
        }
        walk::walk_property_definition(self, it);
    }

    fn visit_method_definition(&mut self, it: &MethodDefinition<'a>) {
        if let Some(key) = it.key.static_name() {
            let name = self.qualified(&key.display().to_string());
            self.names.push((it.value.span.start, name));
        }
        if let Some(class) = self.class_starts.last() {
            self.pending_this.insert(
                it.value.span.start,
                ThisClass {
                    class: *class,
                    is_static: it.r#static,
                },
            );
        }
        walk::walk_method_definition(self, it);
    }

    fn visit_class(&mut self, it: &Class<'a>) {
        self.classes
            .push(it.id.as_ref().map(|id| id.name.as_str().to_owned()));
        self.class_starts.push(it.span.start);
        walk::walk_class(self, it);
        self.class_starts.pop();
        self.classes.pop();
    }

    fn visit_export_default_declaration(&mut self, it: &ExportDefaultDeclaration<'a>) {
        match &it.declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(function)
                if function.id.is_none() =>
            {
                self.names.push((function.span.start, "default".to_owned()));
            }
            declaration => {
                if let Some(expression) = declaration.as_expression() {
                    self.name_value(expression, "default".to_owned());
                }
            }
        }
        walk::walk_export_default_declaration(self, it);
    }

    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        let pushed = match &it.body {
            Some(body) => {
                let name = self.name_for(it.span, it.id.as_ref().map(|id| id.name.as_str()));
                self.lower(
                    name,
                    it.span,
                    it.r#async,
                    it.generator,
                    false,
                    &it.params,
                    Body::Statements(&body.statements),
                )
            }
            None => false,
        };
        walk::walk_function(self, it, flags);
        if pushed {
            self.enclosing.pop();
            self.this_stack.pop();
        }
    }

    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        let name = self.name_for(it.span, None);
        let pushed = self.lower(
            name,
            it.span,
            it.r#async,
            false,
            true,
            &it.params,
            Body::Arrow(&it.body),
        );
        walk::walk_arrow_function_expression(self, it);
        if pushed {
            self.enclosing.pop();
            self.this_stack.pop();
        }
    }
}

//! Abstract values while lowering a function body.
//!
//! See `docs/phase5/ABSTRACT-DOMAIN.md` and `HEAP-AND-ALIASES.md`. The
//! [`Lowerer`](crate::lower::Lowerer) carries a [`ValueState`] (environment
//! and heap) through the statements it lowers, in source order. These are
//! its transfer functions: evaluating an expression to an abstract value,
//! binding patterns, and collecting the provenance of an effect's
//! arguments. Evaluation never emits IR; the values of calls are recorded
//! when the calls are lowered, before the expressions that use them are
//! evaluated.

use oxc_ast::ast::{
    ArrayExpression, ArrayExpressionElement, BindingPattern, ChainElement, Expression,
    FormalParameters, IdentifierReference, ObjectExpression, ObjectPropertyKind, PropertyKind,
};
use oxc_syntax::symbol::SymbolId;

use midwrite_core::absint::{AbsValue, Atom, Env, Heap, ObjectKind, Provenance};
use midwrite_core::ir::Literal;

use crate::bindings::{literal_value, resolve_reference};
use crate::lower::Lowerer;

/// Kleene iterations of a loop body before its changed bindings are widened.
pub(crate) const MAX_LOOP_ITERATIONS: usize = 3;
/// Value-only re-lowerings of loop bodies per function; past it, loop
/// state is havocked (every binding `Top`, every object escaped).
pub(crate) const MAX_DRY_RUNS: usize = 48;

/// Imported classes are numbered in instance objects by their import
/// binding with this bit set (local classes by their start offset).
pub(crate) const IMPORTED_CLASS: u32 = 0x8000_0000;

/// The abstract state at a program point. Copy-on-write: forking a state
/// at a branch shares both parts until one side changes them, and joining
/// a state with an unchanged copy of itself is free.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ValueState {
    pub env: std::rc::Rc<Env<u32>>,
    pub heap: std::rc::Rc<Heap>,
}

/// Counters of the value analysis of one function.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValueStats {
    /// Abstract states created at control-flow forks.
    pub states: usize,
    /// Joins of abstract states.
    pub joins: usize,
    /// Bindings widened to `Top` by joins or loop widening.
    pub widenings: usize,
    /// Kleene iterations over loop bodies.
    pub loop_iterations: usize,
    /// Loops whose state was widened before converging.
    pub loops_widened: usize,
    /// Loops whose state was havocked because the budget ran out.
    pub loops_havocked: usize,
    /// Value-only re-lowerings of loop bodies.
    pub dry_runs: usize,
}

impl ValueStats {
    pub fn add(&mut self, other: &ValueStats) {
        self.states += other.states;
        self.joins += other.joins;
        self.widenings += other.widenings;
        self.loop_iterations += other.loop_iterations;
        self.loops_widened += other.loops_widened;
        self.loops_havocked += other.loops_havocked;
        self.dry_runs += other.dry_runs;
    }
}

impl ValueState {
    pub fn env_mut(&mut self) -> &mut Env<u32> {
        std::rc::Rc::make_mut(&mut self.env)
    }

    pub fn heap_mut(&mut self) -> &mut Heap {
        std::rc::Rc::make_mut(&mut self.heap)
    }

    /// Mark the objects of `value` escaped (no copy when it holds none).
    pub fn escape(&mut self, value: &AbsValue) {
        if value
            .atoms()
            .iter()
            .any(|atom| matches!(atom, Atom::Object { .. }))
        {
            self.heap_mut().escape(value);
        }
    }

    pub fn join(&mut self, other: &ValueState, stats: &mut ValueStats) {
        stats.joins += 1;
        if !std::rc::Rc::ptr_eq(&self.env, &other.env) && *self.env != *other.env {
            let joined = self.env_mut().join(&other.env);
            stats.widenings += joined.widened;
        }
        if !std::rc::Rc::ptr_eq(&self.heap, &other.heap) && *self.heap != *other.heap {
            self.heap_mut().join(&other.heap);
        }
    }

    /// `self ⊑ other` on what exists in `other` (the loop-head order:
    /// bindings and objects created inside a loop body are fresh on every
    /// iteration).
    pub fn leq_on_existing(&self, other: &ValueState) -> bool {
        self.env.leq_on_existing(&other.env) && self.heap.leq_on_existing(&other.heap)
    }

    /// Loop widening: bindings and objects that changed since `previous`
    /// become `Top`.
    pub fn widen_from(&mut self, previous: &ValueState, stats: &mut ValueStats) {
        let previous_env = std::rc::Rc::clone(&previous.env);
        stats.widenings += self.env_mut().widen_from(&previous_env);
        let previous_heap = std::rc::Rc::clone(&previous.heap);
        self.heap_mut().widen_from(&previous_heap);
    }

    /// Forget everything (fixpoint budget exhausted).
    pub fn havoc(&mut self) {
        let keys: Vec<u32> = self.env.iter().map(|(key, _)| *key).collect();
        let env = self.env_mut();
        for key in keys {
            env.set(key, AbsValue::top());
        }
        self.heap_mut().havoc();
    }
}

pub(crate) fn key(symbol: SymbolId) -> u32 {
    u32::try_from(symbol.index()).unwrap_or(u32::MAX)
}

impl<'a> Lowerer<'_> {
    /// Bind the parameters: `param#i` (destructured parameters bind
    /// property paths of it).
    pub(crate) fn bind_parameters(&mut self, params: &FormalParameters<'a>) {
        for (index, param) in params.items.iter().enumerate() {
            let index = u32::try_from(index).unwrap_or(u32::MAX);
            let mut value = AbsValue::atom(Atom::Param { index });
            if let Some(initializer) = &param.initializer {
                value.join(&self.eval(initializer));
            }
            self.bind_pattern(&param.pattern, value);
        }
        if let Some(rest) = &params.rest {
            self.bind_pattern(&rest.rest.argument, AbsValue::top());
        }
    }

    /// Bind every identifier of `pattern` to the corresponding part of
    /// `value` (strong update: each binding is one variable).
    pub(crate) fn bind_pattern(&mut self, pattern: &BindingPattern<'a>, value: AbsValue) {
        match pattern {
            BindingPattern::BindingIdentifier(id) => {
                if let Some(symbol) = id.symbol_id.get() {
                    self.vs.env_mut().set(key(symbol), value);
                }
            }
            BindingPattern::ObjectPattern(object) => {
                for property in &object.properties {
                    let part = match property.key.static_name().filter(|_| !property.computed) {
                        Some(name) => self.vs.heap.read(&value, &name.display().to_string()),
                        None => AbsValue::top(),
                    };
                    self.bind_pattern(&property.value, part);
                }
                if let Some(rest) = &object.rest {
                    self.bind_pattern(&rest.argument, AbsValue::top());
                }
            }
            BindingPattern::ArrayPattern(array) => {
                for (index, element) in array.elements.iter().enumerate() {
                    if let Some(element) = element {
                        let part = self.vs.heap.read(&value, &index.to_string());
                        self.bind_pattern(element, part);
                    }
                }
                if let Some(rest) = &array.rest {
                    self.bind_pattern(&rest.argument, AbsValue::top());
                }
            }
            BindingPattern::AssignmentPattern(assignment) => {
                let mut joined = value;
                joined.join(&self.eval(&assignment.right));
                self.bind_pattern(&assignment.left, joined);
            }
        }
    }

    /// Set every identifier of `pattern` to `Top` (destructuring
    /// assignments are not tracked).
    pub(crate) fn havoc_pattern(&mut self, pattern: &BindingPattern<'a>) {
        self.bind_pattern_top(pattern);
    }

    fn bind_pattern_top(&mut self, pattern: &BindingPattern<'a>) {
        match pattern {
            BindingPattern::BindingIdentifier(id) => {
                if let Some(symbol) = id.symbol_id.get() {
                    self.vs.env_mut().set(key(symbol), AbsValue::top());
                }
            }
            BindingPattern::ObjectPattern(object) => {
                for property in &object.properties {
                    self.bind_pattern_top(&property.value);
                }
                if let Some(rest) = &object.rest {
                    self.bind_pattern_top(&rest.argument);
                }
            }
            BindingPattern::ArrayPattern(array) => {
                for element in array.elements.iter().flatten() {
                    self.bind_pattern_top(element);
                }
                if let Some(rest) = &array.rest {
                    self.bind_pattern_top(&rest.argument);
                }
            }
            BindingPattern::AssignmentPattern(assignment) => {
                self.bind_pattern_top(&assignment.left)
            }
        }
    }

    /// The abstract value of an identifier.
    pub(crate) fn eval_ident(&self, ident: &IdentifierReference<'_>) -> AbsValue {
        let scoping = self.recognizer.scoping;
        let table = self.recognizer.table;
        let Some(symbol) = resolve_reference(scoping, ident) else {
            return if ident.name == "undefined" {
                AbsValue::lit(Literal::Undefined)
            } else {
                AbsValue::top()
            };
        };
        if table.captured_write(symbol) {
            return AbsValue::top();
        }
        if let Some(value) = self.vs.env.get(&key(symbol)) {
            return value.clone();
        }
        if let Some(start) = table.local_function(symbol) {
            return AbsValue::atom(Atom::Function { id: start });
        }
        if let Some(start) = table.class_of(symbol) {
            return AbsValue::atom(Atom::Class { id: start });
        }
        if let Some(literal) = table.literal(symbol) {
            return AbsValue::lit(literal.clone());
        }
        if table.import(symbol).is_some() || self.is_module_level(symbol) {
            return AbsValue::atom(Atom::Static {
                symbol: key(symbol),
            });
        }
        // A variable of an enclosing function, or not assigned yet.
        AbsValue::top()
    }

    pub(crate) fn is_module_level(&self, symbol: SymbolId) -> bool {
        let scoping = self.recognizer.scoping;
        scoping
            .scope_parent_id(scoping.symbol_scope_id(symbol))
            .is_none()
    }

    /// The abstract value of an expression. Calls must have been lowered
    /// already (their values are looked up, not computed).
    pub(crate) fn eval(&mut self, expression: &Expression<'a>) -> AbsValue {
        let expression = expression.get_inner_expression();
        if let Some(literal) = literal_value(expression) {
            return AbsValue::lit(literal);
        }
        match expression {
            Expression::Identifier(ident) => self.eval_ident(ident),
            Expression::TemplateLiteral(template) if template.expressions.is_empty() => template
                .quasis
                .first()
                .and_then(|quasi| quasi.value.cooked.as_ref())
                .and_then(|cooked| cooked.as_str())
                .map_or_else(AbsValue::top, |text| {
                    AbsValue::lit(Literal::String(text.to_owned()))
                }),
            Expression::ObjectExpression(object) => self.eval_object(object),
            Expression::ArrayExpression(array) => self.eval_array(array),
            Expression::ArrowFunctionExpression(function) => AbsValue::atom(Atom::Function {
                id: function.span.start,
            }),
            Expression::FunctionExpression(function) => AbsValue::atom(Atom::Function {
                id: function.span.start,
            }),
            Expression::StaticMemberExpression(member) => {
                let base = self.eval(&member.object);
                self.vs.heap.read(&base, member.property.name.as_str())
            }
            Expression::ComputedMemberExpression(member) => {
                let name = match literal_value(&member.expression) {
                    Some(Literal::String(name)) => Some(name),
                    Some(Literal::Number(number)) => Some(number),
                    _ => None,
                };
                let base = self.eval(&member.object);
                match name {
                    Some(name) => self.vs.heap.read(&base, &name),
                    None => AbsValue::top(),
                }
            }
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => self.call_value(call.span.start),
                ChainElement::StaticMemberExpression(member) => {
                    let base = self.eval(&member.object);
                    let mut value = self.vs.heap.read(&base, member.property.name.as_str());
                    value.join(&AbsValue::lit(Literal::Undefined));
                    value
                }
                _ => AbsValue::top(),
            },
            Expression::CallExpression(call) => self.call_value(call.span.start),
            Expression::NewExpression(new) => self.call_value(new.span.start),
            Expression::AwaitExpression(awaited) => self.eval(&awaited.argument),
            Expression::ConditionalExpression(conditional) => {
                let mut value = self.eval(&conditional.consequent);
                value.join(&self.eval(&conditional.alternate));
                value
            }
            Expression::LogicalExpression(logical) => {
                let mut value = self.eval(&logical.left);
                value.join(&self.eval(&logical.right));
                value
            }
            Expression::SequenceExpression(sequence) => match sequence.expressions.last() {
                Some(last) => self.eval(last),
                None => AbsValue::top(),
            },
            Expression::AssignmentExpression(assignment) => self
                .assigned
                .get(&assignment.span.start)
                .cloned()
                .unwrap_or_else(AbsValue::top),
            Expression::ThisExpression(_) if self.this_class.is_some() => {
                AbsValue::atom(Atom::This)
            }
            _ => AbsValue::top(),
        }
    }

    pub(crate) fn call_value(&self, start: u32) -> AbsValue {
        self.call_values
            .get(&start)
            .cloned()
            .unwrap_or_else(AbsValue::top)
    }

    fn eval_object(&mut self, object: &ObjectExpression<'a>) -> AbsValue {
        let site = object.span.start;
        let atom = self.vs.heap_mut().allocate(site, ObjectKind::Plain);
        let target = AbsValue::atom(atom.clone());
        for property in &object.properties {
            match property {
                ObjectPropertyKind::ObjectProperty(property) => {
                    let name = property
                        .key
                        .static_name()
                        .filter(|_| !property.computed)
                        .map(|name| name.display().to_string());
                    match (name, property.kind) {
                        (Some(name), PropertyKind::Init) => {
                            let value = self.eval(&property.value);
                            self.vs.heap_mut().init(site, &name, value);
                        }
                        // Accessors and computed keys: reads are unknown.
                        (Some(name), _) => {
                            self.vs.heap_mut().init(site, &name, AbsValue::top());
                            self.vs.heap_mut().open(site);
                        }
                        (None, _) => {
                            let value = self.eval(&property.value);
                            self.vs.escape(&value);
                            self.vs.heap_mut().write_unknown(&target);
                        }
                    }
                }
                ObjectPropertyKind::SpreadProperty(spread) => {
                    let source = self.eval(&spread.argument);
                    if !self.copy_properties(site, &source) {
                        self.vs.heap_mut().write_unknown(&target);
                    }
                }
            }
        }
        AbsValue::atom(atom)
    }

    /// `{ ...source }` into the object at `site`, when every property of
    /// `source` is known. Returns `false` otherwise.
    fn copy_properties(&mut self, site: u32, source: &AbsValue) -> bool {
        let props: Vec<(String, AbsValue)> = match source.single() {
            Some(Atom::Object { id }) => match self.vs.heap.get(*id) {
                Some(object) if !object.open && !object.escaped => object
                    .props
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.clone()))
                    .collect(),
                _ => return false,
            },
            Some(Atom::Record { props, open: false }) => props
                .iter()
                .map(|(name, value)| (name.to_string(), value.clone()))
                .collect(),
            _ => return false,
        };
        for (name, value) in props {
            self.vs.heap_mut().init(site, &name, value);
        }
        true
    }

    fn eval_array(&mut self, array: &ArrayExpression<'a>) -> AbsValue {
        let site = array.span.start;
        let atom = self.vs.heap_mut().allocate(site, ObjectKind::Array);
        let mut index = 0usize;
        for element in &array.elements {
            match element {
                ArrayExpressionElement::SpreadElement(spread) => {
                    let value = self.eval(&spread.argument);
                    self.vs.escape(&value);
                    self.vs
                        .heap_mut()
                        .write_unknown(&AbsValue::atom(atom.clone()));
                    // Later indices are unknown.
                    return AbsValue::atom(atom);
                }
                ArrayExpressionElement::Elision(_) => index += 1,
                element => {
                    if let Some(expression) = element.as_expression() {
                        let value = self.eval(expression);
                        self.vs.heap_mut().init(site, &index.to_string(), value);
                    }
                    index += 1;
                }
            }
        }
        AbsValue::atom(atom)
    }

    /// The provenance of an operation's arguments: the abstract value of
    /// each argument (exact), plus the values used inside computations in
    /// them (derived). `skip` names the argument that is a table, which is
    /// static schema data.
    pub(crate) fn provenance(
        &mut self,
        calls: &[&oxc_ast::ast::CallExpression<'a>],
        skip: Option<(u32, usize)>,
    ) -> Provenance {
        let mut provenance = Provenance::default();
        for call in calls {
            for (index, argument) in call.arguments.iter().enumerate() {
                if skip == Some((call.span.end, index)) {
                    continue;
                }
                let Some(expression) = argument.as_expression() else {
                    // A spread argument: unknown contents.
                    provenance.unknown = true;
                    continue;
                };
                let value = self.eval(expression);
                let exported = self.vs.heap.export(&value);
                provenance.add(&exported, true);
                self.derived_inputs(expression, &mut provenance, 0);
            }
        }
        provenance
    }

    /// Values used inside `expression` (identifiers and member paths below
    /// computations), as derived inputs.
    fn derived_inputs(
        &mut self,
        expression: &Expression<'a>,
        provenance: &mut Provenance,
        depth: usize,
    ) {
        if depth > 6 {
            return;
        }
        let expression = expression.get_inner_expression();
        match expression {
            Expression::Identifier(_) | Expression::StaticMemberExpression(_) => {
                if depth > 0 {
                    let value = self.eval(expression);
                    provenance.add(&value, false);
                }
            }
            Expression::TemplateLiteral(template) => {
                for inner in &template.expressions {
                    self.derived_inputs(inner, provenance, depth + 1);
                }
            }
            Expression::BinaryExpression(binary) => {
                self.derived_inputs(&binary.left, provenance, depth + 1);
                self.derived_inputs(&binary.right, provenance, depth + 1);
            }
            Expression::LogicalExpression(logical) => {
                self.derived_inputs(&logical.left, provenance, depth + 1);
                self.derived_inputs(&logical.right, provenance, depth + 1);
            }
            Expression::ConditionalExpression(conditional) => {
                self.derived_inputs(&conditional.consequent, provenance, depth + 1);
                self.derived_inputs(&conditional.alternate, provenance, depth + 1);
            }
            Expression::UnaryExpression(unary) => {
                self.derived_inputs(&unary.argument, provenance, depth + 1);
            }
            Expression::AwaitExpression(awaited) => {
                self.derived_inputs(&awaited.argument, provenance, depth);
            }
            Expression::CallExpression(call) => {
                for argument in &call.arguments {
                    if let Some(inner) = argument.as_expression() {
                        self.derived_inputs(inner, provenance, depth + 1);
                    }
                }
            }
            Expression::ObjectExpression(object) => {
                for property in &object.properties {
                    match property {
                        ObjectPropertyKind::ObjectProperty(property) => {
                            // The property value itself is exact (exported
                            // record); look inside it for derived values.
                            if !matches!(
                                property.value.get_inner_expression(),
                                Expression::Identifier(_) | Expression::StaticMemberExpression(_)
                            ) {
                                self.derived_inputs(&property.value, provenance, depth + 1);
                            }
                        }
                        ObjectPropertyKind::SpreadProperty(spread) => {
                            self.derived_inputs(&spread.argument, provenance, depth + 1);
                        }
                    }
                }
            }
            Expression::ArrayExpression(array) => {
                for element in &array.elements {
                    if let Some(inner) = element.as_expression()
                        && !matches!(
                            inner.get_inner_expression(),
                            Expression::Identifier(_) | Expression::StaticMemberExpression(_)
                        )
                    {
                        self.derived_inputs(inner, provenance, depth + 1);
                    }
                }
            }
            _ => {}
        }
    }
}

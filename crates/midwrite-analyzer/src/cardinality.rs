//! Correlate native-array cardinality guards on immutable, unexposed values.
//!
//! A complementary partition does not increase length; a nonempty slice from `n`
//! rules out `n > original.length`. These implications concern the existing
//! native-array/primitive-value subset, not user-defined coercions or methods.

use std::collections::{HashMap, HashSet};

use oxc_ast::ast::*;
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::Scoping;
use oxc_syntax::{
    operator::{BinaryOperator, UnaryOperator},
    reference::ReferenceId,
    scope::ScopeFlags,
    symbol::SymbolId,
};

use crate::bindings::{literal_value, resolve_reference};
use midwrite_core::ir::{Fact, Literal};

#[derive(Clone, Debug)]
enum Shape {
    Array,
    Alias(SymbolId),
    Partition(SymbolId),
    Tail { source: SymbolId, start: SymbolId },
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Predicate {
    path: Vec<String>,
    negated: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Cardinality {
    shapes: HashMap<SymbolId, Shape>,
    readonly: HashSet<SymbolId>,
    stable: HashSet<SymbolId>,
    objects: HashSet<SymbolId>,
}

fn ident(expression: &Expression<'_>, scoping: &Scoping) -> Option<SymbolId> {
    let Expression::Identifier(id) = expression.get_inner_expression() else {
        return None;
    };
    resolve_reference(scoping, id)
}

fn predicate(
    params: &FormalParameters<'_>,
    body: &Expression<'_>,
    scoping: &Scoping,
) -> Option<Predicate> {
    let [param] = params.items.as_slice() else {
        return None;
    };
    if params.rest.is_some() || param.initializer.is_some() {
        return None;
    }
    let BindingPattern::BindingIdentifier(id) = &param.pattern else {
        return None;
    };
    let symbol = id.symbol_id.get()?;
    if scoping.symbol_is_mutated(symbol) {
        return None;
    }
    fn path(expr: &Expression<'_>, symbol: SymbolId, scoping: &Scoping) -> Option<Vec<String>> {
        match expr.get_inner_expression() {
            Expression::Identifier(id) if resolve_reference(scoping, id) == Some(symbol) => {
                Some(Vec::new())
            }
            Expression::StaticMemberExpression(m) => {
                let mut p = path(&m.object, symbol, scoping)?;
                p.push(m.property.name.to_string());
                Some(p)
            }
            _ => None,
        }
    }
    let mut expression = body.get_inner_expression();
    let mut negated = false;
    while let Expression::UnaryExpression(u) = expression {
        if u.operator != UnaryOperator::LogicalNot {
            return None;
        }
        negated = !negated;
        expression = u.argument.get_inner_expression();
    }
    Some(Predicate {
        path: path(expression, symbol, scoping)?,
        negated,
    })
}

fn returned<'e, 'a>(statements: &'e [Statement<'a>]) -> Option<&'e Expression<'a>> {
    let [Statement::ReturnStatement(ret)] = statements else {
        return None;
    };
    ret.argument.as_ref()
}

fn callback_predicate(
    expression: &Expression<'_>,
    scoping: &Scoping,
    known: &HashMap<SymbolId, Predicate>,
) -> Option<Predicate> {
    match expression.get_inner_expression() {
        Expression::Identifier(id) => known.get(&resolve_reference(scoping, id)?).cloned(),
        Expression::ArrowFunctionExpression(f) if !f.r#async => {
            let body = match &f.body {
                ArrowFunctionBody::FunctionBody(body) => returned(&body.statements)?,
                body => body.as_expression()?,
            };
            predicate(&f.params, body, scoping)
        }
        Expression::FunctionExpression(f) if !f.r#async && !f.generator => {
            predicate(&f.params, returned(&f.body.as_ref()?.statements)?, scoping)
        }
        _ => None,
    }
}

impl Cardinality {
    pub fn collect(program: &Program<'_>, scoping: &Scoping) -> Self {
        struct Predicates<'s> {
            scoping: &'s Scoping,
            known: HashMap<SymbolId, Predicate>,
        }
        impl<'a> Visit<'a> for Predicates<'_> {
            fn visit_function(&mut self, f: &Function<'a>, flags: ScopeFlags) {
                if !f.r#async
                    && !f.generator
                    && let Some(symbol) = f.id.as_ref().and_then(|id| id.symbol_id.get())
                    && !self.scoping.symbol_is_mutated(symbol)
                    && let Some(body) = f.body.as_ref().and_then(|b| returned(&b.statements))
                    && let Some(p) = predicate(&f.params, body, self.scoping)
                {
                    self.known.insert(symbol, p);
                }
                walk::walk_function(self, f, flags);
            }
            fn visit_variable_declarator(&mut self, d: &VariableDeclarator<'a>) {
                if let (BindingPattern::BindingIdentifier(id), Some(init)) = (&d.id, &d.init)
                    && let Some(symbol) = id.symbol_id.get()
                    && !self.scoping.symbol_is_mutated(symbol)
                    && let Some(p) = callback_predicate(init, self.scoping, &self.known)
                {
                    self.known.insert(symbol, p);
                }
                walk::walk_variable_declarator(self, d);
            }
        }
        let mut predicates = Predicates {
            scoping,
            known: HashMap::new(),
        };
        predicates.visit_program(program);

        struct Collector<'s> {
            scoping: &'s Scoping,
            predicates: HashMap<SymbolId, Predicate>,
            shapes: HashMap<SymbolId, Shape>,
            allowed: HashSet<ReferenceId>,
            aliases: HashMap<SymbolId, Vec<SymbolId>>,
            objects: HashSet<SymbolId>,
            denied: HashSet<ReferenceId>,
            unsafe_environment: bool,
        }
        impl Collector<'_> {
            fn allow(&mut self, expression: &Expression<'_>) {
                if let Expression::Identifier(id) = expression.get_inner_expression() {
                    self.allowed.insert(id.reference_id());
                }
            }
            fn filter(&self, expression: &Expression<'_>) -> Option<(SymbolId, Predicate)> {
                let Expression::CallExpression(call) = expression.get_inner_expression() else {
                    return None;
                };
                let Expression::StaticMemberExpression(m) = call.callee.get_inner_expression()
                else {
                    return None;
                };
                if m.property.name != "filter" || call.optional || call.arguments.len() != 1 {
                    return None;
                }
                Some((
                    ident(&m.object, self.scoping)?,
                    callback_predicate(
                        call.arguments[0].as_expression()?,
                        self.scoping,
                        &self.predicates,
                    )?,
                ))
            }
            fn shape(&self, expression: &Expression<'_>) -> Shape {
                if let Some(symbol) = ident(expression, self.scoping) {
                    return Shape::Alias(symbol);
                }
                if let Expression::ArrayExpression(array) = expression.get_inner_expression()
                    && let [
                        ArrayExpressionElement::SpreadElement(a),
                        ArrayExpressionElement::SpreadElement(b),
                    ] = array.elements.as_slice()
                    && let (Some((a, pa)), Some((b, pb))) =
                        (self.filter(&a.argument), self.filter(&b.argument))
                    && a == b
                    && pa.path == pb.path
                    && pa.negated != pb.negated
                {
                    return Shape::Partition(a);
                }
                if matches!(
                    expression.get_inner_expression(),
                    Expression::ArrayExpression(_)
                ) {
                    return Shape::Array;
                }
                if let Expression::CallExpression(call) = expression.get_inner_expression()
                    && !call.optional
                    && call.arguments.len() == 1
                    && let Expression::StaticMemberExpression(m) =
                        call.callee.get_inner_expression()
                    && m.property.name == "slice"
                    && let Some(source) = ident(&m.object, self.scoping)
                    && let Some(start) = call.arguments[0]
                        .as_expression()
                        .and_then(|a| ident(a, self.scoping))
                {
                    return Shape::Tail { source, start };
                }
                Shape::Other
            }
            fn callback_safe(&self, expression: &Expression<'_>) -> bool {
                match expression.get_inner_expression() {
                    Expression::ArrowFunctionExpression(f) => {
                        f.params.items.len() <= 2 && f.params.rest.is_none()
                    }
                    // A named/simple function must be a known pure predicate:
                    // `arguments[2]` and rest could expose/mutate the array.
                    expression => {
                        callback_predicate(expression, self.scoping, &self.predicates).is_some()
                    }
                }
            }
        }
        impl<'a> Visit<'a> for Collector<'_> {
            fn visit_function(&mut self, function: &Function<'a>, flags: ScopeFlags) {
                if let Some(symbol) = function.id.as_ref().and_then(|id| id.symbol_id.get()) {
                    self.objects.insert(symbol);
                }
                walk::walk_function(self, function, flags);
            }
            fn visit_class(&mut self, class: &Class<'a>) {
                if let Some(symbol) = class.id.as_ref().and_then(|id| id.symbol_id.get()) {
                    self.objects.insert(symbol);
                }
                walk::walk_class(self, class);
            }
            fn visit_identifier_reference(&mut self, id: &IdentifierReference<'a>) {
                if matches!(id.name.as_str(), "arguments" | "eval") {
                    self.unsafe_environment = true;
                }
            }
            fn visit_object_property(&mut self, property: &ObjectProperty<'a>) {
                if property.computed
                    || property.kind != PropertyKind::Init
                    || property
                        .key
                        .static_name()
                        .is_some_and(|key| key.display().to_string() == "valueOf")
                {
                    self.unsafe_environment = true;
                }
                walk::walk_object_property(self, property);
            }
            fn visit_method_definition(&mut self, method: &MethodDefinition<'a>) {
                if method.computed
                    || matches!(
                        method.kind,
                        MethodDefinitionKind::Get | MethodDefinitionKind::Set
                    )
                    || method
                        .key
                        .static_name()
                        .is_some_and(|key| key.display().to_string() == "valueOf")
                {
                    self.unsafe_environment = true;
                }
                walk::walk_method_definition(self, method);
            }
            fn visit_assignment_expression(&mut self, assignment: &AssignmentExpression<'a>) {
                struct Targets<'s>(&'s mut HashSet<ReferenceId>);
                impl<'a> Visit<'a> for Targets<'_> {
                    fn visit_identifier_reference(&mut self, id: &IdentifierReference<'a>) {
                        self.0.insert(id.reference_id());
                    }
                }
                Targets(&mut self.denied).visit_assignment_target(&assignment.left);
                walk::walk_assignment_expression(self, assignment);
            }
            fn visit_update_expression(&mut self, update: &UpdateExpression<'a>) {
                if let SimpleAssignmentTarget::StaticMemberExpression(member) = &update.argument
                    && let Expression::Identifier(id) = member.object.get_inner_expression()
                {
                    self.denied.insert(id.reference_id());
                }
                walk::walk_update_expression(self, update);
            }
            fn visit_variable_declaration(&mut self, d: &VariableDeclaration<'a>) {
                if d.kind.is_const() {
                    for d in &d.declarations {
                        if let (BindingPattern::BindingIdentifier(id), Some(init)) =
                            (&d.id, &d.init)
                            && let Some(symbol) = id.symbol_id.get()
                        {
                            let shape = self.shape(init);
                            if let Shape::Alias(source) = shape {
                                self.aliases.entry(source).or_default().push(symbol);
                                self.allow(init);
                            }
                            if matches!(
                                init.get_inner_expression(),
                                Expression::ObjectExpression(_)
                                    | Expression::NewExpression(_)
                                    | Expression::FunctionExpression(_)
                                    | Expression::ArrowFunctionExpression(_)
                            ) {
                                self.objects.insert(symbol);
                            }
                            self.shapes.insert(symbol, shape);
                        }
                    }
                }
                walk::walk_variable_declaration(self, d);
            }
            fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
                if !call.optional
                    && let Expression::StaticMemberExpression(m) =
                        call.callee.get_inner_expression()
                {
                    let method = m.property.name.as_str();
                    let safe = match method {
                        "slice" => {
                            call.arguments.len() <= 2
                                && call
                                    .arguments
                                    .iter()
                                    .all(|a| !matches!(a, Argument::SpreadElement(_)))
                        }
                        "filter" | "map" | "forEach" | "some" | "every" => {
                            call.arguments.len() == 1
                                && call
                                    .arguments
                                    .first()
                                    .and_then(Argument::as_expression)
                                    .is_some_and(|e| self.callback_safe(e))
                        }
                        _ => false,
                    };
                    if safe {
                        self.allow(&m.object);
                    }
                }
                walk::walk_call_expression(self, call);
            }
            fn visit_static_member_expression(&mut self, member: &StaticMemberExpression<'a>) {
                if member.property.name == "length" {
                    self.allow(&member.object);
                }
                if member.property.name == "prototype"
                    && matches!(member.object.get_inner_expression(), Expression::Identifier(id) if id.name == "Array")
                {
                    self.unsafe_environment = true;
                }
                walk::walk_static_member_expression(self, member);
            }
        }
        let mut collector = Collector {
            scoping,
            predicates: predicates.known,
            shapes: HashMap::new(),
            allowed: HashSet::new(),
            aliases: HashMap::new(),
            objects: HashSet::new(),
            denied: HashSet::new(),
            unsafe_environment: false,
        };
        collector.visit_program(program);
        if collector.unsafe_environment {
            return Self::default();
        }
        let mut stable = HashSet::new();
        let mut readonly = HashSet::new();
        fn readonly_array(
            symbol: SymbolId,
            scoping: &Scoping,
            c: &Collector<'_>,
            stack: &mut Vec<SymbolId>,
        ) -> bool {
            if stack.len() >= 8
                || stack.contains(&symbol)
                || scoping.symbol_is_mutated(symbol)
                || scoping.symbol_flags(symbol).is_import()
                || scoping
                    .scope_parent_id(scoping.symbol_scope_id(symbol))
                    .is_none()
                || c.objects.contains(&symbol)
            {
                return false;
            }
            stack.push(symbol);
            let result = scoping
                .get_resolved_reference_ids(symbol)
                .iter()
                .all(|r| c.allowed.contains(r) && !c.denied.contains(r))
                && c.aliases.get(&symbol).is_none_or(|aliases| {
                    aliases
                        .iter()
                        .all(|alias| readonly_array(*alias, scoping, c, stack))
                });
            stack.pop();
            result
        }
        for symbol in scoping.symbol_ids() {
            if !scoping.symbol_is_mutated(symbol)
                && !scoping.symbol_flags(symbol).is_import()
                && scoping
                    .get_resolved_reference_ids(symbol)
                    .iter()
                    .all(|r| !collector.denied.contains(r))
            {
                stable.insert(symbol);
            }
            if readonly_array(symbol, scoping, &collector, &mut Vec::new()) {
                readonly.insert(symbol);
            }
        }
        Self {
            shapes: collector.shapes,
            readonly,
            stable,
            objects: collector.objects,
        }
    }

    fn base(&self, symbol: SymbolId, depth: usize) -> Option<SymbolId> {
        if depth >= 8 || !self.readonly.contains(&symbol) {
            return None;
        }
        match self.shapes.get(&symbol) {
            Some(Shape::Alias(source)) => self.base(*source, depth + 1),
            Some(Shape::Partition(source)) => {
                self.base(*source, depth + 1)?;
                Some(symbol)
            }
            Some(Shape::Tail { .. }) => None,
            _ => Some(symbol),
        }
    }

    fn bounds(&self, symbol: SymbolId) -> Vec<SymbolId> {
        let mut out = Vec::new();
        let mut current = self.base(symbol, 0);
        while let Some(symbol) = current {
            if out.len() >= 8 || out.contains(&symbol) {
                break;
            }
            out.push(symbol);
            current = match self.shapes.get(&symbol) {
                Some(Shape::Partition(source)) => self.base(*source, 0),
                _ => None,
            };
        }
        out
    }

    fn tail(&self, symbol: SymbolId, depth: usize) -> Option<(SymbolId, SymbolId)> {
        if depth >= 8 || !self.readonly.contains(&symbol) {
            return None;
        }
        match self.shapes.get(&symbol)? {
            Shape::Alias(source) => self.tail(*source, depth + 1),
            Shape::Tail { source, start } => Some((self.base(*source, 0)?, *start)),
            _ => None,
        }
    }

    fn scalar(&self, symbol: SymbolId, depth: usize) -> Option<SymbolId> {
        if depth >= 8 || !self.stable.contains(&symbol) || self.objects.contains(&symbol) {
            return None;
        }
        match self.shapes.get(&symbol) {
            Some(Shape::Alias(source)) => self.scalar(*source, depth + 1),
            Some(Shape::Array | Shape::Partition(_) | Shape::Tail { .. }) => None,
            _ => Some(symbol),
        }
    }

    fn length(expression: &Expression<'_>, scoping: &Scoping) -> Option<SymbolId> {
        let Expression::StaticMemberExpression(m) = expression.get_inner_expression() else {
            return None;
        };
        (m.property.name == "length")
            .then(|| ident(&m.object, scoping))
            .flatten()
    }

    fn fact(&self, array: SymbolId, count: SymbolId, value: bool) -> Vec<Fact> {
        let Some(count) = self.scalar(count, 0) else {
            return Vec::new();
        };
        vec![Fact::Truthy {
            variable: format!("$cardinality:{}>{}.length", count.index(), array.index()),
            value,
        }]
    }

    fn nonempty(&self, array: SymbolId, count: SymbolId) -> Vec<Fact> {
        self.bounds(array)
            .into_iter()
            .flat_map(|bound| self.fact(bound, count, false))
            .collect()
    }

    /// Sufficient implications only: an empty tail need not mean growth.
    pub fn facts(&self, expression: &Expression<'_>, truthy: bool, scoping: &Scoping) -> Vec<Fact> {
        if truthy
            && let Some(array) = Self::length(expression, scoping)
            && let Some((base, start)) = self.tail(array, 0)
        {
            return self.nonempty(base, start);
        }
        let Expression::BinaryExpression(b) = expression.get_inner_expression() else {
            return Vec::new();
        };
        let (count, array, value) = match b.operator {
            BinaryOperator::GreaterThan => (
                ident(&b.left, scoping),
                Self::length(&b.right, scoping),
                truthy,
            ),
            BinaryOperator::LessThan => (
                ident(&b.right, scoping),
                Self::length(&b.left, scoping),
                truthy,
            ),
            _ => (None, None, false),
        };
        if let (Some(count), Some(array)) = (count, array)
            && let Some(base) = self.base(array, 0)
        {
            return self.fact(base, count, value);
        }
        let nonempty = match (b.operator, truthy) {
            (BinaryOperator::GreaterThan, true)
            | (BinaryOperator::StrictInequality, true)
            | (BinaryOperator::StrictEquality, false)
                if literal_value(&b.right) == Some(Literal::Number("0".into())) =>
            {
                Self::length(&b.left, scoping)
            }
            (BinaryOperator::LessThan, true)
                if literal_value(&b.left) == Some(Literal::Number("0".into())) =>
            {
                Self::length(&b.right, scoping)
            }
            _ => None,
        };
        if let Some((base, start)) = nonempty.and_then(|array| self.tail(array, 0)) {
            self.nonempty(base, start)
        } else {
            Vec::new()
        }
    }
}

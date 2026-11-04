//! Identifier provenance.
//!
//! Recognition never trusts a name alone when the file says otherwise. This
//! module resolves an identifier, using Oxc's semantic analysis, to where its
//! value comes from: an import, a `const` alias of another binding, a
//! `drizzle(...)` call, a transaction callback parameter, or something that
//! clearly is not a database handle (a literal, a `new` expression, ...).

use std::collections::{HashMap, HashSet};

use midwrite_core::ir::Literal;
use oxc_ast::ast::{
    ArrowFunctionExpression, AssignmentExpression, AssignmentTarget, BindingPattern,
    CallExpression, Class, ClassElement, Expression, Function, IdentifierReference,
    ImportDeclaration, ImportDeclarationSpecifier, MethodDefinitionKind, ObjectPropertyKind,
    SimpleAssignmentTarget, StaticMemberExpression, UnaryExpression, UpdateExpression,
    VariableDeclarator,
};
use oxc_syntax::scope::ScopeId;

use crate::classes::{ClassInfo, SuperRef};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::Scoping;
use oxc_syntax::scope::ScopeFlags;
use oxc_syntax::symbol::SymbolId;

use midwrite_core::ir::TransactionId;

/// How a module binding was imported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImportBinding {
    pub module: String,
    /// The exported name for named imports; `None` for default and namespace imports.
    pub imported: Option<String>,
    pub kind: ImportKind,
    /// `import type { ... }` or `import { type X }`: no runtime value.
    pub type_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ImportKind {
    /// `import { name } from` / `import { name as local } from`
    Named(String),
    /// `import local from`
    Default,
    /// `import * as local from`
    Namespace,
}

#[derive(Clone, Debug)]
enum Initializer {
    /// `const a = b`
    Alias {
        target: Option<SymbolId>,
        name: String,
    },
    /// `const a = f(...)`, with the resolved callee identifier (if any).
    Call {
        callee: Option<SymbolId>,
        /// The callee identifier as written, when the callee is an identifier.
        name: Option<String>,
        /// The call has no arguments.
        no_arguments: bool,
    },
    /// An initializer that evidently is not a database or client handle.
    Contrary(&'static str),
    Other,
}

#[derive(Clone, Debug)]
struct Declarator {
    is_const: bool,
    init: Initializer,
}

/// File-level facts about bindings, collected once per file.
#[derive(Debug, Default)]
pub(crate) struct SymbolTable {
    imports: HashMap<SymbolId, ImportBinding>,
    declarators: HashMap<SymbolId, Declarator>,
    /// Functions declared in this file that a plain call can name: function
    /// declarations with a body that are never reassigned, and `const`
    /// bindings initialized with an arrow function or function expression.
    /// Maps the binding to the start offset of the function.
    functions: HashMap<SymbolId, u32>,
    /// `const` bindings initialized with a primitive literal.
    literals: HashMap<SymbolId, Literal>,
    /// `const` bindings of an object literal whose properties are functions
    /// of this file (`const orders = { async markPaid() {}, close: closeOrder }`),
    /// by property name. Objects whose properties are assigned or deleted
    /// anywhere in the file are removed.
    objects: HashMap<SymbolId, HashMap<String, u32>>,
    /// Classes declared in this file, by start offset.
    classes: HashMap<u32, ClassInfo>,
    /// Bindings naming a class of this file (declarations and `const C =
    /// class {}`).
    class_symbols: HashMap<SymbolId, u32>,
    /// References of each binding that are the object of a property read
    /// (`x.p`, not a call `x.p()` and not a write).
    member_reads: HashMap<SymbolId, usize>,
    /// Bindings whose methods are called (`x.p()`): the call may change them.
    member_called: HashSet<SymbolId>,
    /// Bindings assigned from inside a function nested in the one that
    /// declares them: a call may change them at any time.
    captured_writes: HashSet<SymbolId>,
    /// The `resolve`/`reject` parameters of executors passed directly to
    /// `new Promise(...)`: calling them settles that promise and does
    /// nothing else.
    promise_resolvers: HashSet<SymbolId>,
}

impl SymbolTable {
    pub fn collect(program: &oxc_ast::ast::Program<'_>, scoping: &Scoping) -> Self {
        let mut collector = Collector {
            scoping,
            table: Self::default(),
            object_properties: Vec::new(),
            mutated_objects: Vec::new(),
            functions: Vec::new(),
            class_stack: Vec::new(),
            supers: Vec::new(),
            prototype_writes: Vec::new(),
        };
        collector.visit_program(program);
        // Resolve property values that name a function of this file, now
        // that every function is known.
        let mut table = collector.table;
        for (symbol, name, value) in collector.object_properties {
            let start = match value {
                PropertyValue::Function(start) => Some(start),
                PropertyValue::Binding(binding) => table.functions.get(&binding).copied(),
            };
            if let Some(start) = start {
                table.objects.entry(symbol).or_default().insert(name, start);
            }
        }
        for symbol in &collector.mutated_objects {
            table.objects.remove(symbol);
        }
        table
            .member_called
            .extend(collector.mutated_objects.iter().copied());
        // Superclasses named by identifiers, now that every class and
        // import is known.
        for (class, superclass) in collector.supers {
            let resolved = match superclass {
                PendingSuper::Symbol(symbol, name) => {
                    if let Some(start) = table.class_symbols.get(&symbol) {
                        SuperRef::Local(*start)
                    } else if let Some(import) = table.imports.get(&symbol) {
                        match &import.kind {
                            ImportKind::Named(imported) => SuperRef::Imported {
                                module: import.module.clone(),
                                name: imported.clone(),
                            },
                            ImportKind::Default => SuperRef::Imported {
                                module: import.module.clone(),
                                name: "default".to_owned(),
                            },
                            ImportKind::Namespace => SuperRef::Unknown(name),
                        }
                    } else {
                        SuperRef::Unknown(name)
                    }
                }
                PendingSuper::Global(name) if is_builtin_class(&name) => SuperRef::Builtin(name),
                PendingSuper::Global(name) => SuperRef::Unknown(name),
                PendingSuper::Other(text) => SuperRef::Unknown(text),
            };
            if let Some(info) = table.classes.get_mut(&class) {
                info.superclass = Some(resolved);
            }
        }
        for (symbol, name) in collector.prototype_writes {
            if let Some(start) = table.class_symbols.get(&symbol)
                && let Some(info) = table.classes.get_mut(start)
            {
                info.dynamic.insert(name);
            }
        }
        table
    }

    /// The class (start offset) a binding names.
    pub fn class_of(&self, symbol: SymbolId) -> Option<u32> {
        self.class_symbols.get(&symbol).copied()
    }

    pub fn classes(&self) -> &HashMap<u32, ClassInfo> {
        &self.classes
    }

    /// Whether every reference of `symbol` is a property read (`x.p`), the
    /// binding is never written through or method-called: a property's
    /// value cannot change between two reads by any code the analysis
    /// sees. (Getters are excluded separately: facts only come from object
    /// literals.)
    pub fn member_stable(&self, symbol: SymbolId, scoping: &Scoping) -> bool {
        let total = scoping.get_resolved_reference_ids(symbol).len();
        total > 0
            && !self.member_called.contains(&symbol)
            && self.member_reads.get(&symbol).copied().unwrap_or(0) == total
    }

    /// Whether `symbol` is assigned from a nested function.
    pub fn captured_write(&self, symbol: SymbolId) -> bool {
        self.captured_writes.contains(&symbol)
    }

    /// Whether `symbol` is a `resolve`/`reject` parameter of a `Promise`
    /// executor (and is never reassigned).
    pub fn promise_resolver(&self, symbol: SymbolId) -> bool {
        self.promise_resolvers.contains(&symbol)
    }

    /// The start offset of the same-file function `symbol` is bound to.
    pub fn local_function(&self, symbol: SymbolId) -> Option<u32> {
        self.functions.get(&symbol).copied()
    }

    /// How `symbol` was imported, if it is an import binding.
    pub fn import(&self, symbol: SymbolId) -> Option<&ImportBinding> {
        self.imports.get(&symbol)
    }

    /// The literal a `const` binding is initialized with.
    pub fn literal(&self, symbol: SymbolId) -> Option<&Literal> {
        self.literals.get(&symbol)
    }

    /// The function (by start offset) that property `name` of the object
    /// service `symbol` holds.
    pub fn object_method(&self, symbol: SymbolId, name: &str) -> Option<u32> {
        self.objects.get(&symbol)?.get(name).copied()
    }

    /// Every function property of the object service `symbol`.
    pub fn object_methods(&self, symbol: SymbolId) -> Option<&HashMap<String, u32>> {
        self.objects.get(&symbol)
    }
}

/// The literal value of a primitive literal expression.
pub(crate) fn literal_value(expression: &Expression<'_>) -> Option<Literal> {
    match expression.get_inner_expression() {
        Expression::BooleanLiteral(literal) => Some(Literal::Boolean(literal.value)),
        Expression::NullLiteral(_) => Some(Literal::Null),
        Expression::StringLiteral(literal) => {
            Some(Literal::String(literal.value.as_str()?.to_owned()))
        }
        Expression::NumericLiteral(literal) if literal.value.is_finite() => {
            // `0.0` and `0` are the same number; Rust prints both as `0`.
            Some(Literal::Number(format!("{}", literal.value)))
        }
        _ => None,
    }
}

struct Collector<'s> {
    scoping: &'s Scoping,
    table: SymbolTable,
    /// `(object, property, value)` of `const` object literals.
    object_properties: Vec<(SymbolId, String, PropertyValue)>,
    /// Objects with a property assigned or deleted somewhere.
    mutated_objects: Vec<SymbolId>,
    /// Scopes of the functions enclosing the current position.
    functions: Vec<ScopeId>,
    /// Classes enclosing the current position (start offsets).
    class_stack: Vec<u32>,
    supers: Vec<(u32, PendingSuper)>,
    /// `C.prototype.m = ...`: (class binding, method name).
    prototype_writes: Vec<(SymbolId, String)>,
}

enum PendingSuper {
    Symbol(SymbolId, String),
    Global(String),
    Other(String),
}

/// Global constructors whose instances Midwrite treats as plain data.
pub(crate) fn is_builtin_class(name: &str) -> bool {
    matches!(
        name,
        "Error"
            | "TypeError"
            | "RangeError"
            | "SyntaxError"
            | "ReferenceError"
            | "EvalError"
            | "URIError"
            | "AggregateError"
            | "Map"
            | "Set"
            | "WeakMap"
            | "WeakSet"
            | "Array"
            | "Object"
            | "Date"
            | "Promise"
    )
}

enum PropertyValue {
    /// An inline function, by start offset.
    Function(u32),
    /// An identifier, possibly naming a function of the file.
    Binding(SymbolId),
}

impl<'a> Visit<'a> for Collector<'_> {
    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        let module = it.source.value.as_str().unwrap_or_default().to_owned();
        let declaration_type_only = !it.import_kind.is_value();
        for specifier in it.specifiers.iter().flatten() {
            let (local, imported, kind, type_only) = match specifier {
                ImportDeclarationSpecifier::ImportSpecifier(s) => {
                    let name = s.imported.name().as_str().to_owned();
                    (
                        &s.local,
                        Some(name.clone()),
                        ImportKind::Named(name),
                        !s.import_kind.is_value(),
                    )
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                    (&s.local, None, ImportKind::Default, false)
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                    (&s.local, None, ImportKind::Namespace, false)
                }
            };
            if let Some(symbol) = local.symbol_id.get() {
                self.table.imports.insert(
                    symbol,
                    ImportBinding {
                        module: module.clone(),
                        imported,
                        kind,
                        type_only: declaration_type_only || type_only,
                    },
                );
            }
        }
    }

    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let BindingPattern::BindingIdentifier(id) = &it.id
            && let Some(symbol) = id.symbol_id.get()
        {
            let init = match &it.init {
                Some(init) => self.classify(init),
                None => Initializer::Other,
            };
            let is_const = self.scoping.symbol_flags(symbol).is_const_variable();
            if is_const && let Some(literal) = it.init.as_ref().and_then(literal_value) {
                self.table.literals.insert(symbol, literal);
            }
            if is_const
                && let Some(Expression::ObjectExpression(object)) =
                    it.init.as_ref().map(Expression::get_inner_expression)
            {
                for property in &object.properties {
                    let ObjectPropertyKind::ObjectProperty(property) = property else {
                        continue;
                    };
                    let Some(name) = property.key.static_name().filter(|_| !property.computed)
                    else {
                        continue;
                    };
                    let value = match property.value.get_inner_expression() {
                        Expression::ArrowFunctionExpression(function) => {
                            Some(PropertyValue::Function(function.span.start))
                        }
                        Expression::FunctionExpression(function) if function.body.is_some() => {
                            Some(PropertyValue::Function(function.span.start))
                        }
                        Expression::Identifier(ident) => {
                            resolve_reference(self.scoping, ident).map(PropertyValue::Binding)
                        }
                        _ => None,
                    };
                    if let Some(value) = value {
                        self.object_properties
                            .push((symbol, name.display().to_string(), value));
                    }
                }
            }
            if is_const {
                match it.init.as_ref().map(Expression::get_inner_expression) {
                    Some(Expression::ClassExpression(class)) => {
                        self.table.class_symbols.insert(symbol, class.span.start);
                    }
                    Some(Expression::ArrowFunctionExpression(function)) => {
                        self.table.functions.insert(symbol, function.span.start);
                    }
                    Some(Expression::FunctionExpression(function)) if function.body.is_some() => {
                        self.table.functions.insert(symbol, function.span.start);
                    }
                    _ => {}
                }
            }
            self.table
                .declarators
                .insert(symbol, Declarator { is_const, init });
        }
        walk::walk_variable_declarator(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if let AssignmentTarget::AssignmentTargetIdentifier(ident) = &it.left {
            self.note_write(ident);
        }
        if let AssignmentTarget::StaticMemberExpression(member) = &it.left {
            let property = member.property.name.as_str().to_owned();
            match member.object.get_inner_expression() {
                // `this.m = ...` inside a class.
                Expression::ThisExpression(_) => {
                    if let Some(class) = self.class_stack.last()
                        && let Some(info) = self.table.classes.get_mut(class)
                    {
                        info.dynamic.insert(property);
                    }
                }
                // `C.prototype.m = ...`
                Expression::StaticMemberExpression(inner) if inner.property.name == "prototype" => {
                    if let Expression::Identifier(ident) = inner.object.get_inner_expression()
                        && let Some(symbol) = resolve_reference(self.scoping, ident)
                    {
                        self.prototype_writes.push((symbol, property));
                    }
                }
                _ => {}
            }
        }
        let object = match &it.left {
            AssignmentTarget::StaticMemberExpression(member) => Some(&member.object),
            AssignmentTarget::ComputedMemberExpression(member) => Some(&member.object),
            _ => None,
        };
        if let Some(Expression::Identifier(ident)) = object.map(Expression::get_inner_expression)
            && let Some(symbol) = resolve_reference(self.scoping, ident)
        {
            self.mutated_objects.push(symbol);
        }
        walk::walk_assignment_expression(self, it);
    }

    fn visit_unary_expression(&mut self, it: &UnaryExpression<'a>) {
        if it.operator == oxc_syntax::operator::UnaryOperator::Delete
            && let Some(member) = it.argument.get_inner_expression().as_member_expression()
            && let Expression::Identifier(ident) = member.object().get_inner_expression()
            && let Some(symbol) = resolve_reference(self.scoping, ident)
        {
            self.mutated_objects.push(symbol);
        }
        walk::walk_unary_expression(self, it);
    }

    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        if it.is_declaration()
            && it.body.is_some()
            && let Some(symbol) = it.id.as_ref().and_then(|id| id.symbol_id.get())
            && !self.scoping.symbol_is_mutated(symbol)
        {
            self.table.functions.insert(symbol, it.span.start);
        }
        let pushed = it.scope_id.get().map(|scope| self.functions.push(scope));
        walk::walk_function(self, it, flags);
        if pushed.is_some() {
            self.functions.pop();
        }
    }

    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        let pushed = it.scope_id.get().map(|scope| self.functions.push(scope));
        walk::walk_arrow_function_expression(self, it);
        if pushed.is_some() {
            self.functions.pop();
        }
    }

    fn visit_update_expression(&mut self, it: &UpdateExpression<'a>) {
        if let SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) = &it.argument {
            self.note_write(ident);
        }
        walk::walk_update_expression(self, it);
    }

    fn visit_new_expression(&mut self, it: &oxc_ast::ast::NewExpression<'a>) {
        if let Expression::Identifier(callee) = it.callee.get_inner_expression()
            && callee.name == "Promise"
            && resolve_reference(self.scoping, callee).is_none()
        {
            let params = match it
                .arguments
                .first()
                .and_then(|a| a.as_expression())
                .map(Expression::get_inner_expression)
            {
                Some(Expression::ArrowFunctionExpression(function)) => Some(&function.params),
                Some(Expression::FunctionExpression(function)) => Some(&function.params),
                _ => None,
            };
            for param in params.into_iter().flat_map(|p| p.items.iter().take(2)) {
                if let BindingPattern::BindingIdentifier(id) = &param.pattern
                    && let Some(symbol) = id.symbol_id.get()
                    && !self.scoping.symbol_is_mutated(symbol)
                {
                    self.table.promise_resolvers.insert(symbol);
                }
            }
        }
        walk::walk_new_expression(self, it);
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        if let Expression::Identifier(ident) = it.object.get_inner_expression()
            && let Some(symbol) = resolve_reference(self.scoping, ident)
        {
            *self.table.member_reads.entry(symbol).or_default() += 1;
        }
        walk::walk_static_member_expression(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Some(member) = it.callee.get_inner_expression().as_member_expression()
            && let Expression::Identifier(ident) = member.object().get_inner_expression()
        {
            match resolve_reference(self.scoping, ident) {
                Some(symbol) => {
                    self.table.member_called.insert(symbol);
                }
                // `Object.defineProperty(target, ...)` and friends.
                None if ident.name == "Object"
                    && matches!(
                        member
                            .static_property_name()
                            .and_then(|name| name.as_str())
                            .unwrap_or_default(),
                        "defineProperty" | "defineProperties" | "assign" | "setPrototypeOf"
                    ) =>
                {
                    self.note_reflective_target(it);
                }
                None => {}
            }
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_class(&mut self, it: &Class<'a>) {
        let start = it.span.start;
        let mut info = ClassInfo {
            name: it.id.as_ref().map_or_else(
                || "<anonymous class>".to_owned(),
                |id| id.name.as_str().to_owned(),
            ),
            ..ClassInfo::default()
        };
        if let Some(symbol) = it.id.as_ref().and_then(|id| id.symbol_id.get())
            && !self.scoping.symbol_is_mutated(symbol)
        {
            self.table.class_symbols.insert(symbol, start);
        }
        if let Some(heritage) = &it.heritage {
            let pending = match heritage.expression.get_inner_expression() {
                Expression::Identifier(ident) => match resolve_reference(self.scoping, ident) {
                    Some(symbol) => PendingSuper::Symbol(symbol, ident.name.as_str().to_owned()),
                    None => PendingSuper::Global(ident.name.as_str().to_owned()),
                },
                _ => PendingSuper::Other("an expression".to_owned()),
            };
            self.supers.push((start, pending));
        }
        for element in &it.body.body {
            match element {
                ClassElement::MethodDefinition(method) => {
                    let Some(name) = method.key.static_name().filter(|_| !method.computed) else {
                        continue;
                    };
                    let name = name.display().to_string();
                    let function = method.value.span.start;
                    match method.kind {
                        MethodDefinitionKind::Constructor => info.constructor = Some(function),
                        MethodDefinitionKind::Method if method.r#static => {
                            info.statics.insert(name, function);
                        }
                        MethodDefinitionKind::Method => {
                            info.methods.insert(name, function);
                        }
                        MethodDefinitionKind::Get | MethodDefinitionKind::Set => {
                            info.accessors.insert(name);
                        }
                    }
                }
                ClassElement::PropertyDefinition(property) => {
                    if property.r#static {
                        continue;
                    }
                    let name = property
                        .key
                        .static_name()
                        .filter(|_| !property.computed)
                        .map(|name| name.display().to_string());
                    match property
                        .value
                        .as_ref()
                        .map(Expression::get_inner_expression)
                    {
                        Some(Expression::ArrowFunctionExpression(function)) => {
                            if let Some(name) = name {
                                info.fields.insert(name, function.span.start);
                            }
                        }
                        Some(Expression::FunctionExpression(function)) => {
                            if let Some(name) = name {
                                info.fields.insert(name, function.span.start);
                            }
                        }
                        Some(value) => {
                            if let Some(name) = &name {
                                // A non-function field shadows any method.
                                info.accessors.insert(name.clone());
                            }
                            if contains_call(value) {
                                info.field_initializer_calls = true;
                            }
                        }
                        None => {}
                    }
                }
                ClassElement::AccessorProperty(property) => {
                    if let Some(name) = property.key.static_name() {
                        info.accessors.insert(name.display().to_string());
                    }
                }
                _ => {}
            }
        }
        self.table.classes.insert(start, info);
        self.class_stack.push(start);
        walk::walk_class(self, it);
        self.class_stack.pop();
    }
}

/// Whether an expression (outside nested functions) calls anything.
fn contains_call(expression: &Expression<'_>) -> bool {
    struct Finder {
        found: bool,
    }
    impl<'a> Visit<'a> for Finder {
        fn visit_call_expression(&mut self, _: &CallExpression<'a>) {
            self.found = true;
        }
        fn visit_new_expression(&mut self, _: &oxc_ast::ast::NewExpression<'a>) {
            self.found = true;
        }
        fn visit_tagged_template_expression(
            &mut self,
            _: &oxc_ast::ast::TaggedTemplateExpression<'a>,
        ) {
            self.found = true;
        }
        fn visit_function(&mut self, _: &Function<'a>, _: ScopeFlags) {}
        fn visit_arrow_function_expression(&mut self, _: &ArrowFunctionExpression<'a>) {}
    }
    let mut finder = Finder { found: false };
    finder.visit_expression(expression);
    finder.found
}

impl Collector<'_> {
    /// An identifier written here: remember it when the write happens in a
    /// function nested in the one that declares the binding.
    fn note_write(&mut self, ident: &IdentifierReference<'_>) {
        let Some(symbol) = resolve_reference(self.scoping, ident) else {
            return;
        };
        let declared = function_scope_of(self.scoping, self.scoping.symbol_scope_id(symbol));
        let current = self.functions.last().copied();
        if current.is_some() && Some(declared) != current {
            self.table.captured_writes.insert(symbol);
        }
    }

    /// `Object.defineProperty(this | C | C.prototype, ...)`.
    fn note_reflective_target(&mut self, call: &CallExpression<'_>) {
        let Some(target) = call.arguments.first().and_then(|a| a.as_expression()) else {
            return;
        };
        match target.get_inner_expression() {
            Expression::ThisExpression(_) => {
                if let Some(class) = self.class_stack.last()
                    && let Some(info) = self.table.classes.get_mut(class)
                {
                    info.dynamic.insert("*".to_owned());
                }
            }
            Expression::Identifier(ident) => {
                if let Some(symbol) = resolve_reference(self.scoping, ident) {
                    self.prototype_writes.push((symbol, "*".to_owned()));
                }
            }
            Expression::StaticMemberExpression(member) if member.property.name == "prototype" => {
                if let Expression::Identifier(ident) = member.object.get_inner_expression()
                    && let Some(symbol) = resolve_reference(self.scoping, ident)
                {
                    self.prototype_writes.push((symbol, "*".to_owned()));
                }
            }
            _ => {}
        }
    }
}

/// The scope of the function (or the program) enclosing `scope`.
pub(crate) fn function_scope_of(scoping: &Scoping, scope: ScopeId) -> ScopeId {
    let mut last = scope;
    for ancestor in scoping.scope_ancestors(scope) {
        last = ancestor;
        if scoping.scope_flags(ancestor).is_function() {
            return ancestor;
        }
    }
    last
}

impl Collector<'_> {
    fn classify(&self, init: &Expression<'_>) -> Initializer {
        match init.get_inner_expression() {
            Expression::Identifier(ident) => Initializer::Alias {
                target: resolve_reference(self.scoping, ident),
                name: ident.name.as_str().to_owned(),
            },
            Expression::CallExpression(call) => {
                let ident = match call.callee.get_inner_expression() {
                    Expression::Identifier(ident) => Some(ident),
                    _ => None,
                };
                Initializer::Call {
                    callee: ident.and_then(|ident| resolve_reference(self.scoping, ident)),
                    name: ident.map(|ident| ident.name.as_str().to_owned()),
                    no_arguments: call.arguments.is_empty(),
                }
            }
            Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::NumericLiteral(_)
            | Expression::BigIntLiteral(_)
            | Expression::RegExpLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::TemplateLiteral(_) => Initializer::Contrary("a literal"),
            Expression::ObjectExpression(_) => Initializer::Contrary("an object literal"),
            Expression::ArrayExpression(_) => Initializer::Contrary("an array literal"),
            Expression::NewExpression(_) => Initializer::Contrary("a `new` expression"),
            Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_) => {
                Initializer::Contrary("a function")
            }
            Expression::ClassExpression(_) => Initializer::Contrary("a class"),
            _ => Initializer::Other,
        }
    }
}

pub(crate) fn resolve_reference(
    scoping: &Scoping,
    ident: &IdentifierReference<'_>,
) -> Option<SymbolId> {
    let reference = ident.reference_id.get()?;
    scoping.get_reference(reference).symbol_id()
}

/// Where the value of an identifier comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// Not declared in this file (a global, or an ambient declaration).
    Unresolved,
    Import(ImportBinding),
    /// `const x = drizzle(...)` with `drizzle` imported from `drizzle-orm*`.
    DrizzleInstance {
        module: String,
    },
    /// The parameter of a recognized transaction callback.
    TransactionHandle(TransactionId),
    /// A `const` bound to an unexecuted Drizzle query builder.
    PendingBuilder(SymbolId),
    /// `const a = f()`: the value a call without arguments returned. `callee`
    /// is the function's name (the imported name for a named import).
    CallResult {
        callee: String,
        import: Option<ImportBinding>,
    },
    /// Declared locally (parameter, variable with an opaque initializer, ...).
    Local,
    /// Declared locally with a value that is evidently something else.
    Contrary(&'static str),
}

/// The outcome of resolving an identifier.
#[derive(Clone, Debug)]
pub(crate) struct Resolved {
    /// The identifier as written.
    pub written: String,
    /// The name used for matching: the imported name for named imports, the
    /// alias target for `const` aliases, otherwise the identifier itself.
    pub name: String,
    pub origin: Origin,
    /// `const` aliases followed, outermost first.
    pub aliases: Vec<String>,
}

impl Resolved {
    /// Human-readable resolution, used as recognition evidence.
    pub fn describe(&self) -> String {
        let mut text = match &self.origin {
            Origin::Unresolved => format!("`{}` is not declared in this file", self.name),
            Origin::Import(import) => match &import.imported {
                Some(imported) => format!("`{imported}` is imported from \"{}\"", import.module),
                None => format!("`{}` is imported from \"{}\"", self.name, import.module),
            },
            Origin::DrizzleInstance { module } => {
                format!(
                    "`{}` is initialized by `drizzle(...)` from \"{module}\"",
                    self.name
                )
            }
            Origin::TransactionHandle(_) => {
                format!("`{}` is the transaction callback parameter", self.name)
            }
            Origin::PendingBuilder(_) => format!("`{}` holds a query builder", self.name),
            Origin::CallResult { callee, import } => match import {
                Some(import) => format!(
                    "`{}` is initialized by `{callee}()`, imported from \"{}\"",
                    self.name, import.module
                ),
                None => format!("`{}` is initialized by `{callee}()`", self.name),
            },
            Origin::Local => format!("`{}` is a local binding", self.name),
            Origin::Contrary(what) => format!("`{}` is initialized with {what}", self.name),
        };
        if !self.aliases.is_empty() {
            text.push_str(&format!(
                " (via const alias {})",
                self.aliases
                    .iter()
                    .map(|alias| format!("`{alias}`"))
                    .collect::<Vec<_>>()
                    .join(" → ")
            ));
        }
        text
    }
}

/// Per-function bindings introduced during lowering.
#[derive(Debug, Default)]
pub(crate) struct LocalBindings {
    pub transaction_handles: HashMap<SymbolId, TransactionId>,
    pub pending_builders: HashMap<SymbolId, crate::recognize::Builder>,
    /// Plain identifier parameters of the function being lowered (not of an
    /// enclosing function), with their position and name. Parameters that
    /// are reassigned are left out: their value is not the argument.
    pub params: HashMap<SymbolId, (u32, String)>,
}

const MAX_ALIAS_DEPTH: usize = 8;

pub(crate) fn resolve(
    ident: &IdentifierReference<'_>,
    scoping: &Scoping,
    table: &SymbolTable,
    locals: &LocalBindings,
) -> Resolved {
    let written = ident.name.as_str().to_owned();
    let mut name = written.clone();
    let mut aliases = Vec::new();
    let mut symbol = resolve_reference(scoping, ident);

    for _ in 0..MAX_ALIAS_DEPTH {
        let Some(current) = symbol else {
            return Resolved {
                written,
                name,
                origin: Origin::Unresolved,
                aliases,
            };
        };
        let origin = if let Some(&tx) = locals.transaction_handles.get(&current) {
            Origin::TransactionHandle(tx)
        } else if locals.pending_builders.contains_key(&current) {
            Origin::PendingBuilder(current)
        } else if let Some(import) = table.imports.get(&current) {
            if let Some(imported) = &import.imported {
                name.clone_from(imported);
            }
            Origin::Import(import.clone())
        } else if let Some(declarator) = table.declarators.get(&current) {
            match &declarator.init {
                Initializer::Alias {
                    target,
                    name: target_name,
                } if declarator.is_const => {
                    aliases.push(std::mem::replace(&mut name, target_name.clone()));
                    symbol = *target;
                    continue;
                }
                Initializer::Call {
                    callee,
                    name: callee_name,
                    no_arguments,
                } => match callee.and_then(|callee| table.imports.get(&callee)) {
                    Some(import)
                        if import.module.starts_with("drizzle-orm")
                            && import.imported.as_deref() == Some("drizzle") =>
                    {
                        Origin::DrizzleInstance {
                            module: import.module.clone(),
                        }
                    }
                    import => match callee_name {
                        Some(callee_name) if declarator.is_const && *no_arguments => {
                            Origin::CallResult {
                                callee: import
                                    .and_then(|import| import.imported.clone())
                                    .unwrap_or_else(|| callee_name.clone()),
                                import: import.cloned(),
                            }
                        }
                        _ => Origin::Local,
                    },
                },
                Initializer::Contrary(what) => Origin::Contrary(what),
                _ => Origin::Local,
            }
        } else {
            let flags = scoping.symbol_flags(current);
            if flags.is_function() {
                Origin::Contrary("a function declaration")
            } else if flags.is_class() {
                Origin::Contrary("a class declaration")
            } else {
                Origin::Local
            }
        };
        return Resolved {
            written,
            name,
            origin,
            aliases,
        };
    }
    // Alias chain too deep (or cyclic): treat as an opaque local.
    Resolved {
        written,
        name,
        origin: Origin::Local,
        aliases,
    }
}

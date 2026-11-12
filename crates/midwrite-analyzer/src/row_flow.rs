//! Bounded, source-derived lineage of rows returned by writes.
//!
//! This pass only adds `Never` to a branch arm it can disprove. It never
//! creates effects or supplies an application invariant. Unknown row
//! transformations, row escapes, mutations and exhausted budgets cancel
//! the proof; unrelated unknown values do not supply additional facts.
//! Result operations are recognized only on values originating at the
//! `neverthrow` constructors, not by a local method's name alone.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_semantic::{Scoping, SemanticBuilder};
use oxc_span::{GetSpan, SourceType};
use oxc_syntax::operator::{AssignmentOperator, BinaryOperator, LogicalOperator, UnaryOperator};
use oxc_syntax::scope::ScopeFlags;

use midwrite_core::config::Config;
use midwrite_core::ir::{Fact, Literal, Node};
use midwrite_core::location::LineIndex;

use crate::LoweredModule;
use crate::bindings::{literal_value, resolve_reference};
use crate::context::{ProjectContext, Resolved};
use crate::resolve::Resolution;

const MAX_STEPS: usize = 20_000;
const MAX_CALLS: usize = 128;
const MAX_DEPTH: usize = 8;
const MAX_ALTERNATIVES: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
enum ColumnKind {
    Text,
    Varchar(usize),
    Enum(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct InputId {
    root: (usize, u32, usize),
    reads: Vec<(usize, usize, u32, String)>,
}

impl InputId {
    fn read(&self, frame: &Frame<'_, '_>, span: u32, name: String) -> Value {
        if self.reads.len() >= 8 {
            return Value::Unknown;
        }
        let mut key = self.clone();
        key.reads.push((frame.file, frame.instance, span, name));
        Value::Input(key)
    }
}

impl ColumnKind {
    fn accepts(&self, value: &str) -> bool {
        match self {
            Self::Text => true,
            Self::Varchar(length) => value.chars().count() <= *length,
            Self::Enum(values) => values.iter().any(|v| v == value),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Value {
    Unknown,
    Lit(Literal),
    Input(InputId),
    Record(BTreeMap<String, Value>),
    Array(BTreeMap<String, Value>),
    Row(BTreeMap<String, Value>),
    Rows(Box<Value>),
    Maybe(Box<Value>),
    /// Drizzle skips undefined assignments; the previous/default column
    /// value is unknown. Defined inputs retain their identity.
    Column(Box<Value>, ColumnKind),
    Builder(Box<Value>, bool, BTreeMap<String, ColumnKind>),
    Promise(Box<Value>),
    Table(usize, String),
    Namespace(usize),
    Handle,
    Ok(Box<Value>),
    Err,
    Alternatives(Vec<Value>),
}

impl Value {
    fn undefined() -> Self {
        Self::Lit(Literal::Undefined)
    }

    fn join(self, other: Self) -> Self {
        if self == other {
            return self;
        }
        if self == Self::Unknown || other == Self::Unknown {
            return Self::Unknown;
        }
        match (self, other) {
            (Self::Record(a), Self::Record(b)) => {
                Self::Record(join_fields(a, b, Value::undefined()))
            }
            (Self::Array(a), Self::Array(b)) if a.len() == b.len() => {
                Self::Array(join_fields(a, b, Value::undefined()))
            }
            (Self::Row(a), Self::Row(b)) => Self::Row(join_fields(a, b, Value::Unknown)),
            (Self::Rows(a), Self::Rows(b)) => Self::Rows(Box::new(a.join(*b))),
            (Self::Ok(a), Self::Ok(b)) => Self::Ok(Box::new(a.join(*b))),
            (Self::Promise(a), Self::Promise(b)) => Self::Promise(Box::new(a.join(*b))),
            (a, b) => {
                let mut values = match a {
                    Self::Alternatives(v) => v,
                    v => vec![v],
                };
                for v in match b {
                    Self::Alternatives(v) => v,
                    v => vec![v],
                } {
                    if !values.contains(&v) {
                        values.push(v);
                    }
                }
                if values.len() > MAX_ALTERNATIVES {
                    Self::Unknown
                } else {
                    Self::Alternatives(values)
                }
            }
        }
    }

    fn field(&self, name: &str) -> Self {
        match self {
            Self::Record(fields) => fields
                .get(name)
                .cloned()
                .unwrap_or_else(|| absent_property(name)),
            Self::Array(fields) if name == "length" => {
                Self::Lit(Literal::Number(fields.len().to_string()))
            }
            Self::Array(fields) => fields
                .get(name)
                .cloned()
                .unwrap_or_else(|| absent_property(name)),
            Self::Row(fields) => fields.get(name).cloned().unwrap_or(Self::Unknown),
            Self::Rows(row) if name == "0" => Self::Maybe(row.clone()),
            Self::Ok(value) if name == "value" => *value.clone(),
            Self::Err if name == "value" => Self::undefined(),
            Self::Alternatives(values) => join_values(values.iter().map(|v| v.field(name))),
            Self::Maybe(value) => value.field(name).join(Self::Unknown),
            _ => Self::Unknown,
        }
    }

    fn tracked(&self) -> bool {
        match self {
            Self::Row(_)
            | Self::Rows(_)
            | Self::Builder(..)
            | Self::Table(..)
            | Self::Namespace(_)
            | Self::Handle
            | Self::Ok(_)
            | Self::Err => true,
            Self::Record(fields) | Self::Array(fields) => fields.values().any(Self::tracked),
            Self::Maybe(v) | Self::Column(v, _) | Self::Promise(v) => v.tracked(),
            Self::Alternatives(v) => v.iter().any(Self::tracked),
            _ => false,
        }
    }

    fn row(&self) -> bool {
        match self {
            Self::Row(_) | Self::Rows(_) => true,
            Self::Record(fields) | Self::Array(fields) => fields.values().any(Self::row),
            Self::Maybe(v)
            | Self::Column(v, _)
            | Self::Ok(v)
            | Self::Builder(v, _, _)
            | Self::Promise(v) => v.row(),
            Self::Alternatives(v) => v.iter().any(Self::row),
            _ => false,
        }
    }

    fn table(&self) -> bool {
        match self {
            Self::Table(..) | Self::Namespace(_) => true,
            Self::Record(v) | Self::Array(v) => v.values().any(Self::table),
            Self::Ok(v) | Self::Maybe(v) | Self::Promise(v) => v.table(),
            Self::Alternatives(v) => v.iter().any(Self::table),
            _ => false,
        }
    }

    fn record(&self) -> bool {
        match self {
            Self::Record(_) | Self::Array(_) => true,
            Self::Alternatives(v) => v.iter().any(Self::record),
            _ => false,
        }
    }

    fn forget_records(self) -> Self {
        match self {
            Self::Record(_) | Self::Array(_) => Self::Unknown,
            Self::Ok(v) => Self::Ok(Box::new(v.forget_records())),
            Self::Promise(v) => Self::Promise(Box::new(v.forget_records())),
            Self::Maybe(v) => Self::Maybe(Box::new(v.forget_records())),
            Self::Alternatives(v) => join_values(v.into_iter().map(Self::forget_records)),
            value => value,
        }
    }

    fn column(&self) -> bool {
        match self {
            Self::Column(..) => true,
            Self::Alternatives(v) => v.iter().any(Self::column),
            _ => false,
        }
    }
}

fn join_fields(
    a: BTreeMap<String, Value>,
    b: BTreeMap<String, Value>,
    absent: Value,
) -> BTreeMap<String, Value> {
    a.keys()
        .chain(b.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|key| {
            let missing = if absent == Value::Unknown {
                Value::Unknown
            } else {
                absent_property(&key)
            };
            let value = a
                .get(&key)
                .cloned()
                .unwrap_or_else(|| missing.clone())
                .join(b.get(&key).cloned().unwrap_or(missing));
            (key, value)
        })
        .collect()
}

fn join_values(values: impl IntoIterator<Item = Value>) -> Value {
    values
        .into_iter()
        .reduce(Value::join)
        .unwrap_or(Value::Unknown)
}

fn absent_property(name: &str) -> Value {
    if matches!(
        name,
        "__proto__"
            | "constructor"
            | "toString"
            | "toLocaleString"
            | "valueOf"
            | "hasOwnProperty"
            | "isPrototypeOf"
            | "propertyIsEnumerable"
            | "__defineGetter__"
            | "__defineSetter__"
            | "__lookupGetter__"
            | "__lookupSetter__"
    ) {
        Value::Unknown
    } else {
        Value::undefined()
    }
}

fn adopt(value: Value) -> Value {
    match value {
        Value::Promise(v) => adopt(*v),
        Value::Builder(row, true, _) => Value::Rows(row),
        Value::Builder(_, false, _) => Value::Unknown,
        Value::Alternatives(values) => join_values(values.into_iter().map(adopt)),
        value => value,
    }
}

#[derive(Clone, Default)]
struct State {
    env: BTreeMap<usize, Value>,
    known: BTreeMap<InputId, Literal>,
}

impl State {
    fn havoc_records(&mut self) {
        for value in self.env.values_mut() {
            *value = value.clone().forget_records();
        }
    }
    fn join_environment(&mut self, other: &State) {
        for (key, value) in &mut self.env {
            *value = value
                .clone()
                .join(other.env.get(key).cloned().unwrap_or(Value::Unknown));
        }
        self.known
            .retain(|key, value| other.known.get(key) == Some(value));
    }
    fn resolve(&self, value: Value) -> Value {
        match value {
            Value::Input(ref key) => self
                .known
                .get(key)
                .cloned()
                .map(Value::Lit)
                .unwrap_or(value),
            Value::Column(v, kind) => match self.resolve(*v.clone()) {
                Value::Lit(Literal::Undefined) => Value::Unknown,
                Value::Lit(Literal::String(value)) if kind.accepts(&value) => {
                    Value::Lit(Literal::String(value))
                }
                Value::Lit(_) => Value::Unknown,
                _ => Value::Column(v, kind),
            },
            Value::Alternatives(v) => join_values(v.into_iter().map(|v| self.resolve(v))),
            v => v,
        }
    }
}

#[derive(Clone)]
enum Flow {
    Next(State),
    Return(Value),
    Throw,
}

fn truth(value: &Value) -> Option<bool> {
    match value {
        Value::Lit(v) => Some(v.is_truthy()),
        Value::Record(_)
        | Value::Array(_)
        | Value::Row(_)
        | Value::Rows(_)
        | Value::Builder(..)
        | Value::Promise(_)
        | Value::Handle
        | Value::Namespace(_)
        | Value::Ok(_)
        | Value::Err => Some(true),
        Value::Alternatives(v) => {
            let first = truth(v.first()?)?;
            v.iter().all(|v| truth(v) == Some(first)).then_some(first)
        }
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct Frame<'p, 'a> {
    file: usize,
    source: &'p str,
    program: &'p Program<'a>,
    scoping: &'p Scoping,
    record: bool,
    instance: usize,
}

#[derive(Clone, Copy)]
struct FunctionBody<'p, 'a> {
    params: &'p FormalParameters<'a>,
    body: crate::lower::Body<'p, 'a>,
    span: oxc_span::Span,
    is_async: bool,
}

fn function_expr<'p, 'a>(expression: &'p Expression<'a>) -> Option<FunctionBody<'p, 'a>> {
    match expression.get_inner_expression() {
        Expression::ArrowFunctionExpression(f) => Some(FunctionBody {
            params: &f.params,
            body: crate::lower::Body::Arrow(&f.body),
            span: f.span,
            is_async: f.r#async,
        }),
        Expression::FunctionExpression(f) => function_body(f),
        _ => None,
    }
}

fn function_body<'p, 'a>(f: &'p Function<'a>) -> Option<FunctionBody<'p, 'a>> {
    if f.generator {
        return None;
    }
    Some(FunctionBody {
        params: &f.params,
        body: crate::lower::Body::Statements(&f.body.as_ref()?.statements),
        span: f.span,
        is_async: f.r#async,
    })
}

fn functions<'p, 'a>(program: &'p Program<'a>) -> Vec<(String, FunctionBody<'p, 'a>)> {
    let mut out = Vec::new();
    for statement in &program.body {
        let declaration = match statement {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            other => other.as_declaration(),
        };
        match declaration {
            Some(Declaration::FunctionDeclaration(f)) => {
                if let Some(body) = function_body(f)
                    && let Some(id) = &f.id
                {
                    out.push((id.name.to_string(), body));
                }
            }
            Some(Declaration::VariableDeclaration(v)) => {
                for d in &v.declarations {
                    if let BindingPattern::BindingIdentifier(id) = &d.id
                        && let Some(f) = d.init.as_ref().and_then(function_expr)
                    {
                        out.push((id.name.to_string(), f));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

struct Engine<'c> {
    context: &'c ProjectContext,
    config: &'c Config,
    steps: usize,
    calls: usize,
    depth: usize,
    aborted: bool,
    stack: BTreeSet<(usize, u32)>,
    globals: BTreeMap<(usize, String), Value>,
    columns: BTreeMap<(usize, String), BTreeMap<String, ColumnKind>>,
    observations: BTreeMap<u32, (bool, bool, bool)>,
    row_uses: usize,
    instances: usize,
}

impl Engine<'_> {
    fn cancel(&mut self) {
        self.aborted = true;
    }

    fn resolved(&mut self, state: &State, value: Value) -> Value {
        let result = state.resolve(value.clone());
        if value.column() && matches!(result, Value::Lit(_)) {
            self.row_uses += 1;
        }
        result
    }
    fn tick(&mut self) -> bool {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            self.cancel();
        }
        !self.aborted
    }

    fn symbol(frame: &Frame<'_, '_>, id: &IdentifierReference<'_>) -> Option<usize> {
        resolve_reference(frame.scoping, id).map(|s| s.index())
    }

    fn import(
        &self,
        frame: &Frame<'_, '_>,
        id: &IdentifierReference<'_>,
    ) -> Option<(String, String)> {
        let symbol = resolve_reference(frame.scoping, id)?;
        if !frame.scoping.symbol_flags(symbol).is_import() {
            return None;
        }
        self.context
            .summary(frame.file)
            .imports
            .get(id.name.as_str())
            .cloned()
    }

    fn bind(
        &mut self,
        frame: &Frame<'_, '_>,
        state: &mut State,
        pattern: &BindingPattern<'_>,
        value: Value,
    ) {
        match pattern {
            BindingPattern::BindingIdentifier(id) => {
                if let Some(symbol) = id.symbol_id.get() {
                    state.env.insert(symbol.index(), value);
                }
            }
            BindingPattern::ObjectPattern(object) => {
                for p in &object.properties {
                    let part = if !p.computed {
                        p.key
                            .static_name()
                            .map(|name| match &value {
                                Value::Input(key) => {
                                    key.read(frame, p.span.start, name.display().to_string())
                                }
                                Value::Namespace(file) => {
                                    self.export_value(*file, &name.display().to_string())
                                }
                                _ => value.field(&name.display().to_string()),
                            })
                            .unwrap_or(Value::Unknown)
                    } else {
                        Value::Unknown
                    };
                    self.bind(frame, state, &p.value, part);
                }
            }
            BindingPattern::ArrayPattern(array) => {
                for (i, p) in array.elements.iter().enumerate() {
                    if let Some(p) = p {
                        let part = match &value {
                            Value::Input(key) => key.read(frame, p.span().start, i.to_string()),
                            _ => value.field(&i.to_string()),
                        };
                        self.bind(frame, state, p, part);
                    }
                }
            }
            BindingPattern::AssignmentPattern(assignment) => {
                let value = if value == Value::undefined() {
                    self.expr(frame, state, &assignment.right)
                } else if matches!(
                    value,
                    Value::Input(_)
                        | Value::Unknown
                        | Value::Maybe(_)
                        | Value::Alternatives(_)
                        | Value::Column(..)
                ) {
                    let mut possible = state.clone();
                    self.expr(frame, &mut possible, &assignment.right);
                    state.join_environment(&possible);
                    Value::Unknown
                } else {
                    value
                };
                self.bind(frame, state, &assignment.left, value);
            }
        }
    }

    fn body(
        &mut self,
        frame: &Frame<'_, '_>,
        state: State,
        body: crate::lower::Body<'_, '_>,
    ) -> Vec<Flow> {
        match body {
            crate::lower::Body::Statements(statements) => self.statements(frame, state, statements),
            crate::lower::Body::Arrow(ArrowFunctionBody::FunctionBody(body)) => {
                self.statements(frame, state, &body.statements)
            }
            crate::lower::Body::Arrow(body) => {
                let mut state = state;
                vec![Flow::Return(
                    body.as_expression()
                        .map_or(Value::Unknown, |e| self.expr(frame, &mut state, e)),
                )]
            }
        }
    }

    fn invoke(
        &mut self,
        file: usize,
        local: &str,
        arguments: &[Value],
        known: &BTreeMap<InputId, Literal>,
    ) -> Value {
        self.calls += 1;
        if self.calls > MAX_CALLS || self.depth >= MAX_DEPTH {
            self.cancel();
            return Value::Unknown;
        }
        let Ok(text) = self.context.source(&self.context.files[file]) else {
            self.cancel();
            return Value::Unknown;
        };
        let allocator = Allocator::default();
        let parsed = Parser::new(
            &allocator,
            &text,
            SourceType::from_path(&self.context.files[file]).unwrap_or_else(|_| SourceType::ts()),
        )
        .parse();
        if parsed.fatal_error || parsed.diagnostics.errors().next().is_some() {
            self.cancel();
            return Value::Unknown;
        }
        let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
        let Some((_, f)) = functions(&parsed.program)
            .into_iter()
            .find(|(name, _)| name == local)
        else {
            self.cancel();
            return Value::Unknown;
        };
        if !self.stack.insert((file, f.span.start)) {
            self.cancel();
            return Value::Unknown;
        }
        self.depth += 1;
        let frame = Frame {
            file,
            source: &text,
            program: &parsed.program,
            scoping: semantic.scoping(),
            record: false,
            instance: 0,
        };
        let state = State {
            known: known.clone(),
            ..State::default()
        };
        let value = self.invoke_body(&frame, state, f, arguments);
        self.depth -= 1;
        self.stack.remove(&(file, f.span.start));
        value
    }

    fn invoke_body(
        &mut self,
        frame: &Frame<'_, '_>,
        mut state: State,
        f: FunctionBody<'_, '_>,
        arguments: &[Value],
    ) -> Value {
        self.instances += 1;
        let frame = &Frame {
            instance: self.instances,
            ..*frame
        };
        let mut captures = CapturedWrites {
            scoping: frame.scoping,
            span: f.span,
            writes: false,
        };
        captures.visit_formal_parameters(f.params);
        match f.body {
            crate::lower::Body::Statements(statements) => {
                for s in statements {
                    captures.visit_statement(s);
                }
            }
            crate::lower::Body::Arrow(body) => captures.visit_arrow_function_body(body),
        }
        if captures.writes {
            self.cancel();
            return Value::Unknown;
        }
        for (i, p) in f.params.items.iter().enumerate() {
            let value = arguments.get(i).cloned().unwrap_or_else(Value::undefined);
            let value = if value == Value::undefined() {
                p.initializer
                    .as_ref()
                    .map_or(value, |e| self.expr(frame, &mut state, e))
            } else if let Some(initializer) = &p.initializer
                && matches!(
                    value,
                    Value::Input(_)
                        | Value::Unknown
                        | Value::Maybe(_)
                        | Value::Alternatives(_)
                        | Value::Column(..)
                )
            {
                let mut possible = state.clone();
                self.expr(frame, &mut possible, initializer);
                state.join_environment(&possible);
                Value::Unknown
            } else {
                value
            };
            self.bind(frame, &mut state, &p.pattern, value);
        }
        let value =
            join_values(self.body(frame, state, f.body).into_iter().filter_map(
                |flow| match flow {
                    Flow::Return(value) => Some(value),
                    Flow::Next(_) => Some(Value::undefined()),
                    Flow::Throw => None,
                },
            ));
        if f.is_async {
            Value::Promise(Box::new(adopt(value)))
        } else {
            value
        }
    }

    fn statements(
        &mut self,
        frame: &Frame<'_, '_>,
        state: State,
        statements: &[Statement<'_>],
    ) -> Vec<Flow> {
        let mut flows = vec![Flow::Next(state)];
        for statement in statements {
            let mut next = Vec::new();
            for flow in flows {
                match flow {
                    Flow::Next(state) => next.extend(self.statement(frame, state, statement)),
                    other => next.push(other),
                }
            }
            // Join normal continuations. Only constraints common to both
            // paths survive; lost precision must never prove a branch dead.
            let mut joined: Option<State> = None;
            flows = Vec::new();
            for flow in next {
                match flow {
                    Flow::Next(state) => match &mut joined {
                        None => joined = Some(state),
                        Some(previous) => {
                            for (key, value) in &mut previous.env {
                                *value = value
                                    .clone()
                                    .join(state.env.get(key).cloned().unwrap_or(Value::Unknown));
                            }
                            previous
                                .known
                                .retain(|key, value| state.known.get(key) == Some(value));
                        }
                    },
                    other => flows.push(other),
                }
            }
            if let Some(state) = joined {
                flows.push(Flow::Next(state));
            }
            if self.aborted {
                return vec![Flow::Return(Value::Unknown)];
            }
        }
        flows
    }

    fn statement(
        &mut self,
        frame: &Frame<'_, '_>,
        mut state: State,
        statement: &Statement<'_>,
    ) -> Vec<Flow> {
        if !self.tick() {
            return vec![Flow::Return(Value::Unknown)];
        }
        match statement {
            Statement::VariableDeclaration(declaration) => {
                for d in &declaration.declarations {
                    let value = d
                        .init
                        .as_ref()
                        .map_or_else(Value::undefined, |e| self.expr(frame, &mut state, e));
                    self.bind(frame, &mut state, &d.id, value);
                }
                vec![Flow::Next(state)]
            }
            Statement::ExpressionStatement(expression) => {
                self.expr(frame, &mut state, &expression.expression);
                vec![Flow::Next(state)]
            }
            Statement::ReturnStatement(statement) => vec![Flow::Return(
                statement
                    .argument
                    .as_ref()
                    .map_or_else(Value::undefined, |e| self.expr(frame, &mut state, e)),
            )],
            Statement::ThrowStatement(statement) => {
                if self.expr(frame, &mut state, &statement.argument).row() {
                    self.cancel();
                }
                vec![Flow::Throw]
            }
            Statement::BlockStatement(block) => self.statements(frame, state, &block.body),
            Statement::IfStatement(branch) => {
                let before = self.row_uses;
                let yes = self.assume(frame, state.clone(), &branch.test, true);
                let no = self.assume(frame, state, &branch.test, false);
                if frame.record {
                    let seen = self.observations.entry(branch.span.start).or_default();
                    seen.0 |= !yes.is_empty();
                    seen.1 |= !no.is_empty();
                    seen.2 |= self.row_uses > before;
                }
                let mut out = Vec::new();
                for state in yes {
                    out.extend(self.statement(frame, state, &branch.consequent));
                }
                for state in no {
                    match &branch.alternate {
                        Some(statement) => out.extend(self.statement(frame, state, statement)),
                        None => out.push(Flow::Next(state)),
                    }
                }
                out
            }
            Statement::TryStatement(statement) => {
                let before = state.clone();
                let mut out = self.statements(frame, state, &statement.block.body);
                if let Some(handler) = &statement.handler {
                    let mut state = before;
                    if let Some(p) = &handler.param {
                        self.bind(frame, &mut state, &p.pattern, Value::Unknown);
                    }
                    out.extend(self.statements(frame, state, &handler.body.body));
                }
                // Returning/throwing finalizers and mutation through a
                // finalizer need their own control/value model.
                if let Some(finalizer) = &statement.finalizer {
                    let _ = finalizer;
                    self.cancel();
                }
                out
            }
            Statement::EmptyStatement(_)
            | Statement::TSTypeAliasDeclaration(_)
            | Statement::TSInterfaceDeclaration(_) => vec![Flow::Next(state)],
            _ => {
                self.cancel();
                vec![Flow::Return(Value::Unknown)]
            }
        }
    }

    fn expr(
        &mut self,
        frame: &Frame<'_, '_>,
        state: &mut State,
        expression: &Expression<'_>,
    ) -> Value {
        if !self.tick() {
            return Value::Unknown;
        }
        let expression = expression.get_inner_expression();
        if let Some(literal) = literal_value(expression) {
            return Value::Lit(literal);
        }
        match expression {
            Expression::Identifier(id) => {
                if let Some(value) = Self::symbol(frame, id).and_then(|s| state.env.get(&s)) {
                    return self.resolved(state, value.clone());
                }
                if id.name == "undefined" && resolve_reference(frame.scoping, id).is_none() {
                    return Value::undefined();
                }
                if resolve_reference(frame.scoping, id).is_some_and(|s| {
                    frame
                        .scoping
                        .scope_parent_id(frame.scoping.symbol_scope_id(s))
                        .is_some()
                }) {
                    return Value::Unknown;
                }
                if self
                    .config
                    .database_receivers
                    .iter()
                    .any(|r| r.name() == id.name.as_str())
                    && !contrary_binding(frame.program, id.name.as_str())
                {
                    return Value::Handle;
                }
                self.global(frame, id)
            }
            Expression::ObjectExpression(object) => {
                let mut fields = BTreeMap::new();
                for property in &object.properties {
                    match property {
                        ObjectPropertyKind::ObjectProperty(p)
                            if !p.computed && p.kind == PropertyKind::Init && !p.method =>
                        {
                            let Some(key) = p.key.static_name() else {
                                return Value::Unknown;
                            };
                            if key.display().to_string() == "__proto__" {
                                self.cancel();
                                return Value::Unknown;
                            }
                            fields.insert(
                                key.display().to_string(),
                                self.expr(frame, state, &p.value),
                            );
                        }
                        ObjectPropertyKind::SpreadProperty(p) => {
                            match self.expr(frame, state, &p.argument) {
                                Value::Record(extra) => fields.extend(extra),
                                _ => {
                                    self.cancel();
                                    return Value::Unknown;
                                }
                            }
                        }
                        _ => {
                            self.cancel();
                            return Value::Unknown;
                        }
                    }
                }
                Value::Record(fields)
            }
            Expression::ArrayExpression(array) => {
                let mut fields = BTreeMap::new();
                for (i, value) in array.elements.iter().enumerate() {
                    let Some(e) = value.as_expression() else {
                        self.cancel();
                        return Value::Unknown;
                    };
                    fields.insert(i.to_string(), self.expr(frame, state, e));
                }
                Value::Array(fields)
            }
            Expression::StaticMemberExpression(member) => {
                let base = self.expr(frame, state, &member.object);
                let value = match base {
                    Value::Namespace(file) => {
                        self.export_value(file, member.property.name.as_str())
                    }
                    Value::Input(key) => {
                        key.read(frame, member.span.start, member.property.name.to_string())
                    }
                    base => base.field(member.property.name.as_str()),
                };
                self.resolved(state, value)
            }
            Expression::ComputedMemberExpression(member) => {
                let key = match self.expr(frame, state, &member.expression) {
                    Value::Lit(Literal::String(s) | Literal::Number(s)) => s,
                    _ => return Value::Unknown,
                };
                let base = self.expr(frame, state, &member.object);
                let value = match base {
                    Value::Namespace(file) => self.export_value(file, &key),
                    Value::Input(root) => root.read(frame, member.span.start, key),
                    base => base.field(&key),
                };
                self.resolved(state, value)
            }
            Expression::AwaitExpression(awaited) => {
                adopt(self.expr(frame, state, &awaited.argument))
            }
            Expression::CallExpression(call) => self.call(frame, state, call),
            Expression::ConditionalExpression(conditional) => {
                let yes = self.assume(frame, state.clone(), &conditional.test, true);
                let no = self.assume(frame, state.clone(), &conditional.test, false);
                let mut values = Vec::new();
                for mut s in yes {
                    values.push(self.expr(frame, &mut s, &conditional.consequent));
                }
                for mut s in no {
                    values.push(self.expr(frame, &mut s, &conditional.alternate));
                }
                join_values(values)
            }
            Expression::LogicalExpression(logical) => {
                let left = self.expr(frame, state, &logical.left);
                match (logical.operator, truth(&left), &left) {
                    (
                        LogicalOperator::Coalesce,
                        _,
                        Value::Lit(Literal::Undefined | Literal::Null),
                    ) => self.expr(frame, state, &logical.right),
                    (
                        LogicalOperator::Coalesce,
                        _,
                        Value::Handle
                        | Value::Record(_)
                        | Value::Row(_)
                        | Value::Ok(_)
                        | Value::Err,
                    ) => left,
                    (LogicalOperator::And, Some(false), _)
                    | (LogicalOperator::Or, Some(true), _) => left,
                    (LogicalOperator::And, Some(true), _)
                    | (LogicalOperator::Or, Some(false), _) => {
                        self.expr(frame, state, &logical.right)
                    }
                    _ => {
                        let mut other = state.clone();
                        let right = self.expr(frame, &mut other, &logical.right);
                        state.join_environment(&other);
                        left.join(right)
                    }
                }
            }
            Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::LogicalNot => {
                truth(&self.expr(frame, state, &unary.argument))
                    .map_or(Value::Unknown, |v| Value::Lit(Literal::Boolean(!v)))
            }
            Expression::AssignmentExpression(assignment) => {
                let value = if assignment.operator == AssignmentOperator::Assign {
                    self.expr(frame, state, &assignment.right)
                } else {
                    Value::Unknown
                };
                match &assignment.left {
                    AssignmentTarget::AssignmentTargetIdentifier(id) => {
                        if let Some(s) = Self::symbol(frame, id) {
                            state.env.insert(s, value.clone());
                        }
                    }
                    _ => self.aborted = true,
                }
                value
            }
            Expression::NewExpression(new) => {
                for a in &new.arguments {
                    let value = self.argument(frame, state, a);
                    if value.row() {
                        self.cancel();
                    }
                    if value.record() {
                        state.havoc_records();
                    }
                }
                Value::Unknown
            }
            Expression::BinaryExpression(binary) => {
                let left = self.expr(frame, state, &binary.left);
                let right = self.expr(frame, state, &binary.right);
                match (&left, &right, binary.operator) {
                    (Value::Lit(a), Value::Lit(b), BinaryOperator::StrictEquality) => {
                        Value::Lit(Literal::Boolean(a == b))
                    }
                    (Value::Lit(a), Value::Lit(b), BinaryOperator::StrictInequality) => {
                        Value::Lit(Literal::Boolean(a != b))
                    }
                    _ => Value::Unknown,
                }
            }
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::StaticMemberExpression(member) => {
                    let base = self.expr(frame, state, &member.object);
                    base.field(member.property.name.as_str())
                        .join(Value::undefined())
                }
                _ => {
                    self.cancel();
                    Value::Unknown
                }
            },
            Expression::TemplateLiteral(template) if template.expressions.is_empty() => {
                Value::Unknown
            }
            // Unmodeled expression forms may contain calls or captured
            // writes. Cancel the proof, not just the expression's value.
            _ => {
                self.cancel();
                Value::Unknown
            }
        }
    }

    fn assume(
        &mut self,
        frame: &Frame<'_, '_>,
        mut state: State,
        expression: &Expression<'_>,
        yes: bool,
    ) -> Vec<State> {
        if !self.tick() {
            return vec![state];
        }
        match expression.get_inner_expression() {
            Expression::UnaryExpression(u) if u.operator == UnaryOperator::LogicalNot => {
                return self.assume(frame, state, &u.argument, !yes);
            }
            Expression::LogicalExpression(l)
                if (l.operator == LogicalOperator::And && yes)
                    || (l.operator == LogicalOperator::Or && !yes) =>
            {
                return self
                    .assume(frame, state, &l.left, yes)
                    .into_iter()
                    .flat_map(|s| self.assume(frame, s, &l.right, yes))
                    .collect();
            }
            Expression::LogicalExpression(l)
                if matches!(l.operator, LogicalOperator::And | LogicalOperator::Or) =>
            {
                let mut out = self.assume(frame, state.clone(), &l.left, yes);
                for s in self.assume(frame, state, &l.left, !yes) {
                    out.extend(self.assume(frame, s, &l.right, yes));
                }
                return out;
            }
            Expression::BinaryExpression(b)
                if matches!(
                    b.operator,
                    BinaryOperator::StrictEquality | BinaryOperator::StrictInequality
                ) =>
            {
                let a = self.expr(frame, &mut state, &b.left);
                let b_value = self.expr(frame, &mut state, &b.right);
                let equal = yes == (b.operator == BinaryOperator::StrictEquality);
                match (a, b_value) {
                    (Value::Lit(a), Value::Lit(b)) => {
                        return if (a == b) == equal {
                            vec![state]
                        } else {
                            vec![]
                        };
                    }
                    (Value::Input(key), Value::Lit(lit)) | (Value::Lit(lit), Value::Input(key))
                        if equal =>
                    {
                        if state.known.get(&key).is_some_and(|v| *v != lit) {
                            return vec![];
                        }
                        state.known.insert(key, lit);
                        return vec![state];
                    }
                    _ => {}
                }
            }
            Expression::CallExpression(call) => {
                if let Expression::StaticMemberExpression(member) =
                    call.callee.get_inner_expression()
                    && let Expression::Identifier(id) = member.object.get_inner_expression()
                    && matches!(member.property.name.as_str(), "isOk" | "isErr")
                    && call.arguments.is_empty()
                    && let Some(symbol) = Self::symbol(frame, id)
                    && let Some(value) = state.env.get(&symbol).cloned()
                {
                    let want_ok = yes == (member.property.name == "isOk");
                    let alternatives = match value {
                        Value::Alternatives(v) => v,
                        v => vec![v],
                    };
                    if alternatives
                        .iter()
                        .all(|v| matches!(v, Value::Ok(_) | Value::Err))
                    {
                        let remaining: Vec<_> = alternatives
                            .into_iter()
                            .filter(|v| matches!(v, Value::Ok(_)) == want_ok)
                            .collect();
                        if remaining.is_empty() {
                            return vec![];
                        }
                        state.env.insert(symbol, join_values(remaining));
                        return vec![state];
                    }
                }
            }
            _ => {}
        }
        let value = self.expr(frame, &mut state, expression);
        if truth(&value).is_some_and(|v| v != yes) {
            return vec![];
        }
        if let Expression::Identifier(id) = expression.get_inner_expression()
            && let Some(symbol) = Self::symbol(frame, id)
        {
            let refined = match value {
                Value::Maybe(v) => {
                    if yes {
                        *v
                    } else {
                        Value::undefined()
                    }
                }
                Value::Alternatives(v) => {
                    join_values(v.into_iter().filter(|v| truth(v).is_none_or(|t| t == yes)))
                }
                v => v,
            };
            state.env.insert(symbol, refined);
        }
        vec![state]
    }

    fn call(
        &mut self,
        frame: &Frame<'_, '_>,
        state: &mut State,
        call: &CallExpression<'_>,
    ) -> Value {
        let callee = call.callee.get_inner_expression();
        if call.arguments.is_empty() {
            let text =
                &frame.source[call.callee.span().start as usize..call.callee.span().end as usize];
            if self
                .config
                .database_receivers
                .iter()
                .any(|r| r.name() == format!("{text}()"))
            {
                return Value::Handle;
            }
        }
        if let Expression::StaticMemberExpression(member) = callee {
            let receiver = self.expr(frame, state, &member.object);
            let method = member.property.name.as_str();
            match receiver {
                Value::Handle if self.config.transaction_methods.iter().any(|m| m == method) => {
                    for argument in call.arguments.iter().skip(1) {
                        self.argument(frame, state, argument);
                    }
                    let Some(f) = call
                        .arguments
                        .first()
                        .and_then(Argument::as_expression)
                        .and_then(function_expr)
                    else {
                        return Value::Unknown;
                    };
                    let value = self.invoke_body(frame, state.clone(), f, &[Value::Handle]);
                    state.havoc_records();
                    return Value::Promise(Box::new(adopt(value)));
                }
                Value::Handle if matches!(method, "insert" | "update") => {
                    let mut columns = BTreeMap::new();
                    for argument in &call.arguments {
                        if let Value::Table(file, name) = self.argument(frame, state, argument) {
                            columns = self.columns(file, &name);
                        }
                    }
                    return Value::Builder(Box::new(Value::Unknown), false, columns);
                }
                Value::Builder(row, returning, columns) => match method {
                    "set" | "values" => {
                        let value = call
                            .arguments
                            .first()
                            .and_then(Argument::as_expression)
                            .map_or(Value::Unknown, |e| self.expr(frame, state, e));
                        for argument in call.arguments.iter().skip(1) {
                            self.argument(frame, state, argument);
                        }
                        let Value::Record(fields) = value else {
                            return Value::Builder(Box::new(Value::Unknown), returning, columns);
                        };
                        let fields = fields
                            .into_iter()
                            .map(|(k, v)| {
                                let value = columns.get(&k).map_or(Value::Unknown, |kind| {
                                    Value::Column(Box::new(v), kind.clone())
                                });
                                (k, value)
                            })
                            .collect();
                        return Value::Builder(Box::new(Value::Row(fields)), returning, columns);
                    }
                    "returning" if call.arguments.is_empty() => {
                        return Value::Builder(row, true, columns);
                    }
                    "where" | "onConflictDoNothing" => {
                        for argument in &call.arguments {
                            self.argument(frame, state, argument);
                        }
                        return Value::Builder(row, returning, columns);
                    }
                    _ => {
                        self.cancel();
                        return Value::Unknown;
                    }
                },
                value @ (Value::Ok(_) | Value::Err | Value::Alternatives(_))
                    if matches!(method, "map" | "andThen" | "mapErr" | "isOk" | "isErr") =>
                {
                    let alternatives = match value {
                        Value::Alternatives(v) => v,
                        v => vec![v],
                    };
                    if !alternatives
                        .iter()
                        .all(|v| matches!(v, Value::Ok(_) | Value::Err))
                    {
                        return Value::Unknown;
                    }
                    if matches!(method, "isOk" | "isErr") {
                        for argument in &call.arguments {
                            self.argument(frame, state, argument);
                        }
                        return join_values(alternatives.into_iter().map(|v| {
                            Value::Lit(Literal::Boolean(
                                matches!(v, Value::Ok(_)) == (method == "isOk"),
                            ))
                        }));
                    }
                    let Some(callback) = call.arguments.first().and_then(Argument::as_expression)
                    else {
                        self.cancel();
                        return Value::Unknown;
                    };
                    for argument in call.arguments.iter().skip(1) {
                        self.argument(frame, state, argument);
                    }
                    let mut out = Vec::new();
                    for value in alternatives {
                        match (method, value) {
                            ("mapErr", Value::Ok(v)) => out.push(Value::Ok(v)),
                            ("mapErr", Value::Err) => {
                                // An imported/top-level error mapper cannot
                                // close over the caller's local returned row.
                                match callback.get_inner_expression() {
                                    Expression::Identifier(id) => {
                                        if self.import(frame, id).is_none()
                                            && resolve_reference(frame.scoping, id).is_some_and(
                                                |s| {
                                                    frame
                                                        .scoping
                                                        .scope_parent_id(
                                                            frame.scoping.symbol_scope_id(s),
                                                        )
                                                        .is_some()
                                                },
                                            )
                                        {
                                            self.cancel();
                                        }
                                    }
                                    _ => self.aborted = true,
                                }
                                out.push(Value::Err);
                            }
                            (_, Value::Err) => out.push(Value::Err),
                            (_, Value::Ok(value)) => {
                                let child = Frame {
                                    record: false,
                                    ..*frame
                                };
                                let mapped = if let Some(f) = function_expr(callback) {
                                    self.invoke_body(&child, state.clone(), f, &[*value])
                                } else {
                                    self.cancel();
                                    Value::Unknown
                                };
                                state.havoc_records();
                                out.push(if method == "map" {
                                    Value::Ok(Box::new(mapped))
                                } else {
                                    mapped
                                });
                            }
                            _ => unreachable!(),
                        }
                    }
                    return join_values(out);
                }
                other => {
                    if other.row() {
                        self.cancel();
                    }
                    if other.record() {
                        state.havoc_records();
                    }
                }
            }
        }
        let mut arguments = Vec::new();
        for argument in &call.arguments {
            arguments.push(self.argument(frame, state, argument));
        }
        if let Expression::Identifier(id) = callee {
            if let Some((module, imported)) = self.import(frame, id) {
                if module == "neverthrow" {
                    return match imported.as_str() {
                        "ok" => Value::Ok(Box::new(
                            arguments.first().cloned().unwrap_or_else(Value::undefined),
                        )),
                        "err" => Value::Err,
                        _ => Value::Unknown,
                    };
                }
                if arguments.iter().any(Value::tracked)
                    && let Resolution::File(target) = self
                        .context
                        .resolve(&self.context.files[frame.file], &module)
                    && let Ok(Resolved::Binding { file, local }) =
                        self.context.resolve_export(target, &imported)
                {
                    let value = self.invoke(file, &local, &arguments, &state.known);
                    if arguments.iter().any(Value::record) {
                        state.havoc_records();
                    }
                    return value;
                }
            } else if arguments.iter().any(Value::tracked)
                && resolve_reference(frame.scoping, id).is_some_and(|s| {
                    frame
                        .scoping
                        .scope_parent_id(frame.scoping.symbol_scope_id(s))
                        .is_none()
                })
                && let Some((name, _)) = functions(frame.program)
                    .into_iter()
                    .find(|(name, _)| name == id.name.as_str())
            {
                let value = self.invoke(frame.file, &name, &arguments, &state.known);
                if arguments.iter().any(Value::record) {
                    state.havoc_records();
                }
                return value;
            }
        }
        // A passed row could be changed by code whose semantics are unknown.
        // This cancels refinement rather than assuming that call is pure.
        if arguments.iter().any(Value::row) {
            self.cancel();
        }
        if arguments.iter().any(Value::table) {
            self.cancel();
        }
        if arguments.iter().any(Value::record) {
            state.havoc_records();
        }
        if call
            .arguments
            .iter()
            .any(|a| a.as_expression().and_then(function_expr).is_some())
        {
            self.cancel();
        }
        if state.env.values().any(Value::row)
            && let Expression::Identifier(id) = callee
            && self.import(frame, id).is_none()
            && let Some(symbol) = resolve_reference(frame.scoping, id)
            && frame
                .scoping
                .scope_parent_id(frame.scoping.symbol_scope_id(symbol))
                .is_some()
        {
            self.cancel();
        }
        Value::Unknown
    }

    fn argument(
        &mut self,
        frame: &Frame<'_, '_>,
        state: &mut State,
        argument: &Argument<'_>,
    ) -> Value {
        match argument {
            Argument::SpreadElement(spread) => {
                if self.expr(frame, state, &spread.argument).row() {
                    self.cancel();
                }
                Value::Unknown
            }
            argument => argument
                .as_expression()
                .map_or(Value::Unknown, |e| self.expr(frame, state, e)),
        }
    }

    fn global(&mut self, frame: &Frame<'_, '_>, id: &IdentifierReference<'_>) -> Value {
        if let Some((module, imported)) = self.import(frame, id) {
            let Resolution::File(target) = self
                .context
                .resolve(&self.context.files[frame.file], &module)
            else {
                return Value::Unknown;
            };
            if imported == "*" {
                return Value::Namespace(target);
            }
            return self.export_value(target, &imported);
        }
        self.binding(frame.file, id.name.as_str())
    }

    fn export_value(&mut self, file: usize, name: &str) -> Value {
        match self.context.resolve_export(file, name) {
            Ok(Resolved::Binding { file, local }) => self.binding(file, &local),
            Ok(Resolved::Namespace { file }) => Value::Namespace(file),
            Err(_) => Value::Unknown,
        }
    }

    fn binding(&mut self, file: usize, name: &str) -> Value {
        let name = name.to_owned();
        if let Some(value) = self.globals.get(&(file, name.clone())) {
            return value.clone();
        }
        self.globals.insert((file, name.clone()), Value::Unknown);
        let value = self.literal_global(file, &name);
        self.globals.insert((file, name), value.clone());
        value
    }

    fn global_expression(&mut self, file: usize, expression: &Expression<'_>) -> Value {
        match expression.get_inner_expression() {
            Expression::Identifier(id) => {
                if let Some((module, name)) =
                    self.context.summary(file).imports.get(id.name.as_str())
                {
                    let Resolution::File(target) =
                        self.context.resolve(&self.context.files[file], module)
                    else {
                        return Value::Unknown;
                    };
                    if name == "*" {
                        Value::Namespace(target)
                    } else {
                        self.export_value(target, name)
                    }
                } else {
                    self.binding(file, id.name.as_str())
                }
            }
            Expression::StaticMemberExpression(member) => {
                match self.global_expression(file, &member.object) {
                    Value::Namespace(target) => {
                        self.export_value(target, member.property.name.as_str())
                    }
                    value => value.field(member.property.name.as_str()),
                }
            }
            expression => literal_record(expression),
        }
    }

    fn columns(&mut self, file: usize, name: &str) -> BTreeMap<String, ColumnKind> {
        let key = (file, name.to_owned());
        if let Some(columns) = self.columns.get(&key) {
            return columns.clone();
        }
        let columns = self.read_columns(file, name);
        self.columns.insert(key, columns.clone());
        columns
    }

    fn read_columns(&self, file: usize, name: &str) -> BTreeMap<String, ColumnKind> {
        let Ok(source) = self.context.source(&self.context.files[file]) else {
            return BTreeMap::new();
        };
        let Ok(schema) = crate::schema::extract_schema_source(
            &self.context.rel(&self.context.files[file]),
            &source,
        ) else {
            return BTreeMap::new();
        };
        let Some(table) = schema.tables.iter().find(|t| t.binding == name) else {
            return BTreeMap::new();
        };
        let allocator = Allocator::default();
        let parsed = Parser::new(
            &allocator,
            &source,
            SourceType::from_path(&self.context.files[file]).unwrap_or_else(|_| SourceType::ts()),
        )
        .parse();
        if parsed.fatal_error || parsed.diagnostics.errors().next().is_some() {
            return BTreeMap::new();
        }
        let Some(Expression::CallExpression(call)) =
            global_initializer(&parsed.program, name).map(Expression::get_inner_expression)
        else {
            return BTreeMap::new();
        };
        let Some(Expression::ObjectExpression(properties)) = call
            .arguments
            .get(1)
            .and_then(Argument::as_expression)
            .map(Expression::get_inner_expression)
        else {
            return BTreeMap::new();
        };
        let mut out = BTreeMap::new();
        for column in &table.columns {
            let Some(initializer) = properties.properties.iter().find_map(|p| match p {
                ObjectPropertyKind::ObjectProperty(p)
                    if !p.computed
                        && p.key
                            .static_name()
                            .is_some_and(|n| n.display().to_string() == column.property) =>
                {
                    Some(&p.value)
                }
                _ => None,
            }) else {
                continue;
            };
            let Some(builder) = column_root(initializer) else {
                continue;
            };
            let imported = schema
                .imports
                .get(builder.as_str())
                .filter(|(module, _)| module == "drizzle-orm/pg-core")
                .map(|(_, name)| name.as_str());
            let kind = if imported == Some("text") && column.sql_type == "text" {
                Some(ColumnKind::Text)
            } else if imported == Some("varchar") {
                if column.sql_type == "varchar" {
                    Some(ColumnKind::Text)
                } else {
                    column
                        .sql_type
                        .strip_prefix("varchar(")
                        .and_then(|s| s.strip_suffix(')'))
                        .and_then(|s| s.parse().ok())
                        .map(ColumnKind::Varchar)
                }
            } else if !column.enum_values.is_empty()
                && global_initializer(&parsed.program, &builder).is_some_and(|e| {
                    let Expression::CallExpression(call) = e.get_inner_expression() else {
                        return false;
                    };
                    let Expression::Identifier(callee) = call.callee.get_inner_expression() else {
                        return false;
                    };
                    schema
                        .imports
                        .get(callee.name.as_str())
                        .is_some_and(|(module, name)| {
                            module == "drizzle-orm/pg-core" && name == "pgEnum"
                        })
                })
            {
                Some(ColumnKind::Enum(column.enum_values.clone()))
            } else {
                None
            };
            if let Some(kind) = kind {
                out.insert(column.property.clone(), kind);
            }
        }
        out
    }

    fn literal_global(&mut self, file: usize, name: &str) -> Value {
        let Ok(text) = self.context.source(&self.context.files[file]) else {
            return Value::Unknown;
        };
        let allocator = Allocator::default();
        let parsed = Parser::new(
            &allocator,
            &text,
            SourceType::from_path(&self.context.files[file]).unwrap_or_else(|_| SourceType::ts()),
        )
        .parse();
        if parsed.fatal_error || parsed.diagnostics.errors().next().is_some() {
            return Value::Unknown;
        }
        for statement in &parsed.program.body {
            let declaration = match statement {
                Statement::ExportDeclaration(e) => Some(&e.declaration),
                s => s.as_declaration(),
            };
            if let Some(Declaration::VariableDeclaration(declaration)) = declaration
                && declaration.kind.is_const()
            {
                for d in &declaration.declarations {
                    if let BindingPattern::ObjectPattern(object) = &d.id {
                        for p in &object.properties {
                            if !p.computed
                                && let BindingPattern::BindingIdentifier(id) = &p.value
                                && id.name == name
                                && let Some(key) = p.key.static_name()
                                && let Some(init) = &d.init
                            {
                                return match self.global_expression(file, init) {
                                    Value::Namespace(target) => {
                                        self.export_value(target, &key.display().to_string())
                                    }
                                    value => value.field(&key.display().to_string()),
                                };
                            }
                        }
                    }
                    if let BindingPattern::BindingIdentifier(id) = &d.id
                        && id.name == name
                    {
                        if let Some(init) = &d.init
                            && table_call(init, &self.context.summary(file).imports)
                        {
                            return Value::Table(file, name.to_owned());
                        }
                        let value = d.init.as_ref().map_or(Value::Unknown, literal_record);
                        if value == Value::Unknown
                            && let Some(init) = &d.init
                        {
                            return self.global_expression(file, init);
                        }
                        if let Value::Record(fields) = &value
                            && !immutable_global(
                                self.context,
                                file,
                                name,
                                &fields.keys().cloned().collect(),
                            )
                        {
                            return Value::Unknown;
                        }
                        return value;
                    }
                }
            }
        }
        Value::Unknown
    }
}

fn global_initializer<'p, 'a>(program: &'p Program<'a>, name: &str) -> Option<&'p Expression<'a>> {
    for statement in &program.body {
        let declaration = match statement {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            s => s.as_declaration(),
        };
        if let Some(Declaration::VariableDeclaration(declaration)) = declaration {
            for d in &declaration.declarations {
                if let BindingPattern::BindingIdentifier(id) = &d.id
                    && id.name == name
                {
                    return d.init.as_ref();
                }
            }
        }
    }
    None
}

fn table_call(expression: &Expression<'_>, imports: &BTreeMap<String, (String, String)>) -> bool {
    let Expression::CallExpression(call) = expression.get_inner_expression() else {
        return false;
    };
    let Expression::Identifier(callee) = call.callee.get_inner_expression() else {
        return false;
    };
    imports
        .get(callee.name.as_str())
        .is_some_and(|(module, name)| module == "drizzle-orm/pg-core" && name == "pgTable")
}

fn column_root(expression: &Expression<'_>) -> Option<String> {
    let Expression::CallExpression(call) = expression.get_inner_expression() else {
        return None;
    };
    match call.callee.get_inner_expression() {
        Expression::Identifier(id) => Some(id.name.to_string()),
        Expression::StaticMemberExpression(member)
            if matches!(
                member.property.name.as_str(),
                "notNull" | "default" | "primaryKey" | "unique" | "references" | "$type"
            ) =>
        {
            column_root(&member.object)
        }
        _ => None,
    }
}

fn literal_record(expression: &Expression<'_>) -> Value {
    if let Some(literal) = literal_value(expression) {
        return Value::Lit(literal);
    }
    if let Expression::ObjectExpression(object) = expression.get_inner_expression() {
        let mut fields = BTreeMap::new();
        for property in &object.properties {
            let ObjectPropertyKind::ObjectProperty(p) = property else {
                return Value::Unknown;
            };
            if p.computed || p.method || p.kind != PropertyKind::Init {
                return Value::Unknown;
            }
            let Some(name) = p.key.static_name() else {
                return Value::Unknown;
            };
            if name.display().to_string() == "__proto__" {
                return Value::Unknown;
            }
            let Some(value) = literal_value(&p.value) else {
                return Value::Unknown;
            };
            fields.insert(name.display().to_string(), Value::Lit(value));
        }
        return Value::Record(fields);
    }
    Value::Unknown
}

struct CapturedWrites<'s> {
    scoping: &'s Scoping,
    span: oxc_span::Span,
    writes: bool,
}

impl<'a> Visit<'a> for CapturedWrites<'_> {
    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if let AssignmentTarget::AssignmentTargetIdentifier(id) = &it.left
            && let Some(symbol) = resolve_reference(self.scoping, id)
        {
            let declared = self.scoping.symbol_span(symbol);
            if declared.start < self.span.start || declared.end > self.span.end {
                self.writes = true;
            }
        }
        walk::walk_assignment_expression(self, it);
    }
    fn visit_update_expression(&mut self, _: &UpdateExpression<'a>) {
        self.writes = true;
    }
    fn visit_function(&mut self, _: &Function<'a>, _: ScopeFlags) {
        self.writes = true;
    }
    fn visit_arrow_function_expression(&mut self, _: &ArrowFunctionExpression<'a>) {}
}

fn contrary_binding(program: &Program<'_>, name: &str) -> bool {
    for statement in &program.body {
        let declaration = match statement {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            s => s.as_declaration(),
        };
        if let Some(Declaration::VariableDeclaration(declaration)) = declaration {
            for d in &declaration.declarations {
                if let BindingPattern::BindingIdentifier(id) = &d.id
                    && id.name == name
                    && let Some(init) = &d.init
                {
                    return !matches!(
                        init.get_inner_expression(),
                        Expression::CallExpression(_)
                            | Expression::AwaitExpression(_)
                            | Expression::Identifier(_)
                    );
                }
            }
        }
    }
    false
}

/// Literal records can be used as enum constants only when the analyzed
/// sources neither mutate them nor expose their object identity to unknown
/// code. Import aliases and re-exports are checked by resolved identity.
fn immutable_global(
    context: &ProjectContext,
    owner: usize,
    name: &str,
    primitive_keys: &BTreeSet<String>,
) -> bool {
    let mut aliases = BTreeSet::from([name.to_owned()]);
    for file in 0..context.files.len() {
        for exported in context.summary(file).exports.keys() {
            if context.resolve_export(file, exported).ok()
                == Some(Resolved::Binding {
                    file: owner,
                    local: name.to_owned(),
                })
            {
                aliases.insert(exported.clone());
            }
        }
    }
    for file in 0..context.files.len() {
        let summary = context.summary(file);
        let mut names = BTreeMap::new();
        if file == owner {
            names.insert(name.to_owned(), None);
        }
        for (local, (specifier, imported)) in &summary.imports {
            let Resolution::File(target) = context.resolve(&context.files[file], specifier) else {
                continue;
            };
            if imported == "*" {
                let fields: BTreeSet<_> = aliases
                    .iter()
                    .filter(|exported| {
                        context.resolve_export(target, exported).ok()
                            == Some(Resolved::Binding {
                                file: owner,
                                local: name.to_owned(),
                            })
                    })
                    .cloned()
                    .collect();
                if !fields.is_empty() {
                    names.insert(local.clone(), Some(fields));
                }
            } else if aliases.contains(imported)
                && context.resolve_export(target, imported).ok()
                    == Some(Resolved::Binding {
                        file: owner,
                        local: name.to_owned(),
                    })
            {
                names.insert(local.clone(), None);
            }
        }
        if names.is_empty() {
            continue;
        }
        let Ok(text) = context.source(&context.files[file]) else {
            return false;
        };
        let allocator = Allocator::default();
        let parsed = Parser::new(
            &allocator,
            &text,
            SourceType::from_path(&context.files[file]).unwrap_or_else(|_| SourceType::ts()),
        )
        .parse();
        if parsed.fatal_error || parsed.diagnostics.errors().next().is_some() {
            return false;
        }
        let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
        let scoping = semantic.scoping();
        let watched: BTreeMap<_, _> = scoping
            .symbol_ids()
            .filter_map(|symbol| {
                let global = scoping
                    .scope_parent_id(scoping.symbol_scope_id(symbol))
                    .is_none();
                names
                    .get(scoping.symbol_name(symbol))
                    .filter(|_| global)
                    .cloned()
                    .map(|kind| (symbol.index(), kind))
            })
            .collect();
        let mut uses = GlobalUses {
            scoping,
            watched: &watched,
            imports: &summary.imports,
            primitive_keys,
            readonly: 0,
            unsafe_use: false,
        };
        uses.visit_program(&parsed.program);
        if uses.unsafe_use {
            return false;
        }
    }
    true
}

struct GlobalUses<'s> {
    scoping: &'s Scoping,
    watched: &'s BTreeMap<usize, Option<BTreeSet<String>>>,
    imports: &'s BTreeMap<String, (String, String)>,
    primitive_keys: &'s BTreeSet<String>,
    readonly: usize,
    unsafe_use: bool,
}

impl GlobalUses<'_> {
    fn watched(&self, id: &IdentifierReference<'_>) -> Option<&Option<BTreeSet<String>>> {
        resolve_reference(self.scoping, id).and_then(|s| self.watched.get(&s.index()))
    }

    fn escapes(&self, expression: &Expression<'_>) -> bool {
        match expression.get_inner_expression() {
            Expression::Identifier(id) => self.watched(id).is_some(),
            Expression::StaticMemberExpression(member) if self.primitive_member(member) => false,
            Expression::StaticMemberExpression(member) => {
                match member.object.get_inner_expression() {
                    Expression::Identifier(id) => self.watched(id).is_some_and(|kind| {
                        kind.as_ref()
                            .is_some_and(|fields| fields.contains(member.property.name.as_str()))
                    }),
                    other => self.escapes(other),
                }
            }
            Expression::ObjectExpression(object) => object.properties.iter().any(|p| match p {
                ObjectPropertyKind::ObjectProperty(p) => self.escapes(&p.value),
                ObjectPropertyKind::SpreadProperty(p) => self.escapes(&p.argument),
            }),
            Expression::ArrayExpression(array) => array.elements.iter().any(|e| match e {
                ArrayExpressionElement::SpreadElement(e) => self.escapes(&e.argument),
                _ => e.as_expression().is_some_and(|e| self.escapes(e)),
            }),
            Expression::ComputedMemberExpression(member) => {
                match member.object.get_inner_expression() {
                    Expression::Identifier(id) => self.watched(id).is_some_and(Option::is_some),
                    other => self.escapes(other),
                }
            }
            Expression::ConditionalExpression(e) => {
                self.escapes(&e.consequent) || self.escapes(&e.alternate)
            }
            Expression::LogicalExpression(e) => self.escapes(&e.left) || self.escapes(&e.right),
            Expression::SequenceExpression(e) => e.expressions.iter().any(|e| self.escapes(e)),
            Expression::AwaitExpression(e) => self.escapes(&e.argument),
            _ => false,
        }
    }

    fn primitive_member(&self, member: &StaticMemberExpression<'_>) -> bool {
        if !self.primitive_keys.contains(member.property.name.as_str()) {
            return false;
        }
        match member.object.get_inner_expression() {
            Expression::Identifier(id) => matches!(self.watched(id), Some(None)),
            Expression::StaticMemberExpression(inner) => {
                match inner.object.get_inner_expression() {
                    Expression::Identifier(id) => self.watched(id).is_some_and(|kind| {
                        kind.as_ref()
                            .is_some_and(|fields| fields.contains(inner.property.name.as_str()))
                    }),
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn typebox_reader(&self, call: &CallExpression<'_>) -> bool {
        let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
            return false;
        };
        let Expression::Identifier(id) = member.object.get_inner_expression() else {
            return false;
        };
        matches!(member.property.name.as_str(), "Enum" | "Const")
            && self
                .imports
                .get(id.name.as_str())
                .is_some_and(|(m, n)| m == "@sinclair/typebox" && n == "Type")
            && resolve_reference(self.scoping, id)
                .is_some_and(|s| self.scoping.symbol_flags(s).is_import())
    }

    fn argument_escapes(&self, argument: &Argument<'_>) -> bool {
        match argument {
            Argument::SpreadElement(spread) => self.escapes(&spread.argument),
            argument => argument.as_expression().is_some_and(|e| self.escapes(e)),
        }
    }

    fn target(&self, target: &AssignmentTarget<'_>) -> bool {
        match target {
            AssignmentTarget::AssignmentTargetIdentifier(id) => self.watched(id).is_some(),
            AssignmentTarget::StaticMemberExpression(member) => self.escapes(&member.object),
            AssignmentTarget::ComputedMemberExpression(member) => self.escapes(&member.object),
            _ => true,
        }
    }
}

impl<'a> Visit<'a> for GlobalUses<'_> {
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if self.readonly == 0 && self.watched(it).is_some() {
            self.unsafe_use = true;
        }
    }
    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        if self.readonly == 0 && self.primitive_member(it) {
            return;
        }
        walk::walk_static_member_expression(self, it);
    }
    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        if self.readonly == 0
            && let Expression::Identifier(id) = it.object.get_inner_expression()
            && matches!(self.watched(id), Some(None))
            && let Some(Literal::String(key)) = literal_value(&it.expression)
            && self.primitive_keys.contains(&key)
        {
            return;
        }
        walk::walk_computed_member_expression(self, it);
    }
    fn visit_ts_type_query(&mut self, _: &TSTypeQuery<'a>) {}
    fn visit_export_named_declaration(&mut self, _: &ExportNamedDeclaration<'a>) {}
    fn visit_export_default_declaration(&mut self, it: &ExportDefaultDeclaration<'a>) {
        if let Some(Expression::Identifier(id)) = it.declaration.as_expression()
            && matches!(self.watched(id), Some(None))
        {
            return;
        }
        walk::walk_export_default_declaration(self, it);
    }
    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        self.unsafe_use |= it.body.as_expression().is_some_and(|e| self.escapes(e));
        walk::walk_arrow_function_expression(self, it);
    }
    fn visit_yield_expression(&mut self, it: &YieldExpression<'a>) {
        self.unsafe_use |= it.argument.as_ref().is_some_and(|e| self.escapes(e));
        walk::walk_yield_expression(self, it);
    }
    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        self.unsafe_use |= self.target(&it.left) || self.escapes(&it.right);
        walk::walk_assignment_expression(self, it);
    }
    fn visit_update_expression(&mut self, it: &UpdateExpression<'a>) {
        self.unsafe_use |= match &it.argument {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => self.watched(id).is_some(),
            SimpleAssignmentTarget::StaticMemberExpression(member) => self.escapes(&member.object),
            SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                self.escapes(&member.object)
            }
            _ => true,
        };
        walk::walk_update_expression(self, it);
    }
    fn visit_unary_expression(&mut self, it: &UnaryExpression<'a>) {
        if it.operator == UnaryOperator::Delete {
            self.unsafe_use |= match it.argument.get_inner_expression() {
                Expression::StaticMemberExpression(member) => self.escapes(&member.object),
                Expression::ComputedMemberExpression(member) => self.escapes(&member.object),
                Expression::Identifier(id) => self.watched(id).is_some(),
                _ => true,
            };
        }
        walk::walk_unary_expression(self, it);
    }
    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        self.unsafe_use |= it.init.as_ref().is_some_and(|e| self.escapes(e));
        walk::walk_variable_declarator(self, it);
    }
    fn visit_return_statement(&mut self, it: &ReturnStatement<'a>) {
        self.unsafe_use |= it.argument.as_ref().is_some_and(|e| self.escapes(e));
        walk::walk_return_statement(self, it);
    }
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Expression::StaticMemberExpression(member) = it.callee.get_inner_expression() {
            self.unsafe_use |= self.escapes(&member.object);
        }
        let reader = self.typebox_reader(it);
        let saved = std::mem::replace(&mut self.readonly, 0);
        self.visit_expression(&it.callee);
        for (index, argument) in it.arguments.iter().enumerate() {
            self.readonly = usize::from(reader && index == 0);
            if self.readonly == 0 {
                self.unsafe_use |= self.argument_escapes(argument);
            }
            self.visit_argument(argument);
        }
        self.readonly = saved;
    }
    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        self.unsafe_use |= it.arguments.iter().any(|a| self.argument_escapes(a));
        walk::walk_new_expression(self, it);
    }
    fn visit_tagged_template_expression(&mut self, it: &TaggedTemplateExpression<'a>) {
        self.unsafe_use |= it.quasi.expressions.iter().any(|e| self.escapes(e));
        walk::walk_tagged_template_expression(self, it);
    }
}

/// Project-only refinement: imported helper and constant identities are
/// resolved through exactly the analyzed source set.
pub(crate) fn refine(
    config: &Config,
    paths: &[PathBuf],
    snapshots: &[std::sync::Arc<str>],
    modules: &mut [LoweredModule],
) {
    let root = std::env::current_dir().unwrap_or_default();
    let context = ProjectContext::with_files(root, paths.to_vec());
    context.seed_sources(paths.iter().cloned().zip(snapshots.iter().cloned()));
    let mut engine = Engine {
        context: &context,
        config,
        steps: 0,
        calls: 0,
        depth: 0,
        aborted: false,
        stack: BTreeSet::new(),
        globals: BTreeMap::new(),
        columns: BTreeMap::new(),
        observations: BTreeMap::new(),
        row_uses: 0,
        instances: 0,
    };
    for (file, module) in modules.iter_mut().enumerate() {
        let Ok(text) = context.source(&paths[file]) else {
            continue;
        };
        if !text.contains("returning") {
            continue;
        }
        let allocator = Allocator::default();
        let parsed = Parser::new(
            &allocator,
            &text,
            SourceType::from_path(&paths[file]).unwrap_or_else(|_| SourceType::ts()),
        )
        .parse();
        if parsed.fatal_error || parsed.diagnostics.errors().next().is_some() {
            continue;
        }
        let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
        let frame = Frame {
            file,
            source: &text,
            program: &parsed.program,
            scoping: semantic.scoping(),
            record: true,
            instance: 0,
        };
        let lines = LineIndex::new(&text);
        for (_, f) in functions(&parsed.program) {
            let Some(&index) = module.index.get(&f.span.start) else {
                continue;
            };
            if !text[f.span.start as usize..f.span.end as usize].contains("returning") {
                continue;
            }
            engine.steps = 0;
            engine.calls = 0;
            engine.depth = 0;
            engine.aborted = false;
            engine.stack.clear();
            engine.observations.clear();
            engine.row_uses = 0;
            engine.instances = 0;
            let arguments: Vec<_> = f
                .params
                .items
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    Value::Input(InputId {
                        root: (file, f.span.start, i),
                        reads: Vec::new(),
                    })
                })
                .collect();
            engine.invoke_body(&frame, State::default(), f, &arguments);
            if engine.aborted {
                continue;
            }
            let dead: BTreeMap<_, _> = engine
                .observations
                .iter()
                .filter_map(|(span, seen)| {
                    let (line, column) = lines.position(*span);
                    match seen {
                        (false, true, true) => Some(((line, column), "then")),
                        (true, false, true) => Some(((line, column), "else")),
                        _ => None,
                    }
                })
                .collect();
            add_facts(&mut module.functions[index].body, &dead);
        }
    }
}

fn add_facts(nodes: &mut [Node], dead: &BTreeMap<(u32, u32), &str>) {
    for node in nodes {
        match node {
            Node::Branch(branch) => {
                if let Some(label) = dead.get(&(branch.location.line, branch.location.column)) {
                    for arm in &mut branch.arms {
                        if arm.label == *label && !arm.facts.contains(&Fact::Never) {
                            arm.facts.push(Fact::Never);
                        }
                    }
                }
                for arm in &mut branch.arms {
                    add_facts(&mut arm.body, dead);
                }
            }
            Node::Transaction(scope) => add_facts(&mut scope.body, dead),
            Node::Loop(lp) => add_facts(&mut lp.body, dead),
            Node::Call(call) => add_facts(&mut call.body, dead),
            Node::Switch(sw) => {
                for case in &mut sw.cases {
                    add_facts(&mut case.body, dead);
                }
            }
            Node::Try(t) => {
                add_facts(&mut t.block, dead);
                if let Some(h) = &mut t.handler {
                    add_facts(&mut h.body, dead);
                }
                if let Some(f) = &mut t.finalizer {
                    add_facts(f, dead);
                }
            }
            _ => {}
        }
    }
}

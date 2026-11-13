//! Lowering of one function body from the Oxc AST to the Effect IR.
//!
//! Statements are lowered by explicit recursion so that control-flow shape is
//! preserved. Expressions are lowered with Oxc's [`Visit`] so that every
//! expression form is traversed (in source order, which matches JavaScript's
//! left-to-right evaluation order for the constructs that matter here);
//! only the forms that carry effects or control flow are overridden.
//!
//! Nested functions are *not* lowered into their parent: defining a function
//! does not run it. They are analyzed as separate units, except callbacks
//! passed to a recognized `db.transaction(...)`, which run immediately and
//! are lowered inline as the transaction body.
//!
//! An awaited (or `async`-returned) call of a function declared in the same
//! file is lowered as an unknown effect and recorded as a [`LocalCallSite`];
//! after every function is lowered, the core replaces it with the callee's
//! summary (`midwrite_core::analysis::inline_local_calls`).

use std::collections::{HashMap, HashSet};

use oxc_ast::ast::{
    Argument, ArrayExpressionElement, ArrowFunctionBody, ArrowFunctionExpression,
    AssignmentExpression, AssignmentTarget, AwaitExpression, BindingPattern, CallExpression,
    ChainElement, Class, ConditionalExpression, Expression, ForStatementInit, FormalParameters,
    Function, IdentifierReference, ImportExpression, LogicalExpression, NewExpression,
    ObjectPropertyKind, PropertyKind, SimpleAssignmentTarget, Statement, TaggedTemplateExpression,
    UpdateExpression, VariableDeclaration,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::{GetSpan, Span};
use oxc_syntax::operator::{BinaryOperator, LogicalOperator, UnaryOperator};
use oxc_syntax::scope::ScopeFlags;

use midwrite_core::absint::summary::ParamUse;
use midwrite_core::absint::{AbsValue, Atom, ObjectKind, Provenance};
use midwrite_core::analysis::{CallArgument, RETURN_VARIABLE};
use midwrite_core::ir::{
    Arm, Branch, BranchKind, DbHandle, DeferredHttpMethod, Effect, EffectId, EffectKind, Execution,
    Exit, ExitKind, Fact, FactInput, FunctionIr, Handler, Literal, Loop, LoopKind, Node,
    Recognition, Switch, TransactionId, TransactionInfo, TransactionScope, Try, UnknownReason,
};
use midwrite_core::location::{LineIndex, Location};

use crate::bindings::{ImportKind, LocalBindings, Origin, literal_value, resolve_reference};
use crate::builtins::{self, Builtin, Callback};
use crate::classes::{FieldClass, ReceiverKind};
use crate::recognize::{
    Builder, Classified, DbAccess, Marker, Recognizer, compact, truncate, unwind,
};
use crate::values::{
    IMPORTED_CLASS, MAX_DRY_RUNS, MAX_LOOP_ITERATIONS, ValueState, ValueStats, key,
};

/// A function body to lower.
#[derive(Clone, Copy)]
pub(crate) enum Body<'e, 'a> {
    Statements(&'e [Statement<'a>]),
    Arrow(&'e ArrowFunctionBody<'a>),
    Expression(&'e Expression<'a>),
}

pub(crate) struct Lowered {
    pub ir: FunctionIr,
    /// Start offsets of callbacks lowered inline (transaction bodies). They
    /// must not be analyzed again as separate functions.
    pub consumed: Vec<u32>,
    /// Calls of functions declared in the same file.
    pub local_calls: Vec<LocalCallSite>,
    /// What the body says about inertness and parameter use.
    pub facts: BodyFacts,
    pub stats: ValueStats,
}

/// What `this` is inside the function being lowered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ThisClass {
    /// The class (start offset).
    pub class: u32,
    /// A static method: `this` is the class itself.
    pub is_static: bool,
}

/// A class as named by a call target.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ClassRef {
    /// Declared in this module (start offset).
    Local(u32),
    /// Imported from `module` under `name`.
    Imported { module: String, name: String },
}

/// An argument of a resolved call, for summaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FactArg {
    Other,
    /// A function of this module (start offset).
    Function(u32),
    Param(u32),
    Closure {
        function: u32,
        parameters: u32,
        captures: Box<[FactArg]>,
    },
}

/// A call whose target the frontend resolved (possibly only after linking).
#[derive(Clone, Debug)]
pub(crate) struct FactCall {
    pub target: CallTarget,
    pub args: Vec<FactArg>,
    pub label: String,
}

/// What one function body says about its inertness
/// (`midwrite_core::absint::summary`).
#[derive(Clone, Debug, Default)]
pub(crate) struct BodyFacts {
    /// The first reason the body itself is not inert.
    pub refuted: Option<String>,
    pub calls: Vec<FactCall>,
    pub params: Vec<ParamUse>,
}

/// A summary call fact of a lowered module: target and label are indices
/// into the module's [`FactTable`] (targets and labels repeat a lot).
#[derive(Clone, Debug)]
pub(crate) struct CompactCall {
    pub target: u32,
    pub label: u32,
    /// Empty when every argument is [`FactArg::Other`].
    pub args: Box<[FactArg]>,
}

/// [`BodyFacts`] with interned targets and labels.
#[derive(Clone, Debug, Default)]
pub(crate) struct CompactFacts {
    pub refuted: Option<String>,
    pub calls: Vec<CompactCall>,
    pub params: Vec<ParamUse>,
}

/// The distinct call targets and labels of a module's summary facts.
#[derive(Debug, Default)]
pub(crate) struct FactTable {
    pub targets: Vec<CallTarget>,
    pub labels: Vec<String>,
    target_index: HashMap<CallTarget, u32>,
    label_index: HashMap<String, u32>,
}

impl FactTable {
    pub fn compact(&mut self, facts: BodyFacts, preserve_calls: bool) -> CompactFacts {
        // A body that refutes inertness by itself is not inert whatever its
        // callees are; its calls matter only when they pass one of its
        // parameters on (parameter use propagates to its callers).
        let refuted = facts.refuted.is_some();
        let calls = facts
            .calls
            .into_iter()
            .filter(|call| {
                preserve_calls
                    || !refuted
                    || call
                        .args
                        .iter()
                        .any(|a| matches!(a, FactArg::Param(_) | FactArg::Closure { .. }))
            })
            .map(|call| {
                let target = match self.target_index.get(&call.target) {
                    Some(index) => *index,
                    None => {
                        let index = u32::try_from(self.targets.len()).unwrap_or(u32::MAX);
                        self.targets.push(call.target.clone());
                        self.target_index.insert(call.target, index);
                        index
                    }
                };
                let label = match self.label_index.get(&call.label) {
                    Some(index) => *index,
                    None => {
                        let index = u32::try_from(self.labels.len()).unwrap_or(u32::MAX);
                        self.labels.push(call.label.clone());
                        self.label_index.insert(call.label, index);
                        index
                    }
                };
                let args: Box<[FactArg]> = if call.args.iter().all(|a| *a == FactArg::Other) {
                    Box::default()
                } else {
                    call.args.into_boxed_slice()
                };
                CompactCall {
                    target,
                    label,
                    args,
                }
            })
            .collect();
        // `collect` may reuse the (larger) source allocation: shrink it.
        let mut calls: Vec<CompactCall> = calls;
        calls.shrink_to_fit();
        let mut params = facts.params;
        params.shrink_to_fit();
        CompactFacts {
            refuted: facts.refuted,
            calls,
            params,
        }
    }

    /// Release the lookup maps (only the tables are needed for linking).
    pub fn finish(&mut self) {
        self.target_index = HashMap::new();
        self.label_index = HashMap::new();
        self.targets.shrink_to_fit();
        self.labels.shrink_to_fit();
    }
}

impl BodyFacts {
    fn refute(&mut self, reason: String) {
        if self.refuted.is_none() {
            self.refuted = Some(reason);
        }
    }

    fn param(&mut self, index: u32) -> &mut ParamUse {
        let index = index as usize;
        if index >= self.params.len() {
            self.params.resize(index + 1, ParamUse::default());
        }
        &mut self.params[index]
    }
}

/// A call of a function declared in the same file or imported from another
/// module, standing in the IR as an unknown effect until the callee's summary
/// replaces it.
#[derive(Clone, Debug)]
pub(crate) struct LocalCallSite {
    pub effect: EffectId,
    pub target: CallTarget,
    /// The unknown effect exists only to mark the call (`return f()` in an
    /// `async` function); it is dropped when the call is not summarized.
    pub placeholder: bool,
    /// An unawaited call whose value is used only to resolve a later call.
    pub value_only: bool,
    /// Database handles passed as arguments, by position.
    pub arguments: Vec<Option<CallArgument>>,
    /// Abstract values of the arguments (functions by start offset).
    pub values: Vec<AbsValue>,
    pub receiver: Option<AbsValue>,
    /// The arguments as the callee receives them
    /// (`LocalCall::entry_values`).
    pub entry_values: Vec<AbsValue>,
    /// The number of arguments, when no argument is a spread
    /// (`LocalCall::argument_count`).
    pub argument_count: Option<u32>,
    /// The `CallResult` site of this call's value.
    pub result: Option<u64>,
    /// The caller's binding of the result (a fact variable), and whether
    /// property facts may be attached to it.
    pub binding: Option<(String, bool)>,
    /// Set for an immediately invoked function expression.
    pub immediate: bool,
    /// Placeholder effects of arguments, by parameter.
    pub argument_effects: Vec<(u32, EffectId)>,
}

/// What a plain call names.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CallTarget {
    /// A function declared in this file, by start offset.
    Local(u32),
    /// A function imported from `module` under the exported `name`
    /// (`default` for a default import), or, with `member`, a function
    /// property of an object service exported under that name.
    Imported {
        module: String,
        name: String,
        member: Option<String>,
    },
    /// The function the caller passes for this parameter.
    Param(u32),
    /// The function returned by an earlier call in this frame.
    Returned(u64),
    /// A closure or stable module value, resolved after linking.
    Value(AbsValue),
    /// Method `name` found by dispatch on a receiver of class `class`.
    Method {
        class: ClassRef,
        name: String,
        receiver: ReceiverKind,
    },
    /// The constructor run by `new C(...)` (none when implicit all the way
    /// up a known chain).
    Constructor(ClassRef),
    /// `super(...)` (`name: None`) or `super.name(...)` inside class `class`
    /// (local start offset).
    Super { class: u32, name: Option<String> },
}

/// Whether control may reach the end of a function body (a conservative
/// syntactic check: the body's last statement is not a `return` or
/// `throw`).
fn may_fall_through(body: Body<'_, '_>) -> bool {
    let statements = match body {
        Body::Statements(statements) => statements,
        Body::Arrow(ArrowFunctionBody::FunctionBody(block)) => &block.statements[..],
        Body::Arrow(_) => return false,
        Body::Expression(_) => return false,
    };
    !matches!(
        statements.last(),
        Some(Statement::ReturnStatement(_) | Statement::ThrowStatement(_))
    )
}

/// What [`Lowerer::checkpoint`] saves.
struct Checkpoint {
    next_effect: u32,
    next_inline_call: u32,
    transactions: usize,
    local_calls: usize,
    consumed: usize,
    facts: BodyFacts,
    placeholders: usize,
    returns: AbsValue,
}

/// A call target with what the call site knows about the receiver.
struct ResolvedTarget {
    target: CallTarget,
    /// `this` inside the callee.
    receiver: Option<AbsValue>,
    /// An immediately invoked function expression.
    immediate: bool,
}

impl ResolvedTarget {
    fn new(target: CallTarget) -> Self {
        Self {
            target,
            receiver: None,
            immediate: false,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_function(
    recognizer: &Recognizer<'_>,
    lines: &LineIndex<'_>,
    name: String,
    span: Span,
    is_async: bool,
    is_generator: bool,
    this_class: Option<ThisClass>,
    params: Option<&FormalParameters<'_>>,
    body: Body<'_, '_>,
) -> Lowered {
    // A single-use collection joined by Promise.all cannot have escaped or
    // been mutated between construction and its join. Keep its callbacks
    // connected instead of treating the identifier as an opaque promise.
    struct Joins<'s> {
        scoping: &'s oxc_semantic::Scoping,
        symbols: HashSet<oxc_syntax::symbol::SymbolId>,
    }
    impl<'a> Visit<'a> for Joins<'_> {
        fn visit_await_expression(&mut self, it: &AwaitExpression<'a>) {
            if let Expression::CallExpression(call) = it.argument.get_inner_expression()
                && let Expression::StaticMemberExpression(member) =
                    call.callee.get_inner_expression()
                && member.property.name == "all"
                && let Expression::Identifier(root) = member.object.get_inner_expression()
                && root.name == "Promise"
                && resolve_reference(self.scoping, root).is_none()
                && let Some(Expression::Identifier(collection)) =
                    call.arguments.first().and_then(Argument::as_expression)
                && let Some(symbol) = resolve_reference(self.scoping, collection)
                && !self.scoping.symbol_is_mutated(symbol)
                && self.scoping.get_resolved_references(symbol).count() == 1
            {
                self.symbols.insert(symbol);
            }
            walk::walk_await_expression(self, it);
        }
        fn visit_function(&mut self, _: &Function<'a>, _: ScopeFlags) {}
        fn visit_arrow_function_expression(&mut self, _: &ArrowFunctionExpression<'a>) {}
    }
    let mut joins = Joins {
        scoping: recognizer.scoping,
        symbols: HashSet::new(),
    };
    match body {
        Body::Statements(statements) => {
            for statement in statements {
                joins.visit_statement(statement);
            }
        }
        Body::Arrow(body) => joins.visit_arrow_function_body(body),
        Body::Expression(expression) => joins.visit_expression(expression),
    }
    let mut locals = LocalBindings::default();
    if let Some(params) = params {
        for (index, param) in params.items.iter().enumerate() {
            if let BindingPattern::BindingIdentifier(id) = &param.pattern
                && param.initializer.is_none()
                && let Some(symbol) = id.symbol_id.get()
                && !recognizer.scoping.symbol_is_mutated(symbol)
            {
                let index = u32::try_from(index).unwrap_or(u32::MAX);
                locals
                    .params
                    .insert(symbol, (index, id.name.as_str().to_owned()));
            }
            if let BindingPattern::BindingIdentifier(id) = &param.pattern
                && param.initializer.as_deref().is_none_or(empty_options)
                && let Some(symbol) = id.symbol_id.get()
                && !recognizer.scoping.symbol_is_mutated(symbol)
                && recognizer
                    .table
                    .other_references(symbol, recognizer.scoping)
                    == Some(1)
            {
                let index = u32::try_from(index).unwrap_or(u32::MAX);
                locals
                    .option_params
                    .insert(symbol, (index, id.name.as_str().to_owned()));
            }
        }
    }
    let mut lowerer = Lowerer {
        recognizer,
        lines,
        locals,
        transactions: Vec::new(),
        next_effect: 0,
        out: Vec::new(),
        is_async,
        consumed: Vec::new(),
        local_calls: Vec::new(),
        pending_result: None,
        return_binding: None,
        vs: ValueState::default(),
        call_values: HashMap::new(),
        assigned: HashMap::new(),
        returns: AbsValue::bottom(),
        facts: BodyFacts::default(),
        this_class,
        callback_depth: 0,
        next_site: 0,
        next_inline_call: 0,
        next_opaque: 0,
        stats: ValueStats::default(),
        dry_runs: 0,
        argument_placeholders: Vec::new(),
        read_results: HashSet::new(),
        imported_classes: HashMap::new(),
        early_news: std::collections::HashSet::new(),
        span,
        joined_collections: joins.symbols,
        joined_maps: HashSet::new(),
        joined_bindings: HashSet::new(),
        param_snapshots: HashMap::new(),
        rest_snapshots: HashMap::new(),
        binding_snapshot: false,
        stable_params: params
            .map(|params| stable_parameters(params, recognizer))
            .unwrap_or_default(),
        fact_inputs: std::cell::RefCell::new(std::collections::BTreeMap::new()),
        derived_inputs: HashMap::new(),
        loop_depth: 0,
        row_effects: Vec::new(),
        return_rows: Vec::new(),
        module_initializer: matches!(body, Body::Expression(_)),
        invalid_module_read: std::cell::Cell::new(false),
        capture_defaults: HashMap::new(),
    };
    if is_generator {
        lowerer
            .facts
            .refute("is a generator: its body runs lazily, as the caller iterates".to_owned());
    }
    if let Some(params) = params {
        lowerer.bind_parameters(params);
    }
    if let Some(layout) = recognizer.table.closures.get(&span.start) {
        for (slot, symbol) in layout.symbols.iter().enumerate() {
            let index = layout.parameters + slot as u32;
            let value = AbsValue::atom(Atom::Param { index });
            lowerer.vs.env_mut().set(key(*symbol), value.clone());
            lowerer.param_snapshots.insert(*symbol, value);
            lowerer.locals.params.insert(
                *symbol,
                (index, recognizer.scoping.symbol_name(*symbol).to_owned()),
            );
        }
    }
    lowerer.body(body);
    lowerer.mark_returned_rows(may_fall_through(body));
    if lowerer.invalid_module_read.get() {
        lowerer
            .facts
            .refute("reads a module binding before initialization".to_owned());
        lowerer.returns = AbsValue::top();
    }
    lowerer.out.shrink_to_fit();
    lowerer.transactions.shrink_to_fit();
    let mut returns = std::mem::take(&mut lowerer.returns);
    if may_fall_through(body) {
        // Falling off the end returns `undefined`.
        returns.join(&AbsValue::lit(Literal::Undefined));
    }
    // Nothing to substitute: callers treat a missing value as unknown.
    let returns = (!returns.is_top()
        && !returns.is_bottom()
        && returns.single() != Some(&Atom::lit(Literal::Undefined)))
    .then(|| Box::new(returns));
    let mut stats = lowerer.stats;
    stats.states += 1;
    Lowered {
        ir: FunctionIr {
            name,
            location: lines.location(span.start, span.end),
            is_async,
            transactions: lowerer.transactions,
            body: lowerer.out,
            returns,
            fact_inputs: lowerer.fact_inputs.into_inner(),
        },
        consumed: lowerer.consumed,
        local_calls: lowerer.local_calls,
        facts: lowerer.facts,
        stats,
    }
}

/// How the value of an expression is consumed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Consumer {
    /// `await expr`
    Await,
    /// `return expr` in an async function: the returned thenable is adopted.
    AsyncReturn,
    /// `return expr` in a non-async function: the caller decides.
    SyncReturn,
    /// An element of `await Promise.all([...])`.
    Concurrent,
    /// An argument of a summarized call: it runs only if the callee awaits
    /// that parameter (its effects are placeholders), except for operations
    /// that start by themselves.
    Argument,
}

impl Consumer {
    fn execution(self) -> Execution {
        match self {
            Self::Await => Execution::Awaited,
            Self::AsyncReturn | Self::SyncReturn => Execution::Returned,
            Self::Concurrent | Self::Argument => Execution::Concurrent,
        }
    }

    /// Whether a lazy Drizzle builder consumed this way executes. A builder
    /// returned from a non-async function runs only if the caller awaits the
    /// call; see [`Lowerer::executes_lazy`].
    fn awaits(self) -> bool {
        !matches!(self, Self::SyncReturn)
    }

    /// Whether an unrecognized call consumed this way is recorded as an
    /// unknown effect. Only an explicit `await` (or `Promise.all`) signals
    /// asynchronous work; `return f()` is usually plain value construction.
    fn records_unknown(self) -> bool {
        matches!(self, Self::Await | Self::Concurrent | Self::Argument)
    }

    /// Whether a call of a resolved function consumed this way is replaced
    /// by the callee's summary: the caller waits for it to finish (or, for
    /// concurrent elements, starts it together with others).
    fn summarizes_local_calls(self) -> bool {
        matches!(
            self,
            Self::Await | Self::AsyncReturn | Self::SyncReturn | Self::Concurrent
        )
    }
}

pub(crate) struct Lowerer<'c> {
    pub(crate) invalid_module_read: std::cell::Cell<bool>,
    pub(crate) capture_defaults: HashMap<oxc_syntax::symbol::SymbolId, (u32, Literal)>,
    pub(crate) module_initializer: bool,
    pub(crate) recognizer: &'c Recognizer<'c>,
    lines: &'c LineIndex<'c>,
    pub(crate) locals: LocalBindings,
    joined_collections: HashSet<oxc_syntax::symbol::SymbolId>,
    joined_maps: HashSet<u32>,
    joined_bindings: HashSet<oxc_syntax::symbol::SymbolId>,
    /// Bindings that hold a snapshot of a parameter or of a property path
    /// of one, taken where the function starts or destructures it, with
    /// that value (`param#1.status`).
    pub(crate) param_snapshots: HashMap<oxc_syntax::symbol::SymbolId, AbsValue>,
    /// Rest objects of destructured parameters (`const { id, ...data } =
    /// params`): the destructured value and the keys left out of the rest.
    pub(crate) rest_snapshots: HashMap<oxc_syntax::symbol::SymbolId, (AbsValue, Vec<String>)>,
    /// Set while binding a pattern whose value is read from a parameter
    /// that nothing changes before (see [`Self::snapshot_source`]).
    pub(crate) binding_snapshot: bool,
    /// Parameters a call may pass on as inputs (`stable_parameters`).
    stable_params: HashSet<u32>,
    /// [`FunctionIr::fact_inputs`] of the facts recorded so far.
    fact_inputs: std::cell::RefCell<std::collections::BTreeMap<String, FactInput>>,
    /// `const` bindings of an input read (`const send = options?.send ??
    /// false`, see [`Self::input_expression`]), with that input.
    derived_inputs: HashMap<oxc_syntax::symbol::SymbolId, FactInput>,
    /// Depth of loop bodies being lowered.
    loop_depth: usize,
    /// Writes whose first `RETURNING` row is bound to a variable
    /// (`const [row] = await db.update(t)...returning()`), with it.
    row_effects: Vec<(EffectId, String)>,
    /// Each `return` of the function (not of a callback): the variable it
    /// returns when it returns a bound row (`return row`, `return row ??
    /// null`).
    return_rows: Vec<(Location, Option<String>)>,
    next_inline_call: u32,
    transactions: Vec<TransactionInfo>,
    next_effect: u32,
    /// Nodes of the block currently being lowered.
    out: Vec<Node>,
    /// Whether the function whose body is being lowered is `async`.
    is_async: bool,
    consumed: Vec<u32>,
    local_calls: Vec<LocalCallSite>,
    /// A `const` whose initializer awaits the call starting at this offset
    /// (`const result = await db.transaction(cb)`), with its fact variable
    /// and binding. `None` for the binding: the call is this function's
    /// returned value (`return f(...)`), whose facts are this function's
    /// return facts ([`RETURN_VARIABLE`]); the caller of this function
    /// decides whether its own binding's members are stable.
    pending_result: Option<(u32, String, Option<oxc_syntax::symbol::SymbolId>)>,
    /// The variable bound to the result of the transaction whose callback is
    /// being lowered.
    return_binding: Option<String>,
    // --- Phase 5 value layer -------------------------------------------
    /// Environment and heap at the current point.
    pub(crate) vs: ValueState,
    /// Abstract value of each lowered call and `new`, by start offset.
    pub(crate) call_values: HashMap<u32, AbsValue>,
    /// Value of each assignment expression, by start offset.
    pub(crate) assigned: HashMap<u32, AbsValue>,
    /// Join of the values the function returns.
    returns: AbsValue,
    facts: BodyFacts,
    pub(crate) this_class: Option<ThisClass>,
    /// Depth of inline transaction callbacks being lowered (their `return`
    /// does not return from the function).
    callback_depth: usize,
    next_site: u64,
    next_opaque: u64,
    pub(crate) stats: ValueStats,
    dry_runs: usize,
    /// Effects created for arguments of summarized calls (placeholders).
    argument_placeholders: Vec<EffectId>,
    /// Effects that are database reads (their results are arrays of rows).
    read_results: HashSet<u32>,
    /// Classes of instance objects created from imported classes.
    imported_classes: HashMap<u32, ClassRef>,
    /// `new` expressions lowered early as a method call's receiver, to be
    /// skipped when the walk reaches them.
    early_news: std::collections::HashSet<u32>,
    /// The function's span (closures inside it are its own).
    pub(crate) span: Span,
}

impl<'a> Lowerer<'_> {
    fn body(&mut self, body: Body<'_, 'a>) {
        match body {
            Body::Statements(statements) => self.statements(statements),
            Body::Expression(expression) => {
                self.visit_expression(expression);
                let value = self.eval(expression);
                self.returns.join(&self.vs.heap.export(&value));
                self.exit(ExitKind::Return, expression.span(), None, None);
            }
            Body::Arrow(ArrowFunctionBody::FunctionBody(block)) => {
                self.statements(&block.statements);
            }
            Body::Arrow(expression) => {
                if let Some(expression) = expression.as_expression() {
                    self.returned(expression);
                }
            }
        }
    }

    fn statements(&mut self, statements: &[Statement<'a>]) {
        for statement in statements {
            self.statement(statement);
        }
    }

    fn statement(&mut self, statement: &Statement<'a>) {
        match statement {
            Statement::BlockStatement(block) => self.statements(&block.body),
            Statement::ExpressionStatement(statement) => {
                self.visit_expression(&statement.expression)
            }
            Statement::VariableDeclaration(declaration) => self.variable_declaration(declaration),
            Statement::IfStatement(statement) => {
                self.visit_expression(&statement.test);
                let before = self.fork();
                let then = self.collect(|this| this.statement(&statement.consequent));
                let after_then = std::mem::replace(&mut self.vs, before);
                let otherwise = self.collect(|this| {
                    if let Some(alternate) = &statement.alternate {
                        this.statement(alternate);
                    }
                });
                self.rejoin(&after_then);
                let arms = vec![
                    self.arm_if("then", then, &statement.test, true),
                    self.arm_if("else", otherwise, &statement.test, false),
                ];
                self.push_branch(BranchKind::If, statement.test.span(), statement.span, arms);
            }
            Statement::ReturnStatement(statement) => {
                if self.callback_depth == 0 {
                    let row = self.returned_row(statement.argument.as_ref());
                    let location = self.location(statement.span);
                    self.return_rows.push((location, row));
                }
                match &statement.argument {
                    Some(argument) => self.returned(argument),
                    None if self.callback_depth == 0 => {
                        self.returns.join(&AbsValue::lit(Literal::Undefined));
                    }
                    None => {}
                }
                let mut facts = self.return_facts(statement.argument.as_ref());
                if self.callback_depth == 0 {
                    facts.extend(self.returned_value_facts(statement.argument.as_ref()));
                }
                self.exit(ExitKind::Return, statement.span, None, None);
                if let Some(Node::Exit(exit)) = self.out.last_mut() {
                    exit.facts = facts;
                }
            }
            Statement::ThrowStatement(statement) => {
                self.visit_expression(&statement.argument);
                self.exit(ExitKind::Throw, statement.span, None, None);
            }
            Statement::TryStatement(statement) => {
                let before = self.fork();
                let block = self.collect(|this| this.statements(&statement.block.body));
                let after_block = self.vs.clone();
                let handler = statement.handler.as_ref().map(|handler| {
                    // The handler may start at any point of the block.
                    self.rejoin(&before);
                    if let Some(parameter) = &handler.param {
                        self.havoc_pattern(&parameter.pattern);
                    }
                    let body = self.collect(|this| this.statements(&handler.body.body));
                    Handler {
                        location: self.location(handler.span),
                        body,
                    }
                });
                if handler.is_some() {
                    self.rejoin(&after_block);
                }
                let finalizer = statement
                    .finalizer
                    .as_ref()
                    .map(|finalizer| self.collect(|this| this.statements(&finalizer.body)));
                let empty = block.is_empty()
                    && handler
                        .as_ref()
                        .is_none_or(|handler| handler.body.is_empty())
                    && finalizer.as_ref().is_none_or(Vec::is_empty);
                if !empty {
                    let location = self.location(statement.span);
                    self.out.push(Node::Try(Try {
                        location,
                        block,
                        handler,
                        finalizer,
                    }));
                }
            }
            Statement::ForStatement(statement) => {
                match &statement.init {
                    Some(ForStatementInit::VariableDeclaration(declaration)) => {
                        self.variable_declaration(declaration);
                    }
                    Some(init) => {
                        if let Some(expression) = init.as_expression() {
                            self.visit_expression(expression);
                        }
                    }
                    None => {}
                }
                if let Some(test) = &statement.test {
                    self.visit_expression(test);
                }
                let body = self.loop_body(&|this: &mut Self| {
                    this.statement(&statement.body);
                    if let Some(update) = &statement.update {
                        this.visit_expression(update);
                    }
                    if let Some(test) = &statement.test {
                        this.visit_expression(test);
                    }
                });
                self.push_loop(
                    LoopKind::For,
                    statement.span,
                    statement.body.span(),
                    false,
                    body,
                );
            }
            Statement::ForOfStatement(statement) => {
                self.visit_expression(&statement.right);
                let body = self.loop_body(&|this: &mut Self| {
                    this.loop_variable(&statement.left);
                    this.statement(&statement.body);
                });
                self.push_loop(
                    LoopKind::ForOf,
                    statement.span,
                    statement.body.span(),
                    false,
                    body,
                );
            }
            Statement::ForInStatement(statement) => {
                self.visit_expression(&statement.right);
                let body = self.loop_body(&|this: &mut Self| {
                    this.loop_variable(&statement.left);
                    this.statement(&statement.body);
                });
                self.push_loop(
                    LoopKind::ForIn,
                    statement.span,
                    statement.body.span(),
                    false,
                    body,
                );
            }
            Statement::WhileStatement(statement) => {
                self.visit_expression(&statement.test);
                let body = self.loop_body(&|this: &mut Self| {
                    this.statement(&statement.body);
                    this.visit_expression(&statement.test);
                });
                self.push_loop(
                    LoopKind::While,
                    statement.span,
                    statement.body.span(),
                    false,
                    body,
                );
            }
            Statement::DoWhileStatement(statement) => {
                let body = self.loop_body(&|this: &mut Self| {
                    this.statement(&statement.body);
                    this.visit_expression(&statement.test);
                });
                self.push_loop(
                    LoopKind::DoWhile,
                    statement.span,
                    statement.body.span(),
                    true,
                    body,
                );
            }
            Statement::SwitchStatement(statement) => {
                self.visit_expression(&statement.discriminant);
                for case in &statement.cases {
                    if let Some(test) = &case.test {
                        self.visit_expression(test);
                    }
                }
                let mut cases = Vec::with_capacity(statement.cases.len());
                let mut has_default = false;
                let variable = self.condition_variable(&statement.discriminant);
                // `switch` compares with `===`: entering a case with a literal
                // test means the discriminant equals it; entering `default`
                // means it equals none of them.
                let literals: Vec<Literal> = statement
                    .cases
                    .iter()
                    .filter_map(|case| case.test.as_ref().and_then(|t| self.literal_test(t)))
                    .collect();
                let before = self.fork();
                let mut ends = before.clone();
                for case in &statement.cases {
                    let label = match &case.test {
                        Some(test) => format!("case {}", self.text(test.span())),
                        None => {
                            has_default = true;
                            "default".to_owned()
                        }
                    };
                    // Entered directly, or by falling through the previous case.
                    self.rejoin(&before);
                    let body = self.collect(|this| this.statements(&case.consequent));
                    let end = self.vs.clone();
                    let mut stats = std::mem::take(&mut self.stats);
                    ends.join(&end, &mut stats);
                    self.stats = stats;
                    let facts = match (&variable, &case.test) {
                        (Some(Variable::Free(name)), Some(test)) => self
                            .literal_test(test)
                            .map(|literal| {
                                vec![Fact::Equals {
                                    variable: name.clone(),
                                    literal,
                                }]
                            })
                            .unwrap_or_default(),
                        (Some(Variable::Free(name)), None) => literals
                            .iter()
                            .map(|literal| Fact::NotEquals {
                                variable: name.clone(),
                                literal: literal.clone(),
                            })
                            .collect(),
                        _ => Vec::new(),
                    };
                    cases.push(Arm { label, body, facts });
                }
                self.vs = ends;
                if cases.iter().any(|case| !case.body.is_empty()) {
                    self.out.push(Node::Switch(Switch {
                        discriminant: self.text(statement.discriminant.span()),
                        location: self.location(statement.span),
                        cases,
                        has_default,
                    }));
                }
            }
            Statement::BreakStatement(statement) => match &statement.label {
                None => self.exit(ExitKind::Break, statement.span, None, None),
                Some(label) => {
                    self.facts
                        .refute("contains control flow Midwrite does not model".to_owned());
                    self.exit(
                        ExitKind::Unsupported,
                        statement.span,
                        None,
                        Some(format!("labeled `break {}`", label.name)),
                    )
                }
            },
            Statement::ContinueStatement(statement) => match &statement.label {
                None => self.exit(ExitKind::Continue, statement.span, None, None),
                Some(label) => {
                    self.facts
                        .refute("contains control flow Midwrite does not model".to_owned());
                    self.exit(
                        ExitKind::Unsupported,
                        statement.span,
                        None,
                        Some(format!("labeled `continue {}`", label.name)),
                    )
                }
            },
            Statement::LabeledStatement(statement) => self.statement(&statement.body),
            Statement::WithStatement(statement) => {
                self.visit_expression(&statement.object);
                self.statement(&statement.body);
            }
            // Declarations do not execute anything; nested functions and
            // classes are analyzed separately. Type-only declarations, module
            // declarations, `debugger` and empty statements have no effects.
            _ => {}
        }
    }

    fn variable_declaration(&mut self, declaration: &VariableDeclaration<'a>) {
        for declarator in &declaration.declarations {
            let Some(init) = &declarator.init else {
                self.bind_pattern(&declarator.id, AbsValue::lit(Literal::Undefined));
                continue;
            };
            if declaration.kind.is_const()
                && let BindingPattern::BindingIdentifier(id) = &declarator.id
                && let Some(symbol) = id.symbol_id.get()
                && self.joined_collections.contains(&symbol)
                && let Expression::CallExpression(call) = init.get_inner_expression()
                && self.array_callback_target(call).is_some()
            {
                self.joined_maps.insert(call.span.start);
                self.joined_bindings.insert(symbol);
            }
            // `const result = await db.transaction(cb)`: the callback's
            // returned values tell what `result` holds afterwards.
            let saved = self.pending_result.take();
            if let BindingPattern::BindingIdentifier(id) = &declarator.id
                && let Expression::AwaitExpression(awaited) = init.get_inner_expression()
                && let Expression::CallExpression(call) = awaited.argument.get_inner_expression()
                && let Some(symbol) = id.symbol_id.get()
                && let Some(Variable::Free(variable)) = self.binding_variable(symbol, &id.name)
            {
                self.pending_result = Some((call.span.start, variable, Some(symbol)));
            }
            let before = self.out.len();
            self.visit_expression(init);
            self.pending_result = saved;
            self.note_returned_row(declaration, &declarator.id, init, before);
            let value = self.eval(init);
            self.binding_snapshot = self.snapshot_source(init);
            self.bind_pattern(&declarator.id, value);
            if declaration.kind.is_const()
                && let BindingPattern::BindingIdentifier(id) = &declarator.id
                && let Some(symbol) = id.symbol_id.get()
                && let Expression::Identifier(source) = init.get_inner_expression()
                && let Some(source) = resolve_reference(self.recognizer.scoping, source)
                && let Some(default) = self.capture_defaults.get(&source).cloned()
            {
                self.capture_defaults.insert(symbol, default);
            }
            self.binding_snapshot = false;
            // Read once, where the function runs it once.
            if self.loop_depth == 0
                && self.callback_depth == 0
                && let BindingPattern::BindingIdentifier(id) = &declarator.id
                && let Some(symbol) = id.symbol_id.get()
                && !self.param_snapshots.contains_key(&symbol)
                && let Some(input) = self.input_expression(init, init.span().start)
            {
                self.derived_inputs.insert(symbol, input);
            }
            // `const q = db.update(t).set(v)` binds an unexecuted builder that
            // a later `await q` executes.
            if let BindingPattern::BindingIdentifier(id) = &declarator.id
                && let Some(symbol) = id.symbol_id.get()
                && let Some(builder) = self.lazy_builder(init)
            {
                self.locals.pending_builders.insert(symbol, builder);
            }
        }
    }

    fn lazy_builder(&self, init: &Expression<'a>) -> Option<Builder> {
        let Expression::CallExpression(_) = init.get_inner_expression() else {
            return None;
        };
        match self.recognizer.classify(&unwind(init), &self.locals) {
            Classified::Db {
                builder,
                marker: Marker::Lazy,
                ..
            } => Some(builder),
            _ => None,
        }
    }

    fn returned(&mut self, expression: &Expression<'a>) {
        let consumer = if self.is_async {
            Consumer::AsyncReturn
        } else {
            Consumer::SyncReturn
        };
        // `return f(...)` / `return await f(...)` (through parentheses and
        // type assertions): `f`'s return facts are this function's on that
        // path, so a caller testing a property of the result can correlate
        // it with `f`'s returns.
        let saved = self.pending_result.take();
        if self.callback_depth == 0 {
            let inner = match expression.get_inner_expression() {
                Expression::AwaitExpression(awaited) => awaited.argument.get_inner_expression(),
                other => other,
            };
            if let Expression::CallExpression(call) = inner {
                self.pending_result = Some((call.span.start, RETURN_VARIABLE.to_owned(), None));
            }
        }
        self.consume(expression, consumer);
        self.pending_result = saved;
        if self.callback_depth == 0 {
            let value = self.eval(expression);
            let exported = self.vs.heap.export(&value);
            // The caller receives it: objects in it may be changed there.
            self.vs.escape(&value);
            self.returns.join(&exported);
        }
    }

    /// Whether a lazy builder consumed this way executes. Besides awaited
    /// positions, a builder that a non-async function returns from its own
    /// body (`(db) => db.update(t)...`, `function save() { return
    /// db.insert(t)... }`) is part of what the function does: its summary is
    /// applied only where a caller awaits or adopts the call
    /// ([`Consumer::summarizes_local_calls`]), which is when the builder
    /// runs. Returns of nested callbacks (array callbacks, transaction
    /// callbacks) keep the older rule.
    fn executes_lazy(&self, consumer: Consumer) -> bool {
        consumer.awaits() || (consumer == Consumer::SyncReturn && self.callback_depth == 0)
    }

    /// How a lazy builder consumed this way runs (when
    /// [`Self::executes_lazy`]).
    fn lazy_execution(consumer: Consumer) -> Execution {
        match consumer {
            Consumer::SyncReturn => Execution::ReturnedLazy,
            other => other.execution(),
        }
    }

    /// Lower an expression whose value is awaited, returned or run concurrently.
    fn consume(&mut self, expression: &Expression<'a>, consumer: Consumer) {
        let inner = expression.get_inner_expression();
        if consumer.awaits()
            && matches!(
                inner,
                Expression::Identifier(_) | Expression::StaticMemberExpression(_)
            )
        {
            // Awaiting (or adopting) a parameter: a lazy builder a caller
            // passes for it executes here.
            let value = self.eval(inner);
            self.note_awaited(&value);
        }
        match inner {
            Expression::CallExpression(call) => self.call(call, Some(consumer)),
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => self.call(call, Some(consumer)),
                _ => self.visit_expression(inner),
            },
            Expression::ConditionalExpression(conditional) => {
                self.visit_expression(&conditional.test);
                let before = self.fork();
                let then = self.collect(|this| this.consume(&conditional.consequent, consumer));
                let after_then = std::mem::replace(&mut self.vs, before);
                let otherwise = self.collect(|this| this.consume(&conditional.alternate, consumer));
                self.rejoin(&after_then);
                let arms = vec![
                    self.arm_if("then", then, &conditional.test, true),
                    self.arm_if("else", otherwise, &conditional.test, false),
                ];
                self.push_branch(
                    BranchKind::Conditional,
                    conditional.test.span(),
                    conditional.span,
                    arms,
                );
            }
            Expression::Identifier(ident) if self.executes_lazy(consumer) => {
                let resolved = self.recognizer.resolve(ident, &self.locals);
                if let Origin::PendingBuilder(symbol) = resolved.origin
                    && let Some(builder) = self.locals.pending_builders.get(&symbol).cloned()
                {
                    self.push_db_effect(builder, ident.span, Self::lazy_execution(consumer));
                }
            }
            _ => self.visit_expression(inner),
        }
    }

    /// Lower a call. `consumer` is `None` when the call's result is not
    /// awaited, returned or otherwise consumed by this function.
    fn call(&mut self, call: &CallExpression<'a>, consumer: Option<Consumer>) {
        self.call_unwrapped(call, consumer);
        if let Some(class) = self.declared_instance(call) {
            let id = self.class_id(class);
            let value = self
                .vs
                .heap_mut()
                .allocate(call.span.start, ObjectKind::Instance { class: id });
            self.call_values
                .insert(call.span.start, AbsValue::atom(value));
        }
    }

    /// Whether the receiver of `obj.m(...)` is a binding that holds an
    /// instance of a class of the project.
    fn instance_receiver(&self, call: &CallExpression<'a>) -> bool {
        let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
            return false;
        };
        let Expression::Identifier(object) = member.object.get_inner_expression() else {
            return false;
        };
        match self.eval_ident(object).single() {
            Some(Atom::Object { id }) => self
                .vs
                .heap
                .get(*id)
                .is_some_and(|o| matches!(o.kind, ObjectKind::Instance { .. })),
            _ => false,
        }
    }

    /// `classContracts[].returnedBy`: a call of a function (declared or
    /// imported here) by that name returns an instance of the class.
    fn declared_instance(&self, call: &CallExpression<'a>) -> Option<ClassRef> {
        let Expression::Identifier(callee) = call.callee.get_inner_expression() else {
            return None;
        };
        let symbol = resolve_reference(self.recognizer.scoping, callee)?;
        let table = self.recognizer.table;
        if table.import(symbol).is_none() && table.local_function(symbol).is_none() {
            return None;
        }
        let path = table.path();
        let contract = self.recognizer.config.class_contracts.iter().find(|c| {
            c.returned_by
                .iter()
                .any(|name| name == callee.name.as_str())
        })?;
        if contract.file == path {
            let start = table
                .classes()
                .iter()
                .find(|(_, info)| info.name == contract.class)
                .map(|(start, _)| *start)?;
            return Some(ClassRef::Local(start));
        }
        Some(ClassRef::Imported {
            module: relative_specifier(path, &contract.file),
            name: contract.class.clone(),
        })
    }

    /// The heap identifier of a class (imported classes are numbered in
    /// [`Self::imported_classes`]).
    fn class_id(&mut self, class: ClassRef) -> u32 {
        match class {
            ClassRef::Local(start) => start,
            imported => {
                if let Some((id, _)) = self.imported_classes.iter().find(|(_, c)| **c == imported) {
                    return *id;
                }
                let id = IMPORTED_CLASS | u32::try_from(self.imported_classes.len()).unwrap_or(0);
                self.imported_classes.insert(id, imported);
                id
            }
        }
    }

    fn call_unwrapped(&mut self, call: &CallExpression<'a>, consumer: Option<Consumer>) {
        let chain = unwind_call(call);
        // Operations that start by themselves are not ordered by an
        // argument position: they keep the Phase 4 execution.
        let fire_and_forget = match consumer {
            None | Some(Consumer::Argument) => Execution::FireAndForget,
            Some(consumer) => consumer.execution(),
        };
        let mut classified = self.recognizer.classify(&chain, &self.locals);
        // A binding named like a database receiver (`db`) that holds an
        // instance of a project class (`classContracts[].returnedBy`, `new
        // C()`) is that object: its methods are dispatched, not Drizzle's.
        if matches!(
            classified,
            Classified::Db { .. } | Classified::DbUnknown { .. } | Classified::Transaction { .. }
        ) && self.instance_receiver(call)
        {
            classified = Classified::NotRecognized;
        }
        let value = match classified {
            Classified::Db {
                builder,
                marker,
                calls,
            } => {
                self.arguments(&calls);
                let write = matches!(builder.access, DbAccess::Write(_));
                if write {
                    self.facts
                        .refute(format!("performs a database write (`{}`)", builder.label));
                }
                let execution = match (marker, consumer) {
                    (Marker::Never, _) => None,
                    (Marker::Explicit, _) => Some(fire_and_forget),
                    (Marker::Lazy, Some(consumer)) if self.executes_lazy(consumer) => {
                        Some(Self::lazy_execution(consumer))
                    }
                    (Marker::Lazy, _) => None,
                };
                match execution {
                    Some(execution) => {
                        let end = calls.first().map_or(call.span.end, |first| first.span.end);
                        let references = argument_references(&calls);
                        let provenance = write.then(|| {
                            self.provenance(&calls, calls.first().map(|c| (c.span.end, 0)))
                        });
                        self.push_db_effect_with(
                            builder,
                            Span::new(call.span.start, end),
                            execution,
                            references,
                            provenance,
                        );
                        let id = self.next_effect - 1;
                        if marker == Marker::Lazy && consumer == Some(Consumer::Argument) {
                            self.argument_placeholders.push(EffectId(id));
                        }
                        if !write {
                            self.read_results.insert(id);
                        }
                        AbsValue::atom(Atom::EffectResult { effect: id })
                    }
                    None => self.opaque(),
                }
            }
            Classified::Transaction {
                call: transaction_call,
                handle,
                recognition,
                label,
            } => {
                self.facts
                    .refute(format!("starts a database transaction (`{label}`)"));
                self.transaction(
                    transaction_call,
                    handle,
                    recognition,
                    label,
                    fire_and_forget,
                );
                self.opaque()
            }
            Classified::Rollback { transaction } => {
                self.arguments(&[call]);
                self.exit(ExitKind::Rollback, call.span, Some(transaction), None);
                AbsValue::top()
            }
            Classified::DbUnknown {
                reason,
                detail,
                label,
                marker,
                calls,
            } => {
                self.arguments(&calls);
                self.facts.refute(format!(
                    "performs a database operation Midwrite does not model (`{label}`)"
                ));
                // Like builders, these run when awaited or explicitly executed.
                let executes = marker == Marker::Explicit
                    || (marker == Marker::Lazy && consumer.is_some_and(|c| self.executes_lazy(c)));
                if executes {
                    let execution = if marker == Marker::Lazy {
                        consumer.map_or(fire_and_forget, Self::lazy_execution)
                    } else {
                        fire_and_forget
                    };
                    self.push_unknown(reason, detail, label, call.span, execution);
                    if marker == Marker::Lazy && consumer == Some(Consumer::Argument) {
                        self.argument_placeholders
                            .push(EffectId(self.next_effect - 1));
                    }
                }
                self.opaque()
            }
            Classified::External {
                category,
                best_effort,
                recovery,
                recognition,
                label,
                calls,
                resolved_result,
                deferred,
            } => {
                self.arguments(&calls);
                self.facts
                    .refute(format!("performs a configured external effect (`{label}`)"));
                let references = argument_references(&calls);
                let provenance = self.provenance(&calls, None);
                if let Some(deferred) = deferred {
                    // Unknown here, as a call with unresolved options; each
                    // caller decides it (`Remap::deferred_http`).
                    let detail = format!(
                        "the request method comes from `{}`, parameter {} of this function; {} matches it only for httpMethods {}, which each call of this function decides",
                        deferred.name,
                        deferred.parameter + 1,
                        recognition.detail,
                        deferred.methods.join(", ")
                    );
                    self.push_effect_with(
                        EffectKind::Unknown {
                            reason: UnknownReason::UnrecognizedCall,
                            detail,
                            deferred: Some(Box::new(DeferredHttpMethod {
                                parameter: deferred.parameter,
                                name: deferred.name,
                                when_absent: deferred.when_absent,
                                methods: deferred.methods,
                                category,
                                best_effort,
                                recovery,
                                recognition,
                                provenance: Some(provenance),
                            })),
                        },
                        label,
                        call.span,
                        fire_and_forget,
                        None,
                        references,
                        None,
                    );
                    let value = self.opaque();
                    self.call_values.insert(call.span.start, value);
                    return;
                }
                self.push_effect_with(
                    EffectKind::External {
                        category,
                        best_effort,
                        recovery,
                    },
                    label,
                    call.span,
                    fire_and_forget,
                    Some(recognition),
                    references,
                    Some(provenance),
                );
                resolved_result
                    .as_ref()
                    .filter(|_| {
                        matches!(
                            consumer,
                            Some(Consumer::Await | Consumer::AsyncReturn | Consumer::Concurrent)
                        )
                    })
                    .map(declared_result)
                    .unwrap_or_else(|| {
                        AbsValue::atom(Atom::EffectResult {
                            effect: self.next_effect - 1,
                        })
                    })
            }
            Classified::Concurrent { call: combinator }
                if consumer.is_some_and(Consumer::awaits) =>
            {
                self.concurrent(combinator);
                AbsValue::top()
            }
            Classified::Concurrent { .. } | Classified::NotRecognized => {
                self.plain_call(call, consumer)
            }
        };
        self.call_values.insert(call.span.start, value);
    }

    /// A call that is not a recognized operation: maybe a resolved function,
    /// method or callback, a modeled builtin, or unknown.
    fn plain_call(&mut self, call: &CallExpression<'a>, consumer: Option<Consumer>) -> AbsValue {
        if consumer.is_some_and(Consumer::summarizes_local_calls)
            && self.tracing_callback(call, consumer.unwrap().execution())
        {
            return self.opaque();
        }
        if self.joined_maps.contains(&call.span.start) {
            self.array_callback(call, Execution::Concurrent, false);
            return self.opaque();
        }
        // `xs.forEach(fn)`, or `xs.map(fn)` whose result is not awaited:
        // the callback runs once per element and the method does not wait
        // for the promises it returns.
        if self.array_callback_target(call).is_some()
            && !matches!(
                consumer,
                Some(Consumer::Await | Consumer::Concurrent | Consumer::Argument)
            )
        {
            self.array_callback(call, Execution::FireAndForget, true);
            return self.opaque();
        }
        // The receiver of a method call is evaluated once, before the
        // arguments (as JavaScript does).
        let receiver = self.receiver_value(call);
        self.visit_expression(&call.callee);
        let resolved = self.resolve_target(call, receiver.as_ref());
        let value_only = consumer.is_none();
        let summarize = resolved.is_some() && consumer.is_none_or(Consumer::summarizes_local_calls);
        let mut argument_effects = Vec::new();
        if summarize {
            // Arguments run before the call; operations in them that only
            // run when awaited are placeholders the callee's summary decides.
            for (index, argument) in call.arguments.iter().enumerate() {
                match argument.as_expression() {
                    Some(expression) => {
                        let before = self.argument_placeholders.len();
                        self.consume(expression, Consumer::Argument);
                        let index = u32::try_from(index).unwrap_or(u32::MAX);
                        for effect in self.argument_placeholders.drain(before..) {
                            argument_effects.push((index, effect));
                        }
                    }
                    None => self.visit_argument(argument),
                }
            }
        } else {
            for argument in &call.arguments {
                self.visit_argument(argument);
            }
        }
        let mut values = Vec::with_capacity(call.arguments.len());
        for argument in &call.arguments {
            match argument.as_expression() {
                Some(expression) => {
                    let value = self.eval(expression);
                    values.push(value);
                }
                None => break,
            }
        }
        let builtin = resolved.is_none() && self.builtin_of(call, receiver.as_ref()).is_some();
        // Before `note_call` marks the objects passed as escaped.
        let entry_values: Vec<AbsValue> = values
            .iter()
            .zip(&call.arguments)
            .map(|(v, argument)| {
                let shape = entry_shape(&self.vs.heap.export(v), &self.stable_params);
                match argument.as_expression() {
                    Some(expression) if shape.is_top() => literal_object_entry(expression),
                    _ => shape,
                }
            })
            .collect();
        self.note_call(call, resolved.as_ref(), &values, receiver.as_ref());
        if value_only && !summarize {
            return self.opaque();
        }
        let consumer = consumer.unwrap_or(Consumer::SyncReturn);
        // A modeled builtin is not an unknown call, except where Phase 4
        // recorded it (awaited and concurrent positions keep their
        // behavior; see docs/architecture/value-analysis.md).
        let records = consumer.records_unknown() && !(builtin && consumer == Consumer::Argument);
        if records || summarize {
            let label = self.recognizer.call_label(call);
            self.push_unknown(
                UnknownReason::UnrecognizedCall,
                "no configuration entry or supported API matches it".to_owned(),
                label,
                call.span,
                if consumer == Consumer::SyncReturn {
                    Execution::ReturnedLazy
                } else {
                    consumer.execution()
                },
            );
            if consumer == Consumer::Argument {
                self.argument_placeholders
                    .push(EffectId(self.next_effect - 1));
            }
        }
        let Some(resolved) = resolved.filter(|_| summarize) else {
            return self.opaque();
        };
        let site = self.next_site;
        self.next_site += 1;
        let arguments = self.call_arguments(call);
        let binding = match &self.pending_result {
            Some((start, variable, symbol)) if *start == call.span.start => Some((
                variable.clone(),
                // Property facts on the binding are only made where nothing
                // can have changed it ([`Self::member_variable`]).
                symbol.is_none_or(|symbol| !self.recognizer.table.member_called(symbol)),
            )),
            _ => None,
        };
        let exported: Vec<AbsValue> = values.iter().map(|v| self.vs.heap.export(v)).collect();
        let receiver = resolved.receiver.as_ref().map(|v| self.vs.heap.export(v));
        self.local_calls.push(LocalCallSite {
            effect: EffectId(self.next_effect - 1),
            target: resolved.target,
            placeholder: !consumer.records_unknown(),
            value_only,
            arguments,
            values: exported,
            receiver,
            entry_values,
            argument_count: call
                .arguments
                .iter()
                .all(|argument| !matches!(argument, Argument::SpreadElement(_)))
                .then(|| u32::try_from(call.arguments.len()).unwrap_or(u32::MAX)),
            result: Some(site),
            binding,
            immediate: resolved.immediate,
            argument_effects,
        });
        AbsValue::atom(Atom::CallResult { site })
    }

    /// OpenTelemetry invokes the last argument of startActiveSpan in the
    /// active context and returns its result. Inline an inline callback so
    /// captured parameters remain in their enclosing frame. Other methods
    /// and lookalike objects remain unknown.
    fn tracing_callback(&mut self, call: &CallExpression<'a>, execution: Execution) -> bool {
        let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
            return false;
        };
        let Expression::Identifier(receiver) = member.object.get_inner_expression() else {
            return false;
        };
        if member.property.name != "startActiveSpan"
            || resolve_reference(self.recognizer.scoping, receiver)
                .and_then(|s| self.recognizer.table.sdk_client(s))
                != Some(crate::sdk::Client::Tracer)
        {
            return false;
        }
        let Some(callback) = call
            .arguments
            .last()
            .and_then(Argument::as_expression)
            .map(Expression::get_inner_expression)
        else {
            return false;
        };
        let (body, params, is_async, span) = match callback {
            Expression::ArrowFunctionExpression(f) => {
                (Body::Arrow(&f.body), &f.params, f.r#async, f.span)
            }
            Expression::FunctionExpression(f) => {
                let Some(body) = &f.body else {
                    return false;
                };
                (
                    Body::Statements(&body.statements),
                    &f.params,
                    f.r#async,
                    f.span,
                )
            }
            _ => return false,
        };
        for argument in call.arguments.iter().take(call.arguments.len() - 1) {
            self.visit_argument(argument);
        }
        for param in &params.items {
            self.havoc_pattern(&param.pattern);
        }
        let saved_async = std::mem::replace(&mut self.is_async, is_async);
        let saved_binding = self.return_binding.take();
        self.callback_depth += 1;
        let nodes = self.collect(|this| this.body(body));
        self.callback_depth -= 1;
        self.return_binding = saved_binding;
        self.is_async = saved_async;
        self.consumed.push(span.start);
        let id = midwrite_core::ir::CallId(self.next_inline_call);
        self.next_inline_call += 1;
        self.out.push(Node::Call(midwrite_core::ir::CallScope {
            call: id,
            callee: "<OpenTelemetry callback>".into(),
            label: self.recognizer.call_label(call).into(),
            location: self.location(call.span),
            callee_location: self.location(span),
            execution,
            body: nodes,
            resolution: None,
            inert: false,
            callback_context: false,
        }));
        true
    }

    /// The function a plain call `f(...)` names (a function declared in this
    /// file, or an import), or a method call `obj.f(...)` on an object
    /// service of this file, on an imported binding, or on a namespace
    /// import. Type imports and anything else are `None`.
    fn call_target(&self, call: &CallExpression<'a>) -> Option<CallTarget> {
        let table = self.recognizer.table;
        match call.callee.get_inner_expression() {
            Expression::Identifier(ident) => {
                let symbol = resolve_reference(self.recognizer.scoping, ident)?;
                if let Some(start) = table.local_function(symbol) {
                    return Some(CallTarget::Local(start));
                }
                let import = table.import(symbol)?;
                if import.type_only {
                    return None;
                }
                let name = match &import.kind {
                    ImportKind::Named(name) => name.clone(),
                    ImportKind::Default => "default".to_owned(),
                    ImportKind::Namespace => return None,
                };
                Some(CallTarget::Imported {
                    module: import.module.clone(),
                    name,
                    member: None,
                })
            }
            Expression::StaticMemberExpression(member) => {
                let Expression::Identifier(object) = member.object.get_inner_expression() else {
                    return None;
                };
                let symbol = resolve_reference(self.recognizer.scoping, object)?;
                let property = member.property.name.as_str();
                if let Some(start) = table.object_method(symbol, property) {
                    return Some(CallTarget::Local(start));
                }
                let import = table.import(symbol)?;
                if import.type_only {
                    return None;
                }
                let (name, member) = match &import.kind {
                    ImportKind::Namespace => (property.to_owned(), None),
                    ImportKind::Named(name) => (name.clone(), Some(property.to_owned())),
                    ImportKind::Default => ("default".to_owned(), Some(property.to_owned())),
                };
                Some(CallTarget::Imported {
                    module: import.module.clone(),
                    name,
                    member,
                })
            }
            _ => None,
        }
    }

    /// The database handles a call passes, by argument position. Arguments
    /// after a spread are unknown.
    fn call_arguments(&self, call: &CallExpression<'a>) -> Vec<Option<CallArgument>> {
        let mut out = Vec::with_capacity(call.arguments.len());
        for argument in &call.arguments {
            match argument {
                Argument::SpreadElement(_) => break,
                argument => out.push(
                    argument
                        .as_expression()
                        .and_then(|e| self.recognizer.argument_handle(e, &self.locals)),
                ),
            }
        }
        out
    }

    // --- Phase 5: values, targets, facts ---------------------------------

    fn fork(&mut self) -> ValueState {
        self.stats.states += 1;
        self.vs.clone()
    }

    fn rejoin(&mut self, other: &ValueState) {
        let mut stats = std::mem::take(&mut self.stats);
        self.vs.join(other, &mut stats);
        self.stats = stats;
    }

    fn opaque(&mut self) -> AbsValue {
        let id = self.next_opaque;
        self.next_opaque += 1;
        AbsValue::atom(Atom::Opaque { id })
    }

    /// A value is awaited or adopted: parameters in it are awaited.
    fn note_awaited(&mut self, value: &AbsValue) {
        for atom in value.atoms() {
            if let Atom::Param { index } = atom {
                self.facts.param(*index).awaited = true;
            }
        }
    }

    fn fact_arg(value: &AbsValue) -> FactArg {
        match value.single() {
            Some(Atom::Function { id }) => FactArg::Function(*id),
            Some(Atom::Closure {
                id,
                parameters,
                captures,
            }) => FactArg::Closure {
                function: *id,
                parameters: *parameters,
                captures: captures
                    .iter()
                    .map(|capture| Self::fact_arg(&capture.value))
                    .collect(),
            },
            Some(Atom::Param { index }) => FactArg::Param(*index),
            _ => FactArg::Other,
        }
    }

    fn callback_fact(value: &AbsValue, label: String) -> Option<FactCall> {
        let (id, args) = match value.single()? {
            Atom::Function { id } => (*id, Vec::new()),
            Atom::Closure {
                id,
                parameters,
                captures,
            } => {
                let mut args = vec![FactArg::Other; *parameters as usize];
                args.extend(
                    captures
                        .iter()
                        .map(|capture| Self::fact_arg(&capture.value)),
                );
                (*id, args)
            }
            _ => return None,
        };
        Some(FactCall {
            target: CallTarget::Local(id),
            args,
            label,
        })
    }

    /// Objects in `values` may be changed by the code they are passed to.
    /// A closure defined in this function may run there and change any
    /// object its variables reach.
    fn escape_values(&mut self, values: &[AbsValue]) {
        let mut closure = false;
        for value in values {
            self.vs.escape(value);
            closure |= value.atoms().iter().any(|atom| {
                matches!(atom, Atom::Function { id } | Atom::Closure { id, .. }
                    if *id > self.span.start && *id < self.span.end)
            });
        }
        if closure {
            let all: Vec<AbsValue> = self.vs.env.iter().map(|(_, v)| v.clone()).collect();
            for value in &all {
                self.vs.escape(value);
            }
        }
    }

    fn class_ref(&self, class: u32) -> Option<ClassRef> {
        if class & IMPORTED_CLASS != 0 {
            self.imported_classes.get(&class).cloned()
        } else {
            Some(ClassRef::Local(class))
        }
    }

    /// The target of a call that is not a recognized operation, when the
    /// analysis can name it: a function, an imported function or object
    /// service method, an immediately invoked function expression, a
    /// function value, a parameter (callback), or a method found by
    /// dispatch on a known receiver.
    /// The value of a method call's receiver (`obj` in `obj.m()`), unless it
    /// is `this`, `super` or a global object (`Math`, `console`).
    fn receiver_value(&mut self, call: &CallExpression<'a>) -> Option<AbsValue> {
        let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
            return None;
        };
        match member.object.get_inner_expression() {
            Expression::ThisExpression(_) | Expression::Super(_) => None,
            Expression::Identifier(ident)
                if resolve_reference(self.recognizer.scoping, ident).is_none() =>
            {
                None
            }
            // `new C().m()`: the construction runs first; lower it now so
            // that its instance is known for dispatch, and only once.
            Expression::NewExpression(new) => {
                if !self.call_values.contains_key(&new.span.start) {
                    self.visit_new_expression(new);
                    self.early_news.insert(new.span.start);
                }
                Some(self.eval(&member.object))
            }
            object => Some(self.eval(object)),
        }
    }

    fn resolve_target(
        &mut self,
        call: &CallExpression<'a>,
        receiver: Option<&AbsValue>,
    ) -> Option<ResolvedTarget> {
        if let Expression::Identifier(ident) = call.callee.get_inner_expression()
            && let Some(symbol) = resolve_reference(self.recognizer.scoping, ident)
        {
            if self.recognizer.table.promise_resolver(symbol) {
                return None;
            }
            if let Some((index, _)) = self.locals.params.get(&symbol) {
                return Some(ResolvedTarget::new(CallTarget::Param(*index)));
            }
        }
        if self.module_initializer
            && let Expression::Identifier(ident) = call.callee.get_inner_expression()
            && self.eval_ident(ident).is_top()
        {
            return None;
        }
        if let Some(target) = self.call_target(call) {
            if let CallTarget::Local(start) = target {
                let value = self.function_value(start, 0);
                if matches!(value.single(), Some(Atom::Closure { .. })) {
                    return Some(ResolvedTarget::new(CallTarget::Value(value)));
                }
            }
            return Some(ResolvedTarget::new(target));
        }
        match call.callee.get_inner_expression() {
            Expression::ArrowFunctionExpression(function) => Some(ResolvedTarget {
                immediate: true,
                ..ResolvedTarget::new(CallTarget::Value(
                    self.function_value(function.span.start, 0),
                ))
            }),
            Expression::FunctionExpression(function) if function.body.is_some() => {
                Some(ResolvedTarget {
                    immediate: true,
                    ..ResolvedTarget::new(CallTarget::Value(
                        self.function_value(function.span.start, 0),
                    ))
                })
            }
            Expression::Super(_) => self.this_class.map(|this| {
                ResolvedTarget::new(CallTarget::Super {
                    class: this.class,
                    name: None,
                })
            }),
            Expression::Identifier(ident) => match self.eval_ident(ident).single() {
                Some(Atom::Function { id }) => Some(ResolvedTarget::new(CallTarget::Local(*id))),
                Some(Atom::Closure { .. } | Atom::ModuleValue { .. }) => Some(ResolvedTarget::new(
                    CallTarget::Value(self.eval_ident(ident)),
                )),
                Some(Atom::Param { index }) => Some(ResolvedTarget::new(CallTarget::Param(*index))),
                Some(Atom::CallResult { site }) => {
                    Some(ResolvedTarget::new(CallTarget::Returned(*site)))
                }
                _ => None,
            },
            Expression::CallExpression(_) | Expression::AwaitExpression(_) => {
                match self.eval(&call.callee).single() {
                    Some(Atom::CallResult { site }) => {
                        Some(ResolvedTarget::new(CallTarget::Returned(*site)))
                    }
                    _ => None,
                }
            }
            Expression::StaticMemberExpression(member) => {
                let name = member.property.name.as_str().to_owned();
                match member.object.get_inner_expression() {
                    Expression::Super(_) => {
                        self.this_class.filter(|this| !this.is_static).map(|this| {
                            ResolvedTarget::new(CallTarget::Super {
                                class: this.class,
                                name: Some(name),
                            })
                        })
                    }
                    // `this.client.m()` / `this.#client.m()`: the class the
                    // field always holds (`ClassInfo::field_classes`).
                    Expression::StaticMemberExpression(field)
                        if matches!(
                            field.object.get_inner_expression(),
                            Expression::ThisExpression(_)
                        ) =>
                    {
                        self.field_dispatch(field.property.name.as_str(), name)
                    }
                    Expression::PrivateFieldExpression(field)
                        if matches!(
                            field.object.get_inner_expression(),
                            Expression::ThisExpression(_)
                        ) =>
                    {
                        self.field_dispatch(&format!("#{}", field.field.name), name)
                    }
                    Expression::ThisExpression(_) => self.this_class.map(|this| ResolvedTarget {
                        receiver: Some(AbsValue::atom(Atom::This)),
                        ..ResolvedTarget::new(CallTarget::Method {
                            class: ClassRef::Local(this.class),
                            name,
                            receiver: if this.is_static {
                                ReceiverKind::StaticThis
                            } else {
                                ReceiverKind::This
                            },
                        })
                    }),
                    object => {
                        let value = match receiver {
                            Some(value) => value.clone(),
                            None => self.eval(object),
                        };
                        match value.single() {
                            Some(Atom::Class { id }) => {
                                Some(ResolvedTarget::new(CallTarget::Method {
                                    class: ClassRef::Local(*id),
                                    name,
                                    receiver: ReceiverKind::Static,
                                }))
                            }
                            Some(Atom::Object { id }) => {
                                let object = self.vs.heap.get(*id)?;
                                match object.kind {
                                    ObjectKind::Instance { class } => {
                                        let escaped = object.escaped;
                                        Some(ResolvedTarget {
                                            receiver: Some(value.clone()),
                                            ..ResolvedTarget::new(CallTarget::Method {
                                                class: self.class_ref(class)?,
                                                name,
                                                receiver: ReceiverKind::Exact { escaped },
                                            })
                                        })
                                    }
                                    _ => match self.vs.heap.read(&value, &name).single() {
                                        Some(Atom::Function { id }) => Some(ResolvedTarget {
                                            receiver: Some(value.clone()),
                                            ..ResolvedTarget::new(CallTarget::Local(*id))
                                        }),
                                        _ => None,
                                    },
                                }
                            }
                            Some(Atom::Record { .. }) => match value.field(&name).single() {
                                Some(Atom::Function { id }) => Some(ResolvedTarget {
                                    receiver: Some(value.clone()),
                                    ..ResolvedTarget::new(CallTarget::Local(*id))
                                }),
                                _ => None,
                            },
                            _ => None,
                        }
                    }
                }
            }
            _ => None,
        }
    }

    /// Method `name` of the class that field `field` of `this` always holds
    /// in an instance method of the current class.
    fn field_dispatch(&self, field: &str, name: String) -> Option<ResolvedTarget> {
        let this = self.this_class.filter(|this| !this.is_static)?;
        let table = self.recognizer.table;
        let class = match table.classes().get(&this.class)?.field_classes.get(field)? {
            FieldClass::Constructed(symbol) => {
                if let Some(start) = table.class_of(*symbol) {
                    ClassRef::Local(start)
                } else {
                    let import = table.import(*symbol).filter(|i| !i.type_only)?;
                    let name = match &import.kind {
                        ImportKind::Named(name) => name.clone(),
                        ImportKind::Default => "default".to_owned(),
                        ImportKind::Namespace => return None,
                    };
                    ClassRef::Imported {
                        module: import.module.clone(),
                        name,
                    }
                }
            }
            FieldClass::Declared(class) => ClassRef::Local(
                table
                    .classes()
                    .iter()
                    .find(|(_, info)| info.name == *class)
                    .map(|(start, _)| *start)?,
            ),
        };
        Some(ResolvedTarget {
            receiver: Some(AbsValue::atom(Atom::Field {
                base: Box::new(Atom::This),
                name: field.into(),
            })),
            ..ResolvedTarget::new(CallTarget::Method {
                class,
                name,
                receiver: ReceiverKind::Exact { escaped: false },
            })
        })
    }

    /// The modeled builtin a call names, if any.
    fn builtin_of(
        &mut self,
        call: &CallExpression<'a>,
        receiver: Option<&AbsValue>,
    ) -> Option<Builtin> {
        let scoping = self.recognizer.scoping;
        match call.callee.get_inner_expression() {
            Expression::Identifier(ident) => match resolve_reference(scoping, ident) {
                None => builtins::global_function(ident.name.as_str()),
                Some(symbol)
                    if self.is_query_helper(symbol)
                        || self.recognizer.table.promise_resolver(symbol) =>
                {
                    Some(Builtin {
                        callback: None,
                        mutates: false,
                        terminates: false,
                    })
                }
                Some(_) => None,
            },
            Expression::StaticMemberExpression(member) => {
                let method = member.property.name.as_str();
                match member.object.get_inner_expression() {
                    Expression::Identifier(object)
                        if resolve_reference(scoping, object).is_none() =>
                    {
                        builtins::global_method(object.name.as_str(), method)
                    }
                    object => {
                        let value = match receiver {
                            Some(value) => value.clone(),
                            None => self.eval(object),
                        };
                        if value.is_top() || value.is_bottom() {
                            return None;
                        }
                        let arrays = value.atoms().iter().all(|atom| match atom {
                            Atom::Object { id } => self
                                .vs
                                .heap
                                .get(*id)
                                .is_some_and(|o| o.kind == ObjectKind::Array),
                            Atom::EffectResult { effect } => self.read_results.contains(effect),
                            _ => false,
                        });
                        let strings = value.atoms().iter().all(|atom| {
                            matches!(
                                atom,
                                Atom::Lit {
                                    value: Literal::String(_)
                                }
                            )
                        });
                        if arrays {
                            builtins::array_method(method)
                        } else if strings {
                            builtins::string_method(method)
                        } else {
                            None
                        }
                    }
                }
            }
            _ => None,
        }
    }

    /// A binding imported from `drizzle-orm`: query-expression helpers
    /// (`eq`, `and`, `sql`, `desc`, …) build SQL fragments and perform
    /// nothing.
    fn is_query_helper(&self, symbol: oxc_syntax::symbol::SymbolId) -> bool {
        self.recognizer
            .table
            .import(symbol)
            .is_some_and(|import| import.module.starts_with("drizzle-orm"))
    }

    /// Record what a plain call means for the function's summary.
    fn note_call(
        &mut self,
        call: &CallExpression<'a>,
        resolved: Option<&ResolvedTarget>,
        values: &[AbsValue],
        receiver: Option<&AbsValue>,
    ) {
        if let Some(resolved) = resolved {
            match &resolved.target {
                CallTarget::Param(index) => self.facts.param(*index).invoked = true,
                target => {
                    let args = values.iter().map(Self::fact_arg).collect();
                    let label = self.recognizer.callee_label(&call.callee);
                    self.facts.calls.push(FactCall {
                        target: target.clone(),
                        args,
                        label,
                    });
                }
            }
            self.escape_values(values);
            if let Some(receiver) = &resolved.receiver {
                self.vs.escape(receiver);
            }
            return;
        }
        match self.builtin_of(call, receiver) {
            Some(builtin) => {
                if builtin.terminates {
                    let label = self.recognizer.call_label(call);
                    self.facts.refute(format!("ends the process (`{label}`)"));
                }
                if let Some((index, when)) = builtin.callback
                    && let Some(value) = values.get(index)
                {
                    match value.single() {
                        Some(Atom::Function { .. } | Atom::Closure { .. }) => {
                            let label = self.recognizer.callee_label(&call.callee);
                            self.facts
                                .calls
                                .push(Self::callback_fact(value, label).expect("known callback"));
                            if when == Callback::Deferred {
                                self.escape_values(std::slice::from_ref(value));
                            }
                        }
                        Some(Atom::Param { index }) => self.facts.param(*index).invoked = true,
                        _ if self.facts.refuted.is_none() => {
                            let label = self.recognizer.call_label(call);
                            self.facts.refute(format!(
                                "passes a value that is not a known function to `{label}`, which runs it"
                            ));
                        }
                        _ => {}
                    }
                }
                if builtin.mutates {
                    let target = match call.callee.get_inner_expression() {
                        Expression::StaticMemberExpression(_) if receiver.is_some() => {
                            receiver.cloned().unwrap_or_else(AbsValue::top)
                        }
                        _ => values.first().cloned().unwrap_or_else(AbsValue::top),
                    };
                    self.vs.heap_mut().write_unknown(&target);
                }
                // `Promise.race([p, ...])` (unawaited): elements are awaited
                // by the combinator.
                if let Expression::StaticMemberExpression(member) =
                    call.callee.get_inner_expression()
                    && matches!(
                        member.property.name.as_str(),
                        "all" | "allSettled" | "race" | "any"
                    )
                    && let Some(Argument::ArrayExpression(array)) = call.arguments.first()
                {
                    for element in &array.elements {
                        if let Some(expression) = element.as_expression() {
                            let value = self.eval(expression);
                            self.note_awaited(&value);
                        }
                    }
                }
            }
            None => {
                if self.facts.refuted.is_none() {
                    let label = self.recognizer.call_label(call);
                    self.facts
                        .refute(format!("calls `{label}`, whose target is not known"));
                }
                for value in values {
                    for atom in value.atoms() {
                        if let Atom::Param { index } = atom {
                            self.facts.param(*index).escaped = true;
                        }
                    }
                }
                self.escape_values(values);
                if let Some(receiver) = receiver {
                    self.vs.escape(receiver);
                }
            }
        }
    }

    /// Lower a loop body: iterate the value state to a fixpoint, widen what
    /// keeps changing, then lower the body once from the fixpoint. The first
    /// pass is optimistic: when the entry state is already a fixpoint (the
    /// common case), its IR is kept. Otherwise further iterations are value
    /// only (IR discarded). Effects are still modeled as zero or one
    /// iteration by the path walker.
    fn loop_body(&mut self, lower: &dyn Fn(&mut Self)) -> Vec<Node> {
        self.loop_depth += 1;
        let body = self.loop_body_inner(lower);
        self.loop_depth -= 1;
        body
    }

    fn loop_body_inner(&mut self, lower: &dyn Fn(&mut Self)) -> Vec<Node> {
        let entry = self.fork();
        let counters = (self.next_site, self.next_opaque);
        if self.dry_runs + MAX_LOOP_ITERATIONS + 1 > MAX_DRY_RUNS {
            // Budget exhausted: nothing known survives the loop.
            self.stats.loops_havocked += 1;
            self.vs.havoc();
            let body = self.collect(|this| lower(this));
            self.vs.havoc();
            return body;
        }
        // Optimistic pass from the entry state.
        self.stats.loop_iterations += 1;
        let checkpoint = self.checkpoint();
        let body = self.collect(|this| lower(this));
        let mut head = entry.clone();
        let mut stats = std::mem::take(&mut self.stats);
        head.join(&self.vs, &mut stats);
        self.stats = stats;
        if head.leq_on_existing(&entry) {
            // Keep what the body declared (dead after the loop) out of the
            // state after it: zero iterations or some leave `entry`.
            self.vs = entry;
            return body;
        }
        self.restore(checkpoint);
        let mut converged = false;
        let mut widened = false;
        for iteration in 0..MAX_LOOP_ITERATIONS {
            self.stats.loop_iterations += 1;
            self.vs = head.clone();
            (self.next_site, self.next_opaque) = counters;
            self.dry_run(lower);
            let mut next = entry.clone();
            let mut stats = std::mem::take(&mut self.stats);
            next.join(&self.vs, &mut stats);
            self.stats = stats;
            if next.leq_on_existing(&head) {
                converged = true;
                break;
            }
            let previous = head.clone();
            let mut stats = std::mem::take(&mut self.stats);
            head.join(&next, &mut stats);
            if iteration >= 1 {
                head.widen_from(&previous, &mut stats);
                widened = true;
            }
            self.stats = stats;
        }
        if !converged {
            // One check after widening; havoc if still not stable.
            self.vs = head.clone();
            (self.next_site, self.next_opaque) = counters;
            self.dry_run(lower);
            let mut next = entry.clone();
            let mut stats = std::mem::take(&mut self.stats);
            next.join(&self.vs, &mut stats);
            self.stats = stats;
            if !next.leq_on_existing(&head) {
                self.stats.loops_havocked += 1;
                head.havoc();
            }
        }
        if widened {
            self.stats.loops_widened += 1;
        }
        self.vs = head.clone();
        (self.next_site, self.next_opaque) = counters;
        let body = self.collect(|this| lower(this));
        // Zero iterations, or leaving after some.
        let after = std::mem::replace(&mut self.vs, head);
        self.rejoin(&after);
        body
    }

    /// Everything lowering adds to besides the current block, so that a
    /// speculative lowering can be undone.
    fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            next_effect: self.next_effect,
            next_inline_call: self.next_inline_call,
            transactions: self.transactions.len(),
            local_calls: self.local_calls.len(),
            consumed: self.consumed.len(),
            facts: self.facts.clone(),
            placeholders: self.argument_placeholders.len(),
            returns: self.returns.clone(),
        }
    }

    fn restore(&mut self, checkpoint: Checkpoint) {
        self.next_effect = checkpoint.next_effect;
        self.next_inline_call = checkpoint.next_inline_call;
        self.transactions.truncate(checkpoint.transactions);
        self.local_calls.truncate(checkpoint.local_calls);
        self.consumed.truncate(checkpoint.consumed);
        self.facts = checkpoint.facts;
        self.argument_placeholders.truncate(checkpoint.placeholders);
        self.returns = checkpoint.returns;
    }

    /// Run `lower` for its effect on the value state only: everything it
    /// would add to the IR is discarded.
    fn dry_run(&mut self, lower: &dyn Fn(&mut Self)) {
        self.dry_runs += 1;
        self.stats.dry_runs += 1;
        let out = std::mem::take(&mut self.out);
        let checkpoint = self.checkpoint();
        lower(self);
        self.out = out;
        self.restore(checkpoint);
    }

    /// The loop variable of `for (x of …)` / `for (x in …)`: unknown.
    fn loop_variable(&mut self, left: &oxc_ast::ast::ForStatementLeft<'a>) {
        match left {
            oxc_ast::ast::ForStatementLeft::VariableDeclaration(declaration) => {
                for declarator in &declaration.declarations {
                    self.havoc_pattern(&declarator.id);
                }
            }
            oxc_ast::ast::ForStatementLeft::AssignmentTargetIdentifier(ident) => {
                if let Some(symbol) = resolve_reference(self.recognizer.scoping, ident) {
                    self.vs.env_mut().set(key(symbol), AbsValue::top());
                }
            }
            _ => {}
        }
    }

    /// What a function-level `return` implies about the returned value,
    /// as facts about [`RETURN_VARIABLE`]: literal properties of a returned
    /// object literal (no spread, no accessors), or a returned literal.
    /// `const [row] = await db.update(t)...returning()` (also `delete`, and
    /// `insert` with `onConflictDoNothing`): `row` is truthy whenever the
    /// write changed a row, so a path where it is falsy changed nothing.
    /// `before` is the length of the output before the initializer.
    fn note_returned_row(
        &mut self,
        declaration: &VariableDeclaration<'a>,
        pattern: &BindingPattern<'a>,
        init: &Expression<'a>,
        before: usize,
    ) {
        // One execution per binding: in a loop or a callback (an array
        // callback runs once per element) one row's emptiness says nothing
        // about another iteration's write.
        if self.loop_depth > 0
            || self.callback_depth > 0
            || !declaration.kind.is_const()
            || !returns_changed_rows(init)
        {
            return;
        }
        let BindingPattern::ArrayPattern(array) = pattern else {
            return;
        };
        let Some(Some(BindingPattern::BindingIdentifier(id))) = array.elements.first() else {
            return;
        };
        let Some(symbol) = id.symbol_id.get() else {
            return;
        };
        let Some(Variable::Free(variable)) = self.binding_variable(symbol, &id.name) else {
            return;
        };
        // Exactly the one write the initializer performs, at this level
        // (argument calls such as `eq(...)` may add unknown effects).
        let nested = self.out[before..]
            .iter()
            .any(|node| !matches!(node, Node::Effect(_)));
        let mut writes = self.out[before..].iter_mut().filter_map(|node| match node {
            Node::Effect(effect) if matches!(effect.kind, EffectKind::DatabaseWrite { .. }) => {
                Some(effect)
            }
            _ => None,
        });
        let Some(effect) = writes.next() else {
            return;
        };
        if nested || writes.next().is_some() {
            return;
        }
        effect.nonempty.push(Fact::Truthy {
            variable: variable.clone(),
            value: true,
        });
        let id = effect.id;
        self.row_effects.push((id, variable));
    }

    /// The bound-row variable a `return` returns: `row`, `row ?? null`,
    /// `row ?? undefined`, `row || null`.
    fn returned_row(&self, argument: Option<&Expression<'a>>) -> Option<String> {
        let mut expression = argument?.get_inner_expression();
        if let Expression::LogicalExpression(logical) = expression {
            let nullish = match logical.right.get_inner_expression() {
                Expression::NullLiteral(_) => true,
                Expression::Identifier(id) => id.name == "undefined",
                _ => false,
            };
            if !nullish
                || !matches!(
                    logical.operator,
                    LogicalOperator::Coalesce | LogicalOperator::Or
                )
            {
                return None;
            }
            expression = logical.left.get_inner_expression();
        }
        let Expression::Identifier(id) = expression else {
            return None;
        };
        match self.variable(id)? {
            Variable::Free(variable) => Some(variable),
            Variable::Constant(_) => None,
        }
    }

    /// A write with a bound `RETURNING` row in a function whose every
    /// `return` after it returns that row (and that cannot fall off its
    /// end): the function's result is falsy exactly when the write changed
    /// nothing, so `$ret` truthy also holds whenever it changed a row. A
    /// caller testing its binding of the result decides it.
    fn mark_returned_rows(&mut self, falls_through: bool) {
        let rows = std::mem::take(&mut self.row_effects);
        if falls_through || rows.is_empty() {
            return;
        }
        for (effect, variable) in rows {
            let mut scan = RowScan {
                effect,
                variable: &variable,
                returns: &self.return_rows,
                seen: false,
                ok: true,
            };
            scan.nodes(&self.out, false);
            if scan.seen
                && scan.ok
                && let Some(target) = effect_mut(&mut self.out, effect)
            {
                target.nonempty.push(Fact::Truthy {
                    variable: RETURN_VARIABLE.to_owned(),
                    value: true,
                });
            }
        }
    }

    fn returned_value_facts(&self, argument: Option<&Expression<'a>>) -> Vec<Fact> {
        let ret = RETURN_VARIABLE.to_owned();
        let Some(argument) = argument else {
            return vec![Fact::Equals {
                variable: ret,
                literal: Literal::Undefined,
            }];
        };
        match argument.get_inner_expression() {
            Expression::ObjectExpression(object) => {
                let mut facts = vec![Fact::Object {
                    variable: ret.clone(),
                }];
                // Spreads may set any property: only properties written
                // after the last spread are known (a later one overrides
                // whatever the spread copied).
                let last_spread = object.properties.iter().rposition(|property| {
                    matches!(property, ObjectPropertyKind::SpreadProperty(_))
                });
                let plain = object.properties.iter().all(|property| match property {
                    ObjectPropertyKind::ObjectProperty(p) => {
                        p.kind == PropertyKind::Init && !p.computed
                    }
                    ObjectPropertyKind::SpreadProperty(_) => true,
                });
                if !plain {
                    return facts;
                }
                let known = last_spread.map_or(0, |index| index + 1);
                for property in &object.properties[known..] {
                    let ObjectPropertyKind::ObjectProperty(property) = property else {
                        continue;
                    };
                    let Some(name) = property.key.static_name() else {
                        continue;
                    };
                    let variable = format!("{ret}.{}", name.display());
                    match property.value.get_inner_expression() {
                        Expression::ObjectExpression(_)
                        | Expression::ArrayExpression(_)
                        | Expression::ArrowFunctionExpression(_)
                        | Expression::FunctionExpression(_) => {
                            facts.push(Fact::Object { variable });
                        }
                        value => {
                            if let Some(literal) = self.literal_test(value) {
                                facts.push(Fact::Equals { variable, literal });
                            }
                        }
                    }
                }
                facts
            }
            Expression::ArrayExpression(_)
            | Expression::ArrowFunctionExpression(_)
            | Expression::FunctionExpression(_) => vec![Fact::Object { variable: ret }],
            other => self
                .literal_test(other)
                .map(|literal| {
                    vec![Fact::Equals {
                        variable: ret,
                        literal,
                    }]
                })
                .unwrap_or_default(),
        }
    }

    /// The callback of `xs.map(fn)`, `xs.flatMap(fn)` or `xs.forEach(fn)`
    /// when the analysis can name it (assumption: these methods run the
    /// callback zero or more times during the call and do not await it).
    fn array_callback_target(&self, call: &CallExpression<'a>) -> Option<CallTarget> {
        let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
            return None;
        };
        if !matches!(member.property.name.as_str(), "map" | "flatMap" | "forEach") {
            return None;
        }
        // `Promise.all` and other globals are not arrays.
        if let Expression::Identifier(object) = member.object.get_inner_expression()
            && resolve_reference(self.recognizer.scoping, object).is_none()
        {
            return None;
        }
        match call
            .arguments
            .first()?
            .as_expression()?
            .get_inner_expression()
        {
            Expression::ArrowFunctionExpression(function) => Some(CallTarget::Value(
                self.function_value(function.span.start, 0),
            )),
            Expression::FunctionExpression(function) if function.body.is_some() => Some(
                CallTarget::Value(self.function_value(function.span.start, 0)),
            ),
            Expression::Identifier(ident) => match self.eval_ident(ident).single() {
                Some(Atom::Function { id }) => Some(CallTarget::Local(*id)),
                Some(Atom::Closure { .. } | Atom::ModuleValue { .. }) => {
                    Some(CallTarget::Value(self.eval_ident(ident)))
                }
                Some(Atom::Param { index }) => Some(CallTarget::Param(*index)),
                _ => None,
            },
            _ => None,
        }
    }

    /// Lower `xs.map(fn)`-like calls as a loop (zero or more iterations)
    /// whose body calls the callback with `execution`. `placeholder`: drop
    /// the call when the callback is not summarized (Phase 4 recorded
    /// nothing for such calls).
    fn array_callback(
        &mut self,
        call: &CallExpression<'a>,
        execution: Execution,
        placeholder: bool,
    ) {
        let Some(target) = self.array_callback_target(call) else {
            return;
        };
        walk::walk_call_expression(self, call);
        let mut values = Vec::with_capacity(call.arguments.len());
        for argument in &call.arguments {
            match argument.as_expression() {
                Some(expression) => {
                    let value = self.eval(expression);
                    values.push(value);
                }
                None => break,
            }
        }
        let receiver = self.receiver_value(call);
        self.note_call(call, None, &values, receiver.as_ref());
        let callback = call
            .arguments
            .first()
            .map_or(call.span, |argument| argument.span());
        let effect_label = format!(
            "{} (per element)",
            truncate(
                &compact(self.recognizer.slice(callback.start, callback.end)),
                40
            )
        );
        let mut body = self.callback_invocation(
            target.clone(),
            callback,
            effect_label.clone(),
            execution,
            placeholder,
        );
        // Different elements can take different branches of an async
        // callback. A single representative invocation cannot expose a
        // partial batch containing both an update and an insert.
        if execution == Execution::Concurrent && alternative_callback_paths(call) {
            let again =
                self.callback_invocation(target, callback, effect_label, execution, placeholder);
            body.push(Node::Loop(Loop {
                kind: LoopKind::Callback,
                header: "another element of the joined collection".to_owned(),
                location: self.location(call.span),
                runs_at_least_once: false,
                body: again,
            }));
        }
        self.push_loop(LoopKind::Callback, call.span, callback, false, body);
        let value = self.opaque();
        self.call_values.insert(call.span.start, value);
    }

    fn callback_invocation(
        &mut self,
        target: CallTarget,
        callback: Span,
        effect_label: String,
        execution: Execution,
        placeholder: bool,
    ) -> Vec<Node> {
        self.collect(|this| {
            this.push_unknown(
                UnknownReason::UnrecognizedCall,
                "no configuration entry or supported API matches it".to_owned(),
                effect_label,
                callback,
                execution,
            );
            let site = this.next_site;
            this.next_site += 1;
            this.local_calls.push(LocalCallSite {
                effect: EffectId(this.next_effect - 1),
                target,
                placeholder,
                value_only: false,
                arguments: Vec::new(),
                values: vec![AbsValue::top()],
                receiver: None,
                entry_values: Vec::new(),
                argument_count: None,
                result: Some(site),
                binding: None,
                immediate: false,
                argument_effects: Vec::new(),
            });
        })
    }

    // --- Branch facts ----------------------------------------------------

    /// An arm of a branch on `test`, taken when `test` is truthy (`truthy`)
    /// or falsy.
    fn arm_if(&self, label: &str, body: Vec<Node>, test: &Expression<'a>, truthy: bool) -> Arm {
        Arm {
            label: label.to_owned(),
            body,
            facts: self.facts(test, truthy),
        }
    }

    /// What `condition` evaluating to a truthy (`truthy`) or falsy value
    /// implies about variables. Only implications are recorded; anything not
    /// understood implies nothing.
    fn facts(&self, condition: &Expression<'a>, truthy: bool) -> Vec<Fact> {
        let condition = condition.get_inner_expression();
        let mut facts =
            self.recognizer
                .table
                .cardinality
                .facts(condition, truthy, self.recognizer.scoping);
        facts.extend(self.syntactic_facts(condition, truthy));
        facts
    }

    fn syntactic_facts(&self, condition: &Expression<'a>, truthy: bool) -> Vec<Fact> {
        if let Some(literal) = literal_value(condition) {
            return never_unless(literal.is_truthy() == truthy);
        }
        match condition {
            Expression::Identifier(_)
            | Expression::StaticMemberExpression(_)
            | Expression::ChainExpression(_) => match self.condition_variable(condition) {
                Some(Variable::Free(variable)) => vec![Fact::Truthy {
                    variable,
                    value: truthy,
                }],
                Some(Variable::Constant(literal)) => never_unless(literal.is_truthy() == truthy),
                None => Vec::new(),
            },
            Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::LogicalNot => {
                self.facts(&unary.argument, !truthy)
            }
            Expression::LogicalExpression(logical) => match (logical.operator, truthy) {
                (LogicalOperator::And, true) | (LogicalOperator::Or, false) => {
                    let mut facts = self.facts(&logical.left, truthy);
                    facts.extend(self.facts(&logical.right, truthy));
                    facts
                }
                _ => Vec::new(),
            },
            Expression::BinaryExpression(binary) => {
                let (variable, literal) = match (
                    self.condition_variable(&binary.left),
                    self.literal_test(&binary.right),
                    self.condition_variable(&binary.right),
                    self.literal_test(&binary.left),
                ) {
                    (Some(variable), Some(literal), _, _)
                    | (_, _, Some(variable), Some(literal)) => (variable, literal),
                    _ => return Vec::new(),
                };
                let (strict, equal) = match binary.operator {
                    BinaryOperator::StrictEquality => (true, truthy),
                    BinaryOperator::StrictInequality => (true, !truthy),
                    BinaryOperator::Equality => (false, truthy),
                    BinaryOperator::Inequality => (false, !truthy),
                    _ => return Vec::new(),
                };
                comparison_facts(variable, literal, strict, equal)
            }
            _ => Vec::new(),
        }
    }

    /// Whether destructuring `init` snapshots a parameter that nothing
    /// changes first: a parameter binding (or a binding of a property path
    /// of one) never reassigned, never written through or method-called,
    /// whose only reference other than property reads and database write
    /// payloads is this one.
    fn snapshot_source(&self, init: &Expression<'a>) -> bool {
        let Expression::Identifier(ident) = init.get_inner_expression() else {
            return false;
        };
        let scoping = self.recognizer.scoping;
        resolve_reference(scoping, ident).is_some_and(|symbol| {
            self.param_snapshots.contains_key(&symbol)
                && !scoping.symbol_is_mutated(symbol)
                && self.recognizer.table.other_references(symbol, scoping) == Some(1)
        })
    }

    /// The variable an expression names, if it is an identifier whose value
    /// cannot change within the function: declared in this file, never
    /// assigned after its declaration, and not an import (imports are live
    /// bindings). `const` bindings of a literal are known constants.
    fn condition_variable(&self, expression: &Expression<'a>) -> Option<Variable> {
        let variable = match expression.get_inner_expression() {
            Expression::Identifier(ident) => return self.variable(ident),
            Expression::StaticMemberExpression(member) => self.member_variable(member),
            _ => None,
        };
        variable.or_else(|| self.read_variable(expression))
    }

    /// A property read of a parameter (`options.send`, `options?.send`)
    /// as a variable of its own: an input of the function (see
    /// [`Self::input_expression`]). Two reads may differ, so each read is
    /// a different variable.
    fn read_variable(&self, expression: &Expression<'a>) -> Option<Variable> {
        let expression = expression.get_inner_expression();
        if self.loop_depth > 0 || self.callback_depth > 0 {
            return None;
        }
        let start = expression.span().start;
        let input = self.input_expression(expression, start)?;
        let variable = format!("{}@{start}", compact(&self.text(expression.span())));
        self.fact_inputs
            .borrow_mut()
            .insert(variable.clone(), input);
        Some(Variable::Free(variable))
    }

    /// What `expression` evaluated at offset `at` holds, as an input of the
    /// function: `p`, `p.x`, `p?.x` or any of those `?? literal`, where `p`
    /// is a parameter snapshot never reassigned. Property reads of an
    /// object that code before `at` may have changed (it was passed to a
    /// call) are [`FactInput::late_reads`].
    fn input_expression(&self, expression: &Expression<'a>, at: u32) -> Option<FactInput> {
        self.input_read(expression, at).map(|(input, _)| input)
    }

    /// [`Self::input_expression`], and whether the parameter's object may
    /// have changed before `at` (every property read of it is then late).
    fn input_read(&self, expression: &Expression<'a>, at: u32) -> Option<(FactInput, bool)> {
        let member = |object: &Expression<'a>, name: &str| {
            let (base, changed) = self.input_read(object, at)?;
            if base.default.is_some() {
                return None;
            }
            let value = base.value.field(name);
            let path = value
                .single()
                .is_some_and(|atom| matches!(atom.root(), Atom::Param { .. }));
            path.then(|| {
                let input = FactInput {
                    value,
                    default: None,
                    late_reads: base.late_reads + u32::from(changed),
                };
                (input, changed)
            })
        };
        match expression.get_inner_expression() {
            Expression::Identifier(ident) => {
                let scoping = self.recognizer.scoping;
                let symbol = resolve_reference(scoping, ident)?;
                let value = self.param_snapshots.get(&symbol)?;
                if scoping.symbol_is_mutated(symbol) {
                    return None;
                }
                let changed = !self.recognizer.table.unchanged_before(symbol, at);
                Some((FactInput::new(value.clone()), changed))
            }
            Expression::StaticMemberExpression(it) => member(&it.object, it.property.name.as_str()),
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::StaticMemberExpression(it) => {
                    member(&it.object, it.property.name.as_str())
                }
                _ => None,
            },
            Expression::LogicalExpression(logical)
                if logical.operator == LogicalOperator::Coalesce =>
            {
                let (mut input, changed) = self.input_read(&logical.left, at)?;
                if input.default.is_some() {
                    return None;
                }
                input.default = Some(self.literal_test(&logical.right)?);
                Some((input, changed))
            }
            _ => None,
        }
    }

    /// `x.p` as a fact variable (`x#12.p`): `x` is a variable that never
    /// changes, every reference of it is a property read (so no visible
    /// code writes its properties or calls its methods), and its value is
    /// an object without accessors: an object literal of this function, a
    /// record returned by an analyzed callee, a database row, or the result
    /// of an analyzed call. Getters on objects from unanalyzed code would
    /// make two reads differ, so anything else is not a variable.
    fn member_variable(
        &self,
        member: &oxc_ast::ast::StaticMemberExpression<'a>,
    ) -> Option<Variable> {
        let Expression::Identifier(object) = member.object.get_inner_expression() else {
            return None;
        };
        let scoping = self.recognizer.scoping;
        let symbol = resolve_reference(scoping, object)?;
        // The rest object of a destructured parameter is a fresh plain
        // object: its properties are those of the parameter when it was
        // destructured, except the keys taken out. Besides property reads
        // it may be the payload of a database write, which only reads it.
        if let Some((source, excluded)) = self.rest_snapshots.get(&symbol) {
            let name = member.property.name.as_str();
            if excluded.iter().any(|key| key == name)
                || self.recognizer.table.other_references(symbol, scoping) != Some(0)
            {
                return None;
            }
            let Variable::Free(base) = self.variable(object)? else {
                return None;
            };
            let variable = format!("{base}.{name}");
            self.fact_inputs
                .borrow_mut()
                .insert(variable.clone(), FactInput::new(source.field(name)));
            return Some(Variable::Free(variable));
        }
        // Every reference reads a property, or (outside loops and
        // callbacks, where source order is execution order) every
        // reference before this one does: what the object held when it was
        // bound is what this read sees, even if it escapes afterwards.
        let stable = self.recognizer.table.member_stable(symbol, scoping)
            || (self.loop_depth == 0
                && self.callback_depth == 0
                && self
                    .recognizer
                    .table
                    .unchanged_before(symbol, member.span.start));
        if !stable {
            return None;
        }
        let value = self.eval_ident(object);
        let plain = !value.is_top()
            && !value.is_bottom()
            && value.atoms().iter().all(|atom| match atom {
                Atom::Object { id } => self
                    .vs
                    .heap
                    .get(*id)
                    .is_some_and(|o| o.kind == ObjectKind::Plain && !o.open),
                Atom::Record { open, .. } => !open,
                Atom::CallResult { .. } => true,
                Atom::EffectResult { effect } => self.read_results.contains(effect),
                _ => false,
            });
        if !plain {
            return None;
        }
        match self.variable(object)? {
            Variable::Free(name) => Some(Variable::Free(format!(
                "{name}.{}",
                member.property.name.as_str()
            ))),
            Variable::Constant(_) => None,
        }
    }

    /// A literal a variable can be compared with, including the global
    /// `undefined` (not a binding that shadows it).
    fn literal_test(&self, expression: &Expression<'_>) -> Option<Literal> {
        match expression.get_inner_expression() {
            Expression::Identifier(ident) if ident.name == "undefined" => {
                resolve_reference(self.recognizer.scoping, ident)
                    .is_none()
                    .then_some(Literal::Undefined)
            }
            other => literal_value(other),
        }
    }

    /// What a `return` of the current transaction callback implies about
    /// the variable its transaction's result is bound to.
    fn return_facts(&self, argument: Option<&Expression<'a>>) -> Vec<Fact> {
        let Some(variable) = &self.return_binding else {
            return Vec::new();
        };
        let Some(argument) = argument else {
            return vec![Fact::Equals {
                variable: variable.clone(),
                literal: Literal::Undefined,
            }];
        };
        match argument.get_inner_expression() {
            Expression::ObjectExpression(_)
            | Expression::ArrayExpression(_)
            | Expression::ArrowFunctionExpression(_)
            | Expression::FunctionExpression(_) => vec![Fact::Object {
                variable: variable.clone(),
            }],
            other => self
                .literal_test(other)
                .map(|literal| {
                    vec![Fact::Equals {
                        variable: variable.clone(),
                        literal,
                    }]
                })
                .unwrap_or_default(),
        }
    }

    /// The variable a binding declares, if it can be one (see [`Self::variable`]).
    fn binding_variable(
        &self,
        symbol: oxc_syntax::symbol::SymbolId,
        name: &str,
    ) -> Option<Variable> {
        let scoping = self.recognizer.scoping;
        let flags = scoping.symbol_flags(symbol);
        if !flags.is_variable() || scoping.symbol_is_mutated(symbol) {
            return None;
        }
        Some(Variable::Free(format!("{name}#{}", symbol.index())))
    }

    fn variable(&self, ident: &IdentifierReference<'a>) -> Option<Variable> {
        let scoping = self.recognizer.scoping;
        let symbol = resolve_reference(scoping, ident)?;
        if let Some(literal) = self.recognizer.table.literal(symbol) {
            return Some(Variable::Constant(literal.clone()));
        }
        let flags = scoping.symbol_flags(symbol);
        if !flags.is_variable()
            || flags.is_import()
            || scoping.symbol_is_mutated(symbol)
            || self.recognizer.table.import(symbol).is_some()
        {
            return None;
        }
        if let Some(Atom::Lit { value }) = self.eval_ident(ident).single() {
            return Some(Variable::Constant(value.clone()));
        }
        let variable = format!("{}#{}", ident.name.as_str(), symbol.index());
        let input = match self.param_snapshots.get(&symbol) {
            Some(value) => Some(FactInput::new(value.clone())),
            None => self.derived_inputs.get(&symbol).cloned(),
        };
        if let Some(input) = input {
            self.fact_inputs
                .borrow_mut()
                .insert(variable.clone(), input);
        }
        Some(Variable::Free(variable))
    }

    /// `await Promise.all([a, b, ...])`: the elements run concurrently.
    fn concurrent(&mut self, call: &CallExpression<'a>) {
        if let Some(Expression::Identifier(collection)) =
            call.arguments.first().and_then(Argument::as_expression)
            && resolve_reference(self.recognizer.scoping, collection)
                .is_some_and(|symbol| self.joined_bindings.contains(&symbol))
        {
            return;
        }
        let array = match call.arguments.first() {
            Some(Argument::ArrayExpression(array)) => array,
            // `Promise.all(xs.map(fn))`: the callback runs once per element,
            // all concurrently.
            Some(Argument::CallExpression(inner))
                if self.array_callback_target(inner).is_some() =>
            {
                self.array_callback(inner, Execution::Concurrent, false);
                return;
            }
            _ => {
                walk::walk_call_expression(self, call);
                let label = self.recognizer.call_label(call);
                self.push_unknown(
                    UnknownReason::UnrecognizedCall,
                    "it awaits a collection that is not an array literal, so the operations it runs are not known".to_owned(),
                    label,
                    call.span,
                    Execution::Awaited,
                );
                return;
            }
        };
        for element in &array.elements {
            match element {
                ArrayExpressionElement::SpreadElement(spread) => {
                    self.visit_expression(&spread.argument);
                }
                ArrayExpressionElement::Elision(_) => {}
                element => {
                    if let Some(expression) = element.as_expression() {
                        self.consume(expression, Consumer::Concurrent);
                    }
                }
            }
        }
    }

    fn transaction(
        &mut self,
        call: &CallExpression<'a>,
        handle: DbHandle,
        recognition: Recognition,
        label: String,
        execution: Execution,
    ) {
        let callback = call
            .arguments
            .first()
            .and_then(Argument::as_expression)
            .map(Expression::get_inner_expression);
        let (params, body, is_async, callback_span) = match callback {
            Some(Expression::ArrowFunctionExpression(function)) => (
                &function.params,
                Body::Arrow(&function.body),
                function.r#async,
                function.span,
            ),
            Some(Expression::FunctionExpression(function)) => match &function.body {
                Some(body) => (
                    &function.params,
                    Body::Statements(&body.statements),
                    function.r#async,
                    function.span,
                ),
                None => {
                    return self.opaque_transaction(
                        call,
                        label,
                        execution,
                        "its callback has no body",
                    );
                }
            },
            _ => {
                return self.opaque_transaction(
                    call,
                    label,
                    execution,
                    "its callback is not an inline function, so its body is not analyzed here",
                );
            }
        };
        let (handle_symbol, handle_name) = match params.items.first().map(|param| &param.pattern) {
            None => (None, None),
            Some(BindingPattern::BindingIdentifier(id)) => {
                (id.symbol_id.get(), Some(id.name.as_str().to_owned()))
            }
            Some(_) => {
                return self.opaque_transaction(
                    call,
                    label,
                    execution,
                    "its transaction handle parameter is destructured, so writes cannot be attributed to the transaction",
                );
            }
        };

        let id = TransactionId(u32::try_from(self.transactions.len()).unwrap_or(u32::MAX));
        let (parent, parent_parameter) = match handle {
            DbHandle::Transaction { id } => (Some(id), None),
            DbHandle::Root { .. } => (None, None),
            DbHandle::Parameter { index, .. } => (None, Some(index)),
        };
        self.transactions.push(TransactionInfo {
            id,
            parent,
            label,
            location: self.location(Span::new(call.span.start, call.callee.span().end)),
            handle: handle_name,
            recognition,
            summary: None,
            parent_parameter,
        });
        if let Some(symbol) = handle_symbol {
            self.locals.transaction_handles.insert(symbol, id);
            // A handle, not data.
            self.vs.env_mut().set(
                key(symbol),
                AbsValue::atom(Atom::Static {
                    symbol: key(symbol),
                }),
            );
        }
        self.consumed.push(callback_span.start);
        let before = self.fork();
        self.callback_depth += 1;

        let saved_async = std::mem::replace(&mut self.is_async, is_async);
        // Returns of this callback bind the transaction's result variable,
        // if the transaction call is the awaited initializer of a `const`;
        // returns of nested callbacks do not.
        let binding = match &self.pending_result {
            Some((start, variable, _)) if *start == call.span.start => Some(variable.clone()),
            _ => None,
        };
        let saved_binding = std::mem::replace(&mut self.return_binding, binding);
        let saved_pending = self.pending_result.take();
        let nodes = self.collect(|this| this.body(body));
        self.return_binding = saved_binding;
        self.pending_result = saved_pending;
        self.is_async = saved_async;
        self.callback_depth -= 1;
        // The callback may stop at any point (an exception rolls back).
        self.rejoin(&before);
        self.out.push(Node::Transaction(TransactionScope {
            transaction: id,
            execution,
            body: nodes,
        }));
    }

    fn opaque_transaction(
        &mut self,
        call: &CallExpression<'a>,
        label: String,
        execution: Execution,
        detail: &str,
    ) {
        self.arguments(&[call]);
        self.push_unknown(
            UnknownReason::OpaqueTransaction,
            detail.to_owned(),
            label,
            call.span,
            execution,
        );
    }

    fn arguments(&mut self, calls: &[&CallExpression<'a>]) {
        for call in calls {
            for argument in &call.arguments {
                self.visit_argument(argument);
            }
        }
    }

    // --- IR construction -------------------------------------------------

    fn collect(&mut self, lower: impl FnOnce(&mut Self)) -> Vec<Node> {
        let saved = std::mem::take(&mut self.out);
        lower(self);
        let mut nodes = std::mem::replace(&mut self.out, saved);
        // Bodies are built by pushing and usually hold one or two nodes;
        // the growth slack would otherwise be retained for the whole
        // project analysis (measured: ~60% of midday's IR bytes).
        nodes.shrink_to_fit();
        nodes
    }

    fn push_effect(
        &mut self,
        kind: EffectKind,
        label: String,
        span: Span,
        execution: Execution,
        recognition: Option<Recognition>,
    ) {
        self.push_effect_with(kind, label, span, execution, recognition, Vec::new(), None);
    }

    #[allow(clippy::too_many_arguments)]
    fn push_effect_with(
        &mut self,
        kind: EffectKind,
        label: String,
        span: Span,
        execution: Execution,
        recognition: Option<Recognition>,
        references: Vec<String>,
        provenance: Option<Provenance>,
    ) {
        let id = EffectId(self.next_effect);
        self.next_effect += 1;
        let location = self.location(span);
        self.out.push(Node::Effect(Effect {
            id,
            kind,
            label: label.into(),
            location,
            execution,
            recognition: recognition.map(Box::new),
            summary: None,
            references: references.into_boxed_slice(),
            provenance: provenance.map(Box::new),
            nonempty: Vec::new(),
        }));
    }

    fn push_db_effect(&mut self, builder: Builder, span: Span, execution: Execution) {
        self.push_db_effect_with(builder, span, execution, Vec::new(), None);
    }

    fn push_db_effect_with(
        &mut self,
        builder: Builder,
        span: Span,
        execution: Execution,
        references: Vec<String>,
        provenance: Option<Provenance>,
    ) {
        let Builder {
            access,
            table,
            handle,
            recognition,
            label,
        } = builder;
        let references: Vec<String> = references
            .into_iter()
            .filter(|reference| {
                let root = reference.split('.').next().unwrap_or(reference);
                table.as_deref() != Some(root)
            })
            .collect();
        let kind = match access {
            DbAccess::Read => EffectKind::DatabaseRead { handle, table },
            DbAccess::Write(operation) => EffectKind::DatabaseWrite {
                operation,
                table,
                handle,
            },
        };
        self.push_effect_with(
            kind,
            label,
            span,
            execution,
            Some(recognition),
            references,
            provenance,
        );
    }

    fn push_unknown(
        &mut self,
        reason: UnknownReason,
        detail: String,
        label: String,
        span: Span,
        execution: Execution,
    ) {
        self.push_effect(
            EffectKind::Unknown {
                reason,
                detail,
                deferred: None,
            },
            label,
            span,
            execution,
            None,
        );
    }

    fn push_branch(&mut self, kind: BranchKind, condition: Span, span: Span, arms: Vec<Arm>) {
        if arms.iter().all(|arm| arm.body.is_empty()) {
            return;
        }
        let branch = Branch {
            kind,
            condition: self.text(condition),
            location: self.location(span),
            arms,
        };
        self.out.push(Node::Branch(branch));
    }

    fn push_loop(
        &mut self,
        kind: LoopKind,
        span: Span,
        body_span: Span,
        runs_at_least_once: bool,
        body: Vec<Node>,
    ) {
        if body.is_empty() {
            return;
        }
        let header = if kind == LoopKind::DoWhile {
            "do … while".to_owned()
        } else {
            self.text(Span::new(span.start, body_span.start))
        };
        let lp = Loop {
            kind,
            header,
            location: self.location(span),
            runs_at_least_once,
            body,
        };
        self.out.push(Node::Loop(lp));
    }

    fn exit(
        &mut self,
        kind: ExitKind,
        span: Span,
        transaction: Option<TransactionId>,
        detail: Option<String>,
    ) {
        let location = self.location(span);
        self.out.push(Node::Exit(Exit {
            kind,
            location,
            transaction,
            detail,
            facts: Vec::new(),
        }));
    }

    fn location(&self, span: Span) -> Location {
        self.lines.location(span.start, span.end)
    }

    /// Compact, truncated source text for conditions and headers.
    fn text(&self, span: Span) -> String {
        truncate(&compact(self.recognizer.slice(span.start, span.end)), 60)
    }
}

impl<'a> Visit<'a> for Lowerer<'_> {
    // Defining a function or class runs nothing; they are analyzed separately.
    fn visit_function(&mut self, _function: &Function<'a>, _flags: ScopeFlags) {}

    fn visit_arrow_function_expression(&mut self, _function: &ArrowFunctionExpression<'a>) {}

    fn visit_class(&mut self, _class: &Class<'a>) {}

    fn visit_await_expression(&mut self, it: &AwaitExpression<'a>) {
        self.consume(&it.argument, Consumer::Await);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        self.call(it, None);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        walk::walk_assignment_expression(self, it);
        let value = if it.operator == oxc_syntax::operator::AssignmentOperator::Assign {
            self.eval(&it.right)
        } else {
            AbsValue::top()
        };
        match &it.left {
            AssignmentTarget::AssignmentTargetIdentifier(ident) => {
                if let Some(symbol) = resolve_reference(self.recognizer.scoping, ident) {
                    self.vs.env_mut().set(key(symbol), value.clone());
                }
            }
            AssignmentTarget::StaticMemberExpression(member) => {
                let target = self.eval(&member.object);
                self.vs
                    .heap_mut()
                    .write(&target, member.property.name.as_str(), value.clone());
            }
            AssignmentTarget::ComputedMemberExpression(member) => {
                let target = self.eval(&member.object);
                match literal_value(&member.expression) {
                    Some(Literal::String(name) | Literal::Number(name)) => {
                        self.vs.heap_mut().write(&target, &name, value.clone());
                    }
                    _ => {
                        self.vs.heap_mut().write_unknown(&target);
                        self.vs.escape(&value);
                    }
                }
            }
            target => {
                // Destructuring assignment: the targets become unknown.
                let mut collector = TargetIdentifiers::default();
                collector.visit_assignment_target(target);
                for symbol in collector
                    .idents
                    .iter()
                    .filter_map(|ident| resolve_reference(self.recognizer.scoping, ident))
                    .collect::<Vec<_>>()
                {
                    self.vs.env_mut().set(key(symbol), AbsValue::top());
                }
                self.vs.escape(&value);
            }
        }
        self.assigned.insert(it.span.start, value);
    }

    fn visit_update_expression(&mut self, it: &UpdateExpression<'a>) {
        walk::walk_update_expression(self, it);
        if let SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) = &it.argument
            && let Some(symbol) = resolve_reference(self.recognizer.scoping, ident)
        {
            self.vs.env_mut().set(key(symbol), AbsValue::top());
        }
    }

    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        // Already lowered as the receiver of a method call (`new C().m()`).
        if self.early_news.remove(&it.span.start) {
            return;
        }
        walk::walk_new_expression(self, it);
        let mut values = Vec::with_capacity(it.arguments.len());
        for argument in &it.arguments {
            match argument.as_expression() {
                Some(expression) => {
                    let value = self.eval(expression);
                    values.push(value);
                }
                None => break,
            }
        }
        let label = format!("new {}", self.recognizer.callee_label(&it.callee));
        let table = self.recognizer.table;
        let scoping = self.recognizer.scoping;
        let class = match it.callee.get_inner_expression() {
            Expression::Identifier(ident) => match resolve_reference(scoping, ident) {
                None => {
                    match builtins::global_constructor(ident.name.as_str()) {
                        Some(builtin) => {
                            if let Some((index, _)) = builtin.callback
                                && let Some(value) = values.get(index)
                            {
                                match value.single() {
                                    Some(Atom::Function { .. } | Atom::Closure { .. }) => self.facts.calls.push(Self::callback_fact(value, label.clone()).expect("known callback")),
                                    Some(Atom::Param { index }) => {
                                        self.facts.param(*index).invoked = true;
                                    }
                                    _ => self.facts.refute(format!(
                                        "passes a value that is not a known function to `{label}`, which runs it"
                                    )),
                                }
                            }
                        }
                        None => {
                            self.facts.refute(format!(
                                "constructs `{label}`, whose constructor is not known"
                            ));
                            self.escape_values(&values);
                        }
                    }
                    None
                }
                Some(symbol) => {
                    if let Some(start) = table.class_of(symbol) {
                        Some((start, ClassRef::Local(start)))
                    } else if let Some(import) = table.import(symbol).filter(|i| !i.type_only) {
                        let name = match &import.kind {
                            ImportKind::Named(name) => Some(name.clone()),
                            ImportKind::Default => Some("default".to_owned()),
                            ImportKind::Namespace => None,
                        };
                        name.map(|name| {
                            let id = IMPORTED_CLASS
                                | u32::try_from(self.imported_classes.len()).unwrap_or(0);
                            let class = ClassRef::Imported {
                                module: import.module.clone(),
                                name,
                            };
                            let id = self
                                .imported_classes
                                .iter()
                                .find(|(_, c)| **c == class)
                                .map_or(id, |(k, _)| *k);
                            self.imported_classes.insert(id, class.clone());
                            (id, class)
                        })
                    } else {
                        None
                    }
                }
            },
            _ => None,
        };
        let value = match class {
            Some((id, class)) => {
                let args = values.iter().map(Self::fact_arg).collect();
                self.facts.calls.push(FactCall {
                    target: CallTarget::Constructor(class),
                    args,
                    label,
                });
                self.escape_values(&values);
                AbsValue::atom(
                    self.vs
                        .heap_mut()
                        .allocate(it.span.start, ObjectKind::Instance { class: id }),
                )
            }
            None => {
                if !matches!(it.callee.get_inner_expression(), Expression::Identifier(ident) if resolve_reference(scoping, ident).is_none())
                {
                    self.facts.refute(format!(
                        "constructs `{label}`, whose constructor is not known"
                    ));
                    self.escape_values(&values);
                }
                self.opaque()
            }
        };
        self.call_values.insert(it.span.start, value);
    }

    fn visit_tagged_template_expression(&mut self, it: &TaggedTemplateExpression<'a>) {
        walk::walk_tagged_template_expression(self, it);
        let helper = match it.tag.get_inner_expression() {
            Expression::Identifier(ident) => resolve_reference(self.recognizer.scoping, ident)
                .is_some_and(|symbol| self.is_query_helper(symbol)),
            _ => false,
        };
        if !helper {
            let label = truncate(
                &compact(self.recognizer.slice(it.span.start, it.span.end)),
                60,
            );
            self.facts.refute(format!(
                "calls the template tag of `{label}`, whose target is not known"
            ));
        }
    }

    fn visit_import_expression(&mut self, it: &ImportExpression<'a>) {
        walk::walk_import_expression(self, it);
        self.facts
            .refute("loads a module dynamically (`import(...)`)".to_owned());
    }

    fn visit_conditional_expression(&mut self, it: &ConditionalExpression<'a>) {
        self.visit_expression(&it.test);
        let before = self.fork();
        let then = self.collect(|this| this.visit_expression(&it.consequent));
        let after_then = std::mem::replace(&mut self.vs, before);
        let otherwise = self.collect(|this| this.visit_expression(&it.alternate));
        self.rejoin(&after_then);
        let arms = vec![
            self.arm_if("then", then, &it.test, true),
            self.arm_if("else", otherwise, &it.test, false),
        ];
        self.push_branch(BranchKind::Conditional, it.test.span(), it.span, arms);
    }

    fn visit_logical_expression(&mut self, it: &LogicalExpression<'a>) {
        self.visit_expression(&it.left);
        let before = self.fork();
        let right = self.collect(|this| this.visit_expression(&it.right));
        self.rejoin(&before);
        let condition = Span::new(it.left.span().start, it.right.span().start);
        let (evaluated, skipped) = match it.operator {
            LogicalOperator::And => (self.facts(&it.left, true), self.facts(&it.left, false)),
            LogicalOperator::Or => (self.facts(&it.left, false), self.facts(&it.left, true)),
            // The right operand runs when the left is `null` or `undefined`,
            // both falsy; short-circuiting says nothing about truthiness.
            LogicalOperator::Coalesce => match self.condition_variable(&it.left) {
                Some(Variable::Free(variable)) => (
                    vec![Fact::Truthy {
                        variable,
                        value: false,
                    }],
                    Vec::new(),
                ),
                _ => (Vec::new(), Vec::new()),
            },
        };
        self.push_branch(
            BranchKind::Logical,
            condition,
            it.span,
            vec![
                Arm {
                    label: "evaluates the right operand".to_owned(),
                    body: right,
                    facts: evaluated,
                },
                Arm {
                    label: "short-circuits".to_owned(),
                    body: Vec::new(),
                    facts: skipped,
                },
            ],
        );
    }
}

/// A condition operand that cannot change within the function.
enum Variable {
    /// A variable whose value is unknown, by a name unique in the file.
    Free(String),
    /// A `const` binding of a literal.
    Constant(Literal),
}

/// `[]` when the arm can be taken, `[Never]` when it cannot.
fn never_unless(possible: bool) -> Vec<Fact> {
    if possible {
        Vec::new()
    } else {
        vec![Fact::Never]
    }
}

/// Facts implied by `variable == literal` (`strict`: `===`) holding
/// (`equal`) or not.
/// An exported argument value as the callee sees it on entry
/// (`LocalCall::entry_values`): primitive literals and parameter paths,
/// and closed records of those; everything else unknown. Function and
/// class atoms are dropped, so the value needs no renumbering when linked.
/// A parameter path is kept only for parameters in `stable` (what they
/// held on entry, see [`stable_parameters`]).
fn entry_shape(value: &AbsValue, stable: &HashSet<u32>) -> AbsValue {
    match value.single() {
        Some(Atom::Lit { .. }) => value.clone(),
        Some(atom) => match atom.root() {
            Atom::Param { index } if stable.contains(index) => value.clone(),
            Atom::Record { .. } => entry_record(atom, stable),
            _ => AbsValue::top(),
        },
        // One of several literals (`exists ? "POST" : "PUT"`).
        None if !value.atoms().is_empty()
            && value
                .atoms()
                .iter()
                .all(|atom| matches!(atom, Atom::Lit { .. })) =>
        {
            value.clone()
        }
        None => AbsValue::top(),
    }
}

/// Whether a parameter default leaves the request method unset: an object
/// literal of plain properties, none of them `method`.
fn empty_options(default: &Expression<'_>) -> bool {
    let Expression::ObjectExpression(object) = default.get_inner_expression() else {
        return false;
    };
    object.properties.iter().all(|property| match property {
        ObjectPropertyKind::ObjectProperty(property) => {
            !property.computed
                && property.kind == PropertyKind::Init
                && property
                    .key
                    .static_name()
                    .is_some_and(|name| name != "method")
        }
        ObjectPropertyKind::SpreadProperty(_) => false,
    })
}

/// The entry value of an object literal written as the argument itself,
/// when the heap cannot export it: a literal evaluated again in a loop
/// shares its allocation site with earlier iterations' objects, which
/// makes that object a summary. The object this call receives is the one
/// just created, so its own literal-valued properties are exactly those
/// literals. Other property values are unknown; a spread, computed key
/// or accessor makes the whole value unknown, since it may set any key.
fn literal_object_entry(expression: &Expression<'_>) -> AbsValue {
    let Expression::ObjectExpression(object) = expression.get_inner_expression() else {
        return AbsValue::top();
    };
    let mut props = std::collections::BTreeMap::new();
    for property in &object.properties {
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            return AbsValue::top();
        };
        let Some(name) = property
            .key
            .static_name()
            .filter(|_| !property.computed && property.kind == PropertyKind::Init)
        else {
            return AbsValue::top();
        };
        props.insert(name.display().to_string(), literal_entry(&property.value));
    }
    if props.values().all(AbsValue::is_top) {
        return AbsValue::top();
    }
    AbsValue::atom(Atom::Record {
        props: props
            .into_iter()
            .map(|(name, value)| (name.into_boxed_str(), value))
            .collect(),
        open: false,
    })
}

/// A literal, or a conditional between literals, as a value; otherwise
/// unknown.
fn literal_entry(expression: &Expression<'_>) -> AbsValue {
    if let Expression::ConditionalExpression(conditional) = expression.get_inner_expression() {
        let consequent = literal_entry(&conditional.consequent);
        let alternate = literal_entry(&conditional.alternate);
        if consequent.is_top() || alternate.is_top() {
            return AbsValue::top();
        }
        return consequent.joined(&alternate);
    }
    literal_value(expression).map_or_else(AbsValue::top, AbsValue::lit)
}

fn entry_record(atom: &Atom, stable: &HashSet<u32>) -> AbsValue {
    match atom {
        Atom::Record { props, open: false } => AbsValue::atom(Atom::Record {
            props: props
                .iter()
                .map(|(key, value)| (key.clone(), entry_shape(value, stable)))
                .collect(),
            open: false,
        }),
        _ => AbsValue::top(),
    }
}

/// Parameters whose value at a call is still what the function received:
/// a destructuring pattern (its parts are bound on entry), or a binding
/// never reassigned, never written through or method-called, with at most
/// one reference other than property reads and database write payloads
/// (the call that forwards it).
fn stable_parameters(params: &FormalParameters<'_>, recognizer: &Recognizer<'_>) -> HashSet<u32> {
    let scoping = recognizer.scoping;
    let mut out = HashSet::new();
    for (index, param) in params.items.iter().enumerate() {
        let stable = match &param.pattern {
            BindingPattern::BindingIdentifier(id) => id.symbol_id.get().is_some_and(|symbol| {
                !scoping.symbol_is_mutated(symbol)
                    && recognizer
                        .table
                        .other_references(symbol, scoping)
                        .is_some_and(|n| n <= 1)
            }),
            BindingPattern::ObjectPattern(_) | BindingPattern::ArrayPattern(_) => true,
            BindingPattern::AssignmentPattern(_) => false,
        };
        if stable {
            out.insert(u32::try_from(index).unwrap_or(u32::MAX));
        }
    }
    out
}

fn comparison_facts(variable: Variable, literal: Literal, strict: bool, equal: bool) -> Vec<Fact> {
    let name = match variable {
        Variable::Free(name) => name,
        Variable::Constant(value) => {
            // Both sides are known; only strict comparisons are evaluated.
            return if strict {
                never_unless((value == literal) == equal)
            } else {
                Vec::new()
            };
        }
    };
    match (strict, equal, &literal) {
        (true, true, _) => vec![Fact::Equals {
            variable: name,
            literal,
        }],
        (true, false, _) => vec![Fact::NotEquals {
            variable: name,
            literal,
        }],
        // `x == null` and `x == undefined` hold exactly for null and
        // undefined, which are falsy.
        (false, true, Literal::Null | Literal::Undefined) => vec![Fact::Truthy {
            variable: name,
            value: false,
        }],
        // `x == true` holds only for values that convert to 1: all truthy.
        (false, true, Literal::Boolean(true)) => vec![Fact::Truthy {
            variable: name,
            value: true,
        }],
        _ => Vec::new(),
    }
}

/// Unwind a call expression that is not wrapped in an [`Expression`].
fn declared_result(value: &serde_json::Value) -> AbsValue {
    match value {
        serde_json::Value::Bool(v) => AbsValue::lit(Literal::Boolean(*v)),
        serde_json::Value::String(v) => AbsValue::lit(Literal::String(v.clone())),
        serde_json::Value::Number(v) => {
            v.as_f64()
                .filter(|v| v.is_finite())
                .map_or_else(AbsValue::top, |v| {
                    AbsValue::lit(Literal::Number(if v == 0.0 {
                        "0".to_owned()
                    } else {
                        v.to_string()
                    }))
                })
        }
        serde_json::Value::Null => AbsValue::lit(Literal::Null),
        serde_json::Value::Object(fields) => AbsValue::atom(Atom::Record {
            props: fields
                .iter()
                .map(|(name, value)| (name.clone().into_boxed_str(), declared_result(value)))
                .collect(),
            open: true,
        }),
        serde_json::Value::Array(_) => AbsValue::top(),
    }
}

fn alternative_callback_paths(call: &CallExpression<'_>) -> bool {
    if let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression()
        && let Expression::ArrayExpression(array) = member.object.get_inner_expression()
        && array.elements.len() <= 1
        && !array
            .elements
            .iter()
            .any(|e| matches!(e, ArrayExpressionElement::SpreadElement(_)))
    {
        return false;
    }
    struct Paths {
        branches: usize,
        awaits: usize,
    }
    impl<'a> Visit<'a> for Paths {
        fn visit_if_statement(&mut self, it: &oxc_ast::ast::IfStatement<'a>) {
            self.branches += 1;
            walk::walk_if_statement(self, it);
        }
        fn visit_await_expression(&mut self, it: &AwaitExpression<'a>) {
            self.awaits += 1;
            walk::walk_await_expression(self, it);
        }
        fn visit_function(&mut self, _: &Function<'a>, _: ScopeFlags) {}
        fn visit_arrow_function_expression(&mut self, _: &ArrowFunctionExpression<'a>) {}
    }
    let mut paths = Paths {
        branches: 0,
        awaits: 0,
    };
    match call
        .arguments
        .first()
        .and_then(Argument::as_expression)
        .map(Expression::get_inner_expression)
    {
        Some(Expression::ArrowFunctionExpression(callback)) => {
            paths.visit_arrow_function_body(&callback.body)
        }
        Some(Expression::FunctionExpression(callback)) => {
            if let Some(body) = &callback.body {
                paths.visit_function_body(body);
            }
        }
        _ => return false,
    }
    paths.branches > 0 && paths.awaits > 1
}

fn unwind_call<'e, 'a>(call: &'e CallExpression<'a>) -> crate::recognize::Chain<'e, 'a> {
    let mut chain = unwind(&call.callee);
    chain.segments.push(crate::recognize::Segment::Call(call));
    chain
}

/// The variables the arguments of `calls` refer to: identifiers and
/// member paths of identifiers (`id`, `input.orderId`), sorted and
/// de-duplicated. Callees inside arguments (`eq`, `String`) and function
/// bodies are skipped. Names only: no value flow is tracked.
fn argument_references(calls: &[&CallExpression<'_>]) -> Vec<String> {
    #[derive(Default)]
    struct Collector {
        names: std::collections::BTreeSet<String>,
    }

    fn member_path(expression: &Expression<'_>) -> Option<String> {
        match expression {
            Expression::Identifier(ident) => Some(ident.name.to_string()),
            Expression::StaticMemberExpression(member) => {
                member_path(&member.object).map(|base| format!("{base}.{}", member.property.name))
            }
            _ => None,
        }
    }

    impl<'a> Visit<'a> for Collector {
        fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
            if ident.name != "undefined" {
                self.names.insert(ident.name.to_string());
            }
        }

        fn visit_expression(&mut self, expression: &Expression<'a>) {
            if let Expression::StaticMemberExpression(_) = expression
                && let Some(path) = member_path(expression)
            {
                self.names.insert(path);
                return;
            }
            walk::walk_expression(self, expression);
        }

        fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
            for argument in &call.arguments {
                self.visit_argument(argument);
            }
        }

        fn visit_new_expression(&mut self, new: &oxc_ast::ast::NewExpression<'a>) {
            for argument in &new.arguments {
                self.visit_argument(argument);
            }
        }

        fn visit_function(&mut self, _: &Function<'a>, _: ScopeFlags) {}

        fn visit_arrow_function_expression(&mut self, _: &ArrowFunctionExpression<'a>) {}
    }

    let mut collector = Collector::default();
    for call in calls {
        for argument in &call.arguments {
            collector.visit_argument(argument);
        }
    }
    collector.names.into_iter().collect()
}

/// Identifier references inside an assignment target pattern.
#[derive(Default)]
struct TargetIdentifiers<'e, 'a> {
    idents: Vec<&'e IdentifierReference<'a>>,
}

impl<'e, 'a> TargetIdentifiers<'e, 'a> {
    fn visit_assignment_target(&mut self, target: &'e AssignmentTarget<'a>) {
        use oxc_ast::ast::{AssignmentTargetMaybeDefault, AssignmentTargetProperty};
        match target {
            AssignmentTarget::AssignmentTargetIdentifier(ident) => self.idents.push(ident),
            AssignmentTarget::ObjectAssignmentTarget(object) => {
                for property in &object.properties {
                    match property {
                        AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(p) => {
                            self.idents.push(&p.binding);
                        }
                        AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) => {
                            self.visit_maybe_default(&p.binding);
                        }
                    }
                }
                if let Some(rest) = &object.rest {
                    self.visit_assignment_target(&rest.target);
                }
            }
            AssignmentTarget::ArrayAssignmentTarget(array) => {
                for element in array.elements.iter().flatten() {
                    self.visit_maybe_default(element);
                }
                if let Some(rest) = &array.rest {
                    self.visit_assignment_target(&rest.target);
                }
            }
            _ => {}
        }
        let _: Option<&AssignmentTargetMaybeDefault<'a>> = None;
    }

    fn visit_maybe_default(&mut self, target: &'e oxc_ast::ast::AssignmentTargetMaybeDefault<'a>) {
        use oxc_ast::ast::AssignmentTargetMaybeDefault;
        match target {
            AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(with_default) => {
                self.visit_assignment_target(&with_default.binding);
            }
            other => {
                if let Some(target) = other.as_assignment_target() {
                    self.visit_assignment_target(target);
                }
            }
        }
    }
}

/// `await <db write chain>.returning()` whose rows are exactly the rows the
/// write changed: `update` and `delete`, and `insert` that skips conflicting
/// rows (`onConflictDoNothing`). An upsert returns rows it did not insert.
fn returns_changed_rows(init: &Expression<'_>) -> bool {
    let Expression::AwaitExpression(awaited) = init.get_inner_expression() else {
        return false;
    };
    let mut names = Vec::new();
    let mut expression = awaited.argument.get_inner_expression();
    while let Expression::CallExpression(call) = expression {
        let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
            break;
        };
        names.push(member.property.name.as_str());
        expression = member.object.get_inner_expression();
    }
    let has = |name: &str| names.contains(&name);
    names.first() == Some(&"returning")
        && !has("onConflictDoUpdate")
        && (has("update") || has("delete") || (has("insert") && has("onConflictDoNothing")))
}

/// Finds a write in a function body and checks every `return` that may
/// follow it (in source order, conservatively including later sibling
/// arms) against the row variable it binds. A write inside a loop, or
/// a `return` of anything else, fails the check.
struct RowScan<'s> {
    effect: EffectId,
    variable: &'s str,
    returns: &'s [(Location, Option<String>)],
    seen: bool,
    ok: bool,
}

impl RowScan<'_> {
    fn nodes(&mut self, nodes: &[Node], in_loop: bool) {
        for node in nodes {
            match node {
                Node::Effect(effect) if effect.id == self.effect => {
                    self.seen = true;
                    self.ok &= !in_loop;
                }
                Node::Exit(exit) if self.seen && exit.kind == ExitKind::Return => {
                    let returned = self
                        .returns
                        .iter()
                        .find(|(location, _)| *location == exit.location)
                        .and_then(|(_, row)| row.as_deref());
                    self.ok &= returned == Some(self.variable);
                }
                Node::Branch(branch) => {
                    for arm in &branch.arms {
                        self.nodes(&arm.body, in_loop);
                    }
                }
                Node::Switch(switch) => {
                    for case in &switch.cases {
                        self.nodes(&case.body, in_loop);
                    }
                }
                Node::Try(try_node) => {
                    self.nodes(&try_node.block, in_loop);
                    if let Some(handler) = &try_node.handler {
                        self.nodes(&handler.body, in_loop);
                    }
                    if let Some(finalizer) = &try_node.finalizer {
                        self.nodes(finalizer, in_loop);
                    }
                }
                Node::Loop(lp) => self.nodes(&lp.body, true),
                // Returns of callbacks and of inlined calls are not this
                // function's; a write inside them is not this binding's.
                _ => {}
            }
        }
    }
}

/// The effect `id` among `nodes`, outside callbacks and inlined calls.
fn effect_mut(nodes: &mut [Node], id: EffectId) -> Option<&mut Effect> {
    for node in nodes {
        let found = match node {
            Node::Effect(effect) if effect.id == id => return Some(effect),
            Node::Branch(branch) => branch
                .arms
                .iter_mut()
                .find_map(|arm| effect_mut(&mut arm.body, id)),
            Node::Switch(switch) => switch
                .cases
                .iter_mut()
                .find_map(|case| effect_mut(&mut case.body, id)),
            Node::Try(try_node) => effect_mut(&mut try_node.block, id)
                .or_else(|| {
                    try_node
                        .handler
                        .as_mut()
                        .and_then(|handler| effect_mut(&mut handler.body, id))
                })
                .or_else(|| {
                    try_node
                        .finalizer
                        .as_mut()
                        .and_then(|finalizer| effect_mut(finalizer, id))
                }),
            Node::Loop(lp) => effect_mut(&mut lp.body, id),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// An import specifier for `to` written in `from` (both as diagnostics show
/// them): `apps/a/x.ts` → `apps/b/y.ts` is `../b/y`.
fn relative_specifier(from: &str, to: &str) -> String {
    let from_dir: Vec<&str> = from.split('/').collect();
    let from_dir = &from_dir[..from_dir.len().saturating_sub(1)];
    let to_parts: Vec<&str> = to.split('/').collect();
    let common = from_dir
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> = Vec::new();
    for _ in common..from_dir.len() {
        parts.push("..".to_owned());
    }
    if parts.is_empty() {
        parts.push(".".to_owned());
    }
    for part in &to_parts[common..] {
        parts.push((*part).to_owned());
    }
    let mut specifier = parts.join("/");
    for extension in [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"] {
        if let Some(stripped) = specifier.strip_suffix(extension) {
            specifier = stripped.to_owned();
            break;
        }
    }
    specifier
}

//! Phase 16: bounded loops, caught errors and request bodies
//! (`docs/phase16/LOOP-SEMANTICS.md`, `docs/phase16/FRAMEWORK-REQUESTS.md`).
//! Active only with JavaScript value semantics
//! ([`ExtractRequest::js_values`](midwrite_model::provider::ExtractRequest));
//! without them the Phase 7–15 lowering (loops refused, caught errors
//! opaque) is unchanged.
//!
//! **Loops.** `for (init; test; update)`, `while (test)` and
//! `do ... while (test)` are unrolled into the existing `Let`/`JumpIf`/`Jump`
//! operations: each iteration is lowered again from the source (its own
//! statements, generated values, `await`s and transactions, in order), the
//! test is evaluated before every iteration, `let` declarations of a `for`
//! head get a fresh binding per iteration (copied before the update, as
//! ECMAScript's `CreatePerIterationEnvironment` does), `break` leaves the
//! unrolled loop, `continue` goes to the iteration's update. Nothing is
//! merged across iterations. A loop gets one of these classifications,
//! reported as an extraction assumption starting with [`LOOP_PREFIX`]:
//!
//! * `BOUNDED_UNROLLING`: the counter analysis establishes a bound of at most
//!   [`MAX_UNROLL`] iterations (a comparison of a local counter with a
//!   constant; the counter's value at the loop's entry is a known constant;
//!   one increment toward the bound runs on every iteration; every
//!   assignment of the counter lowered inside the loop, inlined helpers and
//!   closures included, is such an increment). All iterations are unrolled,
//!   nothing follows them.
//! * `TRUNCATED`: no bound is established; [`MAX_UNROLL`] iterations are
//!   unrolled and an execution that would start one more stops at an
//!   unsupported operation ("loop may execute beyond the explored bound").
//!   Executions within the bound are exact.
//! * `UNKNOWN`: the loop's condition is not modeled, or nested loops exceed
//!   [`MAX_UNROLL_PRODUCT`] iterations: the loop is an unsupported operation.
//! * `UNSUPPORTED`: a loop form outside the subset (`var` or destructuring in
//!   a `for` head, `for...in`, `for await`).
//! * `COMPLETED`: `for...of` over an array literal or a list value (Phase 7 /
//!   Phase 14 lowering, every element visited).
//!
//! **Caught errors.** A `catch (e)` whose body uses `e` other than in `throw
//! e` binds the error object the application would see, when the failure's
//! origin is modeled: a database statement's or COMMIT's error in the shape
//! the project's ORM and driver give it (verified versions only, see
//! [`Lowerer::caught_shape`]), or a `throw new Error("...")` lowered in a
//! `try` block whose handler inspects it. Any other origin (a lost
//! connection, an unmodeled `throw`, a failed child task) gives an error
//! object none of whose properties is known: reading one is an unsupported
//! evaluation, never `undefined`.
//!
//! **Requests.** A route handler parameter declared `Request` (or
//! `NextRequest` from `next/server`): `await request.json()` is the actor's
//! argument (the parsed JSON body); `Response.json(body, { status })`,
//! `NextResponse.json(...)` and `new Response(body, { status })` are the
//! value `{ status, body }`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use oxc_ast::ast::{
    Argument, BindingPattern, CallExpression, DoWhileStatement, Expression, ForStatement,
    ForStatementInit, IdentifierReference, NewExpression, ObjectPropertyKind,
    SimpleAssignmentTarget, Statement, ThrowStatement, UpdateExpression, VariableDeclarationKind,
    WhileStatement,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::{GetSpan, Span};
use oxc_syntax::operator::{AssignmentOperator, BinaryOperator, UnaryOperator, UpdateOperator};
use oxc_syntax::scope::ScopeFlags;

use midwrite_model::ir::{CaughtErrors, ErrorShape, Op};
use midwrite_model::value::{BinOp, Builtin, Expr, Value};

use super::{
    Binding, Cx, ImportRef, Lowerer, Module, Res, TOP, find_class, find_function, find_import,
    integer, literal, module_constant, static_key,
};

/// Phase 16: the most iterations one loop is unrolled to.
pub(super) const MAX_UNROLL: usize = 8;
/// Phase 16: the most iterations nested unrolled loops may multiply to.
pub(super) const MAX_UNROLL_PRODUCT: usize = 64;
/// Phase 16: the prefix of every loop classification assumption.
pub const LOOP_PREFIX: &str = "loop ";
/// Phase 16: the prefix of the caught-error shape assumption.
pub const CAUGHT_PREFIX: &str = "caught-errors: ";
/// Phase 16: the prefix of the request-body assumptions.
pub const REQUEST_PREFIX: &str = "request: ";

/// Phase 16 lowering state.
#[derive(Default)]
pub(super) struct Phase16 {
    /// Locals every assignment lowered so far set to the same integer
    /// (`None`: assigned something else, or different integers).
    const_ints: HashMap<String, Option<i64>>,
    /// The product of the iteration counts of the loops being unrolled (0:
    /// none).
    unroll_product: usize,
    /// `try` blocks being lowered whose handler inspects the caught error.
    pub(super) inspecting: u32,
    /// A handler inspecting a caught error was lowered (`throw x` of a local
    /// then keeps the thrown value).
    pub(super) caught_seen: bool,
    /// Request parameters whose body was read.
    bodies_read: HashSet<String>,
    /// The project's caught-error shape, computed once.
    caught_shape: Option<CaughtErrors>,
}

impl Phase16 {
    /// Record an assignment `local = expr` in source order.
    pub(super) fn note_let(&mut self, local: &str, expr: &Expr) {
        let value = match expr {
            Expr::Lit(Value::Int(n)) => Some(*n),
            _ => None,
        };
        match self.const_ints.get(local) {
            None => {
                self.const_ints.insert(local.to_owned(), value);
            }
            Some(Some(old)) if Some(*old) == value => {}
            Some(_) => {
                self.const_ints.insert(local.to_owned(), None);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    For,
    While,
    DoWhile,
}

impl Kind {
    fn text(self) -> &'static str {
        match self {
            Self::For => "`for` loop",
            Self::While => "`while` loop",
            Self::DoWhile => "`do...while` loop",
        }
    }
}

struct Loop<'s, 'a> {
    kind: Kind,
    span: Span,
    test: Option<&'s Expression<'a>>,
    update: Option<&'s Expression<'a>>,
    body: &'s Statement<'a>,
    /// Names declared with `let`/`const` in a `for` head.
    head: Vec<String>,
}

/// The counter analysis' result.
struct Plan {
    counter: String,
    local: String,
    start: i64,
    bound: i64,
    /// The comparison, as written with the counter on the left.
    op: &'static str,
    /// +1: the counter increases toward the bound; -1: it decreases.
    dir: i64,
    /// The increment every iteration performs (positive).
    step: i64,
    /// The most iterations the bound allows.
    max: usize,
    /// The local the bound was read from (it must not change in the loop).
    bound_local: Option<String>,
}

impl Lowerer<'_> {
    // -- loops ------------------------------------------------------------

    /// `for (init; test; update) body`. `false` without JavaScript values
    /// (the caller refuses the loop as before).
    pub(super) fn for_statement(&mut self, m: &Module<'_>, f: &ForStatement<'_>) -> bool {
        if !self.js() {
            return false;
        }
        self.frame_mut().scopes.push(HashMap::new());
        let mut head = Vec::new();
        match &f.init {
            None => {}
            Some(ForStatementInit::VariableDeclaration(d)) => {
                if !matches!(
                    d.kind,
                    VariableDeclarationKind::Let | VariableDeclarationKind::Const
                ) {
                    self.loop_refused(
                        m,
                        f.span,
                        "UNSUPPORTED",
                        "a `var` (or `using`) declaration in a `for` head is not modeled (it is not a per-iteration binding)",
                    );
                    self.frame_mut().scopes.pop();
                    return true;
                }
                for declarator in &d.declarations {
                    match &declarator.id {
                        BindingPattern::BindingIdentifier(id) => {
                            head.push(id.name.as_str().to_owned());
                        }
                        _ => {
                            self.loop_refused(
                                m,
                                f.span,
                                "UNSUPPORTED",
                                "destructuring in a `for` head is not modeled",
                            );
                            self.frame_mut().scopes.pop();
                            return true;
                        }
                    }
                }
                for declarator in &d.declarations {
                    self.declarator(m, declarator);
                }
            }
            Some(init) => {
                if let Some(e) = init.as_expression() {
                    self.expression_statement(m, e, e.span());
                }
            }
        }
        self.unroll(
            m,
            Loop {
                kind: Kind::For,
                span: f.span,
                test: f.test.as_ref(),
                update: f.update.as_ref(),
                body: &f.body,
                head,
            },
        );
        self.frame_mut().scopes.pop();
        true
    }

    /// `while (test) body`.
    pub(super) fn while_statement(&mut self, m: &Module<'_>, w: &WhileStatement<'_>) -> bool {
        if !self.js() {
            return false;
        }
        self.unroll(
            m,
            Loop {
                kind: Kind::While,
                span: w.span,
                test: Some(&w.test),
                update: None,
                body: &w.body,
                head: Vec::new(),
            },
        );
        true
    }

    /// `do body while (test)`.
    pub(super) fn do_while_statement(&mut self, m: &Module<'_>, d: &DoWhileStatement<'_>) -> bool {
        if !self.js() {
            return false;
        }
        self.unroll(
            m,
            Loop {
                kind: Kind::DoWhile,
                span: d.span,
                test: Some(&d.test),
                update: None,
                body: &d.body,
                head: Vec::new(),
            },
        );
        true
    }

    /// A loop the extractor does not unroll: an unsupported operation, and
    /// its classification.
    fn loop_refused(&mut self, m: &Module<'_>, span: Span, class: &str, why: &str) {
        let at = self.src(m, span).label();
        self.assume(format!("{LOOP_PREFIX}{class} at {at}: {why}"));
        self.unknown_op(m, span, why.to_owned());
    }

    /// Phase 16: the classification of a `for...of` (Phase 7/14 lowering).
    pub(super) fn for_of_completed(&mut self, m: &Module<'_>, span: Span, what: &str) {
        if !self.js() {
            return;
        }
        let at = self.src(m, span).label();
        self.assume(format!(
            "{LOOP_PREFIX}COMPLETED at {at}: `for...of` over {what}: every element is visited"
        ));
    }

    fn unroll(&mut self, m: &Module<'_>, lp: Loop<'_, '_>) {
        let what = lp.kind.text();
        let at = self.src(m, lp.span).label();
        let outer = self.p16.unroll_product;
        let plan = self.plan(m, &lp);
        let iterations = match &plan {
            Ok(p) => p.max.min(MAX_UNROLL),
            Err(_) => MAX_UNROLL,
        };
        let product = outer.max(1) * iterations.max(1);
        if product > MAX_UNROLL_PRODUCT {
            self.loop_refused(
                m,
                lp.span,
                "UNKNOWN",
                &format!(
                    "this {what} inside other unrolled loops exceeds the unrolling budget of {MAX_UNROLL_PRODUCT} iterations"
                ),
            );
            return;
        }
        let snapshot: Vec<usize> = self.codes.iter().map(Vec::len).collect();
        let mut counter_locals: HashSet<String> = HashSet::new();
        if let Ok(p) = &plan {
            counter_locals.insert(p.local.clone());
        }
        let mut copies: HashSet<(usize, usize)> = HashSet::new();
        let tries = self.frame().tries;
        self.frame_mut()
            .breaks
            .push((Vec::new(), Some(Vec::new()), tries));
        self.p16.unroll_product = product;
        let mut exits = Vec::new();
        let mut unknown_test = None;
        for k in 0..iterations {
            if !(lp.kind == Kind::DoWhile && k == 0)
                && let Some(test) = lp.test
            {
                let cond = self.expr(
                    m,
                    test,
                    Cx {
                        truthy: true,
                        ..TOP
                    },
                );
                if let Some(detail) = cond.unknown_detail() {
                    unknown_test = Some(detail.to_owned());
                    break;
                }
                exits.push(self.emit(
                    m,
                    test.span(),
                    Op::JumpIf {
                        cond: Expr::not(cond),
                        target: usize::MAX,
                    },
                ));
            }
            self.nested(m, lp.body);
            // `continue` goes to this iteration's update.
            let next = self.here();
            let continues = std::mem::take(
                self.frame_mut()
                    .breaks
                    .last_mut()
                    .expect("pushed above")
                    .1
                    .as_mut()
                    .expect("a loop target"),
            );
            for jump in continues {
                self.patch(jump, next);
            }
            self.next_iteration(m, &lp, &mut counter_locals, &mut copies);
        }
        let classification = if let Some(detail) = unknown_test {
            let why = format!("the loop condition is not modeled: {detail}");
            self.unknown_op(m, lp.span, format!("{what}: {why}"));
            format!("{LOOP_PREFIX}UNKNOWN at {at}: {why}")
        } else {
            let verified = match &plan {
                Ok(p) => self.verify_counter(p, &snapshot, &counter_locals, &copies),
                Err(why) => Err(why.clone()),
            };
            match (&plan, verified) {
                (Ok(p), Ok(())) if p.max <= MAX_UNROLL => format!(
                    "{LOOP_PREFIX}BOUNDED_UNROLLING at {at}: {what} with counter `{}` from {} by {}{} while `{} {} {}`: at most {} iteration(s), each unrolled",
                    p.counter,
                    p.start,
                    if p.dir > 0 { "+" } else { "-" },
                    p.step,
                    p.counter,
                    p.op,
                    p.bound,
                    p.max
                ),
                (_, verified) => {
                    let why = match verified {
                        Err(why) => why,
                        Ok(()) => match &plan {
                            Ok(p) => format!(
                                "the bound allows {} iterations, more than the unrolling limit of {MAX_UNROLL}",
                                p.max
                            ),
                            Err(why) => why.clone(),
                        },
                    };
                    // The residual: one more iteration would start here.
                    if let Some(test) = lp.test {
                        let cond = self.expr(
                            m,
                            test,
                            Cx {
                                truthy: true,
                                ..TOP
                            },
                        );
                        exits.push(self.emit(
                            m,
                            test.span(),
                            Op::JumpIf {
                                cond: Expr::not(cond),
                                target: usize::MAX,
                            },
                        ));
                    }
                    self.unknown_op(
                        m,
                        lp.span,
                        format!(
                            "loop may execute beyond the explored bound: this {what} is unrolled to {iterations} iteration(s) ({why}); an execution that would run iteration {} is not modeled",
                            iterations + 1
                        ),
                    );
                    format!(
                        "{LOOP_PREFIX}TRUNCATED at {at}: {what} unrolled to {iterations} iteration(s); {why}; loop may execute beyond the explored bound (an execution reaching iteration {} stops as unsupported)",
                        iterations + 1
                    )
                }
            }
        };
        let end = self.here();
        let (breaks, _, _) = self.frame_mut().breaks.pop().expect("pushed above");
        for jump in breaks.into_iter().chain(exits) {
            self.patch(jump, end);
        }
        self.p16.unroll_product = outer;
        self.assume(classification);
    }

    /// After an iteration's body: a fresh binding of every `let` of a `for`
    /// head (holding the current value), then the update.
    fn next_iteration(
        &mut self,
        m: &Module<'_>,
        lp: &Loop<'_, '_>,
        counter_locals: &mut HashSet<String>,
        copies: &mut HashSet<(usize, usize)>,
    ) {
        for name in &lp.head {
            let Some(Binding::Local(prev)) = self.lookup(name) else {
                continue;
            };
            let fresh = self.fresh(name);
            let at = self.emit(
                m,
                lp.span,
                Op::Let {
                    local: fresh.clone(),
                    expr: Expr::Local(prev.clone()),
                },
            );
            copies.insert(at);
            if counter_locals.contains(&prev) {
                counter_locals.insert(fresh.clone());
            }
            self.declare(name, Binding::Local(fresh));
        }
        if let Some(update) = lp.update {
            self.expression_statement(m, update, update.span());
        }
    }

    /// The counter analysis: a bound from the source, or why there is none.
    fn plan(&mut self, m: &Module<'_>, lp: &Loop<'_, '_>) -> Result<Plan, String> {
        let Some(test) = lp.test else {
            return Err("the loop has no condition".to_owned());
        };
        let Expression::BinaryExpression(b) = test.get_inner_expression() else {
            return Err(
                "the condition is not a comparison of a counter with a constant".to_owned(),
            );
        };
        let flip = |op: BinaryOperator| match op {
            BinaryOperator::LessThan => Some(BinaryOperator::GreaterThan),
            BinaryOperator::LessEqualThan => Some(BinaryOperator::GreaterEqualThan),
            BinaryOperator::GreaterThan => Some(BinaryOperator::LessThan),
            BinaryOperator::GreaterEqualThan => Some(BinaryOperator::LessEqualThan),
            _ => None,
        };
        let (counter, bound_expr, op) = match (
            b.left.get_inner_expression(),
            b.right.get_inner_expression(),
        ) {
            (Expression::Identifier(id), other) if flip(b.operator).is_some() => {
                (id.name.as_str().to_owned(), other, b.operator)
            }
            (other, Expression::Identifier(id)) if flip(b.operator).is_some() => (
                id.name.as_str().to_owned(),
                other,
                flip(b.operator).expect("checked"),
            ),
            _ => {
                return Err(
                    "the condition is not a comparison of a counter with a constant".to_owned(),
                );
            }
        };
        let Some(Binding::Local(local)) = self.lookup(&counter) else {
            return Err(format!("`{counter}` is not a local variable"));
        };
        let Some(bound) = self.static_int(m, bound_expr) else {
            return Err(format!(
                "the bound `{counter}` is compared with is not a known integer constant"
            ));
        };
        let Some(Some(start)) = self.p16.const_ints.get(&local).copied() else {
            return Err(format!(
                "the value of `{counter}` when the loop starts is not a known integer constant"
            ));
        };
        let (dir, strict, text) = match op {
            BinaryOperator::LessThan => (1, true, "<"),
            BinaryOperator::LessEqualThan => (1, false, "<="),
            BinaryOperator::GreaterThan => (-1, true, ">"),
            _ => (-1, false, ">="),
        };
        let step = match lp.kind {
            Kind::For => lp.update.and_then(|u| increment_of(u, &counter)),
            Kind::While | Kind::DoWhile => {
                if has_continue(lp.body) {
                    return Err(
                        "the body has a `continue`, which can skip the counter's increment"
                            .to_owned(),
                    );
                }
                let statements: Vec<&Statement<'_>> = match lp.body {
                    Statement::BlockStatement(block) => block.body.iter().collect(),
                    other => vec![other],
                };
                statements.into_iter().find_map(|s| match s {
                    Statement::ExpressionStatement(e) => {
                        increment_of(&e.expression, &counter).filter(|k| k.signum() == dir)
                    }
                    _ => None,
                })
            }
        };
        let Some(step) = step.filter(|k| k.signum() == dir) else {
            return Err(format!(
                "no {} of `{counter}` toward the bound runs on every iteration",
                if lp.kind == Kind::For {
                    "update clause"
                } else {
                    "top-level statement of the body"
                }
            ));
        };
        let step = step.abs();
        let dist = dir * (bound - start);
        let mut max = if strict {
            if dist <= 0 {
                0
            } else {
                (dist + step - 1) / step
            }
        } else if dist < 0 {
            0
        } else {
            dist / step + 1
        };
        if lp.kind == Kind::DoWhile {
            max = max.max(1);
        }
        Ok(Plan {
            counter,
            local,
            start,
            bound,
            op: text,
            dir,
            step,
            max: usize::try_from(max).unwrap_or(usize::MAX),
            bound_local: match bound_expr.get_inner_expression() {
                Expression::Identifier(id) => match self.lookup(id.name.as_str()) {
                    Some(Binding::Local(l)) => Some(l),
                    _ => None,
                },
                _ => None,
            },
        })
    }

    /// Every assignment of the counter emitted inside the loop (inlined
    /// helpers, closures and spawned tasks included) is an increment toward
    /// the bound (or a per-iteration copy).
    fn verify_counter(
        &self,
        p: &Plan,
        snapshot: &[usize],
        counter_locals: &HashSet<String>,
        copies: &HashSet<(usize, usize)>,
    ) -> Result<(), String> {
        for (ci, code) in self.codes.iter().enumerate() {
            let from = snapshot.get(ci).copied().unwrap_or(0);
            for (pc, step) in code.iter().enumerate().skip(from) {
                let Op::Let { local, expr } = &step.op else {
                    continue;
                };
                if p.bound_local.as_ref() == Some(local) {
                    return Err(format!(
                        "the bound of `{}` is assigned inside the loop (at {})",
                        p.counter,
                        step.src.label()
                    ));
                }
                if !counter_locals.contains(local) || copies.contains(&(ci, pc)) {
                    continue;
                }
                let toward = match expr {
                    Expr::Bin(BinOp::Add, a, b) => match (a.as_ref(), b.as_ref()) {
                        (Expr::Local(l), Expr::Lit(Value::Int(k))) if l == local => {
                            k.signum() == p.dir
                        }
                        _ => false,
                    },
                    Expr::Bin(BinOp::Sub, a, b) => match (a.as_ref(), b.as_ref()) {
                        (Expr::Local(l), Expr::Lit(Value::Int(k))) if l == local => {
                            -k.signum() == p.dir
                        }
                        _ => false,
                    },
                    _ => false,
                };
                if !toward {
                    return Err(format!(
                        "`{}` is assigned other than by an increment toward the bound (at {})",
                        p.counter,
                        step.src.label()
                    ));
                }
            }
        }
        Ok(())
    }

    /// An integer the source fixes: a literal, a module constant, or a local
    /// every assignment so far set to the same integer.
    fn static_int(&self, m: &Module<'_>, e: &Expression<'_>) -> Option<i64> {
        match e.get_inner_expression() {
            Expression::NumericLiteral(n) => integer(n.value),
            Expression::UnaryExpression(u) if u.operator == UnaryOperator::UnaryNegation => {
                self.static_int(m, &u.argument).and_then(i64::checked_neg)
            }
            Expression::Identifier(id) => match self.lookup(id.name.as_str()) {
                Some(Binding::Local(local)) => self.p16.const_ints.get(&local).copied().flatten(),
                Some(_) => None,
                None => match module_constant(m, id.name.as_str()) {
                    Some(Expr::Lit(Value::Int(n))) => Some(n),
                    _ => None,
                },
            },
            _ => None,
        }
    }

    // -- updates ------------------------------------------------------------

    /// `x++`, `++x`, `x--`, `--x`, `x += e`, `x -= e` as a statement, on a
    /// local variable. `false` when `e` is not one of these.
    pub(super) fn update_statement(
        &mut self,
        m: &Module<'_>,
        e: &Expression<'_>,
        span: Span,
    ) -> bool {
        if !self.js() {
            return false;
        }
        match e.get_inner_expression() {
            Expression::UpdateExpression(u) => {
                self.update_local(m, u, span, true);
                true
            }
            Expression::AssignmentExpression(assign)
                if matches!(
                    assign.operator,
                    AssignmentOperator::Addition | AssignmentOperator::Subtraction
                ) =>
            {
                use oxc_ast::ast::AssignmentTarget;
                let local = match &assign.left {
                    AssignmentTarget::AssignmentTargetIdentifier(id) => {
                        match self.lookup(id.name.as_str()) {
                            Some(Binding::Local(local)) => Some(local),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                let Some(local) = local else {
                    self.unknown_op(
                        m,
                        span,
                        "a compound assignment to something other than a local variable",
                    );
                    return true;
                };
                let right = self.expr(m, &assign.right, TOP);
                let current = Expr::Local(local.clone());
                let value = if assign.operator == AssignmentOperator::Subtraction {
                    Expr::bin(BinOp::Sub, current, right)
                } else if self.static_string(&current) || self.static_string(&right) {
                    Expr::call(Builtin::Concat, vec![current, right])
                } else {
                    Expr::bin(BinOp::Add, current, right)
                };
                self.emit(m, span, Op::Let { local, expr: value });
                true
            }
            _ => false,
        }
    }

    /// `x++` / `++x` / `x--` / `--x`: the new value is assigned; the
    /// expression's value (the old value for the postfix forms) returned,
    /// unless `discard`ed (a statement).
    fn update_local(
        &mut self,
        m: &Module<'_>,
        u: &UpdateExpression<'_>,
        span: Span,
        discard: bool,
    ) -> Expr {
        let local = match &u.argument {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                match self.lookup(id.name.as_str()) {
                    Some(Binding::Local(local)) => Some(local),
                    _ => None,
                }
            }
            _ => None,
        };
        let Some(local) = local else {
            self.unknown_op(
                m,
                span,
                "`++`/`--` of something other than a local variable",
            );
            return Expr::Unknown("`++`/`--` of something other than a local variable".to_owned());
        };
        let old = if u.prefix || discard {
            None
        } else {
            let tmp = self.tmp("old");
            self.emit(
                m,
                span,
                Op::Let {
                    local: tmp.clone(),
                    expr: Expr::Local(local.clone()),
                },
            );
            Some(tmp)
        };
        let op = match u.operator {
            UpdateOperator::Increment => BinOp::Add,
            UpdateOperator::Decrement => BinOp::Sub,
        };
        self.emit(
            m,
            span,
            Op::Let {
                local: local.clone(),
                expr: Expr::bin(op, Expr::Local(local.clone()), Expr::lit(1)),
            },
        );
        Expr::Local(old.unwrap_or(local))
    }

    /// `++x` / `x++` in expression position (hoisted; not inside a
    /// conditionally evaluated operand).
    pub(super) fn update_value(
        &mut self,
        m: &Module<'_>,
        u: &UpdateExpression<'_>,
        cx: Cx,
    ) -> Expr {
        if cx.cond {
            return Expr::Unknown(
                "`++`/`--` inside a conditionally evaluated operand cannot be hoisted exactly"
                    .to_owned(),
            );
        }
        self.update_local(m, u, u.span, false)
    }

    // -- caught errors ---------------------------------------------------------

    /// How database errors reach a `catch` in this project: from the
    /// `package.json` nearest to the module that declares `drizzle-orm`.
    /// Verified (source read, `docs/phase16/LOOP-SEMANTICS.md`): drizzle-orm
    /// 0.45.4 wraps every query error of its pg-core sessions in
    /// `DrizzleQueryError` (`cause` = the driver's error, whose `code` is the
    /// SQLSTATE for node-postgres 8 and postgres-js 3.4.5); its
    /// node-postgres transaction sends COMMIT through the same path, its
    /// postgres-js transaction through postgres-js's `begin` (the driver's
    /// `PostgresError` itself). Other versions or drivers: not verified.
    pub(super) fn caught_shape(&mut self, m: &Module<'_>) -> CaughtErrors {
        if let Some(shape) = self.p16.caught_shape {
            return shape;
        }
        let deps = nearest_dependencies(&m.abs, &self.root);
        let drizzle = deps.get("drizzle-orm").cloned();
        let others = [
            "@neondatabase/serverless",
            "@vercel/postgres",
            "@electric-sql/pglite",
            "@xata.io/client",
            "@aws-sdk/client-rds-data",
            "@planetscale/database",
            "@libsql/client",
            "mysql2",
            "better-sqlite3",
        ];
        let pg = deps.contains_key("pg");
        let postgres = deps.contains_key("postgres");
        let other = others.iter().find(|o| deps.contains_key(**o));
        let version = drizzle.as_deref().and_then(minor_version);
        let (shape, text) = match (version, pg, postgres, other) {
            (Some((0, 45)), true, false, None) => (
                CaughtErrors {
                    statement: Some(ErrorShape::DrizzleQueryError),
                    commit: Some(ErrorShape::DrizzleQueryError),
                },
                "a database error (of a statement or of a transaction's COMMIT) reaches a `catch` as drizzle-orm 0.45's `DrizzleQueryError`: its `code` is undefined and `cause.code` is the SQLSTATE (drizzle-orm 0.45.4 and pg 8.23.1 sources)".to_owned(),
            ),
            (Some((0, 45)), false, true, None) => (
                CaughtErrors {
                    statement: Some(ErrorShape::DrizzleQueryError),
                    commit: Some(ErrorShape::Driver),
                },
                "a statement's database error reaches a `catch` as drizzle-orm 0.45's `DrizzleQueryError` (`code` undefined, `cause.code` the SQLSTATE); a COMMIT's error is postgres-js's `PostgresError` (`code` the SQLSTATE) (drizzle-orm 0.45.4 and postgres 3.4.5 sources)".to_owned(),
            ),
            _ => (
                CaughtErrors::default(),
                format!(
                    "how a database error reaches a `catch` is not verified for this project (drizzle-orm {}, driver {}): reading its properties is unsupported",
                    drizzle.as_deref().unwrap_or("not declared"),
                    match (pg, postgres, other) {
                        (_, _, Some(o)) => (*o).to_owned(),
                        (true, false, None) => "pg".to_owned(),
                        (false, true, None) => "postgres".to_owned(),
                        (true, true, None) => "pg and postgres".to_owned(),
                        (false, false, None) => "not declared".to_owned(),
                    }
                ),
            ),
        };
        self.assume(format!(
            "{CAUGHT_PREFIX}{text}; only the SQLSTATE `code` of a database error and the `name`/`message` of a `throw new Error(\"...\")` are modeled, the error of any other failure (a lost connection, another `throw`) has no known property"
        ));
        self.p16.caught_shape = Some(shape);
        shape
    }

    /// `throw new Error("text")` (or a built-in error subclass) inside a
    /// `try` block whose handler inspects the error: the error object, then
    /// a re-throw of it. `false` otherwise.
    pub(super) fn structured_throw(&mut self, m: &Module<'_>, s: &ThrowStatement<'_>) -> bool {
        if !self.js() {
            return false;
        }
        // `throw e` of a caught error (or, once a handler inspects errors, of
        // a local holding one): the value itself is re-thrown.
        if let Expression::Identifier(id) = s.argument.get_inner_expression() {
            match self.lookup(id.name.as_str()) {
                Some(Binding::Caught(local)) => {
                    self.emit(m, s.span, Op::Rethrow { local });
                    return true;
                }
                Some(Binding::Local(local)) if self.p16.caught_seen => {
                    self.emit(m, s.span, Op::Rethrow { local });
                    return true;
                }
                _ => {}
            }
        }
        if self.p16.inspecting == 0 {
            return false;
        }
        let Some((ctor, message)) = self.builtin_error(m, &s.argument) else {
            return false;
        };
        let tmp = self.tmp("thrown");
        let text = Expr::Lit(Value::str(message.clone()));
        self.emit(
            m,
            s.span,
            Op::Let {
                local: tmp.clone(),
                expr: Expr::Object(vec![
                    (midwrite_model::jsfn::ERROR_REASON.to_owned(), text.clone()),
                    (
                        midwrite_model::jsfn::ERROR_INSTANCE.to_owned(),
                        Expr::lit(true),
                    ),
                    (
                        midwrite_model::engine::THROWN_ERROR.to_owned(),
                        Expr::lit(true),
                    ),
                    ("name".to_owned(), Expr::Lit(Value::str(ctor))),
                    ("message".to_owned(), text),
                    ("code".to_owned(), Expr::undefined()),
                    ("cause".to_owned(), Expr::undefined()),
                ]),
            },
        );
        self.emit(m, s.span, Op::Rethrow { local: tmp });
        true
    }

    /// `new Error("text")` of a global built-in error constructor with at
    /// most one string-literal argument.
    fn builtin_error(&self, m: &Module<'_>, e: &Expression<'_>) -> Option<(String, String)> {
        let Expression::NewExpression(new) = e.get_inner_expression() else {
            return None;
        };
        let Expression::Identifier(id) = new.callee.get_inner_expression() else {
            return None;
        };
        let ctor = id.name.as_str();
        if !matches!(
            ctor,
            "Error"
                | "TypeError"
                | "RangeError"
                | "SyntaxError"
                | "ReferenceError"
                | "EvalError"
                | "URIError"
        ) || !self.is_global(m, ctor)
        {
            return None;
        }
        let message = match new.arguments.as_slice() {
            [] => String::new(),
            [only] => match only.as_expression().and_then(literal) {
                Some(Expr::Lit(Value::Str(s))) => s,
                _ => return None,
            },
            _ => return None,
        };
        Some((ctor.to_owned(), message))
    }

    /// Whether `e` is a property chain (`e.cause`, `e.cause.x`) without
    /// optional links whose root is a caught error.
    pub(super) fn caught_root(&self, e: &Expression<'_>) -> bool {
        match e.get_inner_expression() {
            Expression::Identifier(id) => {
                matches!(self.lookup(id.name.as_str()), Some(Binding::Caught(_)))
            }
            Expression::StaticMemberExpression(member) => {
                !member.optional && self.caught_root(&member.object)
            }
            _ => false,
        }
    }

    /// `e instanceof Error` of a caught error.
    pub(super) fn caught_instanceof(
        &mut self,
        m: &Module<'_>,
        b: &oxc_ast::ast::BinaryExpression<'_>,
    ) -> Option<Expr> {
        if !self.js() || b.operator != BinaryOperator::Instanceof {
            return None;
        }
        let Expression::Identifier(left) = b.left.get_inner_expression() else {
            return None;
        };
        let Some(Binding::Caught(local)) = self.lookup(left.name.as_str()) else {
            return None;
        };
        match b.right.get_inner_expression() {
            Expression::Identifier(right)
                if right.name.as_str() == "Error" && self.is_global(m, "Error") =>
            {
                Some(Expr::call(
                    Builtin::At,
                    vec![
                        Expr::Local(local),
                        Expr::Lit(Value::str(midwrite_model::jsfn::ERROR_INSTANCE)),
                    ],
                ))
            }
            _ => Some(Expr::Unknown(
                "`instanceof` a class other than `Error` of a caught error is not modeled"
                    .to_owned(),
            )),
        }
    }

    /// Whether `name` denotes a global (not a local, an import or a
    /// declaration of the module).
    pub(super) fn is_global(&self, m: &Module<'_>, name: &str) -> bool {
        self.lookup(name).is_none()
            && find_import(m, name).is_none()
            && find_function(m, name).is_none()
            && find_class(m.program, name).is_none()
            && module_constant(m, name).is_none()
            && !declares(m, name)
    }

    // -- requests and responses -------------------------------------------------

    /// A parameter of the entry declared `Request` (global) or `NextRequest`
    /// (from `next/server`).
    pub(super) fn request_param(&self, m: &Module<'_>, type_name: &str) -> bool {
        if !self.js() {
            return false;
        }
        match type_name {
            "Request" => {
                find_import(m, "Request").is_none()
                    && find_class(m.program, "Request").is_none()
                    && !declares(m, "Request")
            }
            "NextRequest" => matches!(
                find_import(m, "NextRequest"),
                Some(ImportRef::Named { source, name }) if source == "next/server" && name == "NextRequest"
            ),
            _ => false,
        }
    }

    /// `await request.json()`, `Response.json(...)`, `NextResponse.json(...)`.
    pub(super) fn http_call(
        &mut self,
        m: &Module<'_>,
        call: &CallExpression<'_>,
        awaited: bool,
    ) -> Option<Res> {
        if !self.js() {
            return None;
        }
        let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
            return None;
        };
        let Expression::Identifier(object) = member.object.get_inner_expression() else {
            return None;
        };
        let name = object.name.as_str();
        let method = member.property.name.as_str();
        if let Some(Binding::Request(local)) = self.lookup(name) {
            if method != "json" || !call.arguments.is_empty() {
                return Some(self.unknown_op(
                    m,
                    call.span,
                    format!(
                        "`{name}.{method}(...)`: only `await {name}.json()` of a request is modeled"
                    ),
                ));
            }
            if !awaited {
                return Some(self.unknown_op(
                    m,
                    call.span,
                    format!("`{name}.json()` is used without `await`"),
                ));
            }
            if !self.p16.bodies_read.insert(local.clone()) {
                return Some(self.unknown_op(
                    m,
                    call.span,
                    format!("the body of `{name}` is read twice (the second read rejects: the body was used)"),
                ));
            }
            self.assume(format!(
                "{REQUEST_PREFIX}`await {name}.json()` is the actor's argument `{local}`: the request body is valid JSON (a body that `json()` rejects is not explored) and holds JSON values only"
            ));
            return Some(Res::Value(Expr::Local(local)));
        }
        let response = (name == "Response" && self.is_global(m, "Response"))
            || (name == "NextResponse"
                && matches!(
                    find_import(m, "NextResponse"),
                    Some(ImportRef::Named { source, name }) if source == "next/server" && name == "NextResponse"
                ));
        if !response || method != "json" {
            return None;
        }
        let what = format!("{name}.json");
        Some(Res::Value(self.response_value(m, &call.arguments, &what)))
    }

    /// `new Response(body, { status })`.
    pub(super) fn new_response(&mut self, m: &Module<'_>, new: &NewExpression<'_>) -> Option<Expr> {
        if !self.js() {
            return None;
        }
        let Expression::Identifier(id) = new.callee.get_inner_expression() else {
            return None;
        };
        if id.name.as_str() != "Response" || !self.is_global(m, "Response") {
            return None;
        }
        Some(self.response_value(m, &new.arguments, "new Response"))
    }

    /// The response `{ status, body }`: the body as given (before its
    /// serialization), the status of the init object (200 without one).
    fn response_value(&mut self, m: &Module<'_>, arguments: &[Argument<'_>], what: &str) -> Expr {
        let mut args = Vec::new();
        for a in arguments {
            match a.as_expression() {
                Some(e) => args.push(e),
                None => return Expr::Unknown(format!("a spread argument of `{what}`")),
            }
        }
        if args.len() > 2 {
            return Expr::Unknown(format!("`{what}` with more than two arguments"));
        }
        let body = match args.first() {
            Some(e) => self.expr(m, e, TOP),
            None => Expr::Lit(Value::Null),
        };
        let status = match args.get(1).map(|e| e.get_inner_expression()) {
            None => Expr::lit(200),
            Some(Expression::ObjectExpression(init)) => {
                let mut status = Expr::lit(200);
                for p in &init.properties {
                    match p {
                        ObjectPropertyKind::ObjectProperty(p) if !p.computed => {
                            match static_key(&p.key).as_deref() {
                                Some("status") => status = self.expr(m, &p.value, TOP),
                                // Headers and the status text are not part of
                                // the modeled response.
                                Some("headers" | "statusText") => {}
                                _ => {
                                    return Expr::Unknown(format!(
                                        "a `{what}` option other than `status`, `headers`, `statusText`"
                                    ));
                                }
                            }
                        }
                        _ => {
                            return Expr::Unknown(format!("a spread or computed `{what}` option"));
                        }
                    }
                }
                status
            }
            Some(_) => {
                return Expr::Unknown(format!("`{what}` options that are not an object literal"));
            }
        };
        self.assume(format!(
            "{REQUEST_PREFIX}a response is the value `{{ status, body }}`: `body` is the value given to `{what}` before its serialization (headers and the status text are not modeled)"
        ));
        Expr::Object(vec![
            ("status".to_owned(), status),
            ("body".to_owned(), body),
        ])
    }
}

/// The signed step of `e` when it is an increment or decrement of `name` by
/// an integer literal (`x++`, `++x`, `x--`, `x += k`, `x -= k`, `x = x + k`,
/// `x = x - k`, `x = k + x`).
fn increment_of(e: &Expression<'_>, name: &str) -> Option<i64> {
    use oxc_ast::ast::AssignmentTarget;
    let is_name = |e: &Expression<'_>| matches!(e.get_inner_expression(), Expression::Identifier(id) if id.name.as_str() == name);
    let int = |e: &Expression<'_>| match e.get_inner_expression() {
        Expression::NumericLiteral(n) => integer(n.value),
        _ => None,
    };
    match e.get_inner_expression() {
        Expression::UpdateExpression(u) => match &u.argument {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(id) if id.name.as_str() == name => {
                Some(match u.operator {
                    UpdateOperator::Increment => 1,
                    UpdateOperator::Decrement => -1,
                })
            }
            _ => None,
        },
        Expression::AssignmentExpression(a) => {
            let AssignmentTarget::AssignmentTargetIdentifier(id) = &a.left else {
                return None;
            };
            if id.name.as_str() != name {
                return None;
            }
            match a.operator {
                AssignmentOperator::Addition => int(&a.right),
                AssignmentOperator::Subtraction => int(&a.right).map(|k| -k),
                AssignmentOperator::Assign => match a.right.get_inner_expression() {
                    Expression::BinaryExpression(b) => match b.operator {
                        BinaryOperator::Addition if is_name(&b.left) => int(&b.right),
                        BinaryOperator::Addition if is_name(&b.right) => int(&b.left),
                        BinaryOperator::Subtraction if is_name(&b.left) => {
                            int(&b.right).map(|k| -k)
                        }
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            }
        }
        _ => None,
    }
    .filter(|k| *k != 0)
}

/// Whether a loop body has a `continue` that targets the loop itself (not
/// one of a nested loop or function).
fn has_continue(body: &Statement<'_>) -> bool {
    struct Find {
        depth: u32,
        found: bool,
    }
    impl<'a> Visit<'a> for Find {
        fn visit_continue_statement(&mut self, it: &oxc_ast::ast::ContinueStatement<'a>) {
            // A labeled `continue` may target this loop too.
            if self.depth == 0 || it.label.is_some() {
                self.found = true;
            }
        }
        fn visit_statement(&mut self, it: &Statement<'a>) {
            let nested = matches!(
                it,
                Statement::ForStatement(_)
                    | Statement::ForInStatement(_)
                    | Statement::ForOfStatement(_)
                    | Statement::WhileStatement(_)
                    | Statement::DoWhileStatement(_)
            );
            if nested {
                self.depth += 1;
            }
            walk::walk_statement(self, it);
            if nested {
                self.depth -= 1;
            }
        }
        fn visit_function(&mut self, _: &oxc_ast::ast::Function<'a>, _: ScopeFlags) {}
        fn visit_arrow_function_expression(
            &mut self,
            _: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
        }
    }
    let mut f = Find {
        depth: 0,
        found: false,
    };
    f.visit_statement(body);
    f.found
}

/// Phase 16: whether a `catch` body uses its parameter other than as the
/// argument of `throw` (then the handler inspects the error).
pub(super) fn catch_inspects(body: &[Statement<'_>], name: &str) -> bool {
    struct Uses<'n> {
        name: &'n str,
        all: u32,
        thrown: u32,
    }
    impl<'a> Visit<'a> for Uses<'_> {
        fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
            if it.name.as_str() == self.name {
                self.all += 1;
            }
        }
        fn visit_throw_statement(&mut self, it: &ThrowStatement<'a>) {
            if matches!(it.argument.get_inner_expression(), Expression::Identifier(id) if id.name.as_str() == self.name)
            {
                self.thrown += 1;
            }
            walk::walk_throw_statement(self, it);
        }
    }
    let mut uses = Uses {
        name,
        all: 0,
        thrown: 0,
    };
    for s in body {
        uses.visit_statement(s);
    }
    uses.all > uses.thrown
}

/// Whether the module declares `name` at its top level (a variable, a
/// function, a class, an enum, an import).
fn declares(m: &Module<'_>, name: &str) -> bool {
    use oxc_ast::ast::Declaration;
    let in_vars = |d: &oxc_ast::ast::VariableDeclaration<'_>| {
        d.declarations.iter().any(
            |x| matches!(&x.id, BindingPattern::BindingIdentifier(id) if id.name.as_str() == name),
        )
    };
    let in_decl = |d: &Declaration<'_>| match d {
        Declaration::VariableDeclaration(v) => in_vars(v),
        Declaration::FunctionDeclaration(f) => f.id.as_ref().is_some_and(|i| i.name == name),
        Declaration::ClassDeclaration(c) => c.id.as_ref().is_some_and(|i| i.name == name),
        _ => false,
    };
    m.program.body.iter().any(|s| match s {
        Statement::VariableDeclaration(v) => in_vars(v),
        Statement::FunctionDeclaration(f) => f.id.as_ref().is_some_and(|i| i.name == name),
        Statement::ClassDeclaration(c) => c.id.as_ref().is_some_and(|i| i.name == name),
        Statement::ExportDeclaration(e) => in_decl(&e.declaration),
        _ => false,
    })
}

/// The dependencies (`dependencies`, `devDependencies`,
/// `peerDependencies`) of the nearest `package.json` above `file` (up to
/// `root`) that declares `drizzle-orm`, else of the nearest one.
fn nearest_dependencies(file: &Path, root: &Path) -> HashMap<String, String> {
    let mut nearest: Option<HashMap<String, String>> = None;
    let mut dir = file.parent();
    while let Some(d) = dir {
        if let Ok(text) = std::fs::read_to_string(d.join("package.json"))
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
        {
            let mut deps = HashMap::new();
            for key in ["peerDependencies", "devDependencies", "dependencies"] {
                if let Some(map) = json.get(key).and_then(|v| v.as_object()) {
                    for (k, v) in map {
                        deps.insert(k.clone(), v.as_str().unwrap_or_default().to_owned());
                    }
                }
            }
            if deps.contains_key("drizzle-orm") {
                return deps;
            }
            if nearest.is_none() {
                nearest = Some(deps);
            }
        }
        if d == root || !d.starts_with(root) {
            break;
        }
        dir = d.parent();
    }
    nearest.unwrap_or_default()
}

/// `(major, minor)` of a version requirement whose lower bound is
/// `major.minor.x` (`0.45.4`, `^0.45.1`, `~0.45.0`); `None` for ranges,
/// tags and other specifiers.
fn minor_version(spec: &str) -> Option<(u32, u32)> {
    let s = spec.trim();
    let s = s
        .strip_prefix('^')
        .or_else(|| s.strip_prefix('~'))
        .or_else(|| s.strip_prefix('='))
        .unwrap_or(s);
    let s = s.strip_prefix('v').unwrap_or(s);
    if s.contains(char::is_whitespace) || s.contains("||") {
        return None;
    }
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?;
    patch.split(['-', '+']).next()?.parse::<u32>().ok()?;
    Some((major, minor))
}

#[cfg(test)]
mod tests {
    use super::minor_version;

    #[test]
    fn versions_with_a_fixed_minor_are_read() {
        assert_eq!(minor_version("0.45.4"), Some((0, 45)));
        assert_eq!(minor_version("^0.45.1"), Some((0, 45)));
        assert_eq!(minor_version("~0.43.1"), Some((0, 43)));
        assert_eq!(minor_version(">=0.45.0"), None);
        assert_eq!(minor_version("latest"), None);
        assert_eq!(minor_version("0.45"), None);
        assert_eq!(minor_version("0.45.0 || 0.46.0"), None);
    }
}

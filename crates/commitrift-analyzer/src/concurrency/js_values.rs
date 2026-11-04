//! Phase 14: lowering of the JavaScript value domain (strings, arrays,
//! `null` vs `undefined`, `Map`/`Set`), enabled by
//! [`ExtractRequest::js_values`](commitrift_model::provider::ExtractRequest)
//! (scenario feature `js-values`). Without it every function here is
//! inactive and the extracted code is the Phase 13 code.
//!
//! What is lowered, and the model built-in it becomes
//! (`commitrift_model::jsfn` implements their ECMAScript behavior):
//!
//! * `undefined` (distinct from `null`): the identifier, `void 0`, `let x;`,
//!   a missing property (`At(obj, "p")`), an out-of-range index, a missing
//!   argument, a helper that returns nothing, `?.` on a nullish base;
//!   `x == null` is a nullish test, `??` falls back on `null`/`undefined`
//!   only;
//! * strings: template literals and `+` with an operand that is statically a
//!   string (`Concat` of `ToString` parts), `String(x)`, `typeof x`,
//!   `.length`, `.includes/.startsWith/.endsWith/.indexOf`,
//!   `.split(literal)`, `.toLowerCase/.toUpperCase/.trim`;
//! * arrays as values: literals, `a[i]`, `.at(literal)`, `.length`,
//!   `.includes/.indexOf/.join`, `.map/.filter/.find/.findIndex/.some/.every`
//!   with an arrow callback whose body is one pure expression of its
//!   parameter, `const [a, b] = list`, `for (const x of list)` (a bounded
//!   loop over a snapshot);
//! * `Map`/`Set`: `new Map(...)`/`new Set(...)`, `.get/.has/.size`, and
//!   `.set/.add/.delete/.clear` only on a `const` local initialized with
//!   `new Map`/`new Set` in the same function that never escapes it (the
//!   mutation is a rebinding of that local, which is sound only without
//!   aliases).
//!
//! Refused (an unknown expression or operation, never a guess): mutating an
//! array (`push`, `sort`, index assignment, ...), comparing two objects by
//! identity, callbacks that call anything or have more than one statement,
//! optional chains with more than one link, `Map`/`Set` mutation that could
//! be observed through an alias.

use std::collections::{HashMap, HashSet};

use oxc_ast::ast::{
    Argument, ArrowFunctionBody, ArrowFunctionExpression, BindingPattern, CallExpression,
    Expression, Function, IdentifierReference, Program, Statement, StaticMemberExpression,
    VariableDeclaration,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::{GetSpan, Span};
use oxc_syntax::operator::{BinaryOperator, UnaryOperator};
use oxc_syntax::scope::ScopeFlags;

use commitrift_model::ir::Op;
use commitrift_model::value::{BinOp, Builtin, EachOp, Expr, Value};

use super::{
    Binding, Cx, Lowerer, Module, Res, TOP, Want, find_class, find_function, find_import, integer,
    module_constant,
};

/// Per-extraction facts of the JavaScript value lowering.
#[derive(Default)]
pub(super) struct JsFacts {
    /// `const` locals bound to a string (template literal, concatenation,
    /// string method): `+` with one of them concatenates.
    strings: HashSet<String>,
    /// `const` `Map` (`true`) or `Set` (`false`) locals that never escape the
    /// function declaring them: their mutating methods rebind the local.
    collections: HashMap<String, bool>,
    /// Every local initialized with `new Map`/`new Set` (whose `get`/`has`
    /// are the collection's: other receivers of `get`/`has`, such as a
    /// `FormData`, are not values of the model).
    kinds: HashMap<String, bool>,
}

/// Methods that mutate an array in place: arrays are values in the model,
/// so these are refused.
const ARRAY_MUTATORS: &[&str] = &[
    "push",
    "pop",
    "shift",
    "unshift",
    "splice",
    "sort",
    "reverse",
    "fill",
    "copyWithin",
];

impl Lowerer<'_> {
    /// Whether the extraction uses JavaScript value semantics.
    pub(super) fn js(&self) -> bool {
        self.request.js_values
    }

    /// `undefined`: a distinct value with JavaScript semantics, `null`
    /// otherwise (Phase 6–13: one value).
    pub(super) fn undef(&self) -> Expr {
        if self.js() {
            Expr::undefined()
        } else {
            Expr::Lit(Value::Null)
        }
    }

    /// `e == null` (`null` or `undefined`).
    pub(super) fn nullish(&self, e: Expr) -> Expr {
        let is_null = Expr::IsNull(Box::new(e.clone()));
        if self.js() {
            Expr::bin(BinOp::Or, is_null, Expr::eq(e, Expr::undefined()))
        } else {
            is_null
        }
    }

    /// `base.name`. With JavaScript values a missing property is
    /// `undefined` (`At(base, "name")`), `length` and `size` are the
    /// built-ins of strings, arrays, `Map` and `Set` (a property of other
    /// objects); without, it is the Phase 6–13 `Field` (missing: `null`).
    pub(super) fn read_prop(&self, base: Expr, name: &str) -> Expr {
        if !self.js() {
            return Expr::Field(Box::new(base), name.to_owned());
        }
        if base.mentions_column() {
            return Expr::Unknown(format!(
                "property `{name}` of a column reference is not a value"
            ));
        }
        match name {
            "length" => Expr::call(Builtin::Length, vec![base]),
            "size" => Expr::call(Builtin::Size, vec![base]),
            _ => Expr::call(Builtin::At, vec![base, Expr::Lit(Value::str(name))]),
        }
    }

    /// Whether `name` denotes the global built-in (not a local, an import or
    /// a declaration of the module).
    fn global(&self, m: &Module<'_>, name: &str) -> bool {
        self.lookup(name).is_none()
            && find_import(m, name).is_none()
            && find_function(m, name).is_none()
            && find_class(m.program, name).is_none()
            && !declares_top_level(m.program, name)
    }

    /// Whether the value of `e` is statically a string.
    pub(super) fn static_string(&self, e: &Expr) -> bool {
        match e {
            Expr::Lit(Value::Str(_)) => true,
            Expr::Call { func, .. } => matches!(
                func,
                Builtin::Concat
                    | Builtin::ToString
                    | Builtin::TypeOf
                    | Builtin::ToLowerCase
                    | Builtin::ToUpperCase
                    | Builtin::Trim
                    | Builtin::Join
            ),
            Expr::Local(l) => {
                self.js_facts.strings.contains(l)
                    || (self.frames.len() == 1
                        && self
                            .param_types
                            .get(l)
                            .is_some_and(|(t, _)| t.trim() == "string"))
            }
            _ => false,
        }
    }

    // -- expressions ------------------------------------------------------

    /// Expression forms that exist only with JavaScript values; `None` lets
    /// the Phase 13 lowering handle `e`.
    pub(super) fn js_expr(&mut self, m: &Module<'_>, e: &Expression<'_>, cx: Cx) -> Option<Expr> {
        if !self.js() {
            return None;
        }
        let value = Cx {
            truthy: false,
            ..cx
        };
        match e {
            Expression::CallExpression(call) => self.value_call(m, call, cx),
            Expression::TemplateLiteral(t) if !t.expressions.is_empty() => {
                let mut parts = Vec::new();
                for (i, quasi) in t.quasis.iter().enumerate() {
                    let Some(text) = quasi.value.cooked.as_ref().and_then(|c| c.as_str()) else {
                        return Some(Expr::Unknown(
                            "a template literal with an invalid escape".to_owned(),
                        ));
                    };
                    if !text.is_empty() {
                        parts.push(Expr::Lit(Value::str(text)));
                    }
                    if let Some(x) = t.expressions.get(i) {
                        parts.push(self.expr(m, x, value));
                    }
                }
                Some(Expr::call(Builtin::Concat, parts))
            }
            Expression::ArrayExpression(array) => {
                let mut items = Vec::new();
                for element in &array.elements {
                    match element.as_expression() {
                        Some(x) => items.push(self.expr(m, x, value)),
                        None => {
                            return Some(Expr::Unknown(
                                "a spread or a hole in an array literal".to_owned(),
                            ));
                        }
                    }
                }
                Some(Expr::Array(items))
            }
            Expression::NewExpression(new) => {
                let func = self.collection_ctor(m, new)?;
                Some(self.collection_args(m, func, &new.arguments, value))
            }
            Expression::UnaryExpression(u) => match u.operator {
                UnaryOperator::Void if pure_literal(&u.argument) => Some(Expr::undefined()),
                UnaryOperator::Typeof => {
                    let x = self.expr(m, &u.argument, value);
                    Some(Expr::call(Builtin::TypeOf, vec![x]))
                }
                _ => None,
            },
            Expression::ComputedMemberExpression(cm) if !cm.optional => {
                let object = cm.object.get_inner_expression();
                let index = match cm.expression.get_inner_expression() {
                    Expression::NumericLiteral(n) => integer(n.value),
                    _ => None,
                };
                if let Expression::Identifier(id) = object
                    && let Some(Binding::Rows { row, .. }) = self.lookup(id.name.as_str())
                {
                    return Some(match index {
                        Some(0) => Expr::Local(row),
                        // An array of at most one row has nothing at 1, 2, ...
                        Some(i) if i > 0 => Expr::undefined(),
                        _ => Expr::Unknown(format!(
                            "`{}[...]`: an array of rows is only modeled through `length` and `[0]`",
                            id.name
                        )),
                    });
                }
                if !self.value_root(m, object) {
                    return None;
                }
                let base = self.expr(m, object, value);
                let key = self.expr(m, &cm.expression, value);
                Some(Expr::call(Builtin::At, vec![base, key]))
            }
            _ => None,
        }
    }

    /// `a == b` / `a != b` (loose), and `a + b`, with JavaScript values;
    /// `None` for every other operator (or inside a query).
    pub(super) fn js_binary(
        &mut self,
        m: &Module<'_>,
        b: &oxc_ast::ast::BinaryExpression<'_>,
        cx: Cx,
    ) -> Option<Expr> {
        if !self.js() || cx.col_ok {
            return None;
        }
        let value = Cx {
            truthy: false,
            ..cx
        };
        match b.operator {
            BinaryOperator::Equality | BinaryOperator::Inequality => {
                let left_nullish = super::is_nullish(&b.left);
                let right_nullish = super::is_nullish(&b.right);
                let left = self.expr(m, &b.left, value);
                let right = self.expr(m, &b.right, value);
                // `null == undefined`; otherwise loose equality agrees with
                // strict equality on the model's primitives of one type (the
                // other cases are what `===` does with them).
                let equal = if right_nullish {
                    self.nullish(left)
                } else if left_nullish {
                    self.nullish(right)
                } else {
                    Expr::bin(
                        BinOp::Or,
                        Expr::bin(
                            BinOp::And,
                            self.nullish(left.clone()),
                            self.nullish(right.clone()),
                        ),
                        Expr::eq(left, right),
                    )
                };
                Some(if b.operator == BinaryOperator::Equality {
                    equal
                } else {
                    Expr::not(equal)
                })
            }
            BinaryOperator::Addition => {
                let left = self.expr(m, &b.left, value);
                let right = self.expr(m, &b.right, value);
                if !self.static_string(&left) && !self.static_string(&right) {
                    return Some(Expr::bin(BinOp::Add, left, right));
                }
                // Left-associative: `(a + b) + c` flattens only when the left
                // side is already a string concatenation.
                let mut parts = match left {
                    Expr::Call {
                        func: Builtin::Concat,
                        args,
                    } => args,
                    other => vec![other],
                };
                parts.push(right);
                Some(Expr::call(Builtin::Concat, parts))
            }
            _ => None,
        }
    }

    /// Whether `e` is a JavaScript value the built-ins apply to (its root is
    /// a local, a literal or a module constant), as opposed to a database
    /// handle, a table, an instance or a module object whose methods are
    /// resolved elsewhere.
    fn value_root(&self, m: &Module<'_>, e: &Expression<'_>) -> bool {
        match e.get_inner_expression() {
            Expression::StringLiteral(_)
            | Expression::TemplateLiteral(_)
            | Expression::ArrayExpression(_) => true,
            Expression::Identifier(id) => match self.lookup(id.name.as_str()) {
                Some(Binding::Local(_) | Binding::Object(_)) => true,
                Some(_) => false,
                // A module constant (`const PREFIX = "x"`); Phase 16: or a
                // module-level array of literals (`RETRYABLE.includes(code)`).
                None => {
                    module_constant(m, id.name.as_str()).is_some()
                        || super::module_list_constant(m, id.name.as_str()).is_some()
                }
            },
            Expression::StaticMemberExpression(member) => {
                !member.optional && self.value_root(m, &member.object)
            }
            Expression::ComputedMemberExpression(member) => {
                !member.optional && self.value_root(m, &member.object)
            }
            Expression::CallExpression(call) => {
                !call.optional
                    && match call.callee.get_inner_expression() {
                        Expression::StaticMemberExpression(member) => {
                            pure_method(member.property.name.as_str()).is_some()
                                && self.value_root(m, &member.object)
                        }
                        Expression::Identifier(id) => id.name.as_str() == "String",
                        _ => false,
                    }
            }
            Expression::LogicalExpression(l) => {
                (self.value_root(m, &l.left) || literalish(&l.left))
                    && (self.value_root(m, &l.right) || literalish(&l.right))
            }
            Expression::ConditionalExpression(c) => {
                (self.value_root(m, &c.consequent) || literalish(&c.consequent))
                    && (self.value_root(m, &c.alternate) || literalish(&c.alternate))
            }
            _ => false,
        }
    }

    /// Whether `e` is statically a `Map` or a `Set`: a local initialized
    /// with `new Map`/`new Set`, or such an expression.
    fn known_collection(&self, m: &Module<'_>, e: &Expression<'_>) -> bool {
        match e.get_inner_expression() {
            Expression::Identifier(id) => match self.lookup(id.name.as_str()) {
                Some(Binding::Local(local)) => self.js_facts.kinds.contains_key(&local),
                _ => false,
            },
            Expression::NewExpression(new) => self.collection_ctor(m, new).is_some(),
            _ => false,
        }
    }

    /// `new Map(...)` / `new Set(...)` of the global constructors.
    fn collection_ctor(
        &self,
        m: &Module<'_>,
        new: &oxc_ast::ast::NewExpression<'_>,
    ) -> Option<Builtin> {
        let Expression::Identifier(id) = new.callee.get_inner_expression() else {
            return None;
        };
        let func = match id.name.as_str() {
            "Map" => Builtin::NewMap,
            "Set" => Builtin::NewSet,
            _ => return None,
        };
        self.global(m, id.name.as_str()).then_some(func)
    }

    fn collection_args(
        &mut self,
        m: &Module<'_>,
        func: Builtin,
        arguments: &[Argument<'_>],
        cx: Cx,
    ) -> Expr {
        if arguments.len() > 1 {
            return Expr::Unknown(format!("`{}` with more than one argument", func.name()));
        }
        let mut args = Vec::new();
        for a in arguments {
            match a.as_expression() {
                Some(x) => args.push(self.expr(m, x, cx)),
                None => return Expr::Unknown("a spread argument".to_owned()),
            }
        }
        Expr::call(func, args)
    }

    // -- calls ----------------------------------------------------------------

    /// A call of a pure built-in on a value (`String(x)`, `s.trim()`,
    /// `xs.includes(x)`, `xs.filter((x) => ...)`, `m.get(k)`): an expression,
    /// with no operation emitted (so it is also exact inside a conditionally
    /// evaluated operand or a callback). `None` when the call is not one.
    fn value_call(&mut self, m: &Module<'_>, call: &CallExpression<'_>, cx: Cx) -> Option<Expr> {
        if !self.js() || call.optional {
            return None;
        }
        let value = Cx {
            truthy: false,
            ..cx
        };
        let mut args: Vec<&Expression<'_>> = Vec::new();
        for a in &call.arguments {
            args.push(a.as_expression()?);
        }
        match call.callee.get_inner_expression() {
            Expression::Identifier(id)
                if id.name.as_str() == "String" && self.global(m, "String") =>
            {
                Some(match args.as_slice() {
                    [] => Expr::Lit(Value::str("")),
                    [x] => {
                        let x = self.expr(m, x, value);
                        Expr::call(Builtin::ToString, vec![x])
                    }
                    _ => Expr::Unknown("`String` with more than one argument".to_owned()),
                })
            }
            Expression::StaticMemberExpression(member) if !member.optional => {
                let method = member.property.name.as_str();
                let shape = pure_method(method)?;
                if !self.value_root(m, &member.object) {
                    return None;
                }
                if matches!(shape, Shape::Fixed(Builtin::MapGet | Builtin::Has, _))
                    && !self.known_collection(m, &member.object)
                {
                    return None;
                }
                let recv = self.expr(m, &member.object, value);
                Some(self.pure_method_call(m, method, shape, recv, &args, value))
            }
            _ => None,
        }
    }

    fn pure_method_call(
        &mut self,
        m: &Module<'_>,
        method: &str,
        shape: Shape,
        recv: Expr,
        args: &[&Expression<'_>],
        cx: Cx,
    ) -> Expr {
        match shape {
            Shape::Fixed(func, n) | Shape::Optional(func, n) => {
                let ok = match shape {
                    Shape::Fixed(..) => args.len() == n,
                    _ => args.len() <= n,
                };
                if !ok {
                    return Expr::Unknown(format!(
                        "`.{method}(...)` with {} argument(s) is not modeled",
                        args.len()
                    ));
                }
                if func == Builtin::Split
                    && !matches!(args[0].get_inner_expression(),
                        Expression::StringLiteral(s) if !s.value.is_empty())
                {
                    return Expr::Unknown(
                        "`.split(...)` is modeled only with a non-empty string literal separator"
                            .to_owned(),
                    );
                }
                let mut all = vec![recv];
                for a in args {
                    all.push(self.expr(m, a, cx));
                }
                Expr::call(func, all)
            }
            Shape::RelativeAt => {
                let index = match args {
                    [a] => match self.expr(m, a, cx) {
                        Expr::Lit(Value::Int(i)) => Some(i),
                        _ => None,
                    },
                    _ => None,
                };
                match index {
                    Some(i) if i >= 0 => Expr::call(Builtin::At, vec![recv, Expr::lit(i)]),
                    // `.at(-k)` is `[length - k]` (`undefined` below 0).
                    Some(i) => Expr::call(
                        Builtin::At,
                        vec![
                            recv.clone(),
                            Expr::bin(
                                BinOp::Add,
                                Expr::call(Builtin::Length, vec![recv]),
                                Expr::lit(i),
                            ),
                        ],
                    ),
                    None => Expr::Unknown(
                        "`.at(...)` is modeled only with an integer literal".to_owned(),
                    ),
                }
            }
            Shape::Callback(op) => {
                let [callback] = args else {
                    return Expr::Unknown(format!(
                        "`.{method}(...)` is modeled only with one arrow callback"
                    ));
                };
                // `filter`/`find`/... use only the truthiness of the result.
                let truthy = op != EachOp::Map;
                match self.callback(m, callback, truthy) {
                    Ok((var, body)) => Expr::Each {
                        op,
                        list: Box::new(recv),
                        var,
                        body: Box::new(body),
                    },
                    Err(why) => Expr::Unknown(format!("`.{method}(...)`: {why}")),
                }
            }
        }
    }

    /// The parameter and body of an array-method callback: an arrow function
    /// of one plain parameter whose body is one expression (or `{ return
    /// expr; }`). The body is lowered with the parameter bound to a fresh
    /// local and as a conditionally evaluated operand, so calls, `await`,
    /// value-producing `&&`/`||`/`??`, `?:` and `?.` (which would need
    /// operations emitted before the statement) are unknown in it; pure
    /// built-ins are allowed.
    fn callback(
        &mut self,
        m: &Module<'_>,
        e: &Expression<'_>,
        truthy: bool,
    ) -> Result<(String, Expr), String> {
        let Expression::ArrowFunctionExpression(arrow) = e.get_inner_expression() else {
            return Err("the callback is not an arrow function".to_owned());
        };
        let (param, body) = arrow_parts(arrow)?;
        let var = self.fresh(&param);
        self.frame_mut().scopes.push(HashMap::new());
        self.declare(&param, Binding::Local(var.clone()));
        let body = self.expr(
            m,
            body,
            Cx {
                col_ok: false,
                cond: true,
                truthy,
            },
        );
        self.frame_mut().scopes.pop();
        Ok((var, body))
    }

    /// Phase 14 early branch of `call_value`: built-in methods on values.
    /// `Map`/`Set` mutation of a tracked collection is a rebinding of its
    /// local; array mutation is refused; pure built-ins are values.
    pub(super) fn value_method(
        &mut self,
        m: &Module<'_>,
        call: &CallExpression<'_>,
        want: Want,
        span: Span,
    ) -> Option<Res> {
        if !self.js() {
            return None;
        }
        // A configured external effect keeps its meaning.
        let text = crate::semantic::member_path(&call.callee);
        if text
            .as_ref()
            .is_some_and(|t| self.request.externals.iter().any(|b| &b.callee == t))
        {
            return None;
        }
        if let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression()
            && let Expression::Identifier(id) = member.object.get_inner_expression()
            && let Some(Binding::Local(local)) = self.lookup(id.name.as_str())
        {
            let name = id.name.as_str().to_owned();
            let method = member.property.name.as_str();
            if ARRAY_MUTATORS.contains(&method) {
                return Some(self.unknown_op(
                    m,
                    span,
                    format!(
                        "`{name}.{method}(...)`: arrays are modeled as values; mutating one in place is not modeled"
                    ),
                ));
            }
            if matches!(method, "set" | "add" | "delete" | "clear") {
                return Some(self.collection_mutation(m, call, &name, &local, method, want, span));
            }
        }
        let e = self.value_call(m, call, TOP)?;
        if want == Want::Discard {
            // Evaluated for its errors (a `TypeError` on `null`), then dropped.
            let tmp = self.tmp("void");
            self.emit(
                m,
                span,
                Op::Let {
                    local: tmp,
                    expr: e,
                },
            );
            return Some(Res::Unit);
        }
        Some(Res::Value(e))
    }

    #[allow(clippy::too_many_arguments)]
    fn collection_mutation(
        &mut self,
        m: &Module<'_>,
        call: &CallExpression<'_>,
        name: &str,
        local: &str,
        method: &str,
        want: Want,
        span: Span,
    ) -> Res {
        let Some(is_map) = self.js_facts.collections.get(local).copied() else {
            return self.unknown_op(
                m,
                span,
                format!(
                    "`{name}.{method}(...)`: a Map/Set is mutated in place only when it is a `const` local initialized with `new Map`/`new Set` in the same function and never escapes it (passed, returned, stored, captured or aliased)"
                ),
            );
        };
        let mut args = Vec::new();
        for a in &call.arguments {
            match a.as_expression() {
                Some(x) => args.push(self.expr(m, x, TOP)),
                None => return self.unknown_op(m, span, "a spread argument"),
            }
        }
        let this = Expr::Local(local.to_owned());
        let (func, arity) = match (method, is_map) {
            ("set", true) => (Builtin::MapSet, 2),
            ("add", false) => (Builtin::SetAdd, 1),
            ("delete", _) => (Builtin::Delete, 1),
            ("clear", true) => (Builtin::NewMap, 0),
            ("clear", false) => (Builtin::NewSet, 0),
            _ => {
                return self.unknown_op(
                    m,
                    span,
                    format!(
                        "`{name}.{method}(...)` is not a method of a {}",
                        if is_map { "Map" } else { "Set" }
                    ),
                );
            }
        };
        if args.len() != arity {
            return self.unknown_op(
                m,
                span,
                format!("`{name}.{method}(...)` with {} argument(s)", args.len()),
            );
        }
        let mut result = Res::Unit;
        if method == "delete" && want != Want::Discard {
            // `delete` returns whether the key was there.
            let tmp = self.tmp("deleted");
            self.emit(
                m,
                span,
                Op::Let {
                    local: tmp.clone(),
                    expr: Expr::call(Builtin::Has, vec![this.clone(), args[0].clone()]),
                },
            );
            result = Res::Value(Expr::Local(tmp));
        } else if want != Want::Discard {
            // The escape analysis admits only statements and `delete`.
            return self.unknown_op(m, span, format!("the result of `{name}.{method}(...)`"));
        }
        let expr = if func == Builtin::NewMap || func == Builtin::NewSet {
            Expr::call(func, Vec::new())
        } else {
            let mut all = vec![this];
            all.extend(args);
            Expr::call(func, all)
        };
        self.emit(
            m,
            span,
            Op::Let {
                local: local.to_owned(),
                expr,
            },
        );
        result
    }

    // -- declarations ---------------------------------------------------------

    /// `const m = new Map(...)` / `new Set(...)`: a local holding the value;
    /// tracked as mutable in place when it never escapes its function.
    /// `None` when the initializer is not such a constructor.
    pub(super) fn declare_collection(
        &mut self,
        m: &Module<'_>,
        d: &oxc_ast::ast::VariableDeclarator<'_>,
        name: &str,
        id_span: Span,
        new: &oxc_ast::ast::NewExpression<'_>,
    ) -> Option<()> {
        if !self.js() {
            return None;
        }
        let func = self.collection_ctor(m, new)?;
        let value = self.collection_args(m, func, &new.arguments, TOP);
        let local = self.fresh(name);
        self.emit(
            m,
            d.span,
            Op::Let {
                local: local.clone(),
                expr: value,
            },
        );
        self.declare(name, Binding::Local(local.clone()));
        self.js_facts
            .kinds
            .insert(local.clone(), func == Builtin::NewMap);
        match collection_escape(m.program, d.span, id_span, name) {
            None => {
                self.js_facts
                    .collections
                    .insert(local, func == Builtin::NewMap);
            }
            Some(why) => self.assume(format!(
                "`{name}` (a {}) is not mutated in place: {why}",
                if func == Builtin::NewMap {
                    "Map"
                } else {
                    "Set"
                }
            )),
        }
        Some(())
    }

    /// After `const name = ...` bound `name` to a local: remember it as a
    /// string when its (only) value is statically one.
    pub(super) fn note_const_string(&mut self, m: &Module<'_>, d_span: Span, name: &str) {
        if !self.js() || !is_const_declarator(m.program, d_span) {
            return;
        }
        let Some(Binding::Local(local)) = self.lookup(name) else {
            return;
        };
        let defined = self.codes[self.cur].iter().rev().find_map(|s| match &s.op {
            Op::Let { local: l, expr } if *l == local => Some(expr.clone()),
            _ => None,
        });
        if defined.is_some_and(|e| self.static_string(&e)) {
            self.js_facts.strings.insert(local);
        }
    }

    /// `const [a, , b] = list`: each name is the element at its index
    /// (`undefined` beyond the end). Iterating a string by code point equals
    /// indexing it only in the Basic Multilingual Plane, which `At` checks.
    pub(super) fn destructure_list(
        &mut self,
        m: &Module<'_>,
        array: &oxc_ast::ast::ArrayPattern<'_>,
        value: Res,
        span: Span,
    ) {
        if array.rest.is_some() {
            self.unknown_op(m, span, "array rest patterns are not supported");
            return;
        }
        // The source value, once.
        let (rows, list) = match value {
            Res::Rows { row, .. } => (Some(row), None),
            Res::Row(row) => (None, Some(Expr::Local(row))),
            Res::Value(e) => (None, Some(e)),
            Res::Unit => (
                None,
                Some(Expr::Unknown(
                    "the result of this call is not modeled as a value".to_owned(),
                )),
            ),
            Res::Affected(_) | Res::Aggregate { .. } => (
                None,
                Some(Expr::Unknown(
                    "an array of rows whose values are not modeled".to_owned(),
                )),
            ),
            Res::Fields(_) => (
                None,
                Some(Expr::Unknown(
                    "a validation result is only modeled through its properties".to_owned(),
                )),
            ),
        };
        let tmp = match list {
            Some(e) => {
                let tmp = self.tmp("list");
                self.emit(
                    m,
                    span,
                    Op::Let {
                        local: tmp.clone(),
                        expr: e,
                    },
                );
                Some(tmp)
            }
            None => None,
        };
        for (i, el) in array.elements.iter().enumerate() {
            let Some(el) = el else { continue };
            let BindingPattern::BindingIdentifier(id) = el else {
                self.unknown_op(
                    m,
                    el.span(),
                    "nested patterns and defaults in array destructuring are not supported",
                );
                continue;
            };
            let expr = match (&rows, &tmp) {
                (Some(row), _) if i == 0 => Expr::Local(row.clone()),
                (Some(_), _) => Expr::undefined(),
                (None, Some(tmp)) => Expr::call(
                    Builtin::At,
                    vec![Expr::Local(tmp.clone()), Expr::lit(i as i64)],
                ),
                (None, None) => unreachable!("one of the two is set"),
            };
            let local = self.fresh(id.name.as_str());
            self.emit(
                m,
                el.span(),
                Op::Let {
                    local: local.clone(),
                    expr,
                },
            );
            self.declare(id.name.as_str(), Binding::Local(local));
        }
    }

    /// Whether the awaited call `e` is a database operation (its chain is
    /// rooted at a database handle): `const [row] = await db.select()...`
    /// keeps its Phase 7 meaning; other calls are destructured as lists.
    pub(super) fn database_call(&mut self, m: &Module<'_>, e: &Expression<'_>) -> bool {
        let Some((root, steps)) = super::flatten(e.get_inner_expression()) else {
            return false;
        };
        if root == "this" {
            return steps
                .first()
                .and_then(|s| s.path.first())
                .is_some_and(|field| self.this_field_handle(field).is_some());
        }
        match root.strip_suffix("()") {
            Some(factory) => self.factory_handle(m, factory, true).is_some(),
            None => self.handle_of(&root).is_some(),
        }
    }

    // -- loops ------------------------------------------------------------------

    /// `for (const x of list)` over a value: a bounded loop over a snapshot
    /// of the list (`i` from 0 while `i < list.length`, `x = list[i]`).
    /// `true` when lowered.
    pub(super) fn for_of_value(
        &mut self,
        m: &Module<'_>,
        f: &oxc_ast::ast::ForOfStatement<'_>,
        name: &str,
    ) -> bool {
        if !self.js() || f.r#await {
            return false;
        }
        if self.known_collection(m, &f.right) {
            self.unknown_op(
                m,
                f.span,
                "iterating a Map or a Set (its entries or items) is not modeled",
            );
            return true;
        }
        let value = self.expr(m, &f.right, TOP);
        let list = self.tmp("iter");
        let index = self.tmp("i");
        let span = f.right.span();
        self.emit(
            m,
            span,
            Op::Let {
                local: list.clone(),
                expr: value,
            },
        );
        self.emit(
            m,
            span,
            Op::Let {
                local: index.clone(),
                expr: Expr::lit(0),
            },
        );
        let head = self.here();
        let exit = self.emit(
            m,
            span,
            Op::JumpIf {
                cond: Expr::not(Expr::bin(
                    BinOp::Lt,
                    Expr::Local(index.clone()),
                    Expr::call(Builtin::Length, vec![Expr::Local(list.clone())]),
                )),
                target: usize::MAX,
            },
        );
        let tries = self.frame().tries;
        self.frame_mut()
            .breaks
            .push((Vec::new(), Some(Vec::new()), tries));
        self.frame_mut().scopes.push(HashMap::new());
        let local = self.fresh(name);
        self.emit(
            m,
            f.left.span(),
            Op::Let {
                local: local.clone(),
                expr: Expr::call(
                    Builtin::At,
                    vec![Expr::Local(list.clone()), Expr::Local(index.clone())],
                ),
            },
        );
        self.declare(name, Binding::Local(local));
        self.statement(m, &f.body);
        self.frame_mut().scopes.pop();
        // `continue` goes to the increment.
        let next = self.here();
        let (breaks, continues, _) = self.frame_mut().breaks.pop().expect("pushed above");
        for jump in continues.unwrap_or_default() {
            self.patch(jump, next);
        }
        self.emit(
            m,
            span,
            Op::Let {
                local: index.clone(),
                expr: Expr::bin(BinOp::Add, Expr::Local(index), Expr::lit(1)),
            },
        );
        self.emit(m, span, Op::Jump { target: head });
        let end = self.here();
        self.patch(exit, end);
        for jump in breaks {
            self.patch(jump, end);
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Shape {
    /// Exactly `n` arguments after the receiver.
    Fixed(Builtin, usize),
    /// At most `n` arguments after the receiver.
    Optional(Builtin, usize),
    /// `.at(i)`: relative indexing.
    RelativeAt,
    /// An array method with a callback.
    Callback(EachOp),
}

/// The pure built-in methods (the receiver's type is checked at run time).
fn pure_method(method: &str) -> Option<Shape> {
    use Builtin as B;
    Some(match method {
        "includes" => Shape::Fixed(B::Includes, 1),
        "indexOf" => Shape::Fixed(B::IndexOf, 1),
        "startsWith" => Shape::Fixed(B::StartsWith, 1),
        "endsWith" => Shape::Fixed(B::EndsWith, 1),
        "split" => Shape::Fixed(B::Split, 1),
        "toLowerCase" => Shape::Fixed(B::ToLowerCase, 0),
        "toUpperCase" => Shape::Fixed(B::ToUpperCase, 0),
        "trim" => Shape::Fixed(B::Trim, 0),
        "join" => Shape::Optional(B::Join, 1),
        "get" => Shape::Fixed(B::MapGet, 1),
        "has" => Shape::Fixed(B::Has, 1),
        "at" => Shape::RelativeAt,
        "map" => Shape::Callback(EachOp::Map),
        "filter" => Shape::Callback(EachOp::Filter),
        "find" => Shape::Callback(EachOp::Find),
        "findIndex" => Shape::Callback(EachOp::FindIndex),
        "some" => Shape::Callback(EachOp::Some),
        "every" => Shape::Callback(EachOp::Every),
        _ => return None,
    })
}

/// An arrow function `(x) => expr` / `x => { return expr; }`: its parameter
/// and its body expression.
fn arrow_parts<'b, 'a>(
    arrow: &'b ArrowFunctionExpression<'a>,
) -> Result<(String, &'b Expression<'a>), String> {
    if arrow.r#async {
        return Err("an async callback".to_owned());
    }
    let params = &arrow.params;
    let param = match params.items.as_slice() {
        [p] if params.rest.is_none() && p.initializer.is_none() => match &p.pattern {
            BindingPattern::BindingIdentifier(id) => id.name.as_str().to_owned(),
            _ => return Err("a destructuring callback parameter".to_owned()),
        },
        _ => return Err("a callback with other than one plain parameter".to_owned()),
    };
    let body = match &arrow.body {
        ArrowFunctionBody::FunctionBody(body) => match body.statements.as_slice() {
            [Statement::ReturnStatement(r)] => match &r.argument {
                Some(e) => e,
                None => return Err("a callback returning nothing".to_owned()),
            },
            _ => {
                return Err(
                    "a callback whose body is not one expression or `{ return expr; }`".to_owned(),
                );
            }
        },
        other => match other.as_expression() {
            Some(e) => e,
            None => return Err("an unexpected callback body".to_owned()),
        },
    };
    Ok((param, body))
}

/// A literal without effects (`void 0`).
fn pure_literal(e: &Expression<'_>) -> bool {
    matches!(
        e.get_inner_expression(),
        Expression::NumericLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
    )
}

/// A literal operand of `||`/`??`/`?:` around a value receiver
/// (`(name ?? "").trim()`).
fn literalish(e: &Expression<'_>) -> bool {
    pure_literal(e) || super::is_nullish(e)
}

/// Whether the module declares `name` at its top level (a variable).
fn declares_top_level(program: &Program<'_>, name: &str) -> bool {
    let declares = |v: &VariableDeclaration<'_>| {
        v.declarations.iter().any(
            |d| matches!(&d.id, BindingPattern::BindingIdentifier(id) if id.name.as_str() == name),
        )
    };
    program.body.iter().any(|s| match s {
        Statement::VariableDeclaration(v) => declares(v),
        Statement::ExportDeclaration(e) => matches!(
            &e.declaration,
            oxc_ast::ast::Declaration::VariableDeclaration(v) if declares(v)
        ),
        _ => false,
    })
}

/// Whether the declarator at `span` belongs to a `const` declaration.
fn is_const_declarator(program: &Program<'_>, span: Span) -> bool {
    struct Find {
        span: Span,
        found: Option<bool>,
    }
    impl<'a> Visit<'a> for Find {
        fn visit_variable_declaration(&mut self, it: &VariableDeclaration<'a>) {
            if it.declarations.iter().any(|d| d.span == self.span) {
                self.found = Some(it.kind.is_const());
                return;
            }
            walk::walk_variable_declaration(self, it);
        }
    }
    let mut find = Find { span, found: None };
    find.visit_program(program);
    find.found == Some(true)
}

// ---------------------------------------------------------------------------
// Map/Set escape analysis
// ---------------------------------------------------------------------------

/// Why the collection `name`, declared by the declarator at `decl` (its
/// identifier at `id`), could be observed through an alias, or `None` when
/// it cannot: the declaration is `const`, inside a function, and every
/// other occurrence of `name` in that function is `name.get(..)`,
/// `name.has(..)`, `name.delete(..)`, `name.size`, or a statement
/// `name.set(..)`/`name.add(..)`/`name.clear()` whose result is discarded,
/// none of them inside a nested function, and `name` is not redeclared.
fn collection_escape(program: &Program<'_>, decl: Span, id: Span, name: &str) -> Option<String> {
    if !is_const_declarator(program, decl) {
        return Some("it is not declared `const`".to_owned());
    }
    // The innermost function body containing the declaration (bodies are
    // visited outer first).
    struct Innermost {
        decl: Span,
        best: Option<Span>,
    }
    impl<'a> Visit<'a> for Innermost {
        fn visit_function_body(&mut self, it: &oxc_ast::ast::FunctionBody<'a>) {
            if it.span.start <= self.decl.start && self.decl.end <= it.span.end {
                self.best = Some(it.span);
            }
            walk::walk_function_body(self, it);
        }
    }
    let mut innermost = Innermost { decl, best: None };
    innermost.visit_program(program);
    let Some(body) = innermost.best else {
        return Some("it is not declared in a function body".to_owned());
    };
    let mut uses = Uses {
        name,
        id,
        body,
        active: false,
        depth: 0,
        escape: None,
    };
    uses.visit_program(program);
    uses.escape
}

struct Uses<'n> {
    name: &'n str,
    /// The declaring identifier.
    id: Span,
    /// The body of the declaring function, and whether the walk is in it.
    body: Span,
    active: bool,
    /// Nesting of functions inside the declaring one.
    depth: u32,
    escape: Option<String>,
}

impl Uses<'_> {
    fn flag(&mut self, why: String) {
        if self.escape.is_none() {
            self.escape = Some(why);
        }
    }

    /// `name.method` as a callee.
    fn own_method<'x>(&self, callee: &'x Expression<'_>) -> Option<&'x str> {
        match callee.get_inner_expression() {
            Expression::StaticMemberExpression(member) if !member.optional => {
                match member.object.get_inner_expression() {
                    Expression::Identifier(id) if id.name.as_str() == self.name => {
                        Some(member.property.name.as_str())
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn nested(&mut self, what: &str) {
        if self.depth > 0 {
            self.flag(format!(
                "`{}{what}` is used in a nested function",
                self.name
            ));
        }
    }
}

impl<'a> Visit<'a> for Uses<'_> {
    fn visit_function_body(&mut self, it: &oxc_ast::ast::FunctionBody<'a>) {
        if !self.active && it.span == self.body {
            self.active = true;
            walk::walk_function_body(self, it);
            self.active = false;
            return;
        }
        walk::walk_function_body(self, it);
    }

    fn visit_statement(&mut self, it: &Statement<'a>) {
        if self.active
            && let Statement::ExpressionStatement(s) = it
            && let Expression::CallExpression(call) = s.expression.get_inner_expression()
            && let Some(method) = self.own_method(&call.callee)
            && matches!(method, "set" | "add" | "delete" | "clear")
        {
            let method = method.to_owned();
            self.nested(&format!(".{method}(...)"));
            for a in &call.arguments {
                self.visit_argument(a);
            }
            return;
        }
        walk::walk_statement(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if self.active
            && let Some(method) = self.own_method(&it.callee)
        {
            let method = method.to_owned();
            self.nested(&format!(".{method}(...)"));
            if !matches!(method.as_str(), "get" | "has" | "delete") {
                self.flag(format!(
                    "the result of `{}.{method}(...)` is used",
                    self.name
                ));
            }
            for a in &it.arguments {
                self.visit_argument(a);
            }
            return;
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        if self.active
            && let Expression::Identifier(id) = it.object.get_inner_expression()
            && id.name.as_str() == self.name
            && it.property.name.as_str() == "size"
        {
            self.nested(".size");
            return;
        }
        walk::walk_static_member_expression(self, it);
    }

    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if self.active && it.name.as_str() == self.name {
            self.flag(format!(
                "`{}` is used as a value (passed, returned, stored, assigned, iterated or captured)",
                self.name
            ));
        }
    }

    fn visit_binding_identifier(&mut self, it: &oxc_ast::ast::BindingIdentifier<'a>) {
        if self.active && it.name.as_str() == self.name && it.span != self.id {
            self.flag(format!("`{}` is redeclared in the function", self.name));
        }
    }

    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        let inner = u32::from(self.active);
        self.depth += inner;
        walk::walk_function(self, it, flags);
        self.depth -= inner;
    }

    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        let inner = u32::from(self.active);
        self.depth += inner;
        walk::walk_arrow_function_expression(self, it);
        self.depth -= inner;
    }
}

// ---------------------------------------------------------------------------
// Parameter annotations (scenario synthesis)
// ---------------------------------------------------------------------------

impl super::TsActorProvider {
    /// Phase 14: the TypeScript annotations of the parameters of the entry
    /// `selector` (relative to `root`), as written, by parameter path in the
    /// extractor's naming (`name`, `input.tags`, `_param0.ids` for a
    /// destructured parameter): a parameter's own annotation, the members of
    /// an inline type literal, or of an interface or type alias declared in
    /// the same module. Used by discovery to type synthesized arguments
    /// (`string[]` versus `string`); anything else is simply absent.
    pub fn param_annotations(
        &self,
        root: &std::path::Path,
        selector: &str,
    ) -> std::collections::BTreeMap<String, String> {
        self.param_signature(root, selector).0
    }

    /// Phase 15: [`Self::param_annotations`] and the parameter paths that
    /// may be omitted (`x?: T`, a member `x?: T`, or a parameter with a
    /// default value): those accept `undefined` whatever their annotation.
    pub fn param_signature(
        &self,
        root: &std::path::Path,
        selector: &str,
    ) -> (
        std::collections::BTreeMap<String, String>,
        std::collections::BTreeSet<String>,
    ) {
        let mut out = std::collections::BTreeMap::new();
        let mut optional = std::collections::BTreeSet::new();
        let Ok(sel) = crate::identity::Selector::parse(selector) else {
            return (out, optional);
        };
        let abs = crate::resolve::absolute(&root.join(&sel.module));
        let Ok(source) = std::fs::read_to_string(&abs) else {
            return (out, optional);
        };
        let allocator = oxc_allocator::Allocator::default();
        let Ok(program) = super::load(&allocator, &abs, &source) else {
            return (out, optional);
        };
        let lines = commitrift_core::location::LineIndex::new(&source);
        let Ok(found) = crate::identity::resolve(&program, &lines, &sel) else {
            return (out, optional);
        };
        for (index, item) in found.node.params().items.iter().enumerate() {
            let base = match &item.pattern {
                BindingPattern::BindingIdentifier(id) => id.name.as_str().to_owned(),
                BindingPattern::ObjectPattern(_) => format!("_param{index}"),
                _ => continue,
            };
            if item.optional || item.initializer.is_some() {
                optional.insert(base.clone());
            }
            if let Some(t) = &item.type_annotation {
                let mut sink = Sink {
                    out: &mut out,
                    optional: &mut optional,
                    file: &abs,
                };
                annotate(&program, &source, &base, &t.type_annotation, 0, &mut sink);
            }
        }
        (out, optional)
    }
}

/// Where [`annotate`] records annotations and optional members.
struct Sink<'s> {
    out: &'s mut std::collections::BTreeMap<String, String>,
    optional: &'s mut std::collections::BTreeSet<String>,
    /// The module being read (Phase 15: imported aliases are followed).
    file: &'s std::path::Path,
}

fn annotate(
    program: &Program<'_>,
    source: &str,
    path: &str,
    t: &oxc_ast::ast::TSType<'_>,
    depth: u32,
    out: &mut Sink<'_>,
) {
    use oxc_ast::ast::{TSSignature, TSType, TSTypeName};
    let span = t.span();
    if let Some(text) = source.get(span.start as usize..span.end as usize) {
        out.out.insert(
            path.to_owned(),
            text.split_whitespace().collect::<Vec<_>>().join(" "),
        );
    }
    if depth >= 3 {
        return;
    }
    let members = |signatures: &[TSSignature<'_>], out: &mut Sink<'_>| {
        for s in signatures {
            if let TSSignature::TSPropertySignature(p) = s
                && !p.computed
                && let Some(key) = super::static_key(&p.key)
                && let Some(t) = &p.type_annotation
            {
                if p.optional {
                    out.optional.insert(format!("{path}.{key}"));
                }
                annotate(
                    program,
                    source,
                    &format!("{path}.{key}"),
                    &t.type_annotation,
                    depth + 1,
                    out,
                );
            }
        }
    };
    match t {
        TSType::TSTypeLiteral(lit) => members(&lit.members, out),
        TSType::TSTypeReference(r) if r.type_arguments.is_none() => {
            let TSTypeName::IdentifierReference(id) = &r.type_name else {
                return;
            };
            let name = id.name.as_str();
            for s in &program.body {
                let declaration = match s {
                    Statement::ExportDeclaration(e) => Some(&e.declaration),
                    other => other.as_declaration(),
                };
                match declaration {
                    Some(oxc_ast::ast::Declaration::TSInterfaceDeclaration(i))
                        if i.id.name.as_str() == name && i.extends.is_empty() =>
                    {
                        members(&i.body.body, out);
                        return;
                    }
                    Some(oxc_ast::ast::Declaration::TSTypeAliasDeclaration(a))
                        if a.id.name.as_str() == name =>
                    {
                        if let TSType::TSTypeLiteral(lit) = &a.type_annotation {
                            members(&lit.members, out);
                        } else if let Some(text) = literal_union(&a.type_annotation) {
                            // Phase 15: `type Kind = "a" | "b"` types the path.
                            out.out.insert(path.to_owned(), text);
                        }
                        return;
                    }
                    _ => {}
                }
            }
            // Phase 15: an alias imported from a project module.
            if let Some(text) = imported_literal_union(program, out.file, name, 0) {
                out.out.insert(path.to_owned(), text);
            }
        }
        _ => {}
    }
}

/// Phase 15: `"a" | "b"` for a type that is a union of string literals.
fn literal_union(t: &oxc_ast::ast::TSType<'_>) -> Option<String> {
    use oxc_ast::ast::{TSLiteral, TSType};
    let one = |t: &TSType<'_>| match t {
        TSType::TSLiteralType(l) => match &l.literal {
            TSLiteral::StringLiteral(s) => Some(format!("{:?}", s.value.as_str()?)),
            _ => None,
        },
        _ => None,
    };
    match t {
        TSType::TSUnionType(u) => {
            let parts: Option<Vec<String>> = u.types.iter().map(one).collect();
            parts.map(|p| p.join(" | "))
        }
        other => one(other),
    }
}

/// Phase 15: a relative import specifier as a project file.
fn relative_module(from: &std::path::Path, specifier: &str) -> Option<std::path::PathBuf> {
    if !(specifier.starts_with("./") || specifier.starts_with("../")) {
        return None;
    }
    let base = crate::resolve::normalize(&from.parent()?.join(specifier));
    let text = base.to_string_lossy().into_owned();
    let stem = text
        .strip_suffix(".js")
        .or_else(|| text.strip_suffix(".mjs"))
        .unwrap_or(&text)
        .to_owned();
    [
        format!("{stem}.ts"),
        format!("{stem}.tsx"),
        text.clone(),
        format!("{text}/index.ts"),
    ]
    .into_iter()
    .map(std::path::PathBuf::from)
    .find(|p| p.is_file())
}

/// Phase 15: the literal union an imported type name stands for, following
/// relative imports and re-exports (`export *`, `export { X } from`) up to
/// a small depth.
fn imported_literal_union(
    program: &Program<'_>,
    file: &std::path::Path,
    name: &str,
    depth: u32,
) -> Option<String> {
    use oxc_ast::ast::ImportDeclarationSpecifier;
    for s in &program.body {
        let Statement::ImportDeclaration(d) = s else {
            continue;
        };
        for spec in d.specifiers.iter().flatten() {
            if let ImportDeclarationSpecifier::ImportSpecifier(x) = spec
                && x.local.name.as_str() == name
            {
                let target = relative_module(file, d.source.value.as_str().unwrap_or_default())?;
                return alias_in_module(&target, x.imported.name().as_str(), depth + 1);
            }
        }
    }
    None
}

fn alias_in_module(file: &std::path::Path, name: &str, depth: u32) -> Option<String> {
    if depth > 4 {
        return None;
    }
    let source = std::fs::read_to_string(file).ok()?;
    let allocator = oxc_allocator::Allocator::default();
    let program = super::load(&allocator, file, &source).ok()?;
    for s in &program.body {
        let declaration = match s {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            other => other.as_declaration(),
        };
        if let Some(oxc_ast::ast::Declaration::TSTypeAliasDeclaration(a)) = declaration
            && a.id.name.as_str() == name
        {
            return literal_union(&a.type_annotation);
        }
    }
    for s in &program.body {
        match s {
            Statement::ExportFromDeclaration(e) => {
                for spec in &e.specifiers {
                    if spec.exported.name().as_str() == name {
                        let target =
                            relative_module(file, e.source.value.as_str().unwrap_or_default())?;
                        return alias_in_module(&target, spec.local.name().as_str(), depth + 1);
                    }
                }
            }
            Statement::ExportAllDeclaration(e) if e.exported.is_none() => {
                if let Some(target) =
                    relative_module(file, e.source.value.as_str().unwrap_or_default())
                    && let Some(found) = alias_in_module(&target, name, depth + 1)
                {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    imported_literal_union(&program, file, name, depth)
}

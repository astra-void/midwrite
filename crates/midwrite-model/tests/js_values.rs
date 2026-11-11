//! The JavaScript value domain of the model (strings,
//! arrays, `null` vs `undefined`, `Map`/`Set`): the ECMAScript behavior of
//! every built-in, its refusals, equality and truthiness, serialization,
//! the DSL syntax, state encoding, and agreement of the partial-order
//! reductions on models whose locals hold lists.

mod common;

use common::por_diff::{Alg, discrepancy, full, run};
use midwrite_model::dsl;
use midwrite_model::engine::{Recorder, apply, enabled, initial_state};
use midwrite_model::explore::{SdporConfig, Verdict, audit_independence, explore};
use midwrite_model::jsfn;
use midwrite_model::model::Model;
use midwrite_model::value::{
    BinOp, Builtin, EachOp, EvalError, Expr, GenId, Locals, MapVal, Mode, Row, SetVal, Value,
};
use serde_json::json;

fn s(text: &str) -> Value {
    Value::str(text)
}

fn list(items: Vec<Value>) -> Value {
    Value::List(items)
}

fn undef() -> Value {
    Value::undefined()
}

fn row(fields: &[(&str, Value)]) -> Value {
    Value::Row(
        fields
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect::<Row>(),
    )
}

fn gen_value(n: u32) -> Value {
    Value::Gen(GenId {
        generator: 0,
        actor: 0,
        n,
    })
}

fn call(func: Builtin, args: Vec<Value>) -> Result<Value, EvalError> {
    let e = Expr::call(func, args.into_iter().map(Expr::Lit).collect());
    e.eval(&Locals::new(), None, Mode::Js)
}

fn ok(func: Builtin, args: Vec<Value>) -> Value {
    call(func, args.clone()).unwrap_or_else(|e| panic!("{}({args:?}): {e}", func.name()))
}

fn is_unknown(r: Result<Value, EvalError>) -> bool {
    matches!(r, Err(EvalError::Unknown(_)))
}

fn is_type_error(r: Result<Value, EvalError>) -> bool {
    matches!(r, Err(EvalError::Type(_)))
}

// ---------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------

#[test]
fn to_string_follows_ecmascript_for_the_modeled_values() {
    use Builtin::ToString as T;
    assert_eq!(ok(T, vec![Value::Null]), s("null"));
    assert_eq!(ok(T, vec![undef()]), s("undefined"));
    assert_eq!(ok(T, vec![Value::Bool(true)]), s("true"));
    assert_eq!(ok(T, vec![Value::Int(-42)]), s("-42"));
    assert_eq!(
        ok(T, vec![Value::Int(jsfn::MAX_SAFE_INTEGER)]),
        s("9007199254740991")
    );
    assert_eq!(
        ok(T, vec![Value::Int(-jsfn::MAX_SAFE_INTEGER)]),
        s("-9007199254740991")
    );
    // 2^53 is not represented exactly by every neighbor: refused.
    assert!(is_unknown(call(
        T,
        vec![Value::Int(jsfn::MAX_SAFE_INTEGER + 1)]
    )));
    assert!(is_unknown(call(T, vec![Value::Int(i64::MIN)])));
    // Arrays: join(","), null and undefined are empty, nested arrays flatten.
    assert_eq!(
        ok(
            T,
            vec![list(vec![
                Value::Int(1),
                Value::Null,
                list(vec![Value::Int(2), undef()]),
                s("a"),
            ])]
        ),
        s("1,,2,,a")
    );
    assert_eq!(ok(T, vec![list(vec![])]), s(""));
    // Objects, generated values, Map, Set: refused.
    assert!(is_unknown(call(T, vec![row(&[("a", Value::Int(1))])])));
    assert!(is_unknown(call(T, vec![gen_value(1)])));
    assert!(is_unknown(call(
        T,
        vec![Value::Map(MapVal { entries: vec![] })]
    )));
    assert!(is_unknown(call(
        T,
        vec![Value::Set(SetVal { items: vec![] })]
    )));
    assert!(is_unknown(call(T, vec![list(vec![row(&[])])])));
}

#[test]
fn concatenation_is_tostring_of_every_part() {
    assert_eq!(
        ok(
            Builtin::Concat,
            vec![
                s("user:"),
                Value::Int(7),
                s(":"),
                Value::Null,
                s(":"),
                undef()
            ]
        ),
        s("user:7:null:undefined")
    );
    assert_eq!(ok(Builtin::Concat, vec![]), s(""));
    assert!(is_unknown(call(
        Builtin::Concat,
        vec![s("id-"), gen_value(0)]
    )));
}

#[test]
fn string_length_counts_utf16_code_units() {
    assert_eq!(ok(Builtin::Length, vec![s("abc")]), Value::Int(3));
    assert_eq!(ok(Builtin::Length, vec![s("")]), Value::Int(0));
    assert_eq!(ok(Builtin::Length, vec![s("é")]), Value::Int(1));
    // A character outside the Basic Multilingual Plane is two code units.
    assert_eq!(ok(Builtin::Length, vec![s("😀")]), Value::Int(2));
    assert_eq!(ok(Builtin::Length, vec![s("a😀b")]), Value::Int(4));
    assert!(is_type_error(call(Builtin::Length, vec![Value::Null])));
    assert!(is_type_error(call(Builtin::Length, vec![undef()])));
    // A generated value's text is not known.
    assert!(is_unknown(call(Builtin::Length, vec![gen_value(0)])));
    // Numbers have no `length`.
    assert_eq!(ok(Builtin::Length, vec![Value::Int(5)]), undef());
    // An object's `length` is a property.
    assert_eq!(
        ok(Builtin::Length, vec![row(&[("length", Value::Int(9))])]),
        Value::Int(9)
    );
    assert_eq!(ok(Builtin::Length, vec![row(&[])]), undef());
}

#[test]
fn includes_starts_and_ends_with_search_substrings() {
    assert_eq!(
        ok(Builtin::Includes, vec![s("admin,owner"), s("owner")]),
        Value::Bool(true)
    );
    assert_eq!(
        ok(Builtin::Includes, vec![s("abc"), s("")]),
        Value::Bool(true)
    );
    assert_eq!(
        ok(Builtin::Includes, vec![s("abc"), s("d")]),
        Value::Bool(false)
    );
    // The search value is converted with ToString.
    assert_eq!(
        ok(Builtin::Includes, vec![s("a1"), Value::Int(1)]),
        Value::Bool(true)
    );
    assert_eq!(
        ok(Builtin::Includes, vec![s("xnull"), Value::Null]),
        Value::Bool(true)
    );
    assert_eq!(
        ok(Builtin::StartsWith, vec![s("admin:1"), s("admin:")]),
        Value::Bool(true)
    );
    assert_eq!(
        ok(Builtin::StartsWith, vec![s("admin"), s("admin:")]),
        Value::Bool(false)
    );
    assert_eq!(
        ok(
            Builtin::EndsWith,
            vec![s("a@example.com"), s("@example.com")]
        ),
        Value::Bool(true)
    );
    assert_eq!(ok(Builtin::EndsWith, vec![s(""), s("")]), Value::Bool(true));
    assert!(is_type_error(call(
        Builtin::StartsWith,
        vec![Value::Null, s("a")]
    )));
    // `startsWith` of an array is not a method: not modeled.
    assert!(is_unknown(call(
        Builtin::StartsWith,
        vec![list(vec![]), s("a")]
    )));
    assert!(is_unknown(call(
        Builtin::Includes,
        vec![s("abc"), gen_value(0)]
    )));
}

#[test]
fn index_of_on_strings_counts_utf16_code_units() {
    assert_eq!(ok(Builtin::IndexOf, vec![s("a😀b"), s("b")]), Value::Int(3));
    assert_eq!(ok(Builtin::IndexOf, vec![s("abc"), s("")]), Value::Int(0));
    assert_eq!(ok(Builtin::IndexOf, vec![s("abc"), s("z")]), Value::Int(-1));
    assert_eq!(ok(Builtin::IndexOf, vec![s("abab"), s("b")]), Value::Int(1));
}

#[test]
fn split_with_a_non_empty_separator_keeps_empty_fields() {
    let parts = |text: &str, sep: &str| ok(Builtin::Split, vec![s(text), s(sep)]);
    assert_eq!(parts("a,b", ","), list(vec![s("a"), s("b")]));
    assert_eq!(parts("a,b,", ","), list(vec![s("a"), s("b"), s("")]));
    assert_eq!(parts(",a", ","), list(vec![s(""), s("a")]));
    assert_eq!(parts("", ","), list(vec![s("")]));
    assert_eq!(parts(",", ","), list(vec![s(""), s("")]));
    assert_eq!(parts("abc", ","), list(vec![s("abc")]));
    // Left to right, non-overlapping.
    assert_eq!(parts("aaa", "aa"), list(vec![s(""), s("a")]));
    assert_eq!(parts("a::b", "::"), list(vec![s("a"), s("b")]));
    // An empty separator splits into UTF-16 code units: not modeled.
    assert!(is_unknown(call(Builtin::Split, vec![s("ab"), s("")])));
    assert!(is_unknown(call(Builtin::Split, vec![s("ab"), undef()])));
    assert!(is_type_error(call(Builtin::Split, vec![undef(), s(",")])));
}

#[test]
fn case_mapping_is_modeled_for_ascii_only() {
    assert_eq!(
        ok(Builtin::ToLowerCase, vec![s("Alice@Example.COM")]),
        s("alice@example.com")
    );
    assert_eq!(ok(Builtin::ToUpperCase, vec![s("a-1_b")]), s("A-1_B"));
    // Unicode case mapping (`"ß".toUpperCase()` is "SS") is not modeled.
    assert!(is_unknown(call(Builtin::ToUpperCase, vec![s("straße")])));
    assert!(is_unknown(call(Builtin::ToLowerCase, vec![s("É")])));
    assert!(is_type_error(call(Builtin::ToLowerCase, vec![Value::Null])));
    assert!(is_unknown(call(Builtin::ToLowerCase, vec![Value::Int(1)])));
}

#[test]
fn trim_removes_exactly_ecmascript_whitespace_and_line_terminators() {
    let trim = |text: &str| ok(Builtin::Trim, vec![s(text)]);
    assert_eq!(trim("  a b  "), s("a b"));
    // ZWNBSP (BOM) is WhiteSpace in ECMAScript.
    assert_eq!(trim("\u{FEFF}x\u{FEFF}"), s("x"));
    assert_eq!(
        trim("\u{0009}\u{000B}\u{000C}\u{00A0}\u{1680}\u{2000}\u{200A}\u{202F}\u{205F}\u{3000}x"),
        s("x")
    );
    assert_eq!(trim("\n\r\u{2028}\u{2029}x\n"), s("x"));
    // NEL (U+0085) is White_Space in Unicode but not in ECMAScript; the
    // Mongolian vowel separator is no longer a space separator.
    assert_eq!(trim("\u{0085}x"), s("\u{0085}x"));
    assert_eq!(trim("\u{180E}x"), s("\u{180E}x"));
    // ZERO WIDTH SPACE is not whitespace either.
    assert_eq!(trim("\u{200B}x"), s("\u{200B}x"));
    assert_eq!(trim(""), s(""));
    for c in ['\u{0085}', '\u{180E}', '\u{200B}'] {
        assert!(!jsfn::is_js_whitespace(c), "{c:?}");
    }
}

#[test]
fn typeof_names_the_types_and_refuses_generated_values() {
    let t = |v: Value| ok(Builtin::TypeOf, vec![v]);
    assert_eq!(t(s("x")), s("string"));
    assert_eq!(t(Value::Int(1)), s("number"));
    assert_eq!(t(Value::Bool(false)), s("boolean"));
    assert_eq!(t(undef()), s("undefined"));
    assert_eq!(t(Value::Null), s("object"));
    assert_eq!(t(list(vec![])), s("object"));
    assert_eq!(t(row(&[])), s("object"));
    assert_eq!(t(Value::Map(MapVal { entries: vec![] })), s("object"));
    assert!(is_unknown(call(Builtin::TypeOf, vec![gen_value(0)])));
}

// ---------------------------------------------------------------------------
// Arrays
// ---------------------------------------------------------------------------

#[test]
fn element_access_is_undefined_out_of_range() {
    let xs = list(vec![s("a"), s("b")]);
    assert_eq!(ok(Builtin::At, vec![xs.clone(), Value::Int(0)]), s("a"));
    assert_eq!(ok(Builtin::At, vec![xs.clone(), Value::Int(1)]), s("b"));
    assert_eq!(ok(Builtin::At, vec![xs.clone(), Value::Int(2)]), undef());
    // `xs[-1]` is a missing property, not the last element.
    assert_eq!(ok(Builtin::At, vec![xs.clone(), Value::Int(-1)]), undef());
    assert_eq!(ok(Builtin::At, vec![list(vec![]), Value::Int(0)]), undef());
    // Strings: one UTF-16 code unit, in the Basic Multilingual Plane.
    assert_eq!(ok(Builtin::At, vec![s("héllo"), Value::Int(1)]), s("é"));
    assert_eq!(ok(Builtin::At, vec![s("ab"), Value::Int(5)]), undef());
    assert!(is_unknown(call(Builtin::At, vec![s("a😀"), Value::Int(0)])));
    // Object properties: missing is undefined.
    let r = row(&[("email", s("a@b"))]);
    assert_eq!(ok(Builtin::At, vec![r.clone(), s("email")]), s("a@b"));
    assert_eq!(ok(Builtin::At, vec![r, s("name")]), undef());
    assert!(is_type_error(call(Builtin::At, vec![Value::Null, s("x")])));
    assert!(is_type_error(call(
        Builtin::At,
        vec![undef(), Value::Int(0)]
    )));
    // Not modeled: a string key on an array, an index on an object.
    assert!(is_unknown(call(Builtin::At, vec![xs, s("0")])));
    assert!(is_unknown(call(Builtin::At, vec![row(&[]), Value::Int(0)])));
}

#[test]
fn array_includes_uses_same_value_zero_and_refuses_identity() {
    let xs = list(vec![Value::Int(1), s("1"), Value::Null, undef()]);
    let has = |x: Value| ok(Builtin::Includes, vec![xs.clone(), x]);
    assert_eq!(has(Value::Int(1)), Value::Bool(true));
    assert_eq!(has(s("1")), Value::Bool(true));
    assert_eq!(has(Value::Int(2)), Value::Bool(false));
    assert_eq!(has(Value::Null), Value::Bool(true));
    assert_eq!(has(undef()), Value::Bool(true));
    assert_eq!(has(Value::Bool(true)), Value::Bool(false));
    // null and undefined are different values.
    let only_null = list(vec![Value::Null]);
    assert_eq!(
        ok(Builtin::Includes, vec![only_null.clone(), undef()]),
        Value::Bool(false)
    );
    assert_eq!(
        ok(Builtin::IndexOf, vec![only_null, Value::Null]),
        Value::Int(0)
    );
    // An object needle is compared by identity: refused. Object elements
    // never equal a primitive.
    let objects = list(vec![row(&[("id", Value::Int(1))])]);
    assert!(is_unknown(call(
        Builtin::Includes,
        vec![objects.clone(), row(&[])]
    )));
    assert!(is_unknown(call(
        Builtin::Includes,
        vec![list(vec![]), list(vec![])]
    )));
    assert_eq!(
        ok(Builtin::Includes, vec![objects, Value::Int(1)]),
        Value::Bool(false)
    );
    // Generated values: identity of the generation event.
    let ids = list(vec![gen_value(1)]);
    assert_eq!(
        ok(Builtin::Includes, vec![ids.clone(), gen_value(1)]),
        Value::Bool(true)
    );
    assert_eq!(
        ok(Builtin::Includes, vec![ids.clone(), gen_value(2)]),
        Value::Bool(false)
    );
    assert_eq!(
        ok(Builtin::Includes, vec![ids, s("gen")]),
        Value::Bool(false)
    );
    assert_eq!(
        ok(
            Builtin::IndexOf,
            vec![list(vec![s("a"), s("b"), s("b")]), s("b")]
        ),
        Value::Int(1)
    );
    assert!(is_type_error(call(
        Builtin::Includes,
        vec![undef(), Value::Int(1)]
    )));
    assert!(is_unknown(call(
        Builtin::Includes,
        vec![Value::Int(5), Value::Int(1)]
    )));
}

#[test]
fn join_converts_elements_and_empties_null_and_undefined() {
    let xs = list(vec![
        s("a"),
        Value::Null,
        Value::Int(3),
        undef(),
        Value::Bool(false),
    ]);
    assert_eq!(ok(Builtin::Join, vec![xs.clone()]), s("a,,3,,false"));
    assert_eq!(
        ok(Builtin::Join, vec![xs.clone(), undef()]),
        s("a,,3,,false")
    );
    assert_eq!(
        ok(Builtin::Join, vec![xs, s(" | ")]),
        s("a |  | 3 |  | false")
    );
    assert_eq!(ok(Builtin::Join, vec![list(vec![]), s("-")]), s(""));
    assert_eq!(
        ok(Builtin::Join, vec![list(vec![s("x")]), Value::Int(1)]),
        s("x")
    );
    assert!(is_unknown(call(Builtin::Join, vec![s("abc")])));
    assert!(is_unknown(call(Builtin::Join, vec![list(vec![row(&[])])])));
}

fn each(op: EachOp, xs: Value, body: Expr) -> Result<Value, EvalError> {
    let e = Expr::Each {
        op,
        list: Box::new(Expr::Lit(xs)),
        var: "x".to_owned(),
        body: Box::new(body),
    };
    e.eval(&Locals::new(), None, Mode::Js)
}

#[test]
fn array_callbacks_follow_javascript_order_and_short_circuit() {
    let xs = list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
    let gt1 = Expr::bin(BinOp::Gt, Expr::local("x"), Expr::lit(1));
    assert_eq!(
        each(
            EachOp::Map,
            xs.clone(),
            Expr::bin(BinOp::Mul, Expr::local("x"), Expr::lit(10))
        ),
        Ok(list(vec![Value::Int(10), Value::Int(20), Value::Int(30)]))
    );
    assert_eq!(
        each(EachOp::Filter, xs.clone(), gt1.clone()),
        Ok(list(vec![Value::Int(2), Value::Int(3)]))
    );
    assert_eq!(
        each(EachOp::Find, xs.clone(), gt1.clone()),
        Ok(Value::Int(2))
    );
    assert_eq!(
        each(EachOp::FindIndex, xs.clone(), gt1.clone()),
        Ok(Value::Int(1))
    );
    assert_eq!(
        each(EachOp::Some, xs.clone(), gt1.clone()),
        Ok(Value::Bool(true))
    );
    assert_eq!(
        each(EachOp::Every, xs.clone(), gt1.clone()),
        Ok(Value::Bool(false))
    );
    let big = Expr::bin(BinOp::Gt, Expr::local("x"), Expr::lit(5));
    // A miss: `find` is undefined, `findIndex` is -1.
    assert_eq!(each(EachOp::Find, xs.clone(), big.clone()), Ok(undef()));
    assert_eq!(
        each(EachOp::FindIndex, xs.clone(), big.clone()),
        Ok(Value::Int(-1))
    );
    assert_eq!(
        each(EachOp::Some, list(vec![]), big.clone()),
        Ok(Value::Bool(false))
    );
    assert_eq!(
        each(EachOp::Every, list(vec![]), big),
        Ok(Value::Bool(true))
    );
    // `find` stops at the first match: the null after it is never read.
    let users = list(vec![row(&[("id", Value::Int(1))]), Value::Null]);
    let id_is_1 = Expr::eq(Expr::field(Expr::local("x"), "id"), Expr::lit(1));
    assert_eq!(
        each(EachOp::Find, users.clone(), id_is_1.clone()),
        Ok(row(&[("id", Value::Int(1))]))
    );
    // `filter` reads every element: reading `id` of null throws.
    assert!(is_type_error(each(EachOp::Filter, users, id_is_1)));
    // Receivers.
    assert!(is_type_error(each(
        EachOp::Map,
        Value::Null,
        Expr::local("x")
    )));
    assert!(is_unknown(each(EachOp::Map, s("abc"), Expr::local("x"))));
    // The callback sees the enclosing locals.
    let mut locals = Locals::new();
    locals.insert("limit".to_owned(), Value::Int(2));
    let e = Expr::Each {
        op: EachOp::Filter,
        list: Box::new(Expr::Lit(xs)),
        var: "x".to_owned(),
        body: Box::new(Expr::bin(BinOp::Le, Expr::local("x"), Expr::local("limit"))),
    };
    assert_eq!(
        e.eval(&locals, None, Mode::Js),
        Ok(list(vec![Value::Int(1), Value::Int(2)]))
    );
    // The callback's parameter is not a local read by the expression.
    let mut read = Vec::new();
    e.locals(&mut read);
    assert_eq!(read, vec!["limit".to_owned()]);
}

// ---------------------------------------------------------------------------
// Map and Set
// ---------------------------------------------------------------------------

#[test]
fn maps_keep_insertion_order_and_compare_keys_with_same_value_zero() {
    let m = ok(
        Builtin::NewMap,
        vec![list(vec![
            list(vec![s("a"), Value::Int(1)]),
            list(vec![s("b"), Value::Int(2)]),
            list(vec![s("a"), Value::Int(3)]),
        ])],
    );
    // A repeated key keeps its first position and takes the last value.
    assert_eq!(
        m,
        Value::Map(MapVal {
            entries: vec![(s("a"), Value::Int(3)), (s("b"), Value::Int(2))]
        })
    );
    assert_eq!(ok(Builtin::MapGet, vec![m.clone(), s("b")]), Value::Int(2));
    assert_eq!(ok(Builtin::MapGet, vec![m.clone(), s("z")]), undef());
    assert_eq!(ok(Builtin::Has, vec![m.clone(), s("a")]), Value::Bool(true));
    let m2 = ok(Builtin::MapSet, vec![m.clone(), s("a"), Value::Int(9)]);
    assert_eq!(
        m2,
        Value::Map(MapVal {
            entries: vec![(s("a"), Value::Int(9)), (s("b"), Value::Int(2))]
        })
    );
    let m3 = ok(Builtin::MapSet, vec![m2, Value::Int(1), s("one")]);
    assert_eq!(ok(Builtin::Size, vec![m3.clone()]), Value::Int(3));
    // `1` and `"1"` are different keys.
    assert_eq!(ok(Builtin::MapGet, vec![m3.clone(), s("1")]), undef());
    let m4 = ok(Builtin::Delete, vec![m3, s("a")]);
    assert_eq!(
        m4,
        Value::Map(MapVal {
            entries: vec![(s("b"), Value::Int(2)), (Value::Int(1), s("one"))]
        })
    );
    // null and undefined are distinct keys.
    let n = ok(
        Builtin::MapSet,
        vec![ok(Builtin::NewMap, vec![]), Value::Null, Value::Int(1)],
    );
    assert_eq!(
        ok(Builtin::Has, vec![n.clone(), undef()]),
        Value::Bool(false)
    );
    assert_eq!(ok(Builtin::Has, vec![n, Value::Null]), Value::Bool(true));
    // `new Map(undefined)` is empty; a non-object entry throws; object keys
    // are refused.
    assert_eq!(
        ok(Builtin::NewMap, vec![undef()]),
        Value::Map(MapVal { entries: vec![] })
    );
    assert!(is_type_error(call(
        Builtin::NewMap,
        vec![list(vec![Value::Int(1)])]
    )));
    assert!(is_unknown(call(
        Builtin::NewMap,
        vec![list(vec![list(vec![row(&[]), Value::Int(1)])])]
    )));
    assert!(is_unknown(call(
        Builtin::MapGet,
        vec![Value::Map(MapVal { entries: vec![] }), list(vec![])]
    )));
    assert!(is_type_error(call(
        Builtin::MapGet,
        vec![Value::Null, s("a")]
    )));
}

#[test]
fn sets_deduplicate_in_insertion_order() {
    let set = ok(
        Builtin::NewSet,
        vec![list(vec![s("b"), s("a"), s("b"), Value::Int(1), s("1")])],
    );
    assert_eq!(
        set,
        Value::Set(SetVal {
            items: vec![s("b"), s("a"), Value::Int(1), s("1")]
        })
    );
    assert_eq!(ok(Builtin::Size, vec![set.clone()]), Value::Int(4));
    let added = ok(Builtin::SetAdd, vec![set.clone(), s("a")]);
    assert_eq!(added, set, "adding a member changes nothing");
    let added = ok(Builtin::SetAdd, vec![set, s("c")]);
    assert_eq!(
        ok(Builtin::Has, vec![added.clone(), s("c")]),
        Value::Bool(true)
    );
    let removed = ok(Builtin::Delete, vec![added, s("b")]);
    assert_eq!(
        removed,
        Value::Set(SetVal {
            items: vec![s("a"), Value::Int(1), s("1"), s("c")]
        })
    );
    // A string iterates by code point.
    assert_eq!(
        ok(Builtin::NewSet, vec![s("a😀a")]),
        Value::Set(SetVal {
            items: vec![s("a"), s("😀")]
        })
    );
    assert!(is_unknown(call(
        Builtin::NewSet,
        vec![list(vec![list(vec![])])]
    )));
    assert!(is_unknown(call(
        Builtin::SetAdd,
        vec![ok(Builtin::NewSet, vec![]), row(&[])]
    )));
    // `size` of other values: a property, or undefined.
    assert_eq!(
        ok(Builtin::Size, vec![row(&[("size", Value::Int(2))])]),
        Value::Int(2)
    );
    assert_eq!(ok(Builtin::Size, vec![s("abc")]), undef());
}

// ---------------------------------------------------------------------------
// Equality, truthiness, arithmetic, properties
// ---------------------------------------------------------------------------

fn bin(op: BinOp, a: Value, b: Value, mode: Mode) -> Result<Value, EvalError> {
    Expr::bin(op, Expr::Lit(a), Expr::Lit(b)).eval(&Locals::new(), None, mode)
}

#[test]
fn undefined_is_strictly_equal_only_to_itself() {
    let js = |op, a, b| bin(op, a, b, Mode::Js);
    assert_eq!(js(BinOp::Eq, undef(), undef()), Ok(Value::Bool(true)));
    assert_eq!(js(BinOp::Eq, undef(), Value::Null), Ok(Value::Bool(false)));
    assert_eq!(js(BinOp::Eq, Value::Null, undef()), Ok(Value::Bool(false)));
    assert_eq!(js(BinOp::Ne, undef(), Value::Null), Ok(Value::Bool(true)));
    assert_eq!(
        js(BinOp::Eq, undef(), Value::Int(0)),
        Ok(Value::Bool(false))
    );
    assert_eq!(js(BinOp::Eq, s(""), undef()), Ok(Value::Bool(false)));
    assert_eq!(js(BinOp::Eq, undef(), list(vec![])), Ok(Value::Bool(false)));
    assert_eq!(js(BinOp::Ne, gen_value(0), undef()), Ok(Value::Bool(true)));
    // Ordering with undefined is not modeled.
    assert!(is_unknown(js(BinOp::Lt, undef(), Value::Int(1))));
    // null keeps its Phase 6–13 behavior.
    assert_eq!(
        js(BinOp::Eq, Value::Null, Value::Null),
        Ok(Value::Bool(true))
    );
    assert_eq!(
        js(BinOp::Eq, Value::Null, list(vec![])),
        Ok(Value::Bool(false))
    );
    // JavaScript values never reach SQL.
    assert!(is_unknown(bin(
        BinOp::Eq,
        undef(),
        Value::Int(1),
        Mode::Sql
    )));
    assert!(is_unknown(bin(
        BinOp::Eq,
        list(vec![]),
        Value::Int(1),
        Mode::Sql
    )));
}

#[test]
fn arrays_maps_and_sets_are_not_compared_by_identity() {
    let js = |op, a, b| bin(op, a, b, Mode::Js);
    assert!(is_unknown(js(BinOp::Eq, list(vec![]), list(vec![]))));
    assert!(is_unknown(js(
        BinOp::Ne,
        list(vec![Value::Int(1)]),
        Value::Int(1)
    )));
    assert!(is_unknown(js(
        BinOp::Eq,
        Value::Set(SetVal { items: vec![] }),
        s("x")
    )));
    assert!(is_unknown(js(BinOp::Lt, list(vec![]), Value::Int(1))));
}

#[test]
fn truthiness_of_the_new_values() {
    assert!(!undef().truthy());
    assert!(list(vec![]).truthy());
    assert!(Value::Map(MapVal { entries: vec![] }).truthy());
    assert!(Value::Set(SetVal { items: vec![] }).truthy());
    assert!(undef().is_nullish() && Value::Null.is_nullish());
    assert!(!undef().is_null());
    let not = Expr::not(Expr::Lit(undef()));
    assert_eq!(
        not.eval(&Locals::new(), None, Mode::Js),
        Ok(Value::Bool(true))
    );
}

#[test]
fn arithmetic_with_the_new_values_is_not_modeled_but_null_keeps_its_rule() {
    let js = |op, a, b| bin(op, a, b, Mode::Js);
    assert!(is_unknown(js(BinOp::Add, undef(), Value::Int(1))));
    assert!(is_unknown(js(BinOp::Add, list(vec![]), Value::Int(1))));
    // The pre-existing approximation `null + x = null` is unchanged (in
    // JavaScript `null + 1` is 1): documented as a known infidelity.
    assert_eq!(js(BinOp::Add, Value::Null, undef()), Ok(Value::Null));
    assert_eq!(js(BinOp::Add, Value::Null, Value::Int(1)), Ok(Value::Null));
}

#[test]
fn fields_of_arrays_and_undefined() {
    let mut locals = Locals::new();
    locals.insert("xs".to_owned(), list(vec![Value::Int(1), Value::Int(2)]));
    locals.insert("u".to_owned(), undef());
    let length = Expr::field(Expr::local("xs"), "length");
    assert_eq!(length.eval(&locals, None, Mode::Js), Ok(Value::Int(2)));
    assert!(is_unknown(Expr::field(Expr::local("xs"), "foo").eval(
        &locals,
        None,
        Mode::Js
    )));
    assert!(is_type_error(Expr::field(Expr::local("u"), "id").eval(
        &locals,
        None,
        Mode::Js
    )));
    // The missing property of a row stays `null` through `Field` (the
    // Phase 6–13 form); `At` is the JavaScript property read.
    locals.insert("r".to_owned(), row(&[]));
    assert_eq!(
        Expr::field(Expr::local("r"), "id").eval(&locals, None, Mode::Js),
        Ok(Value::Null)
    );
    let at = Expr::call(Builtin::At, vec![Expr::local("r"), Expr::Lit(s("id"))]);
    assert_eq!(at.eval(&locals, None, Mode::Js), Ok(undef()));
    assert_eq!(
        at.as_property().map(|(b, n)| (b.clone(), n.to_owned())),
        Some((Expr::local("r"), "id".to_owned()))
    );
    assert_eq!(at.render(), "r.id");
}

#[test]
fn javascript_arguments_and_callbacks_keep_javascript_semantics_in_sql_folding() {
    // `values({ kept: xs.filter((x) => x !== null).length })`: in SQL mode
    // `x != null` would be unknown and drop every element.
    let mut locals = Locals::new();
    locals.insert(
        "xs".to_owned(),
        list(vec![Value::Int(1), Value::Null, Value::Int(2)]),
    );
    let kept = Expr::call(
        Builtin::Length,
        vec![Expr::Each {
            op: EachOp::Filter,
            list: Box::new(Expr::local("xs")),
            var: "x".to_owned(),
            body: Box::new(Expr::bin(
                BinOp::Ne,
                Expr::local("x"),
                Expr::Lit(Value::Null),
            )),
        }],
    );
    assert_eq!(kept.close(&locals, Mode::Sql), Ok(Expr::lit(2)));
    // `String(a === null)` folded: the comparison is JavaScript's.
    locals.insert("a".to_owned(), Value::Null);
    let text = Expr::call(
        Builtin::ToString,
        vec![Expr::eq(Expr::local("a"), Expr::Lit(Value::Null))],
    );
    assert_eq!(text.close(&locals, Mode::Sql), Ok(Expr::Lit(s("true"))));
}

#[test]
fn folding_before_sql_evaluates_javascript_builtins() {
    // `values({ email: input.email.trim().toLowerCase() })`: folded with the
    // actor's locals before the statement is sent.
    let mut locals = Locals::new();
    locals.insert(
        "input".to_owned(),
        row(&[("email", s("  Bob@Example.com "))]),
    );
    let e = Expr::call(
        Builtin::ToLowerCase,
        vec![Expr::call(
            Builtin::Trim,
            vec![Expr::call(
                Builtin::At,
                vec![Expr::local("input"), Expr::Lit(s("email"))],
            )],
        )],
    );
    assert_eq!(
        e.close(&locals, Mode::Sql),
        Ok(Expr::Lit(s("bob@example.com")))
    );
}

// ---------------------------------------------------------------------------
// Serialization, DSL
// ---------------------------------------------------------------------------

#[test]
fn new_values_round_trip_through_scenario_json() {
    let values = vec![
        undef(),
        list(vec![
            Value::Int(1),
            s("a"),
            Value::Null,
            undef(),
            list(vec![]),
        ]),
        Value::Map(MapVal {
            entries: vec![
                (s("k"), list(vec![Value::Bool(true)])),
                (Value::Int(2), undef()),
            ],
        }),
        Value::Set(SetVal {
            items: vec![s("x"), Value::Int(3)],
        }),
        row(&[("tags", list(vec![s("a")])), ("note", undef())]),
    ];
    for v in values {
        let text = serde_json::to_string(&v).unwrap();
        let back: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(back, v, "{text}");
    }
    assert_eq!(
        serde_json::to_value(undef()).unwrap(),
        json!({"$undefined": true})
    );
    assert_eq!(
        serde_json::from_value::<Value>(json!({"$set": ["a"]})).unwrap(),
        Value::Set(SetVal {
            items: vec![s("a")]
        })
    );
    assert_eq!(
        serde_json::from_value::<Value>(json!([1, {"$undefined": true}])).unwrap(),
        list(vec![Value::Int(1), undef()])
    );
    // Arrays are lists, never a reserved form read by position (a derived
    // struct deserializer would read `[1, 2, 3]` as a generated value and
    // `[true]` as `undefined`).
    for (doc, expected) in [
        (
            json!([1, 2, 3]),
            list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
        ),
        (json!([true]), list(vec![Value::Bool(true)])),
        (
            json!([[["a", 1]]]),
            list(vec![list(vec![list(vec![s("a"), Value::Int(1)])])]),
        ),
        (json!([["x"]]), list(vec![list(vec![s("x")])])),
    ] {
        assert_eq!(
            serde_json::from_value::<Value>(doc.clone()).unwrap(),
            expected,
            "{doc}"
        );
    }
    // The reserved forms are unchanged as objects; `$undefined: false` is
    // not `undefined`.
    assert_eq!(
        serde_json::from_value::<Value>(json!({"$generated": 1, "actor": 2, "n": 3})).unwrap(),
        Value::Gen(GenId {
            generator: 1,
            actor: 2,
            n: 3
        })
    );
    assert_ne!(
        serde_json::from_value::<Value>(json!({"$undefined": false})).unwrap(),
        undef()
    );
    // Expressions with built-ins and callbacks (declared `codes`).
    let e = Expr::Each {
        op: EachOp::Some,
        list: Box::new(Expr::call(
            Builtin::Split,
            vec![Expr::local("tags"), Expr::Lit(s(","))],
        )),
        var: "t".to_owned(),
        body: Box::new(Expr::eq(Expr::local("t"), Expr::Lit(s("admin")))),
    };
    let text = serde_json::to_string(&e).unwrap();
    assert_eq!(serde_json::from_str::<Expr>(&text).unwrap(), e);
}

#[test]
fn dsl_parses_lists_undefined_and_builtins() {
    let e = dsl::parse_expr("[1, \"a\", undefined].includes(undefined)").unwrap();
    assert_eq!(
        e.eval(&Locals::new(), None, Mode::Js),
        Ok(Value::Bool(true))
    );
    let e = dsl::parse_expr("concat(\"user:\", String(7), \":\", [1, 2].join(\"-\"))").unwrap();
    assert_eq!(e.eval(&Locals::new(), None, Mode::Js), Ok(s("user:7:1-2")));
    let e = dsl::parse_expr("[3, 1, 2].filter(x => x > 1).map(x => x * 2)").unwrap();
    assert_eq!(
        e.eval(&Locals::new(), None, Mode::Js),
        Ok(list(vec![Value::Int(6), Value::Int(4)]))
    );
    let e = dsl::parse_expr("newSet([\"a\", \"a\"]).add(\"b\").has(\"b\")").unwrap();
    assert_eq!(
        e.eval(&Locals::new(), None, Mode::Js),
        Ok(Value::Bool(true))
    );
    let e = dsl::parse_expr("length(\" x \".trim()) == 1 && [].length == 0").unwrap();
    assert_eq!(
        e.eval(&Locals::new(), None, Mode::Js),
        Ok(Value::Bool(true))
    );
    assert!(dsl::parse_expr("x.frobnicate(1)").is_err());
}

// ---------------------------------------------------------------------------
// Models whose locals hold lists
// ---------------------------------------------------------------------------

/// Two actors add a tag to a row's comma-separated tag list unless it is
/// already there (read-modify-write of a string through list operations).
fn tags_model(program: &str, iso: &str) -> Model {
    midwrite_model::scenario::testing::model_from_json(json!({
        "schemaVersion": 1,
        "name": "tags",
        "database": {
            "isolation": iso,
            "tables": [{ "name": "posts", "columns": ["id", "tags"], "primaryKey": ["id"] }],
            "initialState": { "posts": [{ "id": 1, "tags": "news" }] }
        },
        "actors": [
            { "name": "A", "function": "tag", "program": program, "arguments": { "tag": "a" } },
            { "name": "B", "function": "tag", "program": program, "arguments": { "tag": "b" } }
        ],
        "invariants": [{
            "name": "no-tag-lost",
            "assert": { "and": [
                { "eq": [{ "count": { "table": "posts", "where": { "tags": "news,a" } } }, 0] },
                { "eq": [{ "count": { "table": "posts", "where": { "tags": "news,b" } } }, 0] }
            ] }
        }]
    }))
}

const TAG_LOST_UPDATE: &str = "\
let p = select posts where @id == 1
let tags = p.tags.split(\",\")
if !tags.includes(tag) {
    update posts where @id == 1 set @tags = concat(p.tags, \",\", tag)
}
";

const TAG_LOCKED: &str = "\
begin
let p = select posts where @id == 1 for update
let tags = p.tags.split(\",\")
if !tags.includes(tag) {
    update posts where @id == 1 set @tags = concat(p.tags, \",\", tag)
}
commit
";

#[test]
fn a_read_modify_write_through_list_operations_loses_an_update_and_the_lock_prevents_it() {
    // Without a lock both actors read "news", find their tag missing and
    // write: one tag is lost. With `FOR UPDATE` in a transaction the second
    // actor sees the first tag.
    let lost = tags_model(TAG_LOST_UPDATE, "read-committed");
    let finals = final_tags(&lost);
    assert!(
        finals.contains("news,a") && finals.contains("news,b"),
        "{finals:?}"
    );
    assert_eq!(explore(&lost).verdict, Verdict::CounterexampleFound);
    let locked = tags_model(TAG_LOCKED, "read-committed");
    assert_eq!(
        final_tags(&locked),
        ["news,a,b".to_owned(), "news,b,a".to_owned()]
            .into_iter()
            .collect(),
    );
    let r = explore(&locked);
    assert_eq!(r.verdict, Verdict::NoCounterexampleWithinBounds);
    assert!(r.unsupported.is_empty());
}

fn final_tags(m: &Model) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut stack = vec![initial_state(m)];
    while let Some(s) = stack.pop() {
        if !seen.insert(s.canonical(m)) {
            continue;
        }
        let next = enabled(m, &s);
        if next.is_empty() {
            for r in s.committed_rows(m, 0) {
                if let Some(Value::Str(t)) = r.get("tags") {
                    out.insert(t.clone());
                }
            }
        }
        for t in next {
            if let Ok(x) = apply(m, &s, t, &mut Recorder::off()) {
                stack.push(x.state);
            }
        }
    }
    out
}

/// Actors that keep list-valued locals across database steps: a set of
/// seen keys built step by step, and a list filtered and joined.
fn list_locals_model(iso: &str) -> Model {
    let a = "\
let seen = newSet([])
let first = select posts where @id == 1
let seen = seen.add(first.tags)
let second = select posts where @id == 2
let seen = seen.add(second.tags)
if size(seen) == 1 {
    update posts where @id == 1 set @tags = \"same\"
}
";
    let b = "\
let parts = [\"x\", \"y\", \"z\"].filter(t => t != skip)
update posts where @id == 2 set @tags = parts.join(\"\")
let again = select posts where @id == 2
let found = parts.find(t => t == again.tags)
if found == undefined {
    update posts where @id == 1 set @tags = parts.at(0)
}
";
    midwrite_model::scenario::testing::model_from_json(json!({
        "schemaVersion": 1,
        "name": "list-locals",
        "database": {
            "isolation": iso,
            "tables": [{ "name": "posts", "columns": ["id", "tags"], "primaryKey": ["id"] }],
            "initialState": { "posts": [{ "id": 1, "tags": "xz" }, { "id": 2, "tags": "q" }] }
        },
        "actors": [
            { "name": "A", "function": "a", "program": a, "arguments": {} },
            { "name": "B", "function": "b", "program": b, "arguments": { "skip": "y" } },
            { "name": "C", "function": "c", "program": TAG_LOST_UPDATE, "arguments": { "tag": "c" } }
        ],
        "invariants": [{ "name": "row-1-not-same", "assert": { "eq": [
            { "count": { "table": "posts", "where": { "tags": "same" } } }, 0
        ] } }]
    }))
}

#[test]
fn reductions_agree_on_models_with_list_valued_locals() {
    let algs = [
        Alg::Sleep,
        Alg::Classic,
        Alg::Sdpor(SdporConfig {
            cache: true,
            sleep: true,
            no_enabling_edges: false,
            defects: 0,
        }),
        Alg::Sdpor(SdporConfig {
            cache: false,
            sleep: true,
            no_enabling_edges: false,
            defects: 0,
        }),
    ];
    let mut models: Vec<(String, Model)> = Vec::new();
    for iso in ["read-committed", "repeatable-read", "serializable"] {
        models.push((format!("lost-tag/{iso}"), tags_model(TAG_LOST_UPDATE, iso)));
        models.push((format!("locked-tag/{iso}"), tags_model(TAG_LOCKED, iso)));
        models.push((format!("list-locals/{iso}"), list_locals_model(iso)));
    }
    let mut compared = 0;
    for (label, m) in &models {
        let violations = audit_independence(m, 100_000);
        assert!(
            violations.is_empty(),
            "{label}: {} violations",
            violations.len()
        );
        let f = full(m).unwrap_or_else(|| panic!("{label}: too large for the unreduced search"));
        for alg in algs {
            let r = run(alg, m);
            if let Some(d) = discrepancy(&f, &r, m) {
                panic!(
                    "{label}: {} differs from the unreduced search: {d}",
                    alg.name()
                );
            }
            compared += 1;
        }
    }
    assert_eq!(compared, models.len() * algs.len());
}

#[test]
fn list_valued_locals_are_part_of_the_state() {
    // Two states that differ only in a list-valued local are different
    // states; equal lists are the same state.
    let m = list_locals_model("read-committed");
    let s0 = initial_state(&m);
    let mut a = s0.clone();
    let mut b = s0.clone();
    std::sync::Arc::make_mut(&mut a.actors[0].locals).insert("xs".to_owned(), list(vec![s("a")]));
    std::sync::Arc::make_mut(&mut b.actors[0].locals).insert("xs".to_owned(), list(vec![s("b")]));
    assert_ne!(a.canonical(&m), b.canonical(&m));
    let mut c = s0;
    std::sync::Arc::make_mut(&mut c.actors[0].locals).insert("xs".to_owned(), list(vec![s("a")]));
    assert_eq!(a.canonical(&m), c.canonical(&m));
    // `undefined` and `null` are different states too.
    let mut u = c.clone();
    let mut n = c;
    std::sync::Arc::make_mut(&mut u.actors[0].locals).insert("v".to_owned(), undef());
    std::sync::Arc::make_mut(&mut n.actors[0].locals).insert("v".to_owned(), Value::Null);
    assert_ne!(u.canonical(&m), n.canonical(&m));
}

#[test]
fn a_type_error_in_a_builtin_is_the_javascript_exception() {
    // `null.split(",")` throws a TypeError: the actor fails with it (a
    // modeled exception, as in JavaScript).
    let m = midwrite_model::scenario::testing::model_from_json(json!({
        "schemaVersion": 1,
        "name": "type-error",
        "database": {
            "tables": [{ "name": "posts", "columns": ["id", "tags"], "primaryKey": ["id"] }],
            "initialState": { "posts": [{ "id": 1, "tags": null }] }
        },
        "actors": [{ "name": "A", "function": "a", "program":
            "let p = select posts where @id == 1\nlet parts = p.tags.split(\",\")\n",
            "arguments": {} }],
        "invariants": [{ "name": "true", "assert": { "eq": [0, 0] } }]
    }));
    let mut s = initial_state(&m);
    loop {
        let next = enabled(&m, &s);
        let Some(t) = next.into_iter().next() else {
            break;
        };
        s = apply(&m, &s, t, &mut Recorder::off()).unwrap().state;
    }
    let failure = format!("{:?}", s.actors[0].outcome);
    assert!(failure.contains("TypeError"), "{failure}");
}

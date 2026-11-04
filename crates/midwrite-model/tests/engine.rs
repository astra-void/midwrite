//! Semantic tests of the model: workloads, isolation levels, verdicts.

use midwrite_model::explore::{ExploreOptions, Verdict, explore, explore_with};
use midwrite_model::model::Strategy;
use midwrite_model::scenario::testing::{model_from_programs, model_with};
use midwrite_model::value::Value;
use midwrite_model::witness::{build_witness, render_text};

const PURCHASE: &str = "\
let product = select products where @id == pid
if product == null || product.stock <= 0 {
  fail \"out of stock\"
}
update products where @id == pid set @stock = product.stock - 1
insert orders set @id = oid, @product = pid
";

fn args(oid: &str) -> Vec<(&'static str, Value)> {
    vec![("pid", "p1".into()), ("oid", oid.into())]
}

#[test]
fn lost_update_is_found_without_a_transaction() {
    let a = args("oA");
    let b = args("oB");
    let model = model_from_programs(
        "read-committed",
        &[("A", PURCHASE, &a), ("B", PURCHASE, &b)],
    );
    let r = explore(&model);
    assert_eq!(r.verdict, Verdict::CounterexampleFound);
    let ce = r.counterexample.expect("counterexample");
    let w = build_witness(&model, &ce, true).expect("witness");
    let text = render_text(&model, &w);
    assert!(text.contains("VIOLATED"), "{text}");
    println!("{text}");
}

#[test]
fn atomic_conditional_update_removes_the_violation() {
    let program = "\
update products where @id == pid && @stock > 0 set @stock = @stock - 1 into n
if n == 0 {
  fail \"out of stock\"
}
insert orders set @id = oid, @product = pid
";
    let a = args("oA");
    let b = args("oB");
    let model = model_from_programs("read-committed", &[("A", program, &a), ("B", program, &b)]);
    let r = explore(&model);
    assert_eq!(
        r.verdict,
        Verdict::NoCounterexampleWithinBounds,
        "{:?}",
        r.unsupported
    );
}

#[test]
fn select_for_update_in_a_transaction_removes_the_violation() {
    let program = "\
begin
let product = select products where @id == pid for update
if product == null || product.stock <= 0 {
  fail \"out of stock\"
}
update products where @id == pid set @stock = product.stock - 1
insert orders set @id = oid, @product = pid
commit
";
    let a = args("oA");
    let b = args("oB");
    let model = model_from_programs("read-committed", &[("A", program, &a), ("B", program, &b)]);
    let r = explore(&model);
    assert_eq!(
        r.verdict,
        Verdict::NoCounterexampleWithinBounds,
        "{:?}",
        r.counterexample.map(|c| c.schedule)
    );
}

#[test]
fn repeatable_read_turns_the_lost_update_into_a_serialization_failure() {
    let program = "\
begin
let product = select products where @id == pid
if product == null || product.stock <= 0 {
  fail \"out of stock\"
}
update products where @id == pid set @stock = product.stock - 1
insert orders set @id = oid, @product = pid
commit
";
    let a = args("oA");
    let b = args("oB");
    let rc = model_from_programs("read-committed", &[("A", program, &a), ("B", program, &b)]);
    assert_eq!(explore(&rc).verdict, Verdict::CounterexampleFound);
    let rr = model_from_programs("repeatable-read", &[("A", program, &a), ("B", program, &b)]);
    assert_eq!(explore(&rr).verdict, Verdict::NoCounterexampleWithinBounds);
}

#[test]
fn bfs_dfs_and_por_agree_on_the_verdict() {
    let a = args("oA");
    let b = args("oB");
    let model = model_from_programs(
        "read-committed",
        &[("A", PURCHASE, &a), ("B", PURCHASE, &b)],
    );
    for (por, strategy) in [
        (false, Strategy::Bfs),
        (false, Strategy::Dfs),
        (true, Strategy::Dfs),
    ] {
        let mut o = ExploreOptions::from_model(&model);
        o.por = por;
        o.strategy = strategy;
        let r = explore_with(&model, &o);
        assert_eq!(
            r.verdict,
            Verdict::CounterexampleFound,
            "por={por} {strategy:?}"
        );
    }
}

#[test]
fn unknown_operations_make_the_result_unsupported() {
    let program = "unknown \"raw SQL with a trigger\"\n";
    let a = args("oA");
    let model = model_from_programs("read-committed", &[("A", program, &a)]);
    let r = explore(&model);
    assert_eq!(r.verdict, Verdict::Unsupported);
    assert_eq!(r.unsupported[0].detail, "raw SQL with a trigger");
}

#[test]
fn a_depth_cut_is_inconclusive_not_exhaustive() {
    let a = args("oA");
    let b = args("oB");
    let atomic = "\
update products where @id == pid && @stock > 0 set @stock = @stock - 1 into n
insert orders set @id = oid, @product = pid
";
    let model = model_with(
        "read-committed",
        &[("A", atomic, &a), ("B", atomic, &b)],
        serde_json::json!({ "maxSteps": 2 }),
        serde_json::json!({}),
    );
    let r = explore(&model);
    assert_eq!(r.verdict, Verdict::Inconclusive);
    assert!(r.truncation.max_steps);
}

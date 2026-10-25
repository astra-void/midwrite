//! Differential tests: the model against a real PostgreSQL.
//!
//! These need PostgreSQL server binaries. Without them each test prints a
//! message and returns; set `COMMITRIFT_REQUIRE_REPLAY=1` to make that a
//! failure (the quality gate does).

use commitrift_model::explore::explore;
use commitrift_model::oracle::{PgEnv, run_schedule};
use commitrift_model::scenario::testing::model_from_programs;
use commitrift_model::value::Value;
use commitrift_model::witness::build_witness;

fn env_or_skip() -> Option<PgEnv> {
    match PgEnv::start() {
        Ok(env) => Some(env),
        Err(reason) => {
            assert!(
                std::env::var_os("COMMITRIFT_REQUIRE_REPLAY").is_none(),
                "PostgreSQL is required but unavailable: {reason}"
            );
            eprintln!("skipping: {reason}");
            None
        }
    }
}

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
fn the_lost_update_witness_is_reproduced_by_postgres() {
    let Some(mut env) = env_or_skip() else { return };
    let a = args("oA");
    let b = args("oB");
    let model = model_from_programs(
        "read-committed",
        &[("A", PURCHASE, &a), ("B", PURCHASE, &b)],
    );
    let ce = explore(&model)
        .counterexample
        .expect("model counterexample");
    let w = build_witness(&model, &ce, true).expect("witness");
    let db = env.database(&model).expect("database");
    let run = run_schedule(&env, &db, &model, &w.schedule).expect("oracle");
    for o in &run.observations {
        println!("{o:?}");
    }
    assert!(run.agree, "{run:#?}");
    // Real PostgreSQL ends with stock 0 and two orders: the invariant is
    // violated in the database, not only in the model.
    assert_eq!(run.postgres_tables["orders"].len(), 2);
}

//! Generated values (`Value::Gen`) and generated column defaults.

use midwrite_model::engine::{Recorder, apply, enabled, initial_state};
use midwrite_model::explore::explore;
use midwrite_model::model::Model;
use midwrite_model::scenario::testing::model_from_json;
use midwrite_model::value::{BinOp, Expr, GenId, Locals, Mode, Value};
use serde_json::json;

fn gen_id(generator: u16, actor: u16, n: u32) -> Value {
    Value::Gen(GenId {
        generator,
        actor,
        n,
    })
}

#[test]
fn a_generated_value_equals_itself_only_and_has_no_order() {
    let a = gen_id(0, 0, 0);
    let b = gen_id(0, 1, 0);
    let eval = |op, l: &Value, r: &Value, mode| {
        Expr::bin(op, Expr::Lit(l.clone()), Expr::Lit(r.clone())).eval(&Locals::new(), None, mode)
    };
    for mode in [Mode::Sql, Mode::Js] {
        assert_eq!(eval(BinOp::Eq, &a, &a, mode), Ok(Value::Bool(true)));
        assert_eq!(eval(BinOp::Eq, &a, &b, mode), Ok(Value::Bool(false)));
        assert_eq!(eval(BinOp::Ne, &a, &b, mode), Ok(Value::Bool(true)));
        // Distinct from every concrete value (the distinct-values assumption).
        assert_eq!(
            eval(BinOp::Eq, &a, &Value::str("u1"), mode),
            Ok(Value::Bool(false))
        );
        // No order: the values are random.
        assert!(eval(BinOp::Lt, &a, &b, mode).is_err());
    }
    // SQL: comparison with NULL is unknown.
    assert_eq!(
        eval(BinOp::Eq, &a, &Value::Null, Mode::Sql),
        Ok(Value::Null)
    );
    assert!(a.truthy());
    // Stable serialization (a reserved key) and rendering.
    let text = serde_json::to_string(&a).unwrap();
    assert_eq!(text, r#"{"$generated":0,"actor":0,"n":0}"#);
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), a);
    assert_eq!(b.render(), "gen#0:1.0");
}

/// A table whose key PostgreSQL generates, and a child table filled from
/// the returned key.
fn scenario(programs: &[(&str, &str)], bounds: serde_json::Value) -> Model {
    model_from_json(json!({
        "schemaVersion": 1,
        "name": "generated",
        "database": {
            "tables": [
                { "name": "users", "columns": [
                    { "name": "id", "notNull": true, "type": "text",
                      "default": { "generated": { "side": "database", "generator": "gen_random_uuid()" } } },
                    { "name": "email", "notNull": true, "type": "text" }
                  ], "primaryKey": ["id"] },
                { "name": "profiles", "columns": [
                    { "name": "id", "notNull": true, "type": "text",
                      "default": { "generated": { "side": "application", "generator": "crypto.randomUUID()" } } },
                    { "name": "user", "notNull": true, "type": "text" }
                  ], "primaryKey": ["id"] }
            ],
            "initialState": {}
        },
        "actors": programs.iter().map(|(name, program)| json!({
            "name": name, "function": "f", "program": program, "arguments": {}
        })).collect::<Vec<_>>(),
        "bounds": bounds,
        "invariants": [{ "name": "true", "assert": { "eq": [0, 0] } }]
    }))
}

const SIGNUP: &str = "\
insert users set @email = \"a@example.test\" returning u
insert profiles set @user = u.id
";

fn run_alone(model: &Model) -> midwrite_model::state::State {
    let mut s = initial_state(model);
    let mut rec = Recorder::off();
    while let Some(t) = enabled(model, &s)
        .into_iter()
        .find(|t| !t.is_crash_or_retry())
    {
        s = apply(model, &s, t, &mut rec).expect("supported").state;
    }
    s
}

#[test]
fn a_returned_generated_key_is_the_one_the_child_row_stores() {
    let model = scenario(&[("a", SIGNUP)], json!({}));
    let s = run_alone(&model);
    let users = s.committed_rows(&model, 0);
    let profiles = s.committed_rows(&model, 1);
    assert_eq!(users.len(), 1);
    assert_eq!(profiles.len(), 1);
    let key = &users[0]["id"];
    assert!(matches!(key, Value::Gen(_)), "{key:?}");
    assert_eq!(
        &profiles[0]["user"], key,
        "identity is preserved through RETURNING"
    );
    assert_ne!(&profiles[0]["id"], key, "two generation events, two values");
    // The assumption is stated with every result.
    assert!(
        model
            .assumptions
            .iter()
            .any(|a| a.starts_with("generated-values-distinct"))
    );
}

#[test]
fn different_actors_and_executions_generate_different_values() {
    let model = scenario(
        &[("a", SIGNUP), ("b", SIGNUP.replace("a@", "b@").as_str())],
        json!({}),
    );
    let s = run_alone(&model);
    let users = s.committed_rows(&model, 0);
    assert_eq!(users.len(), 2);
    assert_ne!(users[0]["id"], users[1]["id"]);
    // A rolled-back insert does not give its value back: the next insert
    // of the same actor gets a new one.
    let rollback = "\
begin
insert users set @email = \"r@example.test\" returning first
fail \"abort\"
commit
";
    let model = scenario(&[("a", rollback), ("b", SIGNUP)], json!({}));
    let s = run_alone(&model);
    assert_eq!(s.committed_rows(&model, 0).len(), 1);
    assert_eq!(s.actors[0].gen_seq, 1, "the rolled-back event is consumed");
}

#[test]
fn a_retried_execution_generates_a_new_key() {
    // The insert commits, the process crashes, the request is retried: the
    // second execution inserts a second row with a new key (no collision).
    let model = model_from_json(json!({
        "schemaVersion": 1,
        "name": "retry",
        "database": {
            "tables": [{ "name": "users", "columns": [
                { "name": "id", "notNull": true, "type": "text",
                  "default": { "generated": { "side": "application", "generator": "crypto.randomUUID()" } } },
                { "name": "email", "type": "text" }
            ], "primaryKey": ["id"] }],
            "initialState": {}
        },
        "actors": [{
            "name": "a", "function": "f", "arguments": {},
            "program": "insert users set @email = \"a@example.test\"\ninsert users set @email = \"audit\"\n",
            "retry": { "trigger": "onCrash" }
        }],
        "bounds": { "maxCrashes": 1, "maxRetries": 1 },
        "invariants": [{
            "name": "one-user-per-email", "when": "final",
            "assert": { "le": [{ "count": { "table": "users", "where": { "email": "a@example.test" } } }, 1] }
        }]
    }));
    let result = explore(&model);
    let ce = result.counterexample.expect("the retry duplicates the row");
    // Replay the schedule to its final state.
    let mut s = initial_state(&model);
    let mut rec = Recorder::off();
    for t in &ce.schedule {
        s = apply(&model, &s, *t, &mut rec).expect("supported").state;
    }
    let rows = s.committed_rows(&model, 0);
    let keys: Vec<&Value> = rows
        .iter()
        .filter(|r| r["email"] == Value::str("a@example.test"))
        .map(|r| &r["id"])
        .collect();
    assert_eq!(keys.len(), 2);
    assert_ne!(keys[0], keys[1]);
}

#[test]
fn generation_counters_are_part_of_the_state_identity() {
    let model = scenario(&[("a", SIGNUP)], json!({}));
    let s = initial_state(&model);
    let mut t = s.clone();
    t.actors[0].gen_seq = 1;
    assert_ne!(s.canonical(&model), t.canonical(&model));
    // Without generation the encoding has no generation section.
    let plain = s.canonical(&model);
    let mut again = s.clone();
    again.actors[0].gen_seq = 0;
    assert_eq!(plain, again.canonical(&model));
}

#[test]
fn a_generated_value_cannot_be_initial_data() {
    let doc = json!({
        "schemaVersion": 1,
        "name": "x",
        "database": {
            "tables": [{ "name": "t", "columns": [{ "name": "id", "notNull": true }], "primaryKey": ["id"] }],
            "initialState": { "t": [{ "id": { "$generated": 0, "actor": 0, "n": 0 } }] }
        },
        "actors": [{ "name": "a", "function": "f", "program": "insert t set @id = 1\n", "arguments": {} }],
        "invariants": [{ "name": "true", "assert": { "eq": [0, 0] } }]
    });
    let err = midwrite_model::scenario::load_str(
        &doc.to_string(),
        std::path::Path::new("."),
        &midwrite_model::provider::NoProvider,
    )
    .unwrap_err();
    assert!(err.message.contains("generated value"), "{err}");
}

#[cfg(feature = "postgres")]
#[test]
fn postgres_agrees_with_schedules_over_generated_keys() {
    use midwrite_model::oracle::{PgEnv, run_schedule};
    use midwrite_model::witness::build_witness;
    let mut env = match PgEnv::start() {
        Ok(env) => env,
        Err(reason) => {
            assert!(
                std::env::var_os("MIDWRITE_REQUIRE_REPLAY").is_none(),
                "PostgreSQL is required but unavailable: {reason}"
            );
            return;
        }
    };
    // Two sign-ups racing on one email (UNIQUE): exactly one user and one
    // profile in every schedule; the oracle replays the model's witness.
    let model = model_from_json(json!({
        "schemaVersion": 1,
        "name": "generated-unique",
        "database": {
            "tables": [
                { "name": "users", "columns": [
                    { "name": "id", "notNull": true, "type": "text",
                      "default": { "generated": { "side": "database", "generator": "gen_random_uuid()" } } },
                    { "name": "email", "notNull": true, "type": "text" }
                  ], "primaryKey": ["id"], "unique": [["email"]] },
                { "name": "profiles", "columns": [
                    { "name": "id", "notNull": true, "type": "text",
                      "default": { "generated": { "side": "application", "generator": "crypto.randomUUID()" } } },
                    { "name": "user", "notNull": true, "type": "text" }
                  ], "primaryKey": ["id"] }
            ],
            "initialState": {}
        },
        "actors": [
            { "name": "a", "function": "f", "program": SIGNUP, "arguments": {} },
            { "name": "b", "function": "f", "program": SIGNUP, "arguments": {} }
        ],
        "invariants": [{
            "name": "two-profiles", "when": "final",
            "assert": { "le": [{ "count": { "table": "profiles" } }, 1] }
        }, {
            "name": "never", "when": "final",
            "assert": { "eq": [{ "count": { "table": "users" } }, 2] }
        }]
    }));
    // `never` fails in every schedule (one sign-up fails on the unique
    // email): the witness is a real schedule to replay.
    let ce = explore(&model).counterexample.expect("a schedule");
    let w = build_witness(&model, &ce, true).expect("witness");
    let db = env.database(&model).expect("database");
    let run = run_schedule(&env, &db, &model, &w.schedule).expect("oracle");
    assert!(run.agree, "{run:#?}");
    assert_eq!(run.postgres_tables["users"].len(), 1);
    assert_eq!(run.postgres_tables["profiles"].len(), 1);
}

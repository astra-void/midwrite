//! Phase 16: `UNIQUE ... NULLS NOT DISTINCT` (PostgreSQL 15+) against
//! PostgreSQL's default `NULLS DISTINCT`, model and server in lockstep.
//!
//! Every workload exists twice, once per NULL mode, and runs at READ
//! COMMITTED, REPEATABLE READ and SERIALIZABLE. For each configuration every
//! schedule the model has (they are few) is replayed against a disposable
//! PostgreSQL cluster in lockstep: each statement's outcome (rows, SQLSTATE,
//! which backend waits for which) and the final tables must agree. The
//! model-only tests at the end pin the semantics without a server.

mod common;

use std::collections::BTreeSet;

use commitrift_model::explore::{SdporConfig, Verdict, audit_independence, explore};
use commitrift_model::ir::{Op, StmtKind};
use commitrift_model::model::Model;
use commitrift_model::oracle::{PgEnv, ddl, run_schedule};
use commitrift_model::value::{Expr, Value};
use commitrift_model::witness::enumerate_schedules;
use common::por_diff::{Alg, discrepancy, full, run};
use common::*;
use serde_json::{Value as Json, json};

const ISOLATIONS: [&str; 3] = ["read-committed", "repeatable-read", "serializable"];

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

/// The two NULL modes of every unique rule under test.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Nulls {
    Distinct,
    NotDistinct,
}

impl Nulls {
    fn label(self) -> &'static str {
        match self {
            Self::Distinct => "nulls-distinct",
            Self::NotDistinct => "nulls-not-distinct",
        }
    }
}

/// A table with a text primary key `id`, nullable integer columns `k`, `p`,
/// `q`, a non-null `owner`, `name`, a counter `v`, and one unique rule over
/// `key` in the given mode.
fn table(nulls: Nulls, key: &[&str]) -> Json {
    let mut t = json!({
        "name": "items",
        "columns": [
            {"name": "id", "notNull": true, "type": "text"},
            {"name": "owner", "type": "int"},
            {"name": "k", "type": "int"},
            {"name": "p", "type": "int"},
            {"name": "q", "type": "int"},
            {"name": "name", "type": "text"},
            {"name": "v", "type": "int"}
        ],
        "primaryKey": ["id"]
    });
    match nulls {
        Nulls::Distinct => t["unique"] = json!([key]),
        Nulls::NotDistinct => t["nullsNotDistinct"] = json!([key]),
    }
    json!([t])
}

fn invariants() -> Json {
    json!([{ "name": "rows", "assert": { "ge": [ { "count": { "table": "items", "where": {} } }, 0 ] } }])
}

fn two(
    iso: &str,
    tables: Json,
    initial: Json,
    a: &'static str,
    b: &'static str,
    args_a: Json,
    args_b: Json,
) -> Model {
    build(
        iso,
        tables,
        json!({ "items": initial }),
        vec![actor("A", a, args_a), actor("B", b, args_b)],
        invariants(),
        json!({}),
        json!({}),
    )
}

const INSERT_COMMIT: &str = "\
begin
insert items set @id = rid, @owner = owner, @k = k, @p = p, @q = q, @name = nm
commit
";

const INSERT_ROLLBACK: &str = "\
begin
insert items set @id = rid, @owner = owner, @k = k, @p = p, @q = q, @name = nm
rollback
";

const INSERT_DO_NOTHING: &str = "\
begin
insert items set @id = rid, @owner = owner, @k = k, @p = p, @q = q, @name = nm on conflict do nothing into n
commit
";

const UPSERT_K: &str = "\
begin
insert items set @id = rid, @k = k, @v = 1 on conflict (@k) do update set @v = @v + 1 into n
commit
";

const UPDATE_K_NULL: &str = "\
begin
update items where @id == rid set @k = null into n
commit
";

/// Arguments of one inserting actor.
fn row(rid: &str, owner: Option<i64>, k: Option<i64>, p: Option<i64>, q: Option<i64>) -> Json {
    json!({"rid": rid, "owner": owner, "k": k, "p": p, "q": q, "nm": "x"})
}

/// Make the first INSERT of actor `actor` a multi-row INSERT: `extra` rows
/// (column → value) after the one the program writes, as Drizzle's
/// `.values([...])` produces it (the scenario language has no syntax for it).
fn add_rows(model: &mut Model, actor: usize, extra: &[&[(&str, Value)]]) {
    let step = model.actors[actor].codes[0]
        .iter_mut()
        .find(|s| {
            matches!(
                &s.op,
                Op::Db { stmt } if matches!(stmt.kind, StmtKind::Insert { .. })
            )
        })
        .expect("an insert");
    let Op::Db { stmt } = &mut step.op else {
        unreachable!()
    };
    let StmtKind::Insert { more_rows, .. } = &mut stmt.kind else {
        unreachable!()
    };
    for r in extra {
        more_rows.push(
            r.iter()
                .map(|(c, v)| ((*c).to_owned(), Expr::Lit(v.clone())))
                .collect(),
        );
    }
}

/// Every workload in both NULL modes at every isolation level.
fn configurations() -> Vec<(String, Nulls, Model)> {
    let mut out = Vec::new();
    for nulls in [Nulls::Distinct, Nulls::NotDistinct] {
        for iso in ISOLATIONS {
            let label = |w: &str| format!("{w}/{}/{iso}", nulls.label());
            let single = || table(nulls, &["k"]);
            let composite = || table(nulls, &["owner", "p", "name"]);
            let both = || table(nulls, &["p", "q"]);
            // Single nullable column: two NULLs.
            out.push((
                label("single-null-vs-null"),
                nulls,
                two(
                    iso,
                    single(),
                    json!([]),
                    INSERT_COMMIT,
                    INSERT_COMMIT,
                    row("a", None, None, None, None),
                    row("b", None, None, None, None),
                ),
            ));
            // Distinct non-null values: never a conflict.
            out.push((
                label("single-distinct-values"),
                nulls,
                two(
                    iso,
                    single(),
                    json!([]),
                    INSERT_COMMIT,
                    INSERT_COMMIT,
                    row("a", None, Some(1), None, None),
                    row("b", None, Some(2), None, None),
                ),
            ));
            // The same non-null value: a conflict in both modes.
            out.push((
                label("single-same-value"),
                nulls,
                two(
                    iso,
                    single(),
                    json!([]),
                    INSERT_COMMIT,
                    INSERT_COMMIT,
                    row("a", None, Some(1), None, None),
                    row("b", None, Some(1), None, None),
                ),
            ));
            // A committed NULL in the initial state, then two more.
            out.push((
                label("single-repeated-nulls"),
                nulls,
                two(
                    iso,
                    single(),
                    json!([{"id": "z", "k": null}]),
                    INSERT_COMMIT,
                    INSERT_DO_NOTHING,
                    row("a", None, None, None, None),
                    row("b", None, None, None, None),
                ),
            ));
            // Composite key, one NULL column.
            out.push((
                label("composite-one-null"),
                nulls,
                two(
                    iso,
                    composite(),
                    json!([]),
                    INSERT_COMMIT,
                    INSERT_COMMIT,
                    row("a", Some(1), None, None, None),
                    row("b", Some(1), None, None, None),
                ),
            ));
            // Composite key, NULL in the same column but another owner.
            out.push((
                label("composite-one-null-other-owner"),
                nulls,
                two(
                    iso,
                    composite(),
                    json!([]),
                    INSERT_COMMIT,
                    INSERT_COMMIT,
                    row("a", Some(1), None, None, None),
                    row("b", Some(2), None, None, None),
                ),
            ));
            // Composite key, both columns NULL.
            out.push((
                label("composite-both-null"),
                nulls,
                two(
                    iso,
                    both(),
                    json!([]),
                    INSERT_COMMIT,
                    INSERT_COMMIT,
                    row("a", None, None, None, None),
                    row("b", None, None, None, None),
                ),
            ));
            // Composite key: NULL in different columns never matches.
            out.push((
                label("composite-null-in-other-column"),
                nulls,
                two(
                    iso,
                    both(),
                    json!([]),
                    INSERT_COMMIT,
                    INSERT_COMMIT,
                    row("a", None, None, Some(1), None),
                    row("b", None, None, None, Some(1)),
                ),
            ));
            // The first inserter rolls back: the waiter proceeds.
            out.push((
                label("rollback-then-insert"),
                nulls,
                two(
                    iso,
                    single(),
                    json!([]),
                    INSERT_ROLLBACK,
                    INSERT_COMMIT,
                    row("a", None, None, None, None),
                    row("b", None, None, None, None),
                ),
            ));
            // ON CONFLICT DO NOTHING (every unique index is an arbiter).
            out.push((
                label("do-nothing"),
                nulls,
                two(
                    iso,
                    single(),
                    json!([]),
                    INSERT_DO_NOTHING,
                    INSERT_DO_NOTHING,
                    row("a", None, None, None, None),
                    row("b", None, None, None, None),
                ),
            ));
            // ON CONFLICT (k) DO UPDATE: the arbiter inferred from `k`.
            out.push((
                label("do-update"),
                nulls,
                two(
                    iso,
                    single(),
                    json!([]),
                    UPSERT_K,
                    UPSERT_K,
                    json!({"rid": "a", "k": null}),
                    json!({"rid": "b", "k": null}),
                ),
            ));
            // ON CONFLICT (k) DO UPDATE against a committed NULL row.
            out.push((
                label("do-update-existing-null"),
                nulls,
                two(
                    iso,
                    single(),
                    json!([{"id": "z", "k": null, "v": 10}]),
                    UPSERT_K,
                    UPSERT_K,
                    json!({"rid": "a", "k": null}),
                    json!({"rid": "b", "k": null}),
                ),
            ));
            // UPDATE that sets the key to NULL in two rows.
            out.push((
                label("update-to-null"),
                nulls,
                two(
                    iso,
                    single(),
                    json!([{"id": "a", "k": 1}, {"id": "b", "k": 2}]),
                    UPDATE_K_NULL,
                    UPDATE_K_NULL,
                    json!({"rid": "a"}),
                    json!({"rid": "b"}),
                ),
            ));
            // A multi-row INSERT with two NULL keys in one statement, and a
            // concurrent single-row NULL insert.
            let mut m = two(
                iso,
                single(),
                json!([]),
                INSERT_COMMIT,
                INSERT_COMMIT,
                row("a", None, None, None, None),
                row("b", None, None, None, None),
            );
            add_rows(
                &mut m,
                0,
                &[&[("id", Value::Str("a2".into())), ("k", Value::Null)]],
            );
            out.push((label("multi-row-two-nulls"), nulls, m));
            // The same with ON CONFLICT DO NOTHING.
            let mut m = two(
                iso,
                single(),
                json!([]),
                INSERT_DO_NOTHING,
                INSERT_DO_NOTHING,
                row("a", None, None, None, None),
                row("b", None, None, None, None),
            );
            add_rows(
                &mut m,
                0,
                &[&[("id", Value::Str("a2".into())), ("k", Value::Null)]],
            );
            out.push((label("multi-row-two-nulls-do-nothing"), nulls, m));
        }
    }
    out
}

#[test]
fn nulls_distinct_and_not_distinct_agree_with_postgresql() {
    let Some(mut env) = env_or_skip() else { return };
    eprintln!("PostgreSQL: {}", env.version());
    let configs = configurations();
    let mut bad = Vec::new();
    let mut replays = 0;
    let mut nnd_23505 = 0;
    let mut nd_23505_on_null = 0;
    let mut printed_ddl = BTreeSet::new();
    for (label, nulls, model) in &configs {
        let ddl_text = ddl(model).unwrap();
        if printed_ddl.insert(ddl_text.clone()) {
            eprintln!("DDL ({}): {}", nulls.label(), ddl_text.trim());
        }
        let (all, truncated) = enumerate_schedules(model, 20_000);
        assert!(!truncated, "{label}: enumeration truncated");
        assert!(!all.is_empty(), "{label}: no schedules");
        let db = env
            .database(model)
            .unwrap_or_else(|e| panic!("{label}: {e}"));
        // Workloads whose conflicting keys contain a NULL.
        let workload = label.split('/').next().unwrap_or_default();
        let null_key = !matches!(workload, "single-same-value" | "single-distinct-values");
        let mut outcomes: BTreeSet<String> = BTreeSet::new();
        let mut sql: BTreeSet<String> = BTreeSet::new();
        let mut mismatches = 0;
        for schedule in &all {
            replays += 1;
            match run_schedule(&env, &db, model, schedule) {
                Ok(run) => {
                    for o in &run.observations {
                        outcomes.insert(format!("{} {}", o.actor, o.observed));
                        sql.insert(o.sql.clone());
                    }
                    if run.sqlstates.iter().any(|c| c == "23505") {
                        match nulls {
                            Nulls::NotDistinct => nnd_23505 += 1,
                            Nulls::Distinct if null_key => nd_23505_on_null += 1,
                            Nulls::Distinct => {}
                        }
                    }
                    if !(run.agree && run.tables_agree) {
                        mismatches += 1;
                        bad.push(format!(
                            "{label}: {:?}\n   {}\n   tables: model {:?} postgres {:?}",
                            schedule
                                .iter()
                                .map(|t| format!("{:?}{}.{}", t.kind, t.actor, t.thread))
                                .collect::<Vec<_>>(),
                            run.observations
                                .iter()
                                .filter(|o| !o.agree)
                                .map(|o| format!(
                                    "#{} {}: {} → predicted {} observed {}",
                                    o.transition, o.actor, o.sql, o.predicted, o.observed
                                ))
                                .collect::<Vec<_>>()
                                .join("\n   "),
                            run.model_tables,
                            run.postgres_tables
                        ));
                    }
                }
                Err(e) => {
                    mismatches += 1;
                    bad.push(format!("{label}: infrastructure error: {e}"));
                }
            }
        }
        eprintln!(
            "{label}: {} schedule(s), {mismatches} mismatch(es); outcomes {:?}",
            all.len(),
            outcomes
        );
        for s in sql {
            eprintln!("   SQL: {s}");
        }
    }
    eprintln!(
        "{} configuration(s), {replays} schedule replay(s), {} disagreement(s); schedules with 23505: {nnd_23505} under NULLS NOT DISTINCT, {nd_23505_on_null} on NULL-key workloads under NULLS DISTINCT",
        configs.len(),
        bad.len()
    );
    assert!(
        bad.is_empty(),
        "{} disagreement(s):\n{}",
        bad.len(),
        bad.join("\n")
    );
    // The comparison is not vacuous: NULLS NOT DISTINCT produced unique
    // violations PostgreSQL confirmed, NULLS DISTINCT none on a NULL key.
    assert!(nnd_23505 > 0);
    assert_eq!(nd_23505_on_null, 0);
}

// ---------------------------------------------------------------------
// Model-only semantics (no server).
// ---------------------------------------------------------------------

fn one_null_key_invariant() -> Json {
    json!([{ "name": "one-null-key", "assert": { "le": [ { "count": { "table": "items", "where": { "k": null } } }, 1 ] } }])
}

fn race(nulls: Nulls, iso: &str, a: &'static str, b: &'static str) -> Model {
    build(
        iso,
        table(nulls, &["k"]),
        json!({ "items": [] }),
        vec![
            actor("A", a, row("a", None, None, None, None)),
            actor("B", b, row("b", None, None, None, None)),
        ],
        one_null_key_invariant(),
        json!({}),
        json!({}),
    )
}

#[test]
fn two_null_keys_coexist_only_under_nulls_distinct() {
    for iso in ISOLATIONS {
        for program in [INSERT_COMMIT, INSERT_DO_NOTHING] {
            let nd = explore(&race(Nulls::Distinct, iso, program, program));
            assert_eq!(nd.verdict, Verdict::CounterexampleFound, "{iso}");
            let nnd = explore(&race(Nulls::NotDistinct, iso, program, program));
            assert_eq!(nnd.verdict, Verdict::NoCounterexampleWithinBounds, "{iso}");
        }
    }
}

#[test]
fn the_nulls_not_distinct_rule_is_reported_and_has_its_own_ddl() {
    let m = race(
        Nulls::NotDistinct,
        "read-committed",
        INSERT_COMMIT,
        INSERT_COMMIT,
    );
    let t = m.schema.by_name("items").unwrap();
    assert!(t.unique.is_empty());
    assert_eq!(t.nulls_not_distinct, [vec!["k".to_owned()]]);
    let rules = t.key_rules();
    assert_eq!(rules.len(), 2);
    assert!(rules[1].nulls_not_distinct);
    assert_eq!(rules[1].name, "items_k_key");
    assert!(t.arbiter_nulls_not_distinct(&["k".to_owned()]));
    let sql = ddl(&m).unwrap();
    assert!(sql.contains("UNIQUE NULLS NOT DISTINCT (\"k\")"), "{sql}");
    // The NULLS DISTINCT twin keeps the historical DDL and serialization.
    let d = race(
        Nulls::Distinct,
        "read-committed",
        INSERT_COMMIT,
        INSERT_COMMIT,
    );
    let td = d.schema.by_name("items").unwrap();
    assert!(!ddl(&d).unwrap().contains("NULLS NOT DISTINCT"));
    assert!(!td.arbiter_nulls_not_distinct(&["k".to_owned()]));
    let json = serde_json::to_value(td).unwrap();
    assert!(json.get("nullsNotDistinct").is_none(), "{json}");
    let json = serde_json::to_value(t).unwrap();
    assert_eq!(json["nullsNotDistinct"], json!([["k"]]));
}

#[test]
fn an_initial_state_with_two_null_keys_is_refused_only_under_nulls_not_distinct() {
    let load = |nulls| {
        let doc = json!({
            "schemaVersion": 1,
            "name": "w",
            "database": {
                "isolation": "read-committed",
                "tables": table(nulls, &["k"]),
                "initialState": { "items": [{"id": "a", "k": null}, {"id": "b", "k": null}] }
            },
            "actors": [{"name": "A", "function": "work", "program": "return\n"}],
            "invariants": invariants(),
        });
        commitrift_model::scenario::load_str(
            &doc.to_string(),
            std::path::Path::new("."),
            &commitrift_model::provider::NoProvider,
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    };
    assert!(load(Nulls::Distinct).is_ok());
    let err = load(Nulls::NotDistinct).unwrap_err();
    assert!(
        err.contains("duplicate key violates `items_k_key`"),
        "{err}"
    );
}

#[test]
fn a_declared_rule_over_an_unknown_column_is_refused() {
    let doc = json!({
        "schemaVersion": 1,
        "name": "w",
        "database": {
            "isolation": "read-committed",
            "tables": [{
                "name": "items",
                "columns": [{"name": "id", "notNull": true, "type": "text"}],
                "primaryKey": ["id"],
                "nullsNotDistinct": [["nope"]]
            }],
            "initialState": {}
        },
        "actors": [{"name": "A", "function": "work", "program": "return\n"}],
        "invariants": invariants(),
    });
    let err = commitrift_model::scenario::load_str(
        &doc.to_string(),
        std::path::Path::new("."),
        &commitrift_model::provider::NoProvider,
    )
    .err()
    .unwrap()
    .to_string();
    assert!(err.contains("unknown column `nope`"), "{err}");
}

/// Footprints: inserts of equal NULL keys are dependent under NULLS NOT
/// DISTINCT (and independent under NULLS DISTINCT). Every reduction agrees
/// with the unreduced search on every workload, and the independence
/// relation is sound on every reachable state (`audit_independence`).
#[test]
fn reductions_agree_and_independence_is_sound_on_every_workload() {
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
    let mut compared = 0;
    let configs = configurations();
    for (label, _, m) in &configs {
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
    assert_eq!(compared, configs.len() * algs.len());
}

//! Phase 14 Track D: Drizzle multi-row inserts, predicates (`inArray`,
//! `notInArray`, `not`, `or`, `isNull`, conditional predicates, `undefined`
//! operands), clock reads (`new Date()`, ``sql`now()` ``, `defaultNow()`,
//! `$onUpdate`) and `jsonb` columns, extracted from
//! `fixtures/phase14/drizzle-app`; the SQL the pinned Drizzle generates
//! compared on PostgreSQL with what the model extracts and the oracle
//! renders; earlier scenarios keep their reports and state spaces.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use midwrite_analyzer::TsActorProvider;
use midwrite_model::ir::{OnConflict, Op, StmtKind};
use midwrite_model::provider::{ActorProvider, ExtractRequest, Extracted, TableBinding};
use midwrite_model::value::{Builtin, Expr, Value};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/phase14/drizzle-app")
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tables() -> Vec<TableBinding> {
    let t = |identifier: &str, table: &str, columns: &[&str]| TableBinding {
        identifier: identifier.to_owned(),
        table: table.to_owned(),
        columns: columns.iter().map(|c| (*c).to_owned()).collect(),
        source: None,
    };
    vec![
        t("products", "products", &["sku", "name", "stock"]),
        t("orders", "orders", &["id", "customer", "createdAt"]),
        t(
            "orderItems",
            "order_items",
            &["id", "orderId", "sku", "qty"],
        ),
        t("postTags", "post_tags", &["id", "postId", "tag"]),
        t(
            "comments",
            "comments",
            &["id", "parentId", "body", "status"],
        ),
        t(
            "users",
            "users",
            &["id", "email", "welcomedAt", "updatedAt"],
        ),
        t("counters", "counters", &["name", "hits", "updatedAt"]),
        t("settings", "settings", &["userId", "prefs"]),
    ]
}

fn extract(selector: &str, arguments: Vec<Value>) -> Extracted {
    let root = fixture();
    TsActorProvider::new(&root)
        .extract(&ExtractRequest {
            root: root.clone(),
            file: String::new(),
            function: String::new(),
            arguments,
            tables: tables(),
            externals: Vec::new(),
            transaction_retries: Vec::new(),
            max_inline_depth: 3,
            selector: Some(selector.to_owned()),
            contract: Some(Default::default()),
            replicas: false,
            replica_factories: Vec::new(),
            return_values: false,
            js_values: false,
        })
        .unwrap_or_else(|e| panic!("{selector}: {e}"))
}

fn ops(x: &Extracted) -> Vec<Op> {
    x.codes.iter().flatten().map(|s| s.op.clone()).collect()
}

fn statements(x: &Extracted) -> Vec<StmtKind> {
    ops(x)
        .into_iter()
        .filter_map(|op| match op {
            Op::Db { stmt } => Some(stmt.kind),
            _ => None,
        })
        .collect()
}

fn unknowns(x: &Extracted) -> Vec<String> {
    ops(x)
        .into_iter()
        .filter_map(|op| match op {
            Op::Unknown { detail, .. } => Some(detail),
            _ => None,
        })
        .collect()
}

fn clock(v: &Expr) -> Option<midwrite_model::value::GenId> {
    match v {
        Expr::Lit(Value::Gen(g)) if midwrite_model::db::is_clock(g) => Some(*g),
        _ => None,
    }
}

// ---------------------------------------------------------------------
// D1: multi-row inserts
// ---------------------------------------------------------------------

#[test]
fn values_with_an_array_of_objects_is_one_multi_row_insert() {
    let x = extract(
        "src/services/orders.ts#placeOrder",
        vec![
            Value::Int(1),
            Value::str("c"),
            Value::str("a"),
            Value::str("b"),
        ],
    );
    assert!(unknowns(&x).is_empty(), "{:?}", unknowns(&x));
    let lines = statements(&x)
        .into_iter()
        .find(|k| k.table() == "order_items")
        .expect("the order lines");
    let StmtKind::Insert {
        values,
        more_rows,
        returning,
        on_conflict,
        ..
    } = lines
    else {
        panic!("{lines:?}")
    };
    assert_eq!(on_conflict, OnConflict::Error);
    assert!(
        returning.is_some(),
        "`.returning()` binds the array of rows"
    );
    assert_eq!(more_rows.len(), 1);
    let cols = |r: &[(String, Expr)]| r.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>();
    assert_eq!(cols(&values), vec!["orderId", "sku", "qty"]);
    assert_eq!(cols(&more_rows[0]), vec!["orderId", "sku", "qty"]);
    assert_eq!(more_rows[0][2].1, Expr::lit(2));

    let x = extract(
        "src/services/orders.ts#addTags",
        vec![Value::Int(1), Value::str("x"), Value::str("y")],
    );
    let stmts = statements(&x);
    let [
        StmtKind::Insert {
            on_conflict,
            returning_columns,
            more_rows,
            ..
        },
    ] = stmts.as_slice()
    else {
        panic!("{:?}", statements(&x))
    };
    assert_eq!(*on_conflict, OnConflict::DoNothing);
    assert_eq!(returning_columns.as_deref(), Some(&["tag".to_owned()][..]));
    assert_eq!(more_rows.len(), 1);

    // A multi-row upsert: the `set` reads the clock once per statement.
    let x = extract(
        "src/services/orders.ts#bumpCounters",
        vec![Value::str("a"), Value::str("b")],
    );
    let stmts = statements(&x);
    let [
        StmtKind::Insert {
            upsert: Some(u),
            more_rows,
            ..
        },
    ] = stmts.as_slice()
    else {
        panic!("{:?}", statements(&x))
    };
    assert_eq!(more_rows.len(), 1);
    let stamp = u
        .set
        .iter()
        .find(|(c, _)| c == "updatedAt")
        .expect("updatedAt");
    assert_eq!(
        clock(&stamp.1).map(|g| g.generator),
        Some(midwrite_model::db::CLOCK_APP)
    );
}

#[test]
fn multi_row_shapes_the_model_does_not_represent_stay_unknown() {
    let root = fixture();
    let dir = tempdir("shapes");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    for f in ["src/db", "node_modules"] {
        let _ = std::os::unix::fs::symlink(root.join(f), dir.join(f));
    }
    std::fs::write(
        dir.join("src/shapes.ts"),
        r#"import { db } from "./db/client.js";
import { postTags } from "./db/schema.js";
export async function empty() { await db.insert(postTags).values([]); }
export async function spread(rows: any[]) { await db.insert(postTags).values([...rows]); }
export async function nine() {
  await db.insert(postTags).values([
    { postId: 1, tag: "a" }, { postId: 1, tag: "b" }, { postId: 1, tag: "c" },
    { postId: 1, tag: "d" }, { postId: 1, tag: "e" }, { postId: 1, tag: "f" },
    { postId: 1, tag: "g" }, { postId: 1, tag: "h" }, { postId: 1, tag: "i" },
  ]);
}
export async function aliased() {
  return await db.insert(postTags).values([{ postId: 1, tag: "a" }, { postId: 1, tag: "b" }])
    .returning({ name: postTags.tag });
}
"#,
    )
    .unwrap();
    for (f, why) in [
        ("empty", "values([])"),
        ("spread", "spread"),
        ("nine", "exceeds the bound"),
        ("aliased", "different alias"),
    ] {
        let x = TsActorProvider::new(&dir)
            .extract(&ExtractRequest {
                root: dir.clone(),
                file: String::new(),
                function: String::new(),
                arguments: vec![Value::Null],
                tables: tables(),
                externals: Vec::new(),
                transaction_retries: Vec::new(),
                max_inline_depth: 3,
                selector: Some(format!("src/shapes.ts#{f}")),
                contract: Some(Default::default()),
                replicas: false,
                replica_factories: Vec::new(),
                return_values: false,
                js_values: false,
            })
            .unwrap();
        let u = unknowns(&x);
        assert!(u.iter().any(|d| d.contains(why)), "{f}: {u:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn tempdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("midwrite-phase14-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ---------------------------------------------------------------------
// D4: predicates
// ---------------------------------------------------------------------

fn delete_pred(x: &Extracted) -> Expr {
    statements(x)
        .into_iter()
        .find_map(|k| match k {
            StmtKind::Delete { pred, .. } | StmtKind::Update { pred, .. } => Some(pred),
            _ => None,
        })
        .expect("a predicate")
}

#[test]
fn in_array_and_conditional_predicates_are_lowered_exactly() {
    let x = extract(
        "src/services/comments.ts#hideMany",
        vec![Value::List(vec![Value::Int(1), Value::Int(2)])],
    );
    assert!(unknowns(&x).is_empty(), "{:?}", unknowns(&x));
    let pred = delete_pred(&x);
    let text = pred.render();
    assert!(text.contains("in(@id, ids)"), "{text}");
    assert!(text.contains("@status is null"), "{text}");

    // `parentId === null ? isNull(...) : eq(...)`: the test is a
    // JavaScript condition evaluated before the statement; the operands of
    // the branch not taken are not evaluated.
    let x = extract(
        "src/services/comments.ts#prune",
        vec![Value::Null, Value::List(vec![Value::Int(2)])],
    );
    assert!(unknowns(&x).is_empty(), "{:?}", unknowns(&x));
    let all = ops(&x);
    assert!(all.iter().any(|op| matches!(op, Op::JumpIf { .. })));
    let which = all
        .iter()
        .find_map(|op| match op {
            Op::Let {
                local,
                expr: Expr::Not(_),
            } if local.contains("which") => Some(local.clone()),
            _ => None,
        })
        .expect("the test is a local");
    assert!(delete_pred(&x).render().contains(&which));

    // `undefined` operands of and()/or() are dropped; and() of nothing
    // selects every row.
    let probe = |f: &str| delete_pred(&extract(&format!("src/services/probes.ts#{f}"), vec![]));
    assert!(probe("andWithUndefined").render().contains("true"));
    assert!(probe("orWithUndefined").render().contains("false"));
    assert_eq!(probe("andOfNothing"), Expr::lit(true));
    let not_in = probe("notInArrayEmpty");
    assert!(
        matches!(&not_in, Expr::Not(inner) if matches!(&**inner, Expr::Call { func: Builtin::InList, .. }))
    );
}

// ---------------------------------------------------------------------
// D2: clock reads
// ---------------------------------------------------------------------

#[test]
fn clock_reads_in_statement_arguments_are_per_execution_placeholders() {
    let x = extract("src/services/users.ts#welcomeOnce", vec![Value::Int(9)]);
    assert!(unknowns(&x).is_empty(), "{:?}", unknowns(&x));
    let stmts = statements(&x);
    let [StmtKind::Update { set, pred, .. }] = stmts.as_slice() else {
        panic!("{:?}", statements(&x))
    };
    assert!(clock(&set[0].1).is_some(), "{set:?}");
    assert!(pred.render().contains("@welcomedAt is null"));
    // An insert value and an upsert `set`: two occurrences, two readings.
    let x = extract("src/services/users.ts#touch", vec![Value::str("a")]);
    let stmts = statements(&x);
    let [
        StmtKind::Insert {
            values,
            upsert: Some(u),
            ..
        },
    ] = stmts.as_slice()
    else {
        panic!("{:?}", statements(&x))
    };
    let a = values
        .iter()
        .find_map(|(c, e)| (c == "updatedAt").then(|| clock(e)))
        .flatten();
    let b = u
        .set
        .iter()
        .find_map(|(c, e)| (c == "updatedAt").then(|| clock(e)))
        .flatten();
    assert!(a.is_some() && b.is_some() && a != b, "{a:?} {b:?}");
}

#[test]
fn the_schema_records_on_update_callbacks_and_modes() {
    let schema = midwrite_analyzer::schema::extract_schema(&fixture(), "src/db/schema.ts").unwrap();
    let users = schema.table("users").unwrap();
    let updated = users.column("updatedAt").unwrap();
    assert_eq!(updated.on_update.as_deref(), Some("() => new Date()"));
    assert!(
        !users.unsupported.iter().any(|u| u.contains("updatedAt")),
        "a clock callback is recorded, not unsupported: {:?}",
        users.unsupported
    );
    let orders = schema.table("orders").unwrap();
    assert_eq!(
        orders.column("createdAt").unwrap().sql_type,
        "timestamp with time zone"
    );
    let probe = midwrite_analyzer::schema::extract_schema_source(
        "s.ts",
        r#"import { pgTable, timestamp, date, integer } from "drizzle-orm/pg-core";
export const t = pgTable("t", { id: integer("id").primaryKey(),
  a: timestamp("a", { mode: "string" }), d: date("d"), e: date("e", { mode: "date" }) });"#,
    )
    .unwrap();
    let t = probe.table("t").unwrap();
    assert_eq!(t.column("a").unwrap().mode.as_deref(), Some("string"));
    assert_eq!(t.column("d").unwrap().mode, None);
    assert_eq!(t.column("e").unwrap().mode.as_deref(), Some("date"));
}

// ---------------------------------------------------------------------
// Scenarios read from the source (features `timestamps`, `foreign-keys`)
// ---------------------------------------------------------------------

struct Provider;

impl ActorProvider for Provider {
    fn extract_schema(
        &self,
        root: &Path,
        file: &str,
    ) -> Result<midwrite_model::schema_facts::ExtractedSchema, String> {
        midwrite_analyzer::schema::extract_schema(root, file)
    }
    fn resolve_reference(&self, root: &Path, file: &str, local: &str) -> Option<(String, String)> {
        TsActorProvider::new(root.to_path_buf()).resolve_reference(root, file, local)
    }
    fn extract(&self, request: &ExtractRequest) -> Result<Extracted, String> {
        TsActorProvider::new(request.root.clone()).extract(request)
    }
}

fn scenario(name: &str) -> midwrite_model::model::Model {
    let path = fixture().join("scenarios").join(name);
    midwrite_model::scenario::load_file(&path, &Provider)
        .unwrap_or_else(|e| panic!("{name}: {e}"))
        .0
}

#[test]
fn source_scenarios_with_timestamps_and_multi_row_inserts_are_checked() {
    use midwrite_model::explore::{Verdict, explore};
    let cases = [
        ("welcome-race.json", false),
        ("welcome-once.json", true),
        ("tags-race.json", true),
        ("place-order.json", true),
    ];
    for (name, holds) in cases {
        let m = scenario(name);
        let r = explore(&m);
        assert!(r.unsupported.is_empty(), "{name}: {:?}", r.unsupported);
        assert_eq!(
            r.verdict == Verdict::NoCounterexampleWithinBounds,
            holds,
            "{name}: {:?}",
            r.verdict
        );
        assert!(
            m.assumptions.iter().any(|a| a.contains("clock-readings"))
                == name.starts_with("welcome")
                || name == "place-order.json",
            "{name}: {:?}",
            m.assumptions
        );
    }
    // `welcomedAt` and `updatedAt` are modeled time columns (`$onUpdate`).
    let m = scenario("welcome-race.json");
    let users = m.schema.by_name("users").unwrap();
    let updated = users
        .columns
        .iter()
        .find(|c| c.name == "updatedAt")
        .unwrap();
    assert_eq!(
        updated.on_update,
        Some(midwrite_model::db::ColumnDefault::Clock)
    );
    // Without the feature the time columns stay not modeled (Phase 13).
    let text = std::fs::read_to_string(fixture().join("scenarios/welcome-race.json")).unwrap();
    let without = text
        .replace("\"features\": [\"timestamps\"],", "")
        .replace(", \"welcomedAt\": null, \"updatedAt\": null", "");
    let (m, _) =
        midwrite_model::scenario::load_str(&without, &fixture().join("scenarios"), &Provider)
            .unwrap_or_else(|e| panic!("{e}"));
    let users = m.schema.by_name("users").unwrap();
    assert!(!users.columns.iter().any(|c| c.name == "welcomedAt"));
}

// ---------------------------------------------------------------------
// D5: the SQL Drizzle generates vs. what the oracle renders
// ---------------------------------------------------------------------

/// Probe name → the fixture function with the same predicate.
const PROBED_PREDICATES: &[(&str, &str, &[Value])] = &[
    ("inArray, non-empty with a null", "inArrayWithNull", &[]),
    ("inArray, empty", "inArrayEmpty", &[]),
    ("notInArray, non-empty", "notInArrayNonEmpty", &[]),
    ("notInArray, empty", "notInArrayEmpty", &[]),
    ("isNull / or / eq", "isNullOrEq", &[]),
    ("not(eq)", "notEq", &[]),
    ("and with an undefined operand", "andWithUndefined", &[]),
    ("or with an undefined operand", "orWithUndefined", &[]),
    ("and() of nothing", "andOfNothing", &[]),
    (
        "conditional predicate (parentId === null)",
        "conditionalParent",
        &[Value::Null],
    ),
    (
        "conditional predicate (parentId === 4)",
        "conditionalParent",
        &[Value::Int(4)],
    ),
];

/// The predicates the extractor produces for the probed Drizzle calls,
/// closed (the locals the code computes before the statement evaluated),
/// as recorded in `probe/extracted-predicates.json`. The model crate's
/// `drizzle_sql_and_the_oracle_rendering_select_the_same_rows` runs these
/// and Drizzle's own SQL (`reports/phase14/drizzle-sql/probe.json`) on
/// PostgreSQL and in the model.
#[test]
fn extracted_probe_predicates_are_the_recorded_ones() {
    let mut current = serde_json::Map::new();
    for (name, function, args) in PROBED_PREDICATES {
        let x = extract(&format!("src/services/probes.ts#{function}"), args.to_vec());
        assert!(unknowns(&x).is_empty(), "{function}: {:?}", unknowns(&x));
        let mut locals: midwrite_model::value::Locals =
            x.params.iter().cloned().zip(args.iter().cloned()).collect();
        for op in ops(&x) {
            if let Op::Let { local, expr } = op
                && let Ok(v) = expr.eval(&locals, None, midwrite_model::value::Mode::Js)
            {
                locals.insert(local, v);
            }
        }
        let closed = delete_pred(&x)
            .close(&locals, midwrite_model::value::Mode::Sql)
            .unwrap();
        current.insert((*name).to_owned(), serde_json::to_value(&closed).unwrap());
    }
    let path = fixture().join("probe/extracted-predicates.json");
    let current = serde_json::Value::Object(current);
    if std::env::var_os("MIDWRITE_WRITE_PROBE_PREDICATES").is_some() {
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&current).unwrap() + "\n",
        )
        .unwrap();
        return;
    }
    let recorded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(current, recorded);
}

// ---------------------------------------------------------------------
// D7: earlier scenarios keep their reports and state spaces
// ---------------------------------------------------------------------

/// Scenarios of Phases 6-13 (declared and extracted programs).
const VERSIONED: &[&str] = &[
    "fixtures/concurrency/scenarios/inventory-oversell.json",
    "fixtures/concurrency/scenarios/inventory-row-lock.json",
    "fixtures/concurrency/scenarios/doctors-write-skew-serializable.json",
    "fixtures/concurrency/scenarios/redeem-check-then-act-unique.json",
    "fixtures/concurrency/scenarios/payment-crash-retry.json",
    "fixtures/phase9/scenarios/ledger-pool-of-two.json",
    "fixtures/phase9/scenarios/ledger-idempotent-commit-held.json",
    "fixtures/phase9/scenarios/ledger-settle-update-answered.json",
    "fixtures/phase13/model/replica-read-your-writes.json",
    "fixtures/phase13/request-app/scenarios/reserve-race.json",
    "fixtures/phase13/request-app/scenarios/claim-atomic.json",
    "fixtures/phase13/request-app/scenarios/signup-crash.json",
    "fixtures/phase7/scenarios/team-app-invite-double-submit.json",
    "fixtures/phase8/scenarios/checkout-crash-after-charge.json",
    "fixtures/phase8/scenarios/accept-crash-while-waiting.json",
    "fixtures/phase8/scenarios/job-concurrent-claims-duplicate.json",
    "fixtures/phase10/scenarios/live-shared-process-lost-commit.json",
];

fn fnv(bytes: &[u8], mut h: u64) -> u64 {
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// (report JSON hash, number of reachable states, hash of the sorted
/// canonical encodings of every reachable state).
fn fingerprint(path: &Path) -> serde_json::Value {
    use midwrite_model::engine::{Recorder, apply, enabled, initial_state};
    let (model, load) = midwrite_model::scenario::load_file(path, &Provider)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let report = midwrite_model::report::check(
        &model,
        &midwrite_model::report::CheckOptions {
            validate_postgres: false,
            include_timing: false,
            scenario_file: None,
            source_revision: None,
            tool_version: "versioning-test".to_owned(),
            load,
            source: None,
            profile_storage: false,
        },
    );
    let json = report.to_json();
    let mut seen: BTreeSet<Vec<u8>> = BTreeSet::new();
    let mut stack = vec![initial_state(&model)];
    while let Some(s) = stack.pop() {
        if seen.len() >= 200_000 || !seen.insert(s.canonical(&model)) {
            continue;
        }
        for t in enabled(&model, &s) {
            if let Ok(next) = apply(&model, &s, t, &mut Recorder::off()) {
                stack.push(next.state);
            }
        }
    }
    let mut h = 0xcbf2_9ce4_8422_2325;
    for k in &seen {
        h = fnv(&(k.len() as u64).to_le_bytes(), h);
        h = fnv(k, h);
    }
    serde_json::json!({
        "report": format!("{:016x}", fnv(json.as_bytes(), 0xcbf2_9ce4_8422_2325)),
        "verdict": report.verdict_str(),
        "states": seen.len(),
        "stateBytes": format!("{h:016x}"),
    })
}

#[test]
fn earlier_scenarios_keep_their_reports_and_state_bytes() {
    let golden_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/phase14/versioning-golden.json");
    let mut current = serde_json::Map::new();
    for rel in VERSIONED {
        current.insert((*rel).to_owned(), fingerprint(&repo().join(rel)));
    }
    let current = serde_json::Value::Object(current);
    if std::env::var_os("MIDWRITE_WRITE_VERSIONING_GOLDEN").is_some() {
        std::fs::create_dir_all(golden_path.parent().unwrap()).unwrap();
        std::fs::write(
            &golden_path,
            serde_json::to_string_pretty(&current).unwrap(),
        )
        .unwrap();
        return;
    }
    let golden: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&golden_path).unwrap()).unwrap();
    for rel in VERSIONED {
        assert_eq!(current[*rel], golden[*rel], "{rel}: changed since Phase 13");
    }
}

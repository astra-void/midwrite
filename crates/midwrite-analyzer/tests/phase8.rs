//! Phase 8: Drizzle schema extraction (no execution), scenarios whose
//! tables are read from source (conflicts, visible overrides, fact
//! origins), the transaction-retry helper binding, and the shared semantic
//! facts the frontends now use.

use std::path::{Path, PathBuf};

use midwrite_analyzer::TsActorProvider;
use midwrite_analyzer::schema::{
    SchemaDefault, SchemaLiteral, extract_schema, extract_schema_source,
};
use midwrite_model::db::ColumnDefault;
use midwrite_model::ir::Op;
use midwrite_model::scenario::load_str;
use midwrite_model::schema_facts::FactOrigin;
use midwrite_model::value::Value;
use serde_json::{Value as Json, json};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn every_supported_schema_fact_is_extracted_with_its_line() {
    let s = extract_schema(&repo().join("fixtures/phase8/schemas"), "src/schema.ts").unwrap();
    let accounts = s.table("accounts").unwrap();
    assert_eq!(accounts.primary_key, ["id"]);
    let id = accounts.column("id").unwrap();
    assert!(
        matches!(&id.default, Some(SchemaDefault::Sequence { generated }) if generated == "generatedAlwaysAsIdentity")
    );
    let email = accounts.column("email").unwrap();
    assert_eq!(
        (email.sql_type.as_str(), email.not_null),
        ("varchar(255)", true)
    );
    let role = accounts.column("role").unwrap();
    assert_eq!(role.sql_type, "enum member_role");
    assert_eq!(role.enum_values, ["owner", "member", "viewer"]);
    assert_eq!(
        role.default,
        Some(SchemaDefault::Constant {
            value: SchemaLiteral::Text("member".into())
        })
    );
    // Database expression vs. application-side default.
    assert!(
        matches!(&accounts.column("createdAt").unwrap().default, Some(SchemaDefault::Expression { text }) if text == "now()")
    );
    assert!(matches!(
        &accounts.column("token").unwrap().default,
        Some(SchemaDefault::Application { .. })
    ));
    assert_eq!(
        accounts.column("createdAt").unwrap().sql_type,
        "timestamp with time zone"
    );
    // Single-column uniques, one nullable.
    let uniques: Vec<(Vec<String>, bool)> = accounts
        .unique
        .iter()
        .map(|u| {
            (
                u.columns.clone(),
                accounts.column_by_sql(&u.columns[0]).unwrap().not_null,
            )
        })
        .collect();
    assert_eq!(
        uniques,
        [
            (vec!["email".to_owned()], true),
            (vec!["external_id".to_owned()], false)
        ]
    );
    assert_eq!(accounts.line, 24);

    let m = s.table("memberships").unwrap();
    assert_eq!(m.primary_key, ["account_id", "team_id"]);
    let composite = m
        .unique
        .iter()
        .find(|u| u.name.as_deref() == Some("memberships_team_seat"))
        .unwrap();
    assert_eq!(composite.columns, ["team_id", "seat"]);
    assert!(composite.predicate.is_none());
    // The partial index stays partial.
    let partial = m
        .unique
        .iter()
        .find(|u| u.name.as_deref() == Some("memberships_active_invite"))
        .unwrap();
    assert_eq!(partial.kind, "index");
    assert!(partial.predicate.as_deref().unwrap().contains("archived"));
    let nnd = m
        .unique
        .iter()
        .find(|u| u.name.as_deref() == Some("memberships_invite_any"))
        .unwrap();
    assert!(nnd.nulls_not_distinct);
    assert_eq!(m.indexes, ["memberships_team_idx"]);
    assert_eq!(m.checks.len(), 1);
    assert_eq!(m.foreign_keys[0].columns, ["account_id"]);
    assert_eq!(m.foreign_keys[0].table, "accounts");

    // pgSchema(...).table(...)
    let events = s.table("events").unwrap();
    assert_eq!(events.schema.as_deref(), Some("audit"));
    assert_eq!(events.foreign_keys[0].foreign_columns, ["id"]);
    // A column without an explicit SQL name: reported, not silently assumed.
    assert!(events.unsupported.iter().any(|u| u.contains("casing")));
    // Dynamic table names and spread columns are refused, not guessed.
    assert!(s.table("dynamic").is_none());
    assert!(s.unsupported.iter().any(|u| u.contains("not a literal")));
    let notes = s.table("notes").unwrap();
    assert!(notes.unsupported.iter().any(|u| u.contains("spread")));
    assert!(notes.column("updatedAt").is_none());
}

#[test]
fn schema_extraction_never_executes_the_module() {
    // A module that would throw (or do anything) when executed is only
    // read: the extractor never runs it.
    let source = r#"
        import { pgTable, integer } from "drizzle-orm/pg-core";
        throw new Error("executed!");
        export const t = pgTable("t", { id: integer("id").primaryKey() });
    "#;
    let s = extract_schema_source("x.ts", source).unwrap();
    assert_eq!(s.tables[0].primary_key, ["id"]);
    // Not a schema module, not a parse error either: nothing extracted.
    assert!(
        extract_schema_source("y.ts", "export const x = 1;")
            .unwrap()
            .tables
            .is_empty()
    );
    assert!(extract_schema_source("z.ts", "export const = ;").is_err());
}

#[test]
fn a_composite_primary_key_and_an_index_object_form_are_understood() {
    let source = r#"
        import { pgTable, integer, text, primaryKey, uniqueIndex } from "drizzle-orm/pg-core";
        export const links = pgTable("links", {
            a: integer("a").notNull(),
            b: integer("b").notNull(),
            label: text("label"),
        }, (t) => ({
            pk: primaryKey(t.a, t.b),
            byLabel: uniqueIndex("links_label").on(t.label),
        }));
    "#;
    let s = extract_schema_source("l.ts", source).unwrap();
    let t = &s.tables[0];
    assert_eq!(t.primary_key, ["a", "b"]);
    assert_eq!(t.unique[0].columns, ["label"]);
}

fn shop() -> PathBuf {
    repo().join("fixtures/phase8/shop-app")
}

fn scenario_with_tables(tables: Json) -> String {
    json!({
        "schemaVersion": 1,
        "name": "linked",
        "extraction": {"root": ".", "schema": "src/lib/db/schema.ts"},
        "database": {"tables": tables, "initialState": {}},
        "actors": [{"name": "A", "function": "a", "program": "return\n"}],
        "invariants": [{"name": "trivial", "assert": {"le": [1, 1]}}]
    })
    .to_string()
}

fn load(
    tables: Json,
) -> Result<
    (
        midwrite_model::model::Model,
        midwrite_model::scenario::LoadReport,
    ),
    String,
> {
    load_str(
        &scenario_with_tables(tables),
        &shop(),
        &TsActorProvider::new(shop()),
    )
    .map_err(|e| e.to_string())
}

#[test]
fn a_table_read_from_source_carries_every_fact_with_its_origin() {
    let (model, report) = load(json!([{"from": "orders"}, {"from": "jobEffects"}])).unwrap();
    let orders = model.schema.by_name("orders").unwrap();
    // updatedAt (timestamp, DEFAULT now()) is not modeled: excluded, said so.
    let names: Vec<&str> = orders.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["id", "customerEmail", "amount", "status"]);
    assert_eq!(orders.primary_key, ["id"]);
    let status = orders.columns.iter().find(|c| c.name == "status").unwrap();
    assert_eq!(
        status.default,
        Some(ColumnDefault::Value(Value::Str("unpaid".into())))
    );
    let email = orders
        .columns
        .iter()
        .find(|c| c.name == "customerEmail")
        .unwrap();
    assert_eq!(email.sql_name.as_deref(), Some("customer_email"));
    let effects = model.schema.by_name("job_effects").unwrap();
    assert_eq!(
        effects.unique,
        [vec!["jobId".to_owned(), "effect".to_owned()]]
    );
    assert!(matches!(
        effects.columns[0].default,
        Some(ColumnDefault::Sequence(_))
    ));
    let facts = &report.schema[0].facts;
    assert!(
        facts
            .iter()
            .any(|f| f.origin == FactOrigin::NotModeled && f.fact.contains("updated_at"))
    );
    assert!(
        facts
            .iter()
            .all(|f| f.source.starts_with("src/lib/db/schema.ts:")
                || f.origin != FactOrigin::ExtractedFromSource)
    );
    assert_eq!(report.schema[0].binding.as_deref(), Some("orders"));
}

#[test]
fn a_declared_fact_that_contradicts_the_source_is_refused_unless_overridden() {
    // The scenario says nullable; the source says NOT NULL.
    let err = load(json!([{"from": "orders", "columns": [
        {"name": "id", "notNull": true}, {"name": "status", "notNull": false}
    ]}]))
    .unwrap_err();
    assert!(err.contains("override"), "{err}");
    // Uniqueness the source does not have.
    let err = load(json!([{"from": "jobs", "unique": [["orderId"]]}])).unwrap_err();
    assert!(err.contains("unique"), "{err}");
    // The same, as a visible override with a reason.
    let (model, report) = load(json!([{"from": "jobs", "overrides": {
        "reason": "what if the migration added UNIQUE(order_id)?",
        "unique": [["orderId"]]
    }}]))
    .unwrap();
    assert_eq!(
        model.schema.by_name("jobs").unwrap().unique,
        [vec!["orderId".to_owned()]]
    );
    let o = report.schema[0]
        .facts
        .iter()
        .find(|f| f.origin == FactOrigin::ScenarioOverride)
        .unwrap();
    assert!(o.fact.contains("what if the migration added"), "{o:?}");
    // Unknown bindings and unmodelable explicit columns are errors.
    assert!(
        load(json!([{"from": "nope"}]))
            .unwrap_err()
            .contains("no table")
    );
    assert!(
        load(json!([{"from": "orders", "columns": ["updatedAt"]}]))
            .unwrap_err()
            .contains("does not represent")
    );
}

#[test]
fn partial_and_nulls_not_distinct_uniques_are_not_generalized() {
    let root = repo().join("fixtures/phase8/schemas");
    let text = json!({
        "schemaVersion": 1, "name": "edge",
        "extraction": {"root": ".", "schema": "src/schema.ts"},
        "database": {"tables": [{"from": "memberships"}, {"from": "accounts", "columns": ["id", "email", "externalId", "role"]}], "initialState": {}},
        "actors": [{"name": "A", "function": "a", "program": "return\n"}],
        "invariants": [{"name": "trivial", "assert": {"le": [1, 1]}}]
    })
    .to_string();
    let (model, report) = load_str(&text, &root, &TsActorProvider::new(&root)).unwrap();
    let m = model.schema.by_name("memberships").unwrap();
    // Only the global composite unique; NOT the partial index, NOT the
    // NULLS NOT DISTINCT constraint on a nullable column.
    assert_eq!(m.unique, [vec!["teamId".to_owned(), "seat".to_owned()]]);
    let facts: Vec<&str> = report.schema[0]
        .facts
        .iter()
        .map(|f| f.fact.as_str())
        .collect();
    assert!(
        facts
            .iter()
            .any(|f| f.starts_with("partial unique index `memberships_active_invite`"))
    );
    assert!(
        facts
            .iter()
            .any(|f| f.contains("NULLS NOT DISTINCT on nullable"))
    );
    assert!(
        facts
            .iter()
            .any(|f| f.contains("nullable: NULLs never conflict"))
    );
    assert!(facts.iter().any(|f| f.starts_with("CHECK")));
    // Enums are text in the model, with their domain reported as not enforced.
    let a = model.schema.by_name("accounts").unwrap();
    assert_eq!(a.unique.len(), 2);
    assert!(
        report.schema[1]
            .facts
            .iter()
            .any(|f| f.fact.contains("enum domain"))
    );
}

#[test]
fn a_transaction_retry_helper_becomes_a_retried_begin() {
    let root = shop();
    let text = std::fs::read_to_string(
        repo().join("fixtures/phase8/scenarios/inventory-retry-duplicate-notice.json"),
    )
    .unwrap();
    let base = repo().join("fixtures/phase8/scenarios");
    let (model, report) = load_str(&text, &base, &TsActorProvider::new(&root)).unwrap();
    let begin = model.actors[0]
        .codes
        .iter()
        .flatten()
        .find_map(|s| match &s.op {
            Op::Begin { isolation, retry } => Some((*isolation, *retry)),
            _ => None,
        })
        .unwrap();
    assert_eq!(begin.1, 2, "attempts: 3 → two re-executions");
    assert!(begin.0.is_some());
    assert!(
        report.assumptions[0]
            .1
            .iter()
            .any(|a| a.contains("declared contract: `withTransactionRetry`"))
    );
}

#[test]
fn the_retry_helper_without_a_binding_is_an_unknown_call_not_a_transaction() {
    // Without the declared binding the helper's body is inlined and its
    // loop is not modeled: the extraction must say so, not invent a retry.
    let root = shop();
    let text = std::fs::read_to_string(
        repo().join("fixtures/phase8/scenarios/inventory-retry-duplicate-notice.json"),
    )
    .unwrap();
    let mut doc: Json = serde_json::from_str(&text).unwrap();
    doc["extraction"]
        .as_object_mut()
        .unwrap()
        .remove("transactionRetries");
    let base = repo().join("fixtures/phase8/scenarios");
    let (model, _) = load_str(&doc.to_string(), &base, &TsActorProvider::new(&root)).unwrap();
    assert!(
        model.actors[0]
            .codes
            .iter()
            .flatten()
            .any(|s| matches!(s.op, Op::Unknown { .. }))
    );
    assert!(
        !model.actors[0]
            .codes
            .iter()
            .flatten()
            .any(|s| matches!(s.op, Op::Begin { retry, .. } if retry > 0))
    );
}

#[allow(dead_code)]
fn unused(_: &Path) {}

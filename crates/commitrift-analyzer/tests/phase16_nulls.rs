//! Phase 16: the Drizzle 0.45 spellings of `NULLS NOT DISTINCT` and the
//! one that does not exist (`docs/phase16/NULLS-NOT-DISTINCT.md`).

use std::path::PathBuf;

use commitrift_analyzer::TsActorProvider;
use commitrift_analyzer::schema::{extract_schema, extract_schema_source};
use commitrift_model::scenario::load_str;
use commitrift_model::schema_facts::FactOrigin;
use serde_json::json;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn both_drizzle_spellings_are_read_with_their_null_mode() {
    let s = extract_schema(&repo().join("fixtures/phase16/nulls"), "src/db/schema.ts").unwrap();
    let rule = |table: &str| {
        let t = s.table(table).unwrap();
        assert_eq!(t.unique.len(), 1, "{table}");
        (t.unique[0].nulls_not_distinct, t.unique[0].columns.clone())
    };
    assert_eq!(
        rule("tags"),
        (
            false,
            vec!["owner_id".into(), "group_id".into(), "name".into()]
        )
    );
    assert_eq!(
        rule("badges"),
        (
            true,
            vec!["owner_id".into(), "group_id".into(), "name".into()]
        )
    );
    assert_eq!(rule("devices"), (false, vec!["serial_no".into()]));
    assert_eq!(rule("scanners"), (true, vec!["serial_no".into()]));
}

#[test]
fn a_nulls_not_distinct_index_builder_is_not_a_drizzle_method() {
    let source = r#"
import { integer, pgTable, uniqueIndex } from "drizzle-orm/pg-core";
export const t = pgTable("t", { a: integer("a") }, (t) => [uniqueIndex("t_a").on(t.a).nullsNotDistinct()]);
"#;
    let s = extract_schema_source("schema.ts", source).unwrap();
    let t = s.table("t").unwrap();
    assert!(t.unique.is_empty(), "{:?}", t.unique);
    assert!(
        t.unsupported
            .iter()
            .any(|u| u.contains("has no `nullsNotDistinct()`")),
        "{:?}",
        t.unsupported
    );
}

#[test]
fn the_loader_enforces_extracted_rules_and_keeps_distinct_ones_apart() {
    let root = repo().join("fixtures/phase16/nulls");
    let text = json!({
        "schemaVersion": 1, "name": "nulls",
        "extraction": {"root": ".", "schema": "src/db/schema.ts"},
        "database": {"tables": [{"from": "tags"}, {"from": "badges"}, {"from": "devices"}, {"from": "scanners"}], "initialState": {}},
        "actors": [{"name": "A", "function": "a", "program": "return\n"}],
        "invariants": [{"name": "trivial", "assert": {"le": [1, 1]}}]
    })
    .to_string();
    let (model, report) = load_str(&text, &root, &TsActorProvider::new(&root)).unwrap();
    let set = |cols: &[&str]| cols.iter().map(|c| (*c).to_owned()).collect::<Vec<_>>();
    let tags = model.schema.by_name("tags").unwrap();
    assert_eq!(tags.unique, [set(&["ownerId", "groupId", "name"])]);
    assert!(tags.nulls_not_distinct.is_empty());
    let badges = model.schema.by_name("badges").unwrap();
    assert!(badges.unique.is_empty());
    assert_eq!(
        badges.nulls_not_distinct,
        [set(&["ownerId", "groupId", "name"])]
    );
    let scanners = model.schema.by_name("scanners").unwrap();
    assert_eq!(scanners.nulls_not_distinct, [set(&["serialNo"])]);
    let fact = report
        .schema
        .iter()
        .find(|t| t.table == "badges")
        .unwrap()
        .facts
        .iter()
        .find(|f| f.fact.contains("NULLS NOT DISTINCT on nullable"))
        .unwrap();
    assert_eq!(fact.origin, FactOrigin::ExtractedFromSource);
    assert!(
        model
            .assumptions
            .iter()
            .any(|a| a.starts_with("UNIQUE NULLS NOT DISTINCT (PostgreSQL 15+)")),
        "{:?}",
        model.assumptions
    );
    // A declared list must agree with the source.
    let wrong = json!({
        "schemaVersion": 1, "name": "nulls",
        "extraction": {"root": ".", "schema": "src/db/schema.ts"},
        "database": {"tables": [{"from": "badges", "nullsNotDistinct": [["ownerId"]]}], "initialState": {}},
        "actors": [{"name": "A", "function": "a", "program": "return\n"}],
        "invariants": [{"name": "trivial", "assert": {"le": [1, 1]}}]
    })
    .to_string();
    let err = load_str(&wrong, &root, &TsActorProvider::new(&root))
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("NULLS NOT DISTINCT sets"), "{err}");
}

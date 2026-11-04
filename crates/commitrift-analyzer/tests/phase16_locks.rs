//! Phase 16 (Track C): Drizzle predicates held in locals and multi-row
//! selects (`docs/phase16/DRIZZLE-PREDICATES.md`,
//! `docs/phase16/MULTI-ROW-LOCKS.md`).
//!
//! Each case is extracted from a small TypeScript project; where the
//! semantics matter (when a held predicate's operands are evaluated, what a
//! multi-row lock returns) the extracted code is also run in the model.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use commitrift_analyzer::TsActorProvider;
use commitrift_model::engine::{Recorder, apply, enabled, initial_state};
use commitrift_model::ir::{LockMode, ManyRows, Op, StmtKind};
use commitrift_model::provider::{ActorProvider, ExtractRequest, Extracted, TableBinding};
use commitrift_model::scenario::testing::model_from_json;
use commitrift_model::state::Outcome;
use serde_json::json;

static NEXT: AtomicU32 = AtomicU32::new(0);

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "commitrift-phase16-locks-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        for (name, text) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Self { root }
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

const IMPORTS: &str = r#"import { and, asc, desc, eq, inArray, isNull, or, sql } from "drizzle-orm";
import { db } from "./db";
import { accounts, documents } from "./schema";
"#;

fn tables() -> Vec<TableBinding> {
    let table = |name: &str, columns: &[&str]| TableBinding {
        identifier: name.to_owned(),
        table: name.to_owned(),
        columns: columns.iter().map(|c| (*c).to_owned()).collect(),
        source: None,
    };
    vec![
        table("accounts", &["id", "balance"]),
        table("documents", &["id", "spaceId", "folderId", "title"]),
    ]
}

fn extract_js(files: &[(&str, &str)], function: &str, js_values: bool) -> Extracted {
    let project = Project::new(files);
    TsActorProvider::new(&project.root)
        .extract(&ExtractRequest {
            root: project.root.clone(),
            file: "a.ts".to_owned(),
            function: function.to_owned(),
            arguments: Vec::new(),
            tables: tables(),
            externals: Vec::new(),
            transaction_retries: Vec::new(),
            max_inline_depth: 3,
            selector: None,
            contract: None,
            replicas: false,
            replica_factories: Vec::new(),
            return_values: false,
            js_values,
        })
        .unwrap_or_else(|e| panic!("extraction of `{function}` failed: {e}"))
}

fn extract(body: &str, function: &str) -> Extracted {
    extract_js(&[("a.ts", &format!("{IMPORTS}\n{body}"))], function, true)
}

fn unknowns(e: &Extracted) -> Vec<String> {
    e.codes
        .iter()
        .flatten()
        .filter_map(|s| match &s.op {
            Op::Unknown { detail, .. } => Some(detail.clone()),
            _ => None,
        })
        .collect()
}

/// The statements (kind, predicate, lock, multi-row mode) in code order.
fn selects(e: &Extracted) -> Vec<(String, LockMode, Option<ManyRows>)> {
    e.codes[0]
        .iter()
        .filter_map(|s| match &s.op {
            Op::Db { stmt } => match &stmt.kind {
                StmtKind::Select {
                    pred, lock, many, ..
                } => Some((pred.render(), *lock, many.clone())),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Run one actor of the extracted code alone in the model and return its
/// outcome.
fn run(e: &Extracted, arguments: serde_json::Value, rows: serde_json::Value) -> Outcome {
    let arguments: serde_json::Map<String, serde_json::Value> = e
        .params
        .iter()
        .cloned()
        .zip(arguments.as_array().expect("positional").iter().cloned())
        .collect();
    let model = model_from_json(json!({
        "schemaVersion": 1,
        "name": "phase16",
        "features": ["js-values"],
        "database": {
            "tables": [
                { "name": "accounts", "columns": [
                    {"name": "id", "notNull": true, "type": "int"},
                    {"name": "balance", "type": "int"}
                ], "primaryKey": ["id"] },
                { "name": "documents", "columns": [
                    {"name": "id", "notNull": true, "type": "int"},
                    {"name": "spaceId", "type": "int"},
                    {"name": "folderId", "type": "int"},
                    {"name": "title", "type": "text"}
                ], "primaryKey": ["id"] }
            ],
            "initialState": rows
        },
        "actors": [{
            "name": "A",
            "function": "f",
            "codes": serde_json::to_value(&e.codes).unwrap(),
            "arguments": arguments
        }],
        "invariants": [{ "name": "true", "assert": { "eq": [0, 0] } }]
    }));
    let mut s = initial_state(&model);
    while let Some(t) = enabled(&model, &s).into_iter().next() {
        s = apply(&model, &s, t, &mut Recorder::off())
            .expect("supported")
            .state;
    }
    s.actors[0].outcome.clone()
}

// ---------------------------------------------------------------------
// Predicates held in locals
// ---------------------------------------------------------------------

#[test]
fn a_predicate_in_a_const_and_its_aliases_are_used_by_where() {
    let e = extract(
        r#"export async function find(spaceId: number, title: string) {
  const inSpace = eq(documents.spaceId, spaceId);
  const titled = and(inSpace, eq(documents.title, title));
  const same = titled;
  const [row] = await db.select().from(documents).where(same).limit(1);
  return row;
}"#,
        "find",
    );
    assert!(unknowns(&e).is_empty(), "{:?}", unknowns(&e));
    let s = selects(&e);
    assert_eq!(s.len(), 1);
    assert!(
        s[0].0.contains("@spaceId") && s[0].0.contains("@title"),
        "{}",
        s[0].0
    );
    assert!(
        e.assumptions
            .iter()
            .any(|a| a.contains("`same`") && a.contains("a.ts:")),
        "{:?}",
        e.assumptions
    );
}

#[test]
fn a_held_predicate_keeps_the_values_its_operands_had_where_it_was_built() {
    // Drizzle builds the SQL object when `eq()` is called: the later
    // assignment to `target` does not change the query.
    let body = r#"export async function capture(id: number) {
  let target = id;
  const byId = eq(accounts.id, target);
  target = 999;
  const [row] = await db.select().from(accounts).where(byId).limit(1);
  if (!row) throw new Error("not found");
}"#;
    let e = extract(body, "capture");
    assert!(unknowns(&e).is_empty(), "{:?}", unknowns(&e));
    let rows = json!({ "accounts": [{"id": 1, "balance": 0}] });
    assert_eq!(run(&e, json!([1]), rows.clone()), Outcome::Ok);
    // Used inline, the operand is read where the query is built.
    let inline = body.replace(".where(byId)", ".where(eq(accounts.id, target))");
    let e = extract(&inline, "capture");
    assert!(matches!(run(&e, json!([1]), rows), Outcome::Failed(_)));
}

#[test]
fn a_conditional_between_two_predicates_or_undefined_is_exact() {
    let body = r#"export async function place(spaceId: number, folderId: number | null, kind: string | undefined) {
  const inFolder = folderId === null ? isNull(documents.folderId) : eq(documents.folderId, folderId);
  const titled = kind ? eq(documents.title, kind) : undefined;
  const [row] = await db.select().from(documents).where(and(eq(documents.spaceId, spaceId), inFolder, titled)).limit(1);
  if (!row) throw new Error("none");
}"#;
    let e = extract(body, "place");
    assert!(unknowns(&e).is_empty(), "{:?}", unknowns(&e));
    let rows = json!({ "documents": [
        {"id": 1, "spaceId": 1, "folderId": null, "title": "a"},
        {"id": 2, "spaceId": 1, "folderId": 7, "title": "b"}
    ] });
    // null folder: the root document; any title.
    assert_eq!(run(&e, json!([1, null, null]), rows.clone()), Outcome::Ok);
    // folder 7, title "b" / "a".
    assert_eq!(run(&e, json!([1, 7, "b"]), rows.clone()), Outcome::Ok);
    assert!(matches!(
        run(&e, json!([1, 7, "a"]), rows.clone()),
        Outcome::Failed(_)
    ));
    // folder 8: nothing (no NULL row matches `folderId = 8`).
    assert!(matches!(
        run(&e, json!([1, 8, null]), rows),
        Outcome::Failed(_)
    ));
    // An `undefined` predicate where it is not dropped is refused.
    let e = extract(
        &body
            .replace(
                "where(and(eq(documents.spaceId, spaceId), inFolder, titled))",
                "where(not(titled))",
            )
            .replace("import { and,", "import { and, not,"),
        "place",
    );
    assert!(
        unknowns(&e)
            .iter()
            .any(|d| d.contains("`titled` may be `undefined`")),
        "{:?}",
        unknowns(&e)
    );
}

#[test]
fn project_helpers_returning_a_predicate_are_inlined() {
    let helpers = r#"import { eq, isNull } from "drizzle-orm";
import { documents } from "./schema";
export function inFolderOf(folderId: number | null) {
  return folderId === null ? isNull(documents.folderId) : eq(documents.folderId, folderId);
}
export const titled = (title: string) => eq(documents.title, title);
"#;
    let main = format!(
        "{IMPORTS}\nimport {{ inFolderOf, titled }} from \"./helpers\";\n{}",
        r#"function inSpace(spaceId: number) {
  return eq(documents.spaceId, spaceId);
}
export async function find(spaceId: number, folderId: number | null, title: string) {
  const where = and(inSpace(spaceId), inFolderOf(folderId));
  const [row] = await db.select().from(documents).where(and(where, titled(title))).limit(1);
  if (!row) throw new Error("none");
}"#
    );
    let e = extract_js(&[("a.ts", &main), ("helpers.ts", helpers)], "find", true);
    assert!(unknowns(&e).is_empty(), "{:?}", unknowns(&e));
    let rows = json!({ "documents": [{"id": 1, "spaceId": 1, "folderId": null, "title": "a"}] });
    assert_eq!(run(&e, json!([1, null, "a"]), rows.clone()), Outcome::Ok);
    assert!(matches!(
        run(&e, json!([1, 3, "a"]), rows),
        Outcome::Failed(_)
    ));
}

#[test]
fn unmodeled_held_predicates_stay_unknown_with_the_reason() {
    for (body, reason) in [
        (
            r#"export async function f(spaceId: number) {
  let p = eq(documents.spaceId, spaceId);
  p = and(p, isNull(documents.folderId));
  const [row] = await db.select().from(documents).where(p).limit(1);
}"#,
            "assigned again",
        ),
        (
            r#"export async function f(spaceId: number) {
  const p = eq(documents.title, sql`now()`);
  const [row] = await db.select().from(documents).where(p).limit(1);
}"#,
            "clock reading",
        ),
        (
            r#"export async function f(ids: number[]) {
  const p = inArray(documents.id, ids.map((x) => x + 1));
  const [row] = await db.select().from(documents).where(p).limit(1);
}"#,
            "inArray",
        ),
        (
            r#"export async function f(x: number) {
  const p = mystery(x);
  const [row] = await db.select().from(documents).where(p).limit(1);
}"#,
            "`p`",
        ),
    ] {
        let e = extract(body, "f");
        let u = unknowns(&e);
        assert!(!u.is_empty(), "{body}: an unknown expected");
        assert!(u.iter().any(|d| d.contains(reason)), "{body}: {u:?}");
        // Never approximated as `true`: no select runs.
        assert!(selects(&e).is_empty(), "{body}: {:?}", selects(&e));
    }
}

// ---------------------------------------------------------------------
// Multi-row selects
// ---------------------------------------------------------------------

#[test]
fn a_multi_row_lock_binds_the_array_of_rows() {
    let body = r#"export async function lockBoth(a: number, b: number) {
  await db.transaction(async (tx) => {
    const rows = await tx.select().from(accounts).where(inArray(accounts.id, [a, b])).for("update");
    if (rows.length !== 2) throw new Error("missing");
    let total = 0;
    for (const r of rows) total = total + r.balance;
    if (total !== rows[0].balance + rows[1].balance) throw new Error("order");
  });
}"#;
    let e = extract(body, "lockBoth");
    assert!(unknowns(&e).is_empty(), "{:?}", unknowns(&e));
    let s = selects(&e);
    assert_eq!(
        s[0].2,
        Some(ManyRows {
            limit: None,
            order_by: None
        })
    );
    assert_eq!(s[0].1, LockMode::ForUpdate);
    assert!(
        e.assumptions
            .iter()
            .any(|a| a.contains("multi-row select without orderBy") && a.contains("physical order")),
        "{:?}",
        e.assumptions
    );
    let rows = json!({ "accounts": [{"id": 1, "balance": 5}, {"id": 2, "balance": 7}] });
    assert_eq!(run(&e, json!([2, 1]), rows.clone()), Outcome::Ok);
    assert!(matches!(run(&e, json!([2, 3]), rows), Outcome::Failed(_)));
}

#[test]
fn destructuring_a_lock_without_limit_reads_the_first_locked_row() {
    let e = extract(
        r#"export async function take(id: number) {
  await db.transaction(async (tx) => {
    const [row] = await tx.select().from(accounts).where(eq(accounts.id, id)).for("update", { skipLocked: true });
    if (!row) throw new Error("busy");
  });
}"#,
        "take",
    );
    assert!(unknowns(&e).is_empty(), "{:?}", unknowns(&e));
    let s = selects(&e);
    assert_eq!(s[0].1, LockMode::SkipLocked);
    assert!(s[0].2.is_some());
    let rows = json!({ "accounts": [{"id": 1, "balance": 0}] });
    assert_eq!(run(&e, json!([1]), rows.clone()), Outcome::Ok);
    assert!(matches!(run(&e, json!([4]), rows), Outcome::Failed(_)));
}

#[test]
fn order_by_the_key_limits_and_discarded_locks() {
    let e = extract(
        r#"export async function f(ids: number[]) {
  await db.transaction(async (tx) => {
    await tx.select().from(accounts).where(inArray(accounts.id, ids)).orderBy(asc(accounts.id)).limit(2).for("no key update");
    const rows = await tx.select({ id: accounts.id }).from(accounts).where(inArray(accounts.id, ids)).orderBy(accounts.id);
    if (rows.length === 0) throw new Error("none");
  });
}"#,
        "f",
    );
    assert!(unknowns(&e).is_empty(), "{:?}", unknowns(&e));
    let s = selects(&e);
    assert_eq!(
        s[0].2,
        Some(ManyRows {
            limit: Some(2),
            order_by: Some("id".to_owned())
        })
    );
    assert_eq!(s[0].1, LockMode::ForNoKeyUpdate);
    assert_eq!(s[1].1, LockMode::None);
    assert_eq!(
        s[1].2.as_ref().and_then(|m| m.order_by.clone()),
        Some("id".to_owned())
    );
    // A descending or computed order is refused.
    let e = extract(
        r#"export async function f(ids: number[]) {
  const rows = await db.select().from(accounts).where(inArray(accounts.id, ids)).orderBy(desc(accounts.id)).for("update");
}"#,
        "f",
    );
    assert!(
        unknowns(&e).iter().any(|d| d.contains("orderBy")),
        "{:?}",
        unknowns(&e)
    );
}

#[test]
fn without_javascript_values_multi_row_results_stay_refused() {
    // The Phase 15 behavior: a multi-row result is an array, modeled only
    // with JavaScript value semantics.
    let body = format!(
        "{IMPORTS}\n{}",
        r#"export async function f(id: number) {
  const [row] = await db.select().from(accounts).where(eq(accounts.id, id)).for("update");
  const rows = await db.select().from(accounts).where(eq(accounts.id, id));
}"#
    );
    let e = extract_js(&[("a.ts", &body)], "f", false);
    let u = unknowns(&e);
    assert!(
        u.iter().filter(|d| d.contains("limit(1)")).count() >= 2,
        "{u:?}"
    );
    assert!(selects(&e).iter().all(|s| s.2.is_none()));
}

#[test]
fn single_row_selects_keep_their_phase_15_form() {
    let e = extract(
        r#"export async function f(id: number) {
  const [row] = await db.select().from(accounts).where(eq(accounts.id, id)).limit(1).for("update");
  const rows = await db.select().from(accounts).where(eq(accounts.id, id)).limit(1);
  const [first] = await db.select().from(accounts).where(eq(accounts.id, id));
}"#,
        "f",
    );
    assert!(unknowns(&e).is_empty(), "{:?}", unknowns(&e));
    assert!(
        selects(&e).iter().all(|s| s.2.is_none()),
        "{:?}",
        selects(&e)
    );
    // The serialized statement has no `many` field.
    let text = serde_json::to_string(&e.codes).unwrap();
    assert!(!text.contains("\"many\""), "{text}");
}

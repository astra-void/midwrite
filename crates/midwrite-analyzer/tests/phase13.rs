//! Phase 13: replica-aware database handle provenance (Track C) and source
//! resolution (Track D).

use std::path::{Path, PathBuf};

use midwrite_analyzer::TsActorProvider;
use midwrite_model::ir::{LockMode, Op, StmtKind};
use midwrite_model::provider::{
    ActorProvider, ExtractRequest, Extracted, ReplicaFactoryBinding, TableBinding,
};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/phase13/replica-app")
}

fn tables() -> Vec<TableBinding> {
    let t = |name: &str, columns: &[&str]| TableBinding {
        identifier: name.to_owned(),
        table: name.to_owned(),
        columns: columns.iter().map(|c| (*c).to_owned()).collect(),
        source: None,
    };
    vec![
        t("accounts", &["id", "email", "balance"]),
        t("transfers", &["id", "fromId", "amount"]),
    ]
}

fn factory() -> ReplicaFactoryBinding {
    ReplicaFactoryBinding {
        factory: "src/db/replicas.ts#withReplicas".to_owned(),
        primary_argument: 0,
        primary_only_methods: vec!["usePrimaryOnly".to_owned()],
        primary_properties: vec!["$primary".to_owned()],
        reason: "first-party fixture modeled after drizzle-orm withReplicas".to_owned(),
    }
}

fn extract_with(
    root: &Path,
    selector: &str,
    replicas: bool,
    factories: Vec<ReplicaFactoryBinding>,
) -> Extracted {
    TsActorProvider::new(root)
        .extract(&ExtractRequest {
            root: root.to_path_buf(),
            file: String::new(),
            function: String::new(),
            arguments: Vec::new(),
            tables: tables(),
            externals: Vec::new(),
            transaction_retries: Vec::new(),
            max_inline_depth: 3,
            selector: Some(selector.to_owned()),
            contract: Some(Default::default()),
            replicas,
            replica_factories: factories,
            return_values: false,
            js_values: false,
        })
        .unwrap_or_else(|e| panic!("{selector}: {e}"))
}

fn extract(selector: &str, replicas: bool) -> Extracted {
    extract_with(&fixture(), selector, replicas, vec![factory()])
}

/// Database statements and unknown operations, in code order:
/// `Pool SELECT accounts`, `Tx UPDATE accounts`, `? <detail>`.
fn summary(x: &Extracted) -> Vec<String> {
    x.codes
        .iter()
        .flatten()
        .filter_map(|s| match &s.op {
            Op::Db { stmt } => Some(format!(
                "{:?} {} {}",
                stmt.conn,
                verb(&stmt.kind),
                stmt.kind.table()
            )),
            Op::Begin { .. } => Some("BEGIN".to_owned()),
            Op::Commit => Some("COMMIT".to_owned()),
            Op::Unknown { detail, .. } => Some(format!("? {detail}")),
            _ => None,
        })
        .collect()
}

fn verb(k: &StmtKind) -> &'static str {
    match k {
        StmtKind::Select { lock, .. } if *lock != LockMode::None => "SELECT-FOR-UPDATE",
        StmtKind::Select { .. } => "SELECT",
        StmtKind::SelectCount { .. } => "COUNT",
        StmtKind::Update { .. } => "UPDATE",
        StmtKind::Insert { .. } => "INSERT",
        StmtKind::Delete { .. } => "DELETE",
    }
}

struct Project(PathBuf);

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!("midwrite-a13-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, text) in files {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        Self(dir)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const SCHEMA: &str = r#"
import { integer, pgTable, text } from "drizzle-orm/pg-core";
export const accounts = pgTable("accounts", {
  id: integer("id").primaryKey(),
  email: text("email").notNull(),
  balance: integer("balance").notNull(),
});
"#;

fn assert_summary(selector: &str, replicas: bool, expected: &[&str]) {
    let x = extract(selector, replicas);
    let got = summary(&x);
    let got: Vec<&str> = got.iter().map(String::as_str).collect();
    assert_eq!(got, expected, "{selector}");
}

/// Every primary read of the fixture is an explicit `$primary` or
/// `usePrimaryOnly()` read (or one inside a primary transaction).
const EXPLICIT_PRIMARY_READS: [&str; 3] = [
    "src/queries/accounts.ts#openAccountOnPrimary",
    "src/queries/accounts.ts#balanceOnPrimary",
    "src/builtin/entries.ts#registerOnPrimary",
];

const ENTRIES: [&str; 17] = [
    "src/queries/accounts.ts#openAccount",
    "src/queries/accounts.ts#openAccountOnPrimary",
    "src/queries/accounts.ts#balanceOnPrimary",
    "src/queries/accounts.ts#findAccount",
    "src/queries/accounts.ts#debit",
    "src/queries/accounts.ts#transfer",
    "src/queries/accounts.ts#debitDirect",
    "src/queries/accounts.ts#recordTransfer",
    "src/builtin/entries.ts#register",
    "src/builtin/entries.ts#registerOnPrimary",
    "src/builtin/entries.ts#lockOnReplica",
    "src/builtin/entries.ts#typedReplicated",
    "src/builtin/entries.ts#typedPlain",
    "src/negative/lookalike.ts#lookalike",
    "src/negative/shadowed.ts#shadowed",
    "src/negative/shadowed.ts#viaWrapper",
    "src/negative/shadowed.ts#onPrimary",
];

#[test]
fn a_declared_factory_typed_through_return_type_routes_writes_to_the_primary_and_reads_to_a_replica()
 {
    // Write, then read back: the read is a replica read.
    assert_summary(
        "src/queries/accounts.ts#openAccount",
        true,
        &["Pool INSERT accounts", "Replica SELECT accounts"],
    );
    // The relational query getter reads a replica.
    assert_summary(
        "src/queries/accounts.ts#findAccount",
        true,
        &["Replica SELECT accounts"],
    );
    // A handle obtained by awaiting the factory function's result.
    assert_summary(
        "src/queries/accounts.ts#recordTransfer",
        true,
        &["Pool INSERT transfers", "Replica SELECT transfers"],
    );
    let x = extract("src/queries/accounts.ts#openAccount", true);
    assert!(
        x.assumptions.iter().any(|a| a.contains(
            "declared replica factory `src/db/replicas.ts#withReplicas` (an assumption; its body is not interpreted)"
        )),
        "{:#?}",
        x.assumptions
    );
    assert!(
        x.assumptions.iter().any(|a| a
            .contains("type provenance; casts and other values of the same type are not tracked")),
        "{:#?}",
        x.assumptions
    );
}

#[test]
fn primary_only_methods_primary_properties_and_transactions_use_the_primary() {
    assert_summary(
        "src/queries/accounts.ts#openAccountOnPrimary",
        true,
        &["Pool INSERT accounts", "Pool SELECT accounts"],
    );
    // `$primary` through `Database & { $primary?: Database }`.
    assert_summary(
        "src/queries/accounts.ts#balanceOnPrimary",
        true,
        &["Pool SELECT accounts"],
    );
    // `transaction` of a replicated database is a primary transaction; the
    // union-typed helper inlined with `tx` stays in it.
    assert_summary(
        "src/queries/accounts.ts#transfer",
        true,
        &[
            "BEGIN",
            "Tx SELECT-FOR-UPDATE accounts",
            "Tx UPDATE accounts",
            "Tx INSERT transfers",
            "COMMIT",
        ],
    );
    // The same helper inlined with the replicated database: primary
    // autocommit.
    assert_summary(
        "src/queries/accounts.ts#debitDirect",
        true,
        &["Pool UPDATE accounts"],
    );
}

#[test]
fn a_union_typed_helper_is_not_bound_as_an_entry() {
    let x = extract("src/queries/accounts.ts#debit", true);
    let got = summary(&x);
    assert_eq!(got.len(), 1, "{got:#?}");
    assert!(
        got[0].starts_with(
            "? database handle of an unresolved type: parameter `db: DatabaseOrTransaction`"
        ) && got[0].contains("a union type"),
        "{got:#?}"
    );
}

#[test]
fn drizzle_with_replicas_routing_is_built_in() {
    assert_summary(
        "src/builtin/entries.ts#register",
        true,
        &["Replica SELECT accounts", "Pool INSERT accounts"],
    );
    assert_summary(
        "src/builtin/entries.ts#registerOnPrimary",
        true,
        &["Pool SELECT accounts", "Pool INSERT accounts"],
    );
    assert_summary(
        "src/builtin/entries.ts#typedReplicated",
        true,
        &["Pool UPDATE accounts", "Replica SELECT accounts"],
    );
    // A row lock through a replicated database would run on a hot standby.
    assert_summary(
        "src/builtin/entries.ts#lockOnReplica",
        true,
        &[
            "? read through a read replica with a row lock (`.for(\"update\")`): a hot standby rejects row locks",
        ],
    );
    let x = extract("src/builtin/entries.ts#register", true);
    assert!(
        x.assumptions.iter().any(|a| a.contains(
            "`withReplicas` of drizzle-orm/pg-core at src/builtin/db.ts:9; drizzle-orm `withReplicas` routing (verified in drizzle-orm 0.45.4 pg-core/db.js)"
        )),
        "{:#?}",
        x.assumptions
    );
}

#[test]
fn a_plain_drizzle_type_in_a_replicating_project_writes_the_primary_and_leaves_reads_unknown() {
    let x = extract("src/builtin/entries.ts#typedPlain", true);
    let got = summary(&x);
    assert_eq!(got.len(), 2, "{got:#?}");
    assert_eq!(got[0], "Pool UPDATE accounts");
    assert!(
        got[1].starts_with(
            "? read through a read replica or the primary (the handle's routing is ambiguous)"
        ),
        "{got:#?}"
    );
}

#[test]
fn look_alike_shadowed_and_wrapped_factories_are_neither_replicated_nor_primary() {
    for selector in [
        "src/negative/lookalike.ts#lookalike",
        "src/negative/shadowed.ts#shadowed",
    ] {
        let x = extract(selector, true);
        let got = summary(&x);
        assert_eq!(got.len(), 2, "{selector}: {got:#?}");
        for line in &got {
            assert!(
                line.starts_with(
                    "? database handle of an unresolved type: `db` is a database receiver by name only"
                ),
                "{selector}: {got:#?}"
            );
        }
    }
    // A pass-through wrapper's result is not recognized (arguments are not
    // substituted), so its operations are not bound.
    let x = extract("src/negative/shadowed.ts#viaWrapper", true);
    assert!(
        summary(&x).iter().all(|l| l.starts_with("? ")),
        "{:#?}",
        summary(&x)
    );
    // The recognized primary under another name is the primary.
    assert_summary(
        "src/negative/shadowed.ts#onPrimary",
        true,
        &["Pool INSERT accounts"],
    );
}

#[test]
fn without_replica_modeling_replica_reads_stay_unknown() {
    let x = extract("src/queries/accounts.ts#openAccount", false);
    let got = summary(&x);
    assert_eq!(got[0], "Pool INSERT accounts");
    assert!(
        got[1].starts_with(
            "? read through a read replica (replica modeling not enabled): `db.select(...)` of a replicated database (the declared replica factory `src/db/replicas.ts#withReplicas` at src/db/client.ts:12)"
        ),
        "{got:#?}"
    );
    let x = extract("src/queries/accounts.ts#findAccount", false);
    assert!(
        summary(&x)[0].starts_with(
            "? read through a read replica (replica modeling not enabled): `db.query.accounts.findFirst(...)`"
        ),
        "{:#?}",
        summary(&x)
    );
    // Explicit primary reads do not depend on replica modeling.
    assert_summary(
        "src/queries/accounts.ts#openAccountOnPrimary",
        false,
        &["Pool INSERT accounts", "Pool SELECT accounts"],
    );
}

#[test]
fn an_undeclared_project_factory_stays_unknown() {
    let x = extract_with(
        &fixture(),
        "src/queries/accounts.ts#openAccount",
        true,
        Vec::new(),
    );
    let got = summary(&x);
    assert_eq!(got.len(), 2, "{got:#?}");
    for line in &got {
        assert!(
            line.starts_with("? database handle of an unresolved type: parameter `db: Database`")
                && line.contains("the function does more than return a value"),
            "{got:#?}"
        );
    }
}

#[test]
fn a_misdeclared_factory_is_reported_and_not_applied() {
    let mut bad = factory();
    bad.factory = "src/db/replicas.ts#noSuchFactory".to_owned();
    let x = extract_with(
        &fixture(),
        "src/queries/accounts.ts#openAccount",
        true,
        vec![bad],
    );
    assert!(
        x.assumptions.iter().any(|a| a.starts_with(
            "declared replica factory not applied: replica factory `src/db/replicas.ts#noSuchFactory`"
        )),
        "{:#?}",
        x.assumptions
    );
    assert!(summary(&x).iter().all(|l| l.starts_with("? ")));
}

#[test]
fn no_replica_read_is_ever_bound_to_the_primary() {
    for replicas in [false, true] {
        for factories in [vec![factory()], Vec::new()] {
            for selector in ENTRIES {
                let x = extract_with(&fixture(), selector, replicas, factories.clone());
                let lines = summary(&x);
                let reads_primary = lines
                    .iter()
                    .any(|l| l.starts_with("Pool SELECT") || l.starts_with("Pool COUNT"));
                if reads_primary {
                    assert!(
                        EXPLICIT_PRIMARY_READS.contains(&selector),
                        "{selector} (replicas {replicas}): {lines:#?}"
                    );
                }
                if !replicas {
                    assert!(
                        !lines.iter().any(|l| l.starts_with("Replica ")),
                        "{selector}: a replica read without replica modeling"
                    );
                }
            }
        }
    }
}

#[test]
fn projects_without_replicas_keep_phase_12_bindings() {
    let project = Project::new(
        "plain",
        &[
            ("src/schema.ts", SCHEMA),
            (
                "src/db.ts",
                r#"
import { drizzle, type NodePgDatabase } from "drizzle-orm/node-postgres";
import * as schema from "./schema";
export type Database = NodePgDatabase<typeof schema>;
export const db = drizzle(process.env.URL!, { schema });
"#,
            ),
            (
                "src/entry.ts",
                r#"
import { eq } from "drizzle-orm";
import { db, type Database } from "./db";
import { accounts } from "./schema";
export async function byName(id: number) {
  const [row] = await db.select().from(accounts).where(eq(accounts.id, id)).limit(1);
  return row;
}
export async function byType(handle: Database, id: number) {
  const [row] = await handle.select().from(accounts).where(eq(accounts.id, id)).limit(1);
  return row;
}
"#,
            ),
        ],
    );
    for selector in ["src/entry.ts#byName", "src/entry.ts#byType"] {
        let x = extract_with(&project.0, selector, true, Vec::new());
        assert_eq!(summary(&x), vec!["Pool SELECT accounts"], "{selector}");
    }
}

#[test]
fn with_replicas_of_another_drizzle_dialect_is_not_the_pg_core_contract() {
    let project = Project::new(
        "dialect",
        &[
            ("src/schema.ts", SCHEMA),
            (
                "src/db.ts",
                r#"
import { drizzle } from "drizzle-orm/node-postgres";
import { withReplicas } from "drizzle-orm/mysql-core";
const primary = drizzle(process.env.URL!);
export const db = withReplicas(primary, [primary]);
"#,
            ),
            (
                "src/entry.ts",
                r#"
import { eq } from "drizzle-orm";
import { db } from "./db";
import { accounts } from "./schema";
export async function read(id: number) {
  const [row] = await db.select().from(accounts).where(eq(accounts.id, id)).limit(1);
  return row;
}
"#,
            ),
        ],
    );
    let x = extract_with(&project.0, "src/entry.ts#read", true, Vec::new());
    let got = summary(&x);
    // Not drizzle-orm/pg-core's contract: the database is not recognized, and
    // because the project imports a `withReplicas`, the receiver name `db`
    // alone does not make it the primary either.
    assert_eq!(got.len(), 1, "{got:#?}");
    assert!(
        got[0].starts_with(
            "? database handle of an unresolved type: `db` is a database receiver by name only"
        ) && got[0].contains(
            "`withReplicas` of `drizzle-orm/mysql-core` is not a recognized database constructor"
        ),
        "{got:#?}"
    );
}

#[test]
fn a_pg_core_with_replicas_import_alone_marks_the_project_as_replicating() {
    let project = Project::new(
        "builtin-only",
        &[
            ("src/schema.ts", SCHEMA),
            (
                "src/db.ts",
                r#"
import { drizzle } from "drizzle-orm/node-postgres";
import { withReplicas } from "drizzle-orm/pg-core";
const primary = drizzle(process.env.URL!);
const replica = drizzle(process.env.REPLICA_URL!);
export const db = withReplicas(primary, [replica]);
"#,
            ),
            (
                "src/entry.ts",
                r#"
import { eq } from "drizzle-orm";
import type { NodePgDatabase } from "drizzle-orm/node-postgres";
import { accounts } from "./schema";
export async function typed(handle: NodePgDatabase, id: number) {
  await handle.delete(accounts).where(eq(accounts.id, id));
  const [row] = await handle.select().from(accounts).where(eq(accounts.id, id)).limit(1);
  return row;
}
"#,
            ),
        ],
    );
    let x = extract_with(&project.0, "src/entry.ts#typed", true, Vec::new());
    let got = summary(&x);
    assert_eq!(got.len(), 2, "{got:#?}");
    assert_eq!(got[0], "Pool DELETE accounts");
    assert!(
        got[1].contains("the handle's routing is ambiguous")
            && got[1].contains("`withReplicas` of `drizzle-orm/pg-core` imported by `src/db.ts`"),
        "{got:#?}"
    );
}

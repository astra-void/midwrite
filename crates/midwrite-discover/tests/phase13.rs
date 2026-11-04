//! Phase 13: discovery plumbs declared replica factories and the replica
//! flag into extraction. Static only: nothing here executes application code.

use std::path::PathBuf;

use midwrite_discover::contracts::Contracts;
use midwrite_discover::extract::{MemoProvider, extract_entry};
use midwrite_discover::pipeline::{Options, discover};
use midwrite_discover::project::index;
use midwrite_model::ir::{Conn, Op};

fn replica_app() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/phase13/replica-app")
}

const CONTRACTS: &str = r#"{
  "schemaVersion": 1,
  "kind": "midwrite.contracts",
  "replicaFactories": [{
    "factory": "src/db/replicas.ts#withReplicas",
    "primaryOnlyMethods": ["usePrimaryOnly"],
    "primaryProperties": ["$primary"],
    "reason": "first-party fixture: the factory's getters follow drizzle-orm withReplicas routing"
  }]
}"#;

fn extract_conns(selector: &str, contracts: Contracts) -> (Vec<Conn>, Vec<String>, bool) {
    let root = replica_app();
    let provider = MemoProvider::new(&root);
    let idx = index(&root, contracts, &provider).expect("index");
    let x = extract_entry(&provider, &idx.bindings, selector, None).expect("extracts");
    let mut conns = Vec::new();
    let mut unknown = Vec::new();
    for s in x.codes.iter().flatten() {
        match &s.op {
            Op::Db { stmt } => conns.push(stmt.conn),
            Op::Unknown { detail, .. } => unknown.push(detail.clone()),
            _ => {}
        }
    }
    (conns, unknown, idx.bindings.replicas)
}

#[test]
fn declared_replica_factories_reach_extraction_and_enable_replica_reads() {
    let contracts = Contracts::parse(CONTRACTS).unwrap();
    let (conns, unknown, replicas) =
        extract_conns("src/queries/accounts.ts#openAccount", contracts);
    assert!(
        replicas,
        "a declared factory marks the project as replicating"
    );
    assert_eq!(conns, vec![Conn::Pool, Conn::Replica], "{unknown:#?}");
}

#[test]
fn without_a_declaration_the_project_factory_stays_unknown_but_the_project_replicates() {
    let (conns, unknown, replicas) =
        extract_conns("src/queries/accounts.ts#openAccount", Contracts::default());
    // The fixture also imports drizzle-orm's own `withReplicas`.
    assert!(replicas);
    assert!(conns.is_empty(), "{conns:?}");
    assert!(
        unknown
            .iter()
            .all(|u| u.starts_with("database handle of an unresolved type")),
        "{unknown:#?}"
    );
    // drizzle-orm's own `withReplicas` needs no declaration.
    let (conns, _, _) = extract_conns("src/builtin/entries.ts#register", Contracts::default());
    assert_eq!(conns, vec![Conn::Replica, Conn::Pool]);
}

#[test]
fn discovery_reports_the_replicated_database_and_counts_replica_reads() {
    let dir = std::env::temp_dir().join(format!("midwrite-d13-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("contracts.json");
    std::fs::write(&path, CONTRACTS).unwrap();
    let report = discover(
        &replica_app(),
        &Options {
            contracts: Some(path),
            include_timing: false,
            tool_version: "test".to_owned(),
            ..Options::default()
        },
    )
    .expect("discovery runs");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        report.notes.iter().any(|n| n.contains(
            "the project builds a replicated database (the declared replica factory `src/db/replicas.ts#withReplicas`)"
        )),
        "{:#?}",
        report.notes
    );
    let entry = report
        .entries
        .iter()
        .find(|e| e.selector == "src/queries/accounts.ts#openAccount")
        .expect("the entry is reported");
    assert_eq!(entry.status, "effectful");
    assert_eq!((entry.reads, entry.writes), (1, 1));
}

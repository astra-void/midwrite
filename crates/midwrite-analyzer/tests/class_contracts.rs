//! `classContracts`: a function whose awaited result is an instance of a
//! project class (an RPC stub), and fields that hold another class.
//! `fixtures/class_contracts/rpc_stub` mirrors a Cloudflare Durable Object
//! database stub: `getZeroDB()` → `DbRpcDO` → `this.mainDo` (`ZeroDB`) →
//! `this.db`.

use std::path::{Path, PathBuf};

use midwrite_analyzer::{Analyzer, display_path};
use midwrite_core::config::Config;

fn project() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/class_contracts/rpc_stub");
    std::fs::canonicalize(dir).expect("fixture exists")
}

fn config(contract: bool) -> Config {
    let main = display_path(&project().join("main.ts"));
    let contracts = if contract {
        format!(
            r#","classContracts":[{{"file":{main:?},"class":"DbRpcDO","fields":{{"mainDo":"ZeroDB"}},"returnedBy":["getZeroDB"],"evidence":"main.ts: setMetaData returns new DbRpcDO(this, userId)"}}]"#
        )
    } else {
        String::new()
    };
    Config::from_json(
        &format!(
            r#"{{"databaseReceivers":["db","this.db"],"externalEffects":[{{"function":"clearMailboxChatData","category":"file-write"}}]{contracts}}}"#
        ),
        "test",
    )
    .expect("config")
}

fn diagnostics(contract: bool) -> Vec<(String, String, u32)> {
    let analysis = Analyzer::new(config(contract))
        .analyze_project(&[project()])
        .expect("project");
    assert!(analysis.failures.is_empty(), "{:?}", analysis.failures);
    let mut out: Vec<_> = analysis
        .files
        .iter()
        .flat_map(|f| &f.diagnostics)
        .map(|d| {
            let file = Path::new(&d.location.file)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            (d.code.as_str().to_owned(), file, d.location.line)
        })
        .collect();
    out.sort();
    out
}

#[test]
fn declared_stub_methods_reach_the_durable_object_writes() {
    assert_eq!(
        diagnostics(true),
        vec![
            ("MW1001".to_owned(), "connections.ts".to_owned(), 14),
            ("MW1001".to_owned(), "connections.ts".to_owned(), 20),
        ]
    );
}

#[test]
fn without_the_contract_the_stub_calls_are_unmodelled_database_methods() {
    assert_eq!(
        diagnostics(false),
        vec![
            ("MW9001".to_owned(), "connections.ts".to_owned(), 13),
            ("MW9001".to_owned(), "connections.ts".to_owned(), 19),
        ]
    );
}

#[test]
fn contracts_need_evidence_and_a_declaration() {
    for (json, message) in [
        (
            r#"{"classContracts":[{"file":"a.ts","class":"A","returnedBy":["f"],"evidence":" "}]}"#,
            "evidence",
        ),
        (
            r#"{"classContracts":[{"file":"a.ts","class":"A","evidence":"a.ts:1"}]}"#,
            "neither",
        ),
        (
            r#"{"classContracts":[{"file":"a.ts","class":"A","fields":{"x y":"B"},"evidence":"a.ts:1"}]}"#,
            "fields",
        ),
    ] {
        let error = Config::from_json(json, "test").unwrap_err().to_string();
        assert!(error.contains(message), "{json}: {error}");
    }
}

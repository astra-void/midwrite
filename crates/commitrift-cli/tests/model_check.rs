//! `commitrift model-check` end to end: TypeScript fixtures → extraction →
//! bounded model checking → reports.

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_commitrift"))
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scenario(name: &str) -> PathBuf {
    root()
        .join("fixtures/concurrency/scenarios")
        .join(format!("{name}.json"))
}

fn run_json(name: &str, extra: &[&str]) -> (i32, serde_json::Value, String) {
    let out = bin()
        .arg("model-check")
        .arg(scenario(name))
        .args(["--format", "json", "--no-timing"])
        .args(extra)
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let json = serde_json::from_str(&text).unwrap_or_else(|e| {
        panic!(
            "{name}: not JSON ({e}): {text}\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out.status.code().unwrap(), json, text)
}

#[test]
fn fixture_verdicts_and_exit_codes() {
    let expected = [
        ("inventory-oversell", "COUNTEREXAMPLE_FOUND"),
        ("inventory-oversell-in-transaction", "COUNTEREXAMPLE_FOUND"),
        (
            "inventory-oversell-repeatable-read",
            "NO_COUNTEREXAMPLE_WITHIN_BOUNDS",
        ),
        ("inventory-atomic-update", "NO_COUNTEREXAMPLE_WITHIN_BOUNDS"),
        ("inventory-row-lock", "NO_COUNTEREXAMPLE_WITHIN_BOUNDS"),
        ("doctors-write-skew-read-committed", "COUNTEREXAMPLE_FOUND"),
        ("doctors-write-skew-repeatable-read", "COUNTEREXAMPLE_FOUND"),
        (
            "doctors-write-skew-serializable",
            "NO_COUNTEREXAMPLE_WITHIN_BOUNDS",
        ),
        ("redeem-check-then-act", "COUNTEREXAMPLE_FOUND"),
        (
            "redeem-check-then-act-unique",
            "NO_COUNTEREXAMPLE_WITHIN_BOUNDS",
        ),
        ("redeem-atomic", "NO_COUNTEREXAMPLE_WITHIN_BOUNDS"),
        ("job-double-processing", "COUNTEREXAMPLE_FOUND"),
        ("payment-crash-retry", "COUNTEREXAMPLE_FOUND"),
        (
            "payment-crash-retry-provider-dedup",
            "NO_COUNTEREXAMPLE_WITHIN_BOUNDS",
        ),
    ];
    for (name, verdict) in expected {
        let (code, json, _) = run_json(name, &[]);
        assert_eq!(json["exploration"]["verdict"], verdict, "{name}");
        assert_eq!(
            code,
            if verdict == "COUNTEREXAMPLE_FOUND" {
                1
            } else {
                0
            },
            "{name}"
        );
        // Every actor was extracted from TypeScript, with source locations.
        for a in json["actors"].as_array().unwrap() {
            assert_eq!(a["origin"], "extracted", "{name}");
            assert!(
                a["sourceRefs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|r| r["line"].as_u64().unwrap() > 0)
            );
        }
        if verdict == "COUNTEREXAMPLE_FOUND" {
            assert_eq!(json["evidence"]["level"], "model-counterexample");
            assert!(
                json["diagnostics"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|d| d["code"] == "CP7001")
            );
        } else {
            assert_eq!(json["exploration"]["complete"], true, "{name}");
        }
    }
}

#[test]
fn operations_link_to_effect_ir_and_existing_findings() {
    let (_, json, _) = run_json("job-double-processing", &[]);
    let refs = json["actors"][0]["sourceRefs"].as_array().unwrap();
    // Every operation of processJob is an Effect IR effect of that function.
    assert!(refs.iter().all(|r| r["effectId"].is_u64()), "{refs:#?}");
    // The publish is part of the sequential findings CP1001/CP1002; they are
    // linked, not turned into concurrency findings.
    let publish = refs
        .iter()
        .find(|r| r["text"].as_str().unwrap().starts_with("publishEvent"))
        .unwrap();
    let linked: Vec<&str> = publish["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect();
    assert!(
        linked.iter().any(|d| d.starts_with("CP1001@")),
        "{linked:?}"
    );
    assert!(
        json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["code"].as_str().unwrap().starts_with("CP7"))
    );
}

#[test]
fn json_output_is_deterministic_without_timing() {
    let (_, _, a) = run_json("job-double-processing", &[]);
    let (_, _, b) = run_json("job-double-processing", &[]);
    assert_eq!(a, b);
    assert!(!a.contains("wallMs"));
}

#[test]
fn the_witness_comes_from_extracted_source() {
    let (_, json, _) = run_json("inventory-oversell", &[]);
    let w = &json["counterexample"];
    let texts: Vec<String> = w["steps"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| {
            s["events"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e["text"].as_str().unwrap().to_owned())
        })
        .collect();
    assert_eq!(
        texts
            .iter()
            .filter(|t| t.starts_with("SELECT products"))
            .count(),
        2
    );
    assert_eq!(
        texts
            .iter()
            .filter(|t| t.starts_with("UPDATE products SET stock = 0"))
            .count(),
        2
    );
    let src = &w["steps"][0]["events"][0]["src"];
    assert_eq!(src["file"], "src/purchase.ts");
    assert_eq!(src["function"], "purchase");
    assert_eq!(w["finalState"]["terms"][0]["value"], 2);
}

#[test]
fn html_is_script_free_and_escaped() {
    let dir = std::env::temp_dir().join(format!("cr-mc-html-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("s.json");
    std::fs::write(
        &file,
        serde_json::json!({
            "schemaVersion": 1,
            "name": "<script>alert(1)</script>",
            "description": "\"quoted\" & <b>bold</b>",
            "database": { "tables": [ { "name": "t", "columns": [{"name":"id","notNull":true}, "v"], "primaryKey": ["id"] } ],
                          "initialState": { "t": [ { "id": "<img>", "v": 1 } ] } },
            "actors": [
              { "name": "A", "function": "f", "program": "let r = select t where @id == '<img>'\nupdate t where @id == '<img>' set @v = r.v + 1\n" },
              { "name": "B", "function": "f", "program": "let r = select t where @id == '<img>'\nupdate t where @id == '<img>' set @v = r.v + 1\n" }
            ],
            "invariants": [ { "name": "both-increments", "assert": { "eq": [ { "row": { "table": "t", "where": {"id": "<img>"}, "column": "v" } }, 3 ] } } ]
        })
        .to_string(),
    )
    .unwrap();
    let out = bin()
        .arg("model-check")
        .arg(&file)
        .args(["--format", "html"])
        .output()
        .unwrap();
    let html = String::from_utf8(out.stdout).unwrap();
    assert!(html.contains("script-src 'none'"));
    assert!(!html.contains("<script"), "unescaped script tag");
    assert!(!html.contains("<img>"));
    assert!(html.contains("&lt;img&gt;"));
    assert!(html.contains("VIOLATED"));
    // The JSON report renders through `commitrift report` as well.
    let json_path = dir.join("r.json");
    let st = bin()
        .arg("model-check")
        .arg(&file)
        .args(["--format", "json", "-o"])
        .arg(&json_path)
        .status()
        .unwrap();
    assert_eq!(st.code(), Some(1));
    let out = bin().arg("report").arg(&json_path).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8(out.stdout).unwrap().contains("Request A"));
    let _ = std::fs::remove_dir_all(&dir);
}

fn invalid(doc: serde_json::Value) -> String {
    let dir = std::env::temp_dir().join(format!(
        "cr-mc-bad-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("s.json");
    std::fs::write(&file, doc.to_string()).unwrap();
    let out = bin().arg("model-check").arg(&file).output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8(out.stderr).unwrap()
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn base() -> serde_json::Value {
    serde_json::json!({
        "schemaVersion": 1, "name": "x",
        "database": { "tables": [ { "name": "t", "columns": ["id", "v"], "primaryKey": ["id"] } ], "initialState": { "t": [] } },
        "actors": [ { "name": "A", "function": "f", "program": "let r = select t where @id == 1\n" } ],
        "invariants": [ { "name": "i", "assert": { "ge": [ { "count": { "table": "t", "where": {} } }, 0 ] } } ]
    })
}

#[test]
fn invalid_scenarios_are_rejected_with_the_offending_field() {
    let mut d = base();
    d["actors"][0]["program"] = "let r = select missing where @id == 1\n".into();
    assert!(invalid(d).contains("unknown table `missing`"));
    let mut d = base();
    d["actors"][0]["program"] = "let r = select t where @nope == 1\n".into();
    assert!(invalid(d).contains("unknown column `t.nope`"));
    let mut d = base();
    d["invariants"] = serde_json::json!([]);
    assert!(invalid(d).contains("declare at least one invariant"));
    let mut d = base();
    d["database"]["initialState"] = serde_json::json!({ "other": [] });
    assert!(invalid(d).contains("unknown table `other`"));
    let mut d = base();
    d["database"]["initialState"]["t"] = serde_json::json!([{ "id": 1, "color": "red" }]);
    assert!(invalid(d).contains("unknown column `color`"));
    let mut d = base();
    d["actors"][0]["program"] = "let r = select t where @id == k\n".into();
    assert!(invalid(d).contains("`k` is never assigned"));
    let mut d = base();
    d["invariants"][0]["assert"] =
        serde_json::json!({ "le": [ { "sum": { "table": "t", "where": {} } }, 1 ] });
    assert!(invalid(d).contains("a `column` is required"));
    let mut d = base();
    d["database"]["isolation"] = "snapshot".into();
    assert!(invalid(d).contains("unknown isolation level"));
    let mut d = base();
    d["actors"][0]["program"] = "external mail\n".into();
    assert!(invalid(d).contains("not declared in `channels`"));
    // An extracted function needs every argument.
    let mut d = base();
    d["actors"][0] = serde_json::json!({ "function": "purchase", "file": "src/purchase.ts", "arguments": ["product-1"] });
    d["extraction"] = serde_json::json!({ "root": root().join("fixtures/concurrency/project") });
    let e = invalid(d);
    assert!(e.contains("takes 2 parameter(s)"), "{e}");
}

#[test]
fn explain_knows_the_concurrency_codes() {
    let out = bin().args(["explain", "CP7001"]).output().unwrap();
    assert!(out.status.success());
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("MODEL counterexample")
    );
    let out = bin().arg("explain").output().unwrap();
    assert!(String::from_utf8(out.stdout).unwrap().contains("CP7101"));
}

#[test]
fn postgres_validation_confirms_the_oversell_schedule() {
    let (_, json, _) = run_json("inventory-oversell", &["--validate-postgres"]);
    match json["runtime"]["status"].as_str().unwrap() {
        "blocked" => {
            assert!(
                std::env::var_os("COMMITRIFT_REQUIRE_REPLAY").is_none(),
                "PostgreSQL is required: {}",
                json["runtime"]["note"]
            );
            eprintln!("skipping: {}", json["runtime"]["note"]);
        }
        status => {
            assert_eq!(status, "confirmed", "{:#}", json["runtime"]);
            assert_eq!(json["evidence"]["level"], "postgres-confirmed-schedule");
            assert_eq!(
                json["runtime"]["postgresTables"]["orders"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
        }
    }
}

#[test]
fn the_adapted_corpus_reproduction_is_labeled_and_checked() {
    let run = |name: &str| {
        let out = bin()
            .arg("model-check")
            .arg(
                root()
                    .join("fixtures/concurrency/corpus")
                    .join(format!("{name}.json")),
            )
            .args(["--format", "json", "--no-timing"])
            .output()
            .unwrap();
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()
    };
    let r = run("saas-starter-invite-double-submit");
    assert_eq!(r["exploration"]["verdict"], "COUNTEREXAMPLE_FOUND");
    assert!(
        r["scenario"]["description"]
            .as_str()
            .unwrap()
            .starts_with("ADAPTED ISOLATED REPRODUCTION")
    );
    let r = run("saas-starter-invite-unique-mitigation");
    assert_eq!(
        r["exploration"]["verdict"],
        "NO_COUNTEREXAMPLE_WITHIN_BOUNDS"
    );
}

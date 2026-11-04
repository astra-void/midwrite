//! Tests of the compiled `midwrite` binary.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures() -> PathBuf {
    root().join("fixtures")
}

/// Run the binary from the fixtures directory, so that its `midwrite.json`
/// is picked up and paths in the output are short.
fn run(args: &[&str]) -> Output {
    run_in(&fixtures(), args)
}

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_midwrite"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("binary runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("utf-8 stdout")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("utf-8 stderr")
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("valid JSON on stdout")
}

#[test]
fn vulnerable_file_fails_with_text_diagnostic() {
    let output = run(&["check", "vulnerable/db_commit_then_publish.ts"]);
    assert_eq!(output.status.code(), Some(1));
    let text = stdout(&output);
    assert!(
        text.contains(
            "warning[MW1001]: database commit followed by an independent external effect"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "--> vulnerable/db_commit_then_publish.ts:12:9 (in `completeOrder`, confidence: high)"
        ),
        "{text}"
    );
    assert!(text.contains("failure sequence:"), "{text}");
    assert!(text.contains("help:"), "{text}");
    assert!(text.contains("config: midwrite.json"), "{text}");
}

#[test]
fn safe_directory_passes() {
    let output = run(&["check", "safe"]);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    assert!(stdout(&output).contains("0 warning(s), 0 error(s), 0 info"));
}

#[test]
fn json_output_follows_the_documented_schema() {
    let output = run(&[
        "check",
        "vulnerable/publish_inside_transaction.ts",
        "--format",
        "json",
    ]);
    assert_eq!(output.status.code(), Some(1));
    let report = json(&output);
    assert_eq!(report["schemaVersion"], 1);
    assert_eq!(report["tool"]["name"], "midwrite");
    assert_eq!(report["config"], "midwrite.json");
    assert_eq!(report["summary"]["filesDiscovered"], 1);
    assert_eq!(report["summary"]["filesAnalyzed"], 1);
    assert_eq!(report["summary"]["diagnostics"]["warning"], 1);
    assert_eq!(report["failures"].as_array().unwrap().len(), 0);

    let d = &report["diagnostics"][0];
    assert_eq!(d["code"], "MW1003");
    assert_eq!(d["severity"], "warning");
    assert_eq!(d["kind"], "potentialHazard");
    assert_eq!(d["confidence"], "high");
    assert_eq!(d["reproduced"], false);
    assert_eq!(d["function"], "completeOrder");
    assert_eq!(
        d["location"]["file"],
        "vulnerable/publish_inside_transaction.ts"
    );
    assert_eq!(d["location"]["line"], 9);
    assert_eq!(d["location"]["column"], 11);
    for field in ["title", "message", "mitigation"] {
        assert!(d[field].as_str().is_some_and(|s| !s.is_empty()), "{field}");
    }
    for field in [
        "effects",
        "failureSequence",
        "pathConditions",
        "evidence",
        "limitations",
    ] {
        assert!(d[field].is_array(), "{field}");
    }
    let roles: Vec<&str> = d["effects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["transaction", "stagedWrite", "completedFirst"]);
}

#[test]
fn output_is_deterministic() {
    let first = run(&["check", ".", "--format", "json"]);
    let second = run(&["check", ".", "--format", "json"]);
    assert_eq!(first.stdout, second.stdout);
    let text_first = run(&["check", "."]);
    let text_second = run(&["check", "."]);
    assert_eq!(text_first.stdout, text_second.stdout);
}

#[test]
fn parse_failures_are_reported_separately() {
    let output = run(&["check", "unsupported/syntax_error.ts", "--format", "json"]);
    assert_eq!(output.status.code(), Some(3), "incomplete scan");
    let report = json(&output);
    assert_eq!(report["summary"]["filesFailed"], 1);
    assert_eq!(report["summary"]["filesAnalyzed"], 0);
    assert_eq!(report["failures"][0]["kind"], "parse");
    assert_eq!(report["failures"][0]["line"], 4);
    assert_eq!(report["diagnostics"].as_array().unwrap().len(), 0);

    let text = stdout(&run(&["check", "unsupported/syntax_error.ts"]));
    assert!(
        text.contains("error: could not parse unsupported/syntax_error.ts:4:"),
        "{text}"
    );
    assert!(text.contains("results are incomplete"), "{text}");
}

#[test]
fn findings_take_precedence_over_parse_failures_in_the_exit_code() {
    let output = run(&[
        "check",
        "unsupported/syntax_error.ts",
        "vulnerable/db_commit_then_publish.ts",
    ]);
    assert_eq!(output.status.code(), Some(1));
    // Under the default `recommended` policy two sequential writes are an
    // informational review candidate, so only the parse failure remains.
    let review = run(&[
        "check",
        "unsupported/syntax_error.ts",
        "vulnerable/independent_writes.ts",
    ]);
    assert_eq!(review.status.code(), Some(3));
    let strict = run(&[
        "check",
        "unsupported/syntax_error.ts",
        "vulnerable/independent_writes.ts",
        "--policy",
        "strict",
    ]);
    assert_eq!(strict.status.code(), Some(1));
}

#[test]
fn fail_on_threshold_is_configurable() {
    // MW9001 is informational: it does not fail the default threshold.
    let default = run(&["check", "unsupported/unknown_effect.ts"]);
    assert_eq!(default.status.code(), Some(0));
    let info = run(&[
        "check",
        "unsupported/unknown_effect.ts",
        "--fail-on",
        "info",
    ]);
    assert_eq!(info.status.code(), Some(1));
    let never = run(&["check", "vulnerable", "--fail-on", "never"]);
    assert_eq!(never.status.code(), Some(0));
    let error = run(&["check", "vulnerable", "--fail-on", "error"]);
    assert_eq!(
        error.status.code(),
        Some(0),
        "no Phase 1 code has error severity"
    );
}

#[test]
fn config_is_discovered_in_the_working_directory_or_given_explicitly() {
    // `stripe.charges.create` is only an effect in fixtures/midwrite.json.
    let with_config = json(&run(&[
        "check",
        "vulnerable/charge_then_write.ts",
        "--format",
        "json",
    ]));
    assert_eq!(with_config["diagnostics"].as_array().unwrap().len(), 2);

    let elsewhere = run_in(
        &root(),
        &[
            "check",
            "fixtures/vulnerable/charge_then_write.ts",
            "--format",
            "json",
        ],
    );
    let defaults = json(&elsewhere);
    assert_eq!(defaults["config"], Value::Null);
    // With the defaults neither `stripe.charges.create` nor `sendEmail` is a
    // known effect: no hazard is claimed, and the unknown call after the
    // commit is reported as a limitation.
    let diagnostics = defaults["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], "MW9001");
    assert_eq!(diagnostics[0]["location"]["line"], 13);

    let explicit = json(&run_in(
        &root(),
        &[
            "check",
            "fixtures/vulnerable/charge_then_write.ts",
            "--config",
            "fixtures/midwrite.json",
            "--format",
            "json",
        ],
    ));
    assert_eq!(explicit["diagnostics"].as_array().unwrap().len(), 2);
}

#[test]
fn invalid_config_is_a_usage_error() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bad-config");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("midwrite.json"),
        r#"{ "databaseReceiver": ["db"] }"#,
    )
    .unwrap();
    std::fs::write(dir.join("a.ts"), "export {};\n").unwrap();
    let output = run_in(&dir, &["check", "."]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("invalid configuration"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn missing_and_unsupported_paths_are_usage_errors() {
    let missing = run(&["check", "does-not-exist"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(stderr(&missing).contains("does not exist"));

    let unsupported = run(&["check", "midwrite.json"]);
    assert_eq!(unsupported.status.code(), Some(2));
    assert!(stderr(&unsupported).contains("not a TypeScript source file"));
}

#[test]
fn explain_prints_code_documentation() {
    let output = run(&["explain", "MW1001"]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(text.starts_with("MW1001: database commit followed by an independent external effect"));
    assert!(text.contains("Transactional outbox"));

    let lowercase = run(&["explain", "cp9001"]);
    assert_eq!(lowercase.status.code(), Some(0));

    let list = stdout(&run(&["explain"]));
    for code in ["MW1001", "MW1002", "MW1003", "MW1004", "MW1005", "MW9001"] {
        assert!(list.contains(code), "{code} listed");
    }

    let unknown = run(&["explain", "MW0000"]);
    assert_eq!(unknown.status.code(), Some(2));
    assert!(stderr(&unknown).contains("unknown diagnostic code"));
}

#[test]
fn sarif_output_is_valid_sarif_2_1_0() {
    let output = run(&[
        "check",
        "vulnerable/charge_then_write.ts",
        "--format",
        "sarif",
    ]);
    assert_eq!(output.status.code(), Some(1));
    let sarif = json(&output);
    assert_eq!(sarif["version"], "2.1.0");
    let run = &sarif["runs"][0];
    assert_eq!(run["tool"]["driver"]["name"], "midwrite");
    assert_eq!(run["tool"]["driver"]["rules"].as_array().unwrap().len(), 6);
    let results = run["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["ruleId"], "MW1002");
    assert_eq!(results[0]["level"], "warning");
    assert_eq!(
        results[0]["locations"][0]["physicalLocation"]["region"]["startLine"],
        11
    );
    assert!(
        results[0]["partialFingerprints"]["crashproof/v1"]
            .as_str()
            .is_some_and(|f| f.len() >= 16)
    );
}

#[test]
fn ir_command_prints_the_effect_ir() {
    let output = run(&["ir", "safe/transactional_outbox.ts"]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(text.contains("async fn completeOrder"), "{text}");
    assert!(text.contains("tx#0 begin db.transaction"), "{text}");
    assert!(
        text.contains("database update on `orders`: tx.update(orders) via tx#0"),
        "{text}"
    );
    assert!(
        text.contains("database insert on `outbox`: tx.insert(outbox) via tx#0"),
        "{text}"
    );
    assert!(text.contains("tx#0 commit"), "{text}");

    let json_output = run(&["ir", "safe/transactional_outbox.ts", "--format", "json"]);
    let files = json(&json_output);
    assert_eq!(files[0]["functions"][0]["body"][0]["node"], "transaction");

    let broken = run(&["ir", "unsupported/syntax_error.ts"]);
    assert_eq!(broken.status.code(), Some(3));
    assert!(stderr(&broken).contains("failed to parse"));
}

#[test]
fn call_summaries_are_bounded_and_can_be_disabled() {
    let summarized = json(&run(&[
        "check",
        "vulnerable/local_helper.ts",
        "--format",
        "json",
    ]));
    let d = &summarized["diagnostics"][0];
    assert_eq!(d["code"], "MW1001");
    let roles: Vec<&str> = d["effects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["completedFirst", "notYetPerformed", "callSite"]);

    let disabled = run(&[
        "check",
        "vulnerable/local_helper.ts",
        "--max-call-depth",
        "0",
        "--format",
        "json",
    ]);
    assert_eq!(disabled.status.code(), Some(0), "only MW9001 remains");
    assert_eq!(json(&disabled)["diagnostics"][0]["code"], "MW9001");

    let ir = stdout(&run(&["ir", "vulnerable/local_helper.ts"]));
    assert!(ir.contains("call#0 notify(id) inlines `notify`"), "{ir}");
    assert!(
        ir.contains("<summary via call#0 notify; every path>"),
        "{ir}"
    );
}

#[test]
fn documentation_covers_every_code() {
    let diagnostics = std::fs::read_to_string(root().join("docs/DIAGNOSTICS.md")).unwrap();
    let readme = std::fs::read_to_string(root().join("README.md")).unwrap();
    let list = stdout(&run(&["explain"]));
    for line in list
        .lines()
        .filter(|line| line.trim_start().starts_with("CP"))
    {
        let code = line.split_whitespace().next().unwrap();
        assert!(
            diagnostics.contains(&format!("## {code}:")),
            "{code} has a section in DIAGNOSTICS.md"
        );
        assert!(
            readme.contains(&format!("`{code}`")),
            "{code} is listed in the README"
        );
    }
}

// --- Phase 2: CI workflow ------------------------------------------------------

fn scratch(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

const PUBLISH_AFTER_WRITE: &str = "export async function pay(id: string) {\n  await db.update(orders).set({ id });\n  await queue.publish(\"paid\", { id });\n}\n";

#[test]
fn baselines_fail_only_on_new_findings() {
    let dir = scratch("baseline", &[("src/pay.ts", PUBLISH_AFTER_WRITE)]);
    let written = run_in(
        &dir,
        &["check", ".", "--write-baseline", "midwrite-baseline.json"],
    );
    assert_eq!(
        written.status.code(),
        Some(0),
        "writing a baseline does not fail"
    );
    let baseline: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("midwrite-baseline.json")).unwrap())
            .unwrap();
    assert_eq!(baseline["version"], 1);
    assert_eq!(baseline["findings"].as_array().unwrap().len(), 1);

    // Unchanged code: the finding is still reported, but does not fail.
    let checked = run_in(
        &dir,
        &["check", ".", "--baseline", "midwrite-baseline.json"],
    );
    assert_eq!(checked.status.code(), Some(0), "{}", stdout(&checked));
    assert!(stdout(&checked).contains("(in baseline)"));
    assert!(stdout(&checked).contains("1 already in the baseline"));

    // Moving the code (new lines above it) keeps the match.
    std::fs::write(
        dir.join("src/pay.ts"),
        format!("// a comment\n\n{PUBLISH_AFTER_WRITE}"),
    )
    .unwrap();
    let moved = run_in(
        &dir,
        &[
            "check",
            ".",
            "--baseline",
            "midwrite-baseline.json",
            "--format",
            "json",
        ],
    );
    assert_eq!(moved.status.code(), Some(0));
    let report = json(&moved);
    assert_eq!(report["diagnostics"][0]["baseline"], "existing");
    assert_eq!(report["summary"]["baseline"]["new"], 0);

    // Renaming the file: matched by the file-independent identity.
    std::fs::rename(dir.join("src/pay.ts"), dir.join("src/payments.ts")).unwrap();
    let renamed = run_in(
        &dir,
        &["check", ".", "--baseline", "midwrite-baseline.json"],
    );
    assert_eq!(renamed.status.code(), Some(0), "{}", stdout(&renamed));

    // A new hazard fails the run; the old one stays visible.
    std::fs::write(
        dir.join("src/refund.ts"),
        PUBLISH_AFTER_WRITE
            .replace("pay(", "refund(")
            .replace("paid", "refunded"),
    )
    .unwrap();
    let new = run_in(
        &dir,
        &[
            "check",
            ".",
            "--baseline",
            "midwrite-baseline.json",
            "--format",
            "json",
        ],
    );
    assert_eq!(new.status.code(), Some(1));
    let report = json(&new);
    assert_eq!(report["summary"]["baseline"]["new"], 1);
    assert_eq!(report["summary"]["baseline"]["existing"], 1);

    // Parse failures are never hidden by a baseline.
    std::fs::write(dir.join("src/broken.ts"), "export const = ;\n").unwrap();
    std::fs::remove_file(dir.join("src/refund.ts")).unwrap();
    let broken = run_in(
        &dir,
        &["check", ".", "--baseline", "midwrite-baseline.json"],
    );
    assert_eq!(broken.status.code(), Some(3));
}

#[test]
fn json_reports_carry_phase_2_fields() {
    let output = run(&[
        "check",
        "vulnerable/db_commit_then_publish.ts",
        "--format",
        "json",
    ]);
    let report = json(&output);
    let d = &report["diagnostics"][0];
    assert_eq!(d["pathCertainty"], "straightLine");
    assert_eq!(d["evidenceStrength"], "configuredName");
    assert!(d["fingerprint"].as_str().is_some_and(|f| f.len() == 16));
    assert!(d.get("baseline").is_none(), "only present with --baseline");
    let analysis = &report["summary"]["analysis"];
    assert_eq!(analysis["functionsWithEffects"], 1);
    assert_eq!(analysis["truncatedFunctions"], 0);
    assert!(analysis["crossFileCallsResolved"].is_number());
}

#[test]
fn policies_change_severity_not_codes() {
    let recommended = json(&run(&[
        "check",
        "vulnerable/independent_writes.ts",
        "--format",
        "json",
    ]));
    let strict = json(&run(&[
        "check",
        "vulnerable/independent_writes.ts",
        "--format",
        "json",
        "--policy",
        "strict",
    ]));
    assert_eq!(recommended["diagnostics"][0]["code"], "MW1004");
    assert_eq!(recommended["diagnostics"][0]["severity"], "info");
    assert_eq!(strict["diagnostics"][0]["code"], "MW1004");
    assert_eq!(strict["diagnostics"][0]["severity"], "warning");
    // A write that bypasses its transaction stays a warning.
    let bypass = json(&run(&[
        "check",
        "vulnerable/write_bypasses_transaction.ts",
        "--format",
        "json",
    ]));
    assert!(
        bypass["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "MW1004" && d["severity"] == "warning"),
        "{bypass}"
    );
    // The audit policy also reports unknown calls before the first commit.
    let audit = json(&run(&[
        "check",
        "vulnerable/charge_then_write.ts",
        "--format",
        "json",
        "--policy",
        "audit",
    ]));
    let default = json(&run(&[
        "check",
        "vulnerable/charge_then_write.ts",
        "--format",
        "json",
    ]));
    assert!(
        audit["diagnostics"].as_array().unwrap().len()
            >= default["diagnostics"].as_array().unwrap().len()
    );
}

#[test]
fn cross_module_findings_link_to_other_files() {
    let output = run(&["check", "projects/named_import", "--format", "sarif"]);
    assert_eq!(output.status.code(), Some(1));
    let sarif = json(&output);
    let result = &sarif["runs"][0]["results"][0];
    assert_eq!(result["ruleId"], "MW1001");
    assert_eq!(
        result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
        "projects/named_import/actions/checkout.ts"
    );
    let related: Vec<&str> = result["relatedLocations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            r["physicalLocation"]["artifactLocation"]["uri"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert!(
        related.contains(&"projects/named_import/services/order.ts"),
        "{related:?}"
    );
}

#[test]
fn legacy_crashproof_executable_behaves_like_midwrite() {
    let args = [
        "check",
        "vulnerable/db_commit_then_publish.ts",
        "--format",
        "json",
    ];
    let legacy = Command::new(env!("CARGO_BIN_EXE_crashproof"))
        .args(args)
        .current_dir(fixtures())
        .output()
        .unwrap();
    let current = run(&args);
    assert_eq!(legacy.status.code(), current.status.code());
    assert_eq!(legacy.stdout, current.stdout, "same report");
    assert!(stderr(&legacy).contains("has been renamed to `midwrite`"));
    assert!(!stderr(&current).contains("renamed"));
}

#[test]
fn legacy_configuration_is_a_fallback_and_never_merged() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("legacy-config");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("a.ts"),
        "export async function f(id: string) {\n  await store.update(orders).set({ status: \"paid\" });\n  await bus.send(\"x\", { id });\n}\n",
    )
    .unwrap();
    let legacy = r#"{ "databaseReceivers": ["store"], "externalEffects": [{ "receiver": "bus", "method": "send", "category": "message-publish" }] }"#;
    std::fs::write(dir.join("crashproof.json"), legacy).unwrap();

    // Only the legacy file: it is read, with a note.
    let output = run_in(&dir, &["check", ".", "--format", "json"]);
    let report = json(&output);
    assert_eq!(report["config"], "crashproof.json");
    assert_eq!(report["diagnostics"][0]["code"], "MW1001");
    assert!(
        stderr(&output).contains("legacy configuration"),
        "{}",
        stderr(&output)
    );

    // Both files: only midwrite.json is read; nothing from the legacy file
    // (its receivers) leaks in.
    std::fs::write(dir.join("midwrite.json"), "{}").unwrap();
    let output = run_in(&dir, &["check", ".", "--format", "json"]);
    let report = json(&output);
    assert_eq!(report["config"], "midwrite.json");
    assert_eq!(report["diagnostics"].as_array().unwrap().len(), 0);
    assert!(
        stderr(&output).contains("ignoring crashproof.json"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn legacy_thread_variable_is_honored() {
    let output = Command::new(env!("CARGO_BIN_EXE_midwrite"))
        .args(["check", ".", "--format", "json", "--fail-on", "never"])
        .current_dir(fixtures())
        .env("CRASHPROOF_THREADS", "1")
        .output()
        .unwrap();
    let parallel = run(&["check", ".", "--format", "json", "--fail-on", "never"]);
    assert_eq!(
        output.stdout, parallel.stdout,
        "results do not depend on threads"
    );
}

#[test]
fn trace_command_prints_static_candidates() {
    let output = run(&["trace", "vulnerable/db_commit_then_publish.ts"]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(text.contains("STATIC_CANDIDATE"), "{text}");
    assert!(
        text.contains("failure injection boundary (after-db-commit / before-event-publish)"),
        "{text}"
    );
    assert!(text.contains("not reproduced failures"), "{text}");

    let report = json(&run(&["trace", "vulnerable", "--format", "json"]));
    assert_eq!(report["kind"], "commitrift.traces");
    assert_eq!(report["schemaVersion"], 2);
    let check = json(&run(&[
        "check",
        "vulnerable",
        "--format",
        "json",
        "--fail-on",
        "never",
    ]));
    let fingerprints: Vec<&str> = check["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["fingerprint"].as_str().unwrap())
        .collect();
    for trace in report["traces"].as_array().unwrap() {
        assert!(fingerprints.contains(&trace["findingId"].as_str().unwrap()));
    }
    let id = report["traces"][0]["findingId"].as_str().unwrap();
    let one = json(&run(&[
        "trace",
        "vulnerable",
        "--format",
        "json",
        "--finding",
        id,
    ]));
    assert_eq!(one["traces"].as_array().unwrap().len(), 1);
    assert_eq!(
        run(&["trace", "vulnerable", "--finding", "nope"])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn html_reports_escape_untrusted_text_and_load_nothing() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("html-report");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("a.ts"),
        "export async function f(id: string) {\n  await db.update(orders).set({ note: \"</code><script>alert(1)</script>\" });\n  await queue.publish(\"<img src=x onerror=alert(2)>\", { id });\n}\n",
    )
    .unwrap();
    let traces = run_in(&dir, &["trace", ".", "--format", "json"]);
    std::fs::write(dir.join("traces.json"), &traces.stdout).unwrap();
    let output = run_in(&dir, &["report", "traces.json", "--format", "html"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let html = stdout(&output);
    assert!(html.starts_with("<!DOCTYPE html>"));
    assert!(html.contains("script-src 'none'"), "a CSP forbids scripts");
    assert!(!html.contains("<script"), "no script element at all");
    assert!(!html.contains("<img"), "no injected element");
    assert!(html.contains("&lt;img src=x onerror=alert(2)&gt;"));
    assert!(
        !html.contains("http://") && !html.contains("https://"),
        "nothing is loaded"
    );
    assert!(html.contains("STATIC CANDIDATE — not executed"));
    assert!(html.contains("SIMULATED — abstract model, not executed"));
    assert!(html.contains("failure injection boundary"));

    // Output to a file; other JSON is rejected.
    let output = run_in(&dir, &["report", "traces.json", "-o", "out.html"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(dir.join("out.html").is_file());
    let check = run_in(
        &dir,
        &["check", ".", "--format", "json", "--fail-on", "never"],
    );
    std::fs::write(dir.join("check.json"), &check.stdout).unwrap();
    let output = run_in(&dir, &["report", "check.json"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("neither a trace report"));
}

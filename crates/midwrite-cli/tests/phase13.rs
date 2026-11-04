//! Phase 13: return values, the response step, upserts and read replicas,
//! end to end on the first-party request application
//! (`fixtures/phase13/request-app`): the model's verdict on the extracted
//! TypeScript, and the original code run under the model's counterexample
//! against a disposable PostgreSQL.
//!
//! Runs that need PostgreSQL or Node skip without them unless
//! `MIDWRITE_REQUIRE_REPLAY=1`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value as Json;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_midwrite"))
}

macro_rules! require_runtime {
    () => {
        let missing = midwrite_replay::postgres::Binaries::discover()
            .err()
            .or_else(|| {
                Command::new("node")
                    .arg("--version")
                    .output()
                    .is_err()
                    .then(|| "node is not installed".to_owned())
            });
        if let Some(why) = missing {
            assert!(
                std::env::var_os("MIDWRITE_REQUIRE_REPLAY").is_none(),
                "PostgreSQL and Node are required: {why}"
            );
            eprintln!("skipping: {why}");
            return;
        }
    };
}

fn app() -> PathBuf {
    repo().join("fixtures/phase13/request-app")
}

fn build_fixture(dir: &Path) {
    if dir.join("dist/verify").is_dir() {
        return;
    }
    let status = Command::new("npm")
        .args(["run", "build"])
        .current_dir(dir)
        .status()
        .expect("npm");
    assert!(status.success(), "building {} failed", dir.display());
}

fn model_check(scenario: &Path, extra: &[&str]) -> Json {
    let output = bin()
        .arg("model-check")
        .arg(scenario)
        .args(extra)
        .args(["--format", "json", "--no-timing"])
        .output()
        .unwrap();
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "{}: {e}\n{}",
            scenario.display(),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn scenario(name: &str) -> PathBuf {
    app().join("scenarios").join(format!("{name}.json"))
}

fn verdict(mc: &Json) -> &str {
    mc["exploration"]["verdict"].as_str().unwrap_or_default()
}

/// The counterexample of `name` runs on the original code three times and
/// every run reproduces it at `level`.
fn reproduces(name: &str, level: &str) -> Json {
    let mc = model_check(
        &scenario(name),
        &[
            "--verify-source",
            "--allow-execution",
            "--source-repeat",
            "3",
        ],
    );
    assert_eq!(verdict(&mc), "COUNTEREXAMPLE_FOUND", "{name}: {mc:#}");
    assert_eq!(mc["evidence"]["level"], level, "{name}: {mc:#}");
    let runs = mc["sourceVerification"]["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 3);
    assert!(
        runs.iter().all(|r| r["reproduced"] == true),
        "{name}: {mc:#}"
    );
    mc
}

fn holds(name: &str) {
    let mc = model_check(&scenario(name), &[]);
    assert_eq!(
        verdict(&mc),
        "NO_COUNTEREXAMPLE_WITHIN_BOUNDS",
        "{name}: {mc:#}"
    );
    assert!(
        mc["exploration"]["unsupported"]
            .as_array()
            .is_none_or(|u| u.is_empty()),
        "{name}"
    );
}

#[test]
fn both_callers_told_they_claimed_the_task_reproduces_on_the_original_code() {
    require_runtime!();
    build_fixture(&app());
    // The database state is acceptable in every run; the violation is in
    // the values the original functions returned to their callers.
    let mc = reproduces("claim-race", "original-code-reproduced");
    let invariant = mc["invariants"][0]["formula"].as_str().unwrap_or_default();
    assert!(invariant.contains("returning true"), "{invariant}");
    holds("claim-atomic");
}

#[test]
fn a_crash_after_the_insert_and_before_the_response_duplicates_it_in_the_original_code() {
    require_runtime!();
    build_fixture(&app());
    let mc = reproduces("signup-crash", "original-code-crash-reproduced");
    let crash = &mc["sourceVerification"]["recovery"]["crashes"][0];
    assert!(
        crash["boundary"]
            .as_str()
            .unwrap_or_default()
            .contains("RESPONSE"),
        "the process is killed at the response checkpoint: {crash:#}"
    );
    holds("signup-idempotent");
}

#[test]
fn upserts_match_the_original_code() {
    require_runtime!();
    build_fixture(&app());
    reproduces("reserve-race", "original-code-reproduced");
    holds("reserve-if-free");
    holds("count-views");
}

#[test]
fn a_stale_replica_read_is_a_model_counterexample_that_is_not_replayed() {
    let path = repo().join("fixtures/phase13/model/replica-read-your-writes.json");
    let mc = model_check(&path, &[]);
    assert_eq!(verdict(&mc), "COUNTEREXAMPLE_FOUND", "{mc:#}");
    // Without the feature the replica read is refused, never bound to the
    // primary.
    let text = std::fs::read_to_string(&path).unwrap();
    let without = text.replace("\"features\": [\"replicas\"],", "");
    let tmp =
        std::env::temp_dir().join(format!("midwrite-p13-{}-replica.json", std::process::id()));
    std::fs::write(&tmp, without).unwrap();
    let output = bin()
        .arg("model-check")
        .arg(&tmp)
        .args(["--format", "json", "--no-timing"])
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&tmp);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("feature `replicas`")
            || String::from_utf8_lossy(&output.stdout).contains("feature `replicas`")
    );
}

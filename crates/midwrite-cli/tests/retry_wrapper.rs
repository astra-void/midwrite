//! Source coverage end to end on the first-party request
//! application (`fixtures/apps/retry-wrapper-app`): a callback passed to the
//! application's own retry wrapper is inlined where the wrapper calls it,
//! discovery finds the write the wrapper repeats after a lost COMMIT
//! acknowledgment, and the original code follows the model's
//! counterexample against a disposable PostgreSQL.
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
    repo().join("fixtures/apps/retry-wrapper-app")
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

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "midwrite-phase14-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn discover(extra: &[&str]) -> Json {
    let output = bin()
        .arg("discover")
        .arg(app())
        .args(["--model-check", "--format", "json", "--no-timing"])
        .args(extra)
        .output()
        .unwrap();
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!("{e}\n{}", String::from_utf8_lossy(&output.stderr));
    })
}

fn candidate<'a>(report: &'a Json, class: &str, entry: &str) -> &'a Json {
    report["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| {
            c["class"] == class
                && c["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|e| e["selector"].as_str().unwrap().ends_with(entry))
        })
        .unwrap_or_else(|| panic!("no {class} candidate of {entry}: {report:#}"))
}

#[test]
fn the_write_a_retry_wrapper_repeats_after_a_lost_commit_is_found_and_the_gated_one_is_not() {
    let r = discover(&["--entry", "payments"]);
    let c = candidate(&r, "ambiguous-commit", "#settlePayment");
    assert_eq!(c["status"], "MODEL_COUNTEREXAMPLE", "{c:#}");
    assert!(
        c["assumptions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["text"]
                .as_str()
                .is_some_and(|t| t.contains("transaction's COMMIT can be lost"))),
        "{c:#}"
    );
    let safe = candidate(&r, "ambiguous-commit", "#settlePaymentOnce");
    assert_eq!(safe["status"], "NO_VIOLATION_WITHIN_BOUNDS", "{safe:#}");
    // Every candidate says what to do next.
    for c in r["candidates"].as_array().unwrap() {
        assert!(
            !c["nextStep"].as_str().unwrap_or_default().is_empty(),
            "{c:#}"
        );
    }
}

#[test]
fn a_lost_commit_inside_the_retry_wrapper_reproduces_on_the_original_code() {
    require_runtime!();
    build_fixture(&app());
    let out = Temp::new("retry");
    let r = discover(&[
        "--entry",
        "payments.ts#settlePayment",
        "--emit-scenarios",
        out.0.to_str().unwrap(),
    ]);
    let c = candidate(&r, "ambiguous-commit", "#settlePayment");
    let file = c["scenarios"][0]["file"].as_str().unwrap();
    let output = bin()
        .arg("model-check")
        .arg(out.0.join(file))
        .args([
            "--verify-source",
            "--allow-execution",
            "--source-repeat",
            "3",
            "--format",
            "json",
            "--no-timing",
        ])
        .output()
        .unwrap();
    let mc: Json = serde_json::from_slice(&output.stdout).unwrap();
    let sv = &mc["sourceVerification"];
    assert_eq!(sv["status"], "reproduced", "{sv:#}");
    assert_eq!(
        mc["evidence"]["level"], "original-code-connection-loss-reproduced",
        "{mc:#}"
    );
    // The retried transaction committed a second payment, the first one's
    // generated key matched at the loss checkpoint.
    for run in sv["runs"].as_array().unwrap() {
        assert_eq!(run["recoveryAgrees"], true, "{run:#}");
    }
}

#[test]
fn a_resubmitted_multi_row_insert_duplicates_lines_in_the_original_code() {
    // Slice 3: one two-row INSERT, a crash after it committed and before
    // the response, the client's resubmission; the keyed variant holds.
    let r = discover(&["--entry", "items.ts"]);
    let c = candidate(&r, "resubmitted-write", "#addOrderItems");
    assert_eq!(c["status"], "MODEL_COUNTEREXAMPLE", "{c:#}");
    let safe = candidate(&r, "resubmitted-write", "#addOrderItemsIdempotent");
    assert_eq!(safe["status"], "NO_VIOLATION_WITHIN_BOUNDS", "{safe:#}");
    require_runtime!();
    build_fixture(&app());
    let out = Temp::new("items");
    let r = discover(&[
        "--entry",
        "items.ts#addOrderItems",
        "--emit-scenarios",
        out.0.to_str().unwrap(),
    ]);
    let c = candidate(&r, "resubmitted-write", "#addOrderItems");
    let file = c["scenarios"][0]["file"].as_str().unwrap();
    let output = bin()
        .arg("model-check")
        .arg(out.0.join(file))
        .args([
            "--verify-source",
            "--allow-execution",
            "--source-repeat",
            "3",
            "--format",
            "json",
            "--no-timing",
        ])
        .output()
        .unwrap();
    let mc: Json = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(mc["sourceVerification"]["status"], "reproduced", "{mc:#}");
    assert_eq!(mc["evidence"]["level"], "original-code-crash-reproduced");
}

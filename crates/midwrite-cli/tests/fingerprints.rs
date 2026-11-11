use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

const CALLBACK: &str = include_str!("../../../fixtures/vulnerable/fingerprint_callback.ts");

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_midwrite"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)))
}

#[test]
fn callback_fingerprints_match_baselines_and_sarif_after_source_edits() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("callback-fingerprints");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("pay.ts"), CALLBACK).unwrap();
    let written = run(
        &dir,
        &[
            "check",
            "pay.ts",
            "--write-baseline",
            "baseline.json",
            "--format",
            "json",
        ],
    );
    assert_eq!(written.status.code(), Some(0));
    let original = json(&written);
    assert_eq!(original["diagnostics"].as_array().unwrap().len(), 1);
    let id = original["diagnostics"][0]["fingerprint"].as_str().unwrap();
    assert_eq!(id.len(), 16);

    let moved = format!(
        "// added above the callback\n\n{}",
        CALLBACK.replace("register(async", "register(\n  async")
    );
    std::fs::write(dir.join("pay.ts"), &moved).unwrap();
    let checked = run(
        &dir,
        &[
            "check",
            "pay.ts",
            "--baseline",
            "baseline.json",
            "--format",
            "json",
        ],
    );
    assert_eq!(checked.status.code(), Some(0));
    let report = json(&checked);
    assert_eq!(report["diagnostics"][0]["fingerprint"], id);
    assert_eq!(report["diagnostics"][0]["baseline"], "existing");
    assert_eq!(
        report["summary"]["baseline"],
        serde_json::json!({"new": 0, "existing": 1, "resolved": 0})
    );

    let sarif = run(
        &dir,
        &[
            "check",
            "pay.ts",
            "--baseline",
            "baseline.json",
            "--format",
            "sarif",
        ],
    );
    assert_eq!(sarif.status.code(), Some(0));
    let sarif = json(&sarif);
    assert_eq!(
        sarif["runs"][0]["results"][0]["partialFingerprints"]["midwrite/v1"],
        id
    );
    assert_eq!(sarif["runs"][0]["results"][0]["baselineState"], "unchanged");

    std::fs::write(
        dir.join("pay.ts"),
        moved.replace("update(orders)", "update(payments)"),
    )
    .unwrap();
    let changed = run(
        &dir,
        &[
            "check",
            "pay.ts",
            "--baseline",
            "baseline.json",
            "--format",
            "json",
        ],
    );
    assert_eq!(changed.status.code(), Some(1));
    let report = json(&changed);
    assert_ne!(report["diagnostics"][0]["fingerprint"], id);
    assert_eq!(report["diagnostics"][0]["baseline"], "new");
    assert_eq!(
        report["summary"]["baseline"],
        serde_json::json!({"new": 1, "existing": 0, "resolved": 1})
    );
    std::fs::remove_dir_all(dir).unwrap();
}

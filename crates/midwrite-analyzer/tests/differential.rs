//! Differential tests (Phase 5, Milestone 10.3): for small programs under
//! `fixtures/differential/`, compare the operations on the statically
//! reported path with the operations a controlled execution performs.
//!
//! The programs use a fake, Drizzle- and queue-shaped runtime
//! (`fixtures/differential/runtime.ts`) that records each operation when it
//! completes; no database is involved. They are run with Node.js
//! (`--experimental-transform-types`). Without Node.js the tests are skipped
//! with a message; set `MIDWRITE_REQUIRE_REPLAY=1` to make that a failure.
//!
//! A finite set of agreeing runs is evidence for these programs only, not a
//! proof that the static model matches JavaScript semantics in general.

use std::path::{Path, PathBuf};
use std::process::Command;

use midwrite_analyzer::Analyzer;
use midwrite_core::config::Config;
use midwrite_core::trace::{Completeness, FailureTrace, StepKind};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn node_missing() -> Option<String> {
    match Command::new("node").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            let major: u32 = version
                .trim()
                .trim_start_matches('v')
                .split('.')
                .next()
                .and_then(|m| m.parse().ok())
                .unwrap_or(0);
            // `--experimental-transform-types` exists since Node.js 22.7.
            (major < 22).then(|| format!("Node.js {} is too old (22.7+ needed)", version.trim()))
        }
        _ => Some("`node` is not installed".to_owned()),
    }
}

macro_rules! require_node {
    () => {
        if let Some(reason) = node_missing() {
            assert!(
                std::env::var_os("MIDWRITE_REQUIRE_REPLAY").is_none(),
                "differential tests require Node.js: {reason}"
            );
            eprintln!("SKIPPED: {reason}");
            return;
        }
    };
}

/// The operations the program performs at runtime, in completion order.
fn observed(program: &str) -> Vec<String> {
    let dir = fixtures().join("differential");
    let output = Command::new("node")
        .args([
            "--experimental-transform-types",
            "--no-warnings",
            "run.mjs",
            &format!("./{program}"),
        ])
        .current_dir(&dir)
        .output()
        .expect("node runs");
    assert!(
        output.status.success(),
        "{program}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON operation log")
}

/// The trace of the finding `code` in `function`.
fn predicted(program: &str, code: &str, function: &str) -> FailureTrace {
    let config = Config::load(&fixtures().join("midwrite.json")).unwrap();
    let report = Analyzer::new(config)
        .trace_project(&[fixtures().join("differential").join(program)])
        .unwrap()
        .into_trace_report(None);
    report
        .traces
        .into_iter()
        .find(|t| t.code == code && t.function == function)
        .unwrap_or_else(|| panic!("{program}: no {code} trace in {function}"))
}

/// The recognized operations of a trace in the runtime's notation.
fn operations(trace: &FailureTrace) -> Vec<String> {
    trace
        .steps
        .iter()
        .filter_map(|step| match step.kind {
            StepKind::DbWrite => Some(format!(
                "write:{}:{}",
                step.operation.as_deref().unwrap_or("?"),
                step.table.as_deref().unwrap_or("?")
            )),
            // The topic: the first string literal of the label, or the
            // parameter the helper received (concurrent.ts passes it).
            StepKind::External => Some(match step.label.split('"').nth(1) {
                Some(topic) => format!("external:{topic}"),
                None => "external:<argument>".to_owned(),
            }),
            _ => None,
        })
        .collect()
}

#[test]
fn ordered_programs_match_their_static_paths() {
    require_node!();
    for (program, function) in [
        ("sequential.ts", "completeOrder"),
        ("callback.ts", "completeOrder"),
        ("class_dispatch.ts", "OrderService.complete"),
    ] {
        let trace = predicted(program, "MW1001", function);
        assert_eq!(trace.completeness, Completeness::Complete, "{program}");
        assert_eq!(operations(&trace), observed(program), "{program}");
    }
}

#[test]
fn concurrent_programs_agree_on_the_set_not_the_order() {
    require_node!();
    let trace = predicted("concurrent.ts", "MW1001", "completeOrder");
    // Statically the completion order of the two publications is not
    // determined: the trace is an approximation.
    assert_eq!(trace.completeness, Completeness::Approximated);
    let static_ops = operations(&trace);
    let runtime = observed("concurrent.ts");
    assert_eq!(
        static_ops.first(),
        runtime.first(),
        "the commit comes first"
    );
    assert_eq!(static_ops.len(), runtime.len());
    // The topics are parameters of the helper statically; at runtime the
    // slower publication completes last, opposite to source order.
    assert!(runtime[1..].iter().all(|op| op.starts_with("external:")));
    assert_eq!(runtime[2], "external:order.completed.slow");
}

#[test]
fn a_rejected_helper_stops_the_path_after_its_commit() {
    require_node!();
    let trace = predicted("rejected.ts", "MW1001", "completeOrder");
    let static_ops = operations(&trace);
    let runtime = observed("rejected.ts");
    // The run takes the throwing path: the commit happened, the publication
    // did not. Statically the publication is conditional on the helper's
    // branch, which the trace records between the two operations.
    assert_eq!(runtime, ["write:update:orders"]);
    assert_eq!(static_ops[0], runtime[0]);
    let write = trace
        .steps
        .iter()
        .position(|s| s.kind == StepKind::DbWrite)
        .unwrap();
    let external = trace
        .steps
        .iter()
        .position(|s| s.kind == StepKind::External)
        .unwrap();
    assert!(
        trace.steps[write..external]
            .iter()
            .any(|s| s.kind == StepKind::Branch),
        "{:#?}",
        trace.steps
    );
}

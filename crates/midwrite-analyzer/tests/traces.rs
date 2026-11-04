//! Failure traces (Phase 3, Milestone 1): structure, validation against the
//! analysis model, determinism, and agreement with the diagnostics.

use std::path::{Path, PathBuf};

use midwrite_analyzer::Analyzer;
use midwrite_core::analysis::AnalysisOptions;
use midwrite_core::config::Config;
use midwrite_core::trace::{
    BoundaryKind, Completeness, FailureTrace, StepKind, StepRole, StepState, TraceReport,
};
use midwrite_core::verification::{FailureMode, VerificationLevel};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn analyzer() -> Analyzer {
    Analyzer::new(Config::load(&fixtures_dir().join("midwrite.json")).expect("fixture config"))
}

fn traces_of(analyzer: &Analyzer, inputs: &[PathBuf]) -> TraceReport {
    analyzer
        .trace_project(inputs)
        .expect("inputs exist")
        .into_trace_report(None)
}

/// A scratch directory with the given files (removed on drop).
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let dir =
            std::env::temp_dir().join(format!("midwrite-traces-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (file, source) in files {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, source).unwrap();
        }
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn single(report: &TraceReport, code: &str) -> FailureTrace {
    let matching: Vec<&FailureTrace> = report.traces.iter().filter(|t| t.code == code).collect();
    assert_eq!(matching.len(), 1, "expected one {code} trace: {report:#?}");
    matching[0].clone()
}

fn kinds(trace: &FailureTrace) -> Vec<StepKind> {
    trace.steps.iter().map(|s| s.kind).collect()
}

#[test]
fn traces_do_not_change_the_diagnostics() {
    let analyzer = analyzer();
    let plain = analyzer
        .analyze_paths(&[fixtures_dir()], None)
        .expect("fixtures");
    let traced = analyzer
        .trace_project(&[fixtures_dir()])
        .expect("fixtures")
        .into_report(None);
    assert_eq!(
        serde_json::to_string(&plain).unwrap(),
        serde_json::to_string(&traced).unwrap()
    );
}

#[test]
fn every_fixture_trace_is_valid_deterministic_and_linked_to_a_finding() {
    let analyzer = analyzer();
    let report = traces_of(&analyzer, &[fixtures_dir()]);
    let again = traces_of(&analyzer, &[fixtures_dir()]);
    assert_eq!(report, again, "trace output is deterministic");
    assert!(report.traces.len() >= 50, "{}", report.traces.len());

    let diagnostics = analyzer
        .analyze_paths(&[fixtures_dir()], None)
        .expect("fixtures")
        .diagnostics;
    assert_eq!(
        report.summary.findings,
        diagnostics.len(),
        "every diagnostic is traced or skipped"
    );
    let mut ids = std::collections::BTreeSet::new();
    for trace in &report.traces {
        assert!(
            trace.validate().is_empty(),
            "{}: {:?}",
            trace.id,
            trace.validate()
        );
        assert_eq!(trace.verification, VerificationLevel::StaticCandidate);
        assert_eq!(trace.simulation.verification, VerificationLevel::Simulated);
        assert_eq!(trace.simulation.failure_mode, FailureMode::ProcessKill);
        assert_eq!(trace.id, trace.compute_id());
        assert!(
            ids.insert(trace.id.clone()),
            "duplicate trace id {}",
            trace.id
        );
        let diagnostic = diagnostics
            .iter()
            .find(|d| d.fingerprint == trace.finding_id)
            .unwrap_or_else(|| panic!("{} has no diagnostic", trace.finding_id));
        assert_eq!(diagnostic.code.as_str(), trace.code);
        assert_eq!(diagnostic.location, trace.location);
        assert!(!trace.assumptions.is_empty());
        assert!(
            trace
                .limitations
                .iter()
                .any(|l| l.contains("static candidate")),
            "{}",
            trace.id
        );
        assert!(
            !trace.injection.checkpoints.is_empty(),
            "{}: no checkpoint hint",
            trace.id
        );
        // No operation is fabricated: every effect step has a source location.
        for step in &trace.steps {
            assert!(step.location.is_some(), "{}: step {}", trace.id, step.index);
        }
    }
    for skipped in &report.skipped {
        assert_eq!(skipped.code, "MW9001", "only limitations lack a trace");
    }
}

#[test]
fn db_commit_then_external_effect() {
    let report = traces_of(
        &analyzer(),
        &[fixtures_dir().join("vulnerable/db_commit_then_publish.ts")],
    );
    let trace = single(&report, "MW1001");
    assert_eq!(kinds(&trace), [StepKind::DbWrite, StepKind::External]);
    assert_eq!(trace.completeness, Completeness::Complete);
    assert_eq!(
        trace.injection.boundary,
        BoundaryKind::AfterCommitBeforeExternal
    );
    assert_eq!(trace.injection.after_step, 0);
    assert_eq!(trace.injection.before_step, Some(1));
    let names: Vec<&str> = trace
        .injection
        .checkpoints
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, ["after-db-commit", "before-event-publish"]);
    assert_eq!(
        trace.injection.checkpoints[0].resource.as_deref(),
        Some("orders")
    );
    assert_eq!(trace.steps[0].at_failure, StepState::Committed);
    assert_eq!(trace.steps[0].role, Some(StepRole::CompletedFirst));
    assert_eq!(trace.steps[1].at_failure, StepState::NotReached);
    assert_eq!(trace.steps[1].role, Some(StepRole::NotYetPerformed));
    assert_eq!(trace.simulation.durable, [0]);
    assert_eq!(trace.simulation.not_performed, [1]);
    assert!(trace.assumptions.iter().any(|a| a.contains("auto-commit")));
}

#[test]
fn external_effect_then_db_commit() {
    let report = traces_of(
        &analyzer(),
        &[fixtures_dir().join("vulnerable/publish_then_db_commit.ts")],
    );
    let trace = single(&report, "MW1002");
    assert_eq!(
        trace.injection.boundary,
        BoundaryKind::AfterExternalBeforeCommit
    );
    assert_eq!(kinds(&trace), [StepKind::External, StepKind::DbWrite]);
    assert_eq!(trace.steps[0].at_failure, StepState::Performed);
    assert_eq!(trace.injection.checkpoints[1].name, "before-db-write");
}

#[test]
fn external_effect_inside_an_active_transaction() {
    let report = traces_of(
        &analyzer(),
        &[fixtures_dir().join("vulnerable/publish_inside_transaction.ts")],
    );
    let trace = single(&report, "MW1003");
    assert_eq!(
        kinds(&trace),
        [
            StepKind::TxBegin,
            StepKind::DbWrite,
            StepKind::External,
            StepKind::TxCommit
        ]
    );
    assert_eq!(trace.steps[1].at_failure, StepState::Staged);
    assert_eq!(trace.steps[1].transaction.as_deref(), Some("tx#0"));
    assert_eq!(trace.simulation.lost, [1]);
    assert_eq!(trace.simulation.durable, [2]);
    assert_eq!(trace.injection.open_transactions, ["tx#0"]);
    assert_eq!(trace.injection.checkpoints[1].name, "before-tx-commit");
}

#[test]
fn transactional_outbox_and_exclusive_branches_have_no_trace() {
    let report = traces_of(
        &analyzer(),
        &[
            fixtures_dir().join("safe/transactional_outbox.ts"),
            fixtures_dir().join("safe/mutually_exclusive.ts"),
        ],
    );
    assert!(report.traces.is_empty(), "{report:#?}");
    assert_eq!(report.summary.findings, 0);
}

#[test]
fn cross_module_effects_keep_their_call_chain_and_file() {
    let report = traces_of(&analyzer(), &[fixtures_dir().join("projects/call_chain")]);
    let trace = single(&report, "MW1001");
    let write = &trace.steps[0];
    assert_eq!(write.kind, StepKind::DbWrite);
    let chain: Vec<&str> = write.call_chain.iter().map(|f| f.callee.as_str()).collect();
    assert_eq!(chain, ["completeOrder", "updateOrder"]);
    let file = &write.location.as_ref().unwrap().file;
    assert!(file.ends_with("repository.ts"), "{file}");
    assert!(trace.location.file.ends_with("action.ts"));
}

#[test]
fn transaction_handles_passed_to_helpers_stage_their_writes() {
    let report = traces_of(&analyzer(), &[fixtures_dir().join("projects/tx_handle")]);
    let trace = single(&report, "MW1003");
    let helper_writes: Vec<_> = trace
        .steps
        .iter()
        .filter(|s| s.kind == StepKind::DbWrite && !s.call_chain.is_empty())
        .collect();
    assert!(!helper_writes.is_empty());
    for write in helper_writes {
        assert_eq!(write.at_failure, StepState::Staged, "{}", write.label);
        assert_eq!(write.transaction.as_deref(), Some("tx#0"));
    }
}

#[test]
fn conditional_effects_record_the_branch_as_an_assumption() {
    let report = traces_of(
        &analyzer(),
        &[fixtures_dir().join("vulnerable/conditional_publish.ts")],
    );
    let trace = single(&report, "MW1001");
    assert_eq!(
        kinds(&trace),
        [StepKind::DbWrite, StepKind::Branch, StepKind::External]
    );
    assert_eq!(trace.injection.window, [1]);
    assert_eq!(trace.steps[1].role, Some(StepRole::InWindow));
    assert!(trace.assumptions.iter().any(|a| a.contains("step(s) 1")));
}

#[test]
fn unknown_effects_in_the_window_make_the_trace_incomplete() {
    let scratch = Scratch::new(
        "unknown",
        &[(
            "a.ts",
            r#"
import { db } from "@/db";
import { notify } from "some-package";
export async function f(id: string) {
  await db.update(orders).set({ status: "paid" });
  await notify(id);
  await queue.publish("order.paid", { id });
}
"#,
        )],
    );
    let report = traces_of(&analyzer(), std::slice::from_ref(&scratch.0));
    let trace = single(&report, "MW1001");
    assert_eq!(trace.completeness, Completeness::Incomplete);
    assert_eq!(trace.steps[1].kind, StepKind::Unknown);
    assert!(
        trace
            .limitations
            .iter()
            .any(|l| l.contains("semantics are unknown"))
    );
}

#[test]
fn truncated_functions_are_skipped_or_marked() {
    let mut body =
        String::from("import { db } from \"@/db\";\nexport async function f(a: boolean[]) {\n");
    for i in 0..12 {
        body.push_str(&format!(
            "  if (a[{i}]) {{ await db.insert(t{i}).values({{}}); }}\n"
        ));
    }
    body.push_str("  await queue.publish(\"x\", {});\n}\n");
    let scratch = Scratch::new("truncated", &[("a.ts", &body)]);
    let analyzer = analyzer().with_options(AnalysisOptions {
        max_paths: 8,
        ..AnalysisOptions::default()
    });
    let report = traces_of(&analyzer, std::slice::from_ref(&scratch.0));
    assert!(
        report
            .skipped
            .iter()
            .any(|s| s.code == "MW9001" && s.reason.contains("limitation")),
        "{report:#?}"
    );
    assert!(!report.traces.is_empty());
    for trace in &report.traces {
        assert!(
            trace.limitations.iter().any(|l| l.contains("path budget")),
            "{}",
            trace.id
        );
        assert!(trace.validate().is_empty());
    }
}

#[test]
fn trace_ids_survive_moving_code() {
    let source = r#"
import { db } from "@/db";
export async function f(id: string) {
  await db.update(orders).set({ status: "paid" });
  await queue.publish("order.paid", { id });
}
"#;
    let scratch = Scratch::new("move", &[("a.ts", source)]);
    let first = single(
        &traces_of(&analyzer(), std::slice::from_ref(&scratch.0)),
        "MW1001",
    );
    let moved = format!("\n\n// a comment\n{source}");
    std::fs::write(scratch.0.join("a.ts"), moved).unwrap();
    let second = single(
        &traces_of(&analyzer(), std::slice::from_ref(&scratch.0)),
        "MW1001",
    );
    assert_eq!(first.finding_id, second.finding_id);
    assert_eq!(first.id, second.id);
    assert_ne!(first.location.line, second.location.line);
}

#[test]
fn duplicate_findings_get_the_same_suffixed_ids_as_the_report() {
    let source = r#"
import { db } from "@/db";
export async function f(id: string, again: boolean) {
  await db.update(orders).set({ status: "paid" });
  await queue.publish("order.paid", { id });
  if (again) {
    await db.update(orders).set({ status: "paid" });
    await queue.publish("order.paid", { id });
  }
}
"#;
    let scratch = Scratch::new("dupes", &[("a.ts", source)]);
    let analyzer = analyzer();
    let report = analyzer
        .analyze_paths(std::slice::from_ref(&scratch.0), None)
        .unwrap();
    let traces = traces_of(&analyzer, std::slice::from_ref(&scratch.0));
    let mut report_ids: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|d| d.fingerprint.as_str())
        .collect();
    let mut trace_ids: Vec<&str> = traces
        .traces
        .iter()
        .map(|t| t.finding_id.as_str())
        .chain(traces.skipped.iter().map(|s| s.finding_id.as_str()))
        .collect();
    report_ids.sort_unstable();
    trace_ids.sort_unstable();
    assert_eq!(report_ids, trace_ids);
}

#[test]
fn trace_report_round_trips_through_json() {
    let report = traces_of(
        &analyzer(),
        &[fixtures_dir().join("vulnerable/publish_inside_transaction.ts")],
    );
    let json = serde_json::to_string(&report).unwrap();
    let back: TraceReport = serde_json::from_str(&json).unwrap();
    assert_eq!(back, report);
    assert_eq!(back.kind, "commitrift.traces");
    // Phase 5: schema v2 adds the optional per-trace `valueRelation`.
    assert_eq!(back.schema_version, 2);
    // A v1 report (no `valueRelation`) is still read.
    let mut v1: serde_json::Value = serde_json::from_str(&json).unwrap();
    v1["schemaVersion"] = serde_json::json!(1);
    for trace in v1["traces"].as_array_mut().unwrap() {
        trace.as_object_mut().unwrap().remove("valueRelation");
    }
    let old: TraceReport = serde_json::from_value(v1).unwrap();
    assert!(old.traces.iter().all(|t| t.value_relation.is_none()));
}

#[test]
fn replay_harness_applications_have_the_documented_findings() {
    let analyze = |harness: &str| {
        let root = fixtures_dir().join("replay").join(harness);
        let analyzer =
            Analyzer::new(Config::load(&root.join("midwrite.json")).expect("harness config"));
        analyzer
            .trace_project_at(&root, &[PathBuf::from("src")])
            .expect("harness sources")
            .into_trace_report(None)
    };
    let vulnerable = analyze("order-vulnerable");
    let mut found: Vec<(String, String, String)> = vulnerable
        .traces
        .iter()
        .map(|t| (t.code.clone(), t.function.clone(), t.location.file.clone()))
        .collect();
    found.sort();
    assert_eq!(
        found,
        [
            (
                "MW1001".into(),
                "completeOrder".into(),
                "src/orders.ts".into()
            ),
            (
                "MW1001".into(),
                "completeOrderWithCompensation".into(),
                "src/orders.ts".into()
            ),
            (
                "MW1004".into(),
                "completeOrderWithCompensation".into(),
                "src/orders.ts".into()
            ),
        ]
    );
    for trace in &vulnerable.traces {
        assert!(trace.validate().is_empty());
    }
    // The outbox version: no finding for the order flow; the relay's
    // publish-then-acknowledge window (at-least-once delivery) is MW1002.
    let outbox = analyze("order-outbox");
    let found: Vec<(&str, &str)> = outbox
        .traces
        .iter()
        .map(|t| (t.code.as_str(), t.function.as_str()))
        .collect();
    assert_eq!(found, [("MW1002", "relayOutbox")]);
    assert!(outbox.skipped.is_empty());
}

/// Phase 4: the application on the real Drizzle ORM (`drizzle(pool)` from
/// `drizzle-orm/node-postgres`, NodeNext `.js` import specifiers).
#[test]
fn the_real_drizzle_application_has_exactly_the_documented_findings() {
    let root = fixtures_dir().join("replay/drizzle-node");
    let analyzer =
        Analyzer::new(Config::load(&root.join("midwrite.json")).expect("harness config"));
    let report = analyzer
        .trace_project_at(&root, &[PathBuf::from("src")])
        .expect("harness sources")
        .into_trace_report(None);
    let mut found: Vec<(String, String, String, String)> = report
        .traces
        .iter()
        .map(|t| {
            (
                t.code.clone(),
                t.function.clone(),
                t.location.file.clone(),
                t.finding_id.clone(),
            )
        })
        .collect();
    found.sort();
    // The fingerprints are pinned: replay cases and generated harnesses
    // link to them.
    assert_eq!(
        found,
        [
            (
                "MW1001".into(),
                "completeOrder".into(),
                "src/service.ts".into(),
                "4f53a6215ac16913".into()
            ),
            (
                "MW1002".into(),
                "relayOutbox".into(),
                "src/relay.ts".into(),
                "70af7a37dbb95d80".into()
            ),
            (
                "MW1003".into(),
                "shipOrder".into(),
                "src/service.ts".into(),
                "fdb2734b893502d8".into()
            ),
        ]
    );
    assert!(report.skipped.is_empty());
    for trace in &report.traces {
        assert!(trace.validate().is_empty());
        assert_eq!(trace.completeness, Completeness::Complete);
    }
    // No finding for the transactional payment or the outbox write.
    assert!(
        !report
            .traces
            .iter()
            .any(|t| t.function == "recordPayment" || t.function == "confirmOrder")
    );
}

#[test]
fn the_isolated_invoice_pattern_reaches_the_write_through_a_cross_module_helper() {
    let root = fixtures_dir().join("replay/invoice-pattern");
    let analyzer = Analyzer::new(Config::load(&root.join("midwrite.json")).unwrap());
    let report = analyzer
        .trace_project_at(&root, &[PathBuf::from("src")])
        .unwrap()
        .into_trace_report(None);
    let trace = single(&report, "MW1001");
    assert_eq!(trace.function, "finalizeInvoice");
    let write = &trace.steps[0];
    assert_eq!(write.kind, StepKind::DbWrite);
    assert_eq!(
        write.location.as_ref().unwrap().file,
        "src/queries/invoices.ts"
    );
    assert_eq!(write.call_chain[0].callee, "updateInvoice");
    // The helper writes on its `db` parameter, bound to the caller's root
    // handle: an auto-commit write.
    assert_eq!(write.at_failure, StepState::Committed);
    assert_eq!(trace.injection.checkpoints[0].name, "after-db-commit");
    assert_eq!(
        trace.injection.checkpoints[0].resource.as_deref(),
        Some("invoices")
    );
}

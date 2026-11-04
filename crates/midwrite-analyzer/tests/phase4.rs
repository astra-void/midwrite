//! Phase 4 static behavior: function-level recovery contracts (Milestone 5).

use std::path::PathBuf;

use midwrite_analyzer::Analyzer;
use midwrite_core::analysis::AnalysisOptions;
use midwrite_core::config::Config;
use midwrite_core::diagnostic::{Diagnostic, Policy, RecoveryDeclaration, Severity};

const SOURCE: &str = r#"
export async function processJob(id: string) {
  await db.update(jobs).set({ status: "done" }).where(eq(jobs.id, id));
  await queue.publish("job.done", { id });
}
export async function retryFailedJobs(id: string) {
  await db.update(jobs).set({ status: "done" }).where(eq(jobs.id, id));
  await queue.publish("job.done", { id });
}
export async function twoWrites(id: string) {
  await db.update(jobs).set({ status: "done" }).where(eq(jobs.id, id));
  await db.insert(audit).values({ id });
}
"#;

fn config(contracts: &str) -> Config {
    Config::from_json(
        &format!(r#"{{ "recoveryContracts": {contracts} }}"#),
        "test",
    )
    .unwrap()
}

fn analyze(config: Config, policy: Policy) -> Vec<Diagnostic> {
    Analyzer::new(config)
        .with_options(AnalysisOptions {
            policy,
            ..AnalysisOptions::default()
        })
        .analyze_source("src/jobs.ts", SOURCE)
        .unwrap()
        .diagnostics
}

fn find<'d>(diagnostics: &'d [Diagnostic], function: &str, code: &str) -> &'d Diagnostic {
    diagnostics
        .iter()
        .find(|d| d.function == function && d.code.as_str() == code)
        .unwrap_or_else(|| panic!("{function} {code}: {diagnostics:#?}"))
}

const RETRIED: &str = r#"[{
  "function": "processJob",
  "file": "src/jobs.ts",
  "strategy": "retried",
  "covers": ["MW1001"],
  "evidence": "queues/jobs.config.ts:14 attempts 3; stalled jobs are re-run once",
  "limits": "gives up after 3 attempts"
}]"#;

#[test]
fn a_contract_keeps_the_finding_and_its_identity_and_lowers_only_recommended_severity() {
    let plain = analyze(Config::default(), Policy::Recommended);
    let with = analyze(config(RETRIED), Policy::Recommended);
    let before = find(&plain, "processJob", "MW1001");
    let after = find(&with, "processJob", "MW1001");
    assert_eq!(before.severity, Severity::Warning);
    // Still reported, same identity, same location and evidence of the window.
    assert_eq!(after.fingerprint, before.fingerprint);
    assert_eq!(after.location, before.location);
    assert_eq!(after.failure_sequence, before.failure_sequence);
    assert_eq!(after.severity, Severity::Info);
    let note = &after.recovery[0];
    assert_eq!(note.declaration, RecoveryDeclaration::Retried);
    assert_eq!(note.scope, "function");
    assert_eq!(note.source, "configuration");
    assert!(
        !note.verified,
        "a configuration assertion is never verified"
    );
    assert_eq!(note.effect_on_finding, "informational");
    assert!(note.evidence.as_deref().unwrap().contains("attempts 3"));
    let limitation = after.limitations.last().unwrap();
    assert!(limitation.contains("does not verify"), "{limitation}");
    assert!(limitation.contains("performed again"), "{limitation}");
    assert!(
        limitation.contains("gives up after 3 attempts"),
        "{limitation}"
    );
    // The same number of findings: nothing was suppressed.
    assert_eq!(plain.len(), with.len());

    let strict = analyze(config(RETRIED), Policy::Strict);
    let strict = find(&strict, "processJob", "MW1001");
    assert_eq!(strict.severity, Severity::Warning);
    assert_eq!(strict.recovery[0].effect_on_finding, "noteOnly");
}

#[test]
fn contracts_apply_only_to_the_named_function_file_and_codes() {
    let with = analyze(config(RETRIED), Policy::Recommended);
    // Same shape, other function: untouched, even though its name says "retry".
    let other = find(&with, "retryFailedJobs", "MW1001");
    assert_eq!(other.severity, Severity::Warning);
    assert!(other.recovery.is_empty());

    let other_file = analyze(
        config(&RETRIED.replace("src/jobs.ts", "src/other.ts")),
        Policy::Recommended,
    );
    assert!(
        find(&other_file, "processJob", "MW1001")
            .recovery
            .is_empty()
    );

    let other_code = analyze(
        config(&RETRIED.replace(r#"["MW1001"]"#, r#"["MW1004"]"#)),
        Policy::Recommended,
    );
    assert!(
        find(&other_code, "processJob", "MW1001")
            .recovery
            .is_empty()
    );
}

#[test]
fn recovery_is_never_inferred_from_function_names() {
    let plain = analyze(Config::default(), Policy::Recommended);
    for function in ["processJob", "retryFailedJobs"] {
        let finding = find(&plain, function, "MW1001");
        assert_eq!(finding.severity, Severity::Warning, "{function}");
        assert!(finding.recovery.is_empty(), "{function}");
    }
}

#[test]
fn reconciliation_contracts_cannot_cover_effects_that_never_committed() {
    let error = Config::from_json(
        r#"{ "recoveryContracts": [{ "function": "f", "strategy": "reconciled", "covers": ["MW1002"], "evidence": "jobs/reconcile.ts nightly" }] }"#,
        "test",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("cannot cover MW1002"), "{error}");
    let reconciled = analyze(
        config(
            r#"[{ "function": "twoWrites", "strategy": "reconciled", "covers": ["MW1004"], "evidence": "jobs/reconcile.ts:20 nightly audit rebuild" }]"#,
        ),
        Policy::Recommended,
    );
    let finding = find(&reconciled, "twoWrites", "MW1004");
    assert_eq!(
        finding.recovery[0].declaration,
        RecoveryDeclaration::Reconciled
    );
}

#[test]
fn traces_are_unchanged_by_contracts() {
    let dir = std::env::temp_dir().join(format!("cr-phase4-contracts-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/jobs.ts"), SOURCE).unwrap();
    let traces = |config: Config| {
        Analyzer::new(config)
            .trace_project_at(&dir, &[PathBuf::from("src")])
            .unwrap()
            .into_trace_report(None)
    };
    let plain = traces(Config::default());
    let with = traces(config(RETRIED));
    assert_eq!(plain.traces, with.traces);
    std::fs::remove_dir_all(&dir).unwrap();
}

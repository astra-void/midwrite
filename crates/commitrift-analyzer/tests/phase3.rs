//! Phase 3 static behavior: recovery declarations (Milestone 4.2).

use commitrift_analyzer::Analyzer;
use commitrift_core::analysis::AnalysisOptions;
use commitrift_core::config::Config;
use commitrift_core::diagnostic::{Diagnostic, Policy, Severity};
use commitrift_core::ir::Recovery;

const RECOVERY_CONFIG: &str = r#"{
  "externalEffects": [
    { "function": "requestInvoiceEmail", "category": "email", "recovery": "reconciled" },
    { "function": "publishEvent", "category": "message-publish", "recovery": "idempotentConsumer" },
    { "function": "sendWebhook", "category": "http-request" }
  ]
}"#;

fn analyze(policy: Policy, source: &str) -> Vec<Diagnostic> {
    Analyzer::new(Config::from_json(RECOVERY_CONFIG, "test").unwrap())
        .with_options(AnalysisOptions {
            policy,
            ..AnalysisOptions::default()
        })
        .analyze_source("test.ts", source)
        .unwrap()
        .diagnostics
}

fn find<'d>(diagnostics: &'d [Diagnostic], function: &str) -> &'d Diagnostic {
    diagnostics
        .iter()
        .find(|d| d.function == function)
        .unwrap_or_else(|| panic!("{function}: {diagnostics:#?}"))
}

const SOURCE: &str = r#"
export async function reconciled(id: string) {
  await db.update(invoices).set({ status: "sent" });
  await requestInvoiceEmail({ id });
}
export async function undeclared(id: string) {
  await db.update(invoices).set({ status: "sent" });
  await sendWebhook({ id });
}
export async function reconciledFirst(id: string) {
  await requestInvoiceEmail({ id });
  await db.update(invoices).set({ status: "sent" });
}
export async function idempotent(id: string) {
  await db.update(invoices).set({ status: "sent" });
  await publishEvent({ id });
}
export async function idempotentFirst(id: string) {
  await publishEvent({ id });
  await db.update(invoices).set({ status: "sent" });
}
"#;

#[test]
fn a_reconciled_lost_effect_is_informational_but_still_reported() {
    let diagnostics = analyze(Policy::Recommended, SOURCE);
    let reconciled = find(&diagnostics, "reconciled");
    assert_eq!(
        reconciled.code.as_str(),
        "CP1001",
        "the window is still reported"
    );
    assert_eq!(reconciled.severity, Severity::Info);
    assert_eq!(reconciled.recovery.len(), 1);
    let note = &reconciled.recovery[0];
    assert_eq!(note.declaration, Recovery::Reconciled);
    assert_eq!(note.source, "configuration");
    assert!(!note.verified);
    assert_eq!(note.effect_on_finding, "informational");
    assert!(
        reconciled
            .limitations
            .iter()
            .any(|l| l.contains("does not verify"))
    );
    // Finding identity does not depend on declarations: baselines keep
    // matching when a team adds one.
    let undeclared_config = RECOVERY_CONFIG.replace(r#", "recovery": "reconciled""#, "");
    let plain = Analyzer::new(Config::from_json(&undeclared_config, "test").unwrap())
        .analyze_source("test.ts", SOURCE)
        .unwrap()
        .diagnostics;
    let before = find(&plain, "reconciled");
    assert_eq!(before.severity, Severity::Warning);
    assert_eq!(before.fingerprint, reconciled.fingerprint);
}

#[test]
fn undeclared_effects_and_effects_performed_first_are_unaffected() {
    let diagnostics = analyze(Policy::Recommended, SOURCE);
    let undeclared = find(&diagnostics, "undeclared");
    assert_eq!(undeclared.severity, Severity::Warning);
    assert!(undeclared.recovery.is_empty());
    // CP1002: the reconciled effect is performed; the claim is about the
    // commit, which the declaration does not cover.
    let first = find(&diagnostics, "reconciledFirst");
    assert_eq!(first.code.as_str(), "CP1002");
    assert_eq!(first.severity, Severity::Warning);
    assert!(first.recovery.is_empty());
}

#[test]
fn idempotent_consumers_add_a_note_without_changing_severity() {
    let diagnostics = analyze(Policy::Recommended, SOURCE);
    // A lost idempotent effect is still lost.
    let lost = find(&diagnostics, "idempotent");
    assert_eq!(lost.severity, Severity::Warning);
    assert!(lost.recovery.is_empty());
    // An idempotent effect performed before a commit that never happens:
    // a note, still a warning.
    let first = find(&diagnostics, "idempotentFirst");
    assert_eq!(first.code.as_str(), "CP1002");
    assert_eq!(first.severity, Severity::Warning);
    assert_eq!(first.recovery[0].declaration, Recovery::IdempotentConsumer);
    assert_eq!(first.recovery[0].effect_on_finding, "noteOnly");
}

#[test]
fn strict_policy_ignores_declarations_for_severity() {
    let diagnostics = analyze(Policy::Strict, SOURCE);
    assert!(diagnostics.iter().all(|d| d.severity == Severity::Warning));
    assert!(!find(&diagnostics, "reconciled").recovery.is_empty());
}

#[test]
fn recovery_is_never_inferred_from_names() {
    // Functions and variables named like recovery mechanisms change nothing.
    let source = r#"
export async function retryWithOutbox(idempotencyKey: string) {
  await db.update(orders).set({ status: "paid" });
  await queue.publish("order.completed", { idempotencyKey, outbox: true, retry: 3 });
}
"#;
    let diagnostics = Analyzer::new(Config::default())
        .analyze_source("test.ts", source)
        .unwrap()
        .diagnostics;
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].severity, Severity::Warning);
    assert!(diagnostics[0].recovery.is_empty());
}

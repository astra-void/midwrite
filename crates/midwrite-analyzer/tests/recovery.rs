//! Static recovery handling: declared recovery of external effects
//! (`externalEffects[].recovery`) and function-level recovery contracts
//! (`recoveryContracts`).

/// Recovery declared on an external effect.
mod declarations {
    use midwrite_analyzer::Analyzer;
    use midwrite_core::analysis::AnalysisOptions;
    use midwrite_core::config::Config;
    use midwrite_core::diagnostic::{Diagnostic, Policy, Severity};
    use midwrite_core::ir::Recovery;

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
            "MW1001",
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
        // MW1002: the reconciled effect is performed; the claim is about the
        // commit, which the declaration does not cover.
        let first = find(&diagnostics, "reconciledFirst");
        assert_eq!(first.code.as_str(), "MW1002");
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
        assert_eq!(first.code.as_str(), "MW1002");
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
}

/// Recovery contracts declared for a function.
mod contracts {
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
}

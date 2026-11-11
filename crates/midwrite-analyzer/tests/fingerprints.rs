//! Finding identity across source edits and report/trace output.

use std::path::{Path, PathBuf};

use midwrite_analyzer::Analyzer;
use midwrite_core::{config::Config, diagnostic::Diagnostic, report::Report};

const CALLBACK: &str = include_str!("../../../fixtures/vulnerable/fingerprint_callback.ts");

fn analyzer() -> Analyzer {
    Analyzer::new(
        Config::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/midwrite.json"))
            .unwrap(),
    )
}

fn findings(path: &str, source: &str) -> Vec<Diagnostic> {
    let file = analyzer().analyze_source(path, source).unwrap();
    Report::new(None, 1, file.ir.functions.len(), file.diagnostics, vec![]).diagnostics
}

#[test]
fn anonymous_callback_fingerprints_survive_moves_and_unrelated_edits() {
    let original = findings("src/pay.ts", CALLBACK);
    assert_eq!(original.len(), 1);
    let edited = format!(
        "// Added elsewhere\nconst version = 2;\n\n{}",
        CALLBACK
            .replace("register(async", "register(\n    async")
            .replace("  await db", "  const unrelated = 42;\n  await db")
            .replace("db.update(orders)", "db\n    .update(orders)")
    );
    let moved = findings("src/pay.ts", &edited);
    assert_eq!(moved.len(), 1);
    // The display name and location still show the actual source position.
    assert_ne!(original[0].function, moved[0].function);
    assert_ne!(original[0].location, moved[0].location);
    assert_eq!(original[0].fingerprint, moved[0].fingerprint);
    assert_eq!(
        original[0].compute_fingerprint(None),
        moved[0].compute_fingerprint(None)
    );
}

#[test]
fn identical_anonymous_callbacks_keep_unique_ids_when_lines_shift() {
    let (_, body) = CALLBACK.split_once("register(").unwrap();
    let source = format!("{CALLBACK}\nregister({body}");
    let original = findings("src/pay.ts", &source);
    let moved = findings("src/pay.ts", &format!("// new header\n\n{source}"));
    assert_eq!(original.len(), 2);
    assert_eq!(
        original[1].fingerprint,
        format!("{}-2", original[0].fingerprint)
    );
    assert_eq!(
        original.iter().map(|d| &d.fingerprint).collect::<Vec<_>>(),
        moved.iter().map(|d| &d.fingerprint).collect::<Vec<_>>()
    );
}

#[test]
fn fingerprints_distinguish_rules_functions_files_and_operations() {
    let source = CALLBACK
        .replace(
            "register(async (id: string) =>",
            "async function pay(id: string)",
        )
        .replace("\n});", "\n}");
    let original = findings("src/pay.ts", &source);
    assert_eq!(original.len(), 1);
    for edited in [
        source.replace("function pay", "function refund"),
        source.replace("update(orders)", "update(payments)"),
        source.replace("queue.publish(\"order.paid\", { id })", "mailer.send({ id })"),
        source.replace(
            "  await db.update(orders).set({ status: \"paid\" });\n  await queue.publish(\"order.paid\", { id });",
            "  await queue.publish(\"order.paid\", { id });\n  await db.update(orders).set({ status: \"paid\" });",
        ),
    ] {
        let changed = findings("src/pay.ts", &edited);
        assert_eq!(changed.len(), 1, "{edited}");
        assert_ne!(original[0].fingerprint, changed[0].fingerprint, "{edited}");
    }
    let relocated = findings("src/other.ts", &source);
    assert_ne!(original[0].fingerprint, relocated[0].fingerprint);
    assert_eq!(
        original[0].compute_fingerprint(None),
        relocated[0].compute_fingerprint(None)
    );
}

#[test]
fn callback_report_and_trace_ids_agree_after_moving_code() {
    let dir = std::env::temp_dir().join(format!("midwrite-fingerprints-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let scan = |source: &str| {
        std::fs::write(dir.join("pay.ts"), source).unwrap();
        let project = analyzer()
            .trace_project_at(&dir, &[PathBuf::from("pay.ts")])
            .unwrap();
        let report = project.clone().into_report(None);
        let traces = project.into_trace_report(None);
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(traces.traces.len(), 1);
        assert_eq!(
            report.diagnostics[0].fingerprint,
            traces.traces[0].finding_id
        );
        report.diagnostics[0].fingerprint.clone()
    };
    let before = scan(CALLBACK);
    let after = scan(&format!("// unrelated addition\n\n{CALLBACK}"));
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(before, after);
}

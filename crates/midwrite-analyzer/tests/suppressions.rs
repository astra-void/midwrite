use std::path::{Path, PathBuf};

use midwrite_analyzer::Analyzer;
use midwrite_core::config::Config;

const SOURCE: &str = "async function completeOrder() {\n  await db.insert(orders).values({});\n  await queue.publish('order.completed', {});\n}";

fn analyze(source: &str) -> midwrite_analyzer::FileAnalysis {
    Analyzer::default()
        .analyze_source("orders.ts", source)
        .expect("valid source and directives")
}

#[test]
fn leading_and_trailing_comments_suppress_the_primary_location_only() {
    assert_eq!(analyze(SOURCE).diagnostics.len(), 1);
    for source in [
        SOURCE.replace(
            "  await queue",
            "  // midwrite-ignore MW1001: compensated by nightly reconciliation job\n  await queue",
        ),
        SOURCE.replace(
            "{});\n}",
            "{}); // midwrite-ignore MW1001: reconciliation: runs nightly\n}",
        ),
    ] {
        let analysis = analyze(&source);
        assert!(analysis.diagnostics.is_empty(), "{analysis:#?}");
        // Reporting directives do not erase operations from summaries.
        assert_eq!(
            analysis.ir.functions[0].effects().len(),
            analyze(SOURCE).ir.functions[0].effects().len()
        );
    }
    for directive in [
        "// midwrite-ignore MW1002: another rule",
        "// midwrite-ignore MW1001: reason\n", // A blank line breaks adjacency.
        "/* midwrite-ignore MW1001: block comments are not directives */",
        "// Example: midwrite-ignore MW1001: prose is not a directive",
        "const example = '// midwrite-ignore MW1001:';",
        "const example = `// midwrite-ignore MW1001:`;",
    ] {
        let source = SOURCE.replace("  await queue", &format!("  {directive}\n  await queue"));
        assert_eq!(analyze(&source).diagnostics.len(), 1, "{source}");
    }
    let source = SOURCE.replace(
        "  await db",
        "  // midwrite-ignore MW1001: a related location is not the primary location\n  await db",
    );
    assert_eq!(analyze(&source).diagnostics.len(), 1);
}

#[test]
fn suppression_keeps_other_rules_and_other_lines() {
    let source = SOURCE.replace(
        "\n}",
        "\n  // midwrite-ignore MW1001: second publication is reconciled\n  await queue.publish('audit', {});\n}",
    );
    let analysis = analyze(&source);
    let codes: Vec<_> = analysis
        .diagnostics
        .iter()
        .map(|d| d.code.as_str())
        .collect();
    assert_eq!(codes, ["MW1001", "MW1005"]);
}

#[test]
fn invalid_directives_are_rejected_with_a_source_position() {
    for directive in [
        "",
        "MW1001",
        "MW1001:",
        "MW1001: \t",
        "MW9999: reason",
        "*: reason",
        "MW1001, MW1002: reason",
        ": reason",
    ] {
        let source = SOURCE.replace(
            "  await queue",
            &format!("  // midwrite-ignore {directive}\n  await queue"),
        );
        let failure = Analyzer::default()
            .analyze_source("orders.ts", &source)
            .unwrap_err();
        assert!(
            failure.message.contains("invalid midwrite-ignore"),
            "{failure}"
        );
        assert_eq!((failure.line, failure.column), (Some(3), Some(3)));
    }
}

#[test]
fn unicode_reason_and_crlf_are_supported() {
    let source = SOURCE
        .replace(
            "  await queue",
            "  // midwrite-ignore MW1001: 야간 정산 작업\n  await queue",
        )
        .replace('\n', "\r\n");
    assert!(analyze(&source).diagnostics.is_empty());
}

#[test]
fn suppression_does_not_hide_syntax_errors() {
    let failure = Analyzer::default()
        .analyze_source(
            "orders.ts",
            "// midwrite-ignore MW9001: reason\nasync function (",
        )
        .unwrap_err();
    assert!(!failure.message.contains("invalid midwrite-ignore"));
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

#[test]
fn project_reports_and_traces_filter_the_same_findings() {
    let analyzer = Analyzer::new(Config::load(&fixture("midwrite.json")).unwrap());
    let inputs = [
        fixture("safe/inline_suppression.ts"),
        fixture("vulnerable/db_commit_then_publish.ts"),
    ];
    let report = analyzer.analyze_project(&inputs).unwrap().into_report(None);
    assert!(report.failures.is_empty());
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.summary.diagnostics.warning, 1);
    let traced = analyzer.trace_project(&inputs).unwrap();
    assert_eq!(
        serde_json::to_value(traced.clone().into_report(None)).unwrap(),
        serde_json::to_value(&report).unwrap()
    );
    let traces = traced.into_trace_report(None);
    assert!(traces.skipped.is_empty());
    assert_eq!(traces.traces.len(), 1);
    assert_eq!(
        traces.traces[0].finding_id,
        report.diagnostics[0].fingerprint
    );
}

#[test]
fn call_site_directives_preserve_summaries_and_trace_pairing() {
    let analyzer = Analyzer::default();
    let inputs = [fixture("projects/inline_suppression")];
    let project = analyzer.trace_project(&inputs).unwrap();
    assert!(project.failures.is_empty());
    let report = project.clone().into_report(None);
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].function, "unreconciled");
    let traces = project.into_trace_report(None);
    assert_eq!(traces.traces.len(), 1);
    assert!(traces.skipped.is_empty());
    assert_eq!(traces.traces[0].function, "unreconciled");
    assert_eq!(
        traces.traces[0].finding_id,
        report.diagnostics[0].fingerprint
    );
}

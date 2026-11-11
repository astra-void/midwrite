//! Output formats.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::{Value, json};

use midwrite_core::diagnostic::{
    BaselineState, Diagnostic, DiagnosticCode, PathCertainty, Severity,
};
use midwrite_core::location::{LineIndex, SourceLocation};
use midwrite_core::report::Report;

/// Human-readable output.
pub fn text(report: &Report, verbose: bool) -> String {
    let mut out = String::new();
    let mut sources = SourceCache::default();
    for diagnostic in &report.diagnostics {
        write_diagnostic(&mut out, diagnostic, &mut sources, verbose);
        out.push('\n');
    }
    for failure in &report.failures {
        let position = match (failure.line, failure.column) {
            (Some(line), Some(column)) => format!(":{line}:{column}"),
            _ => String::new(),
        };
        let what = match failure.kind {
            midwrite_core::report::FailureKind::Read => "could not read",
            midwrite_core::report::FailureKind::Parse => "could not parse",
        };
        let _ = writeln!(
            out,
            "error: {what} {}{position}: {} (file not analyzed)",
            failure.file, failure.message
        );
    }
    if !report.failures.is_empty() {
        out.push('\n');
    }

    let summary = &report.summary;
    let config = report.config.as_deref().unwrap_or("built-in defaults");
    let _ = writeln!(
        out,
        "midwrite: analyzed {} of {} file(s), {} function(s); {} warning(s), {} error(s), {} info; config: {config}",
        summary.files_analyzed,
        summary.files_discovered,
        summary.functions_analyzed,
        summary.diagnostics.warning,
        summary.diagnostics.error,
        summary.diagnostics.info,
    );
    if let Some(baseline) = &summary.baseline {
        let _ = writeln!(
            out,
            "midwrite: baseline: {} new, {} already in the baseline, {} baseline finding(s) no longer reported",
            baseline.new, baseline.existing, baseline.resolved
        );
    }
    if summary.files_failed > 0 {
        let _ = writeln!(
            out,
            "midwrite: {} file(s) could not be analyzed; results are incomplete",
            summary.files_failed
        );
    }
    if !report.diagnostics.is_empty() {
        out.push_str("Findings are potential hazards, not reproduced failures. Run `midwrite explain <CODE>` for details.\n");
    }
    out
}

fn write_diagnostic(
    out: &mut String,
    diagnostic: &Diagnostic,
    sources: &mut SourceCache,
    verbose: bool,
) {
    let location = &diagnostic.location;
    let marker = match diagnostic.baseline {
        Some(BaselineState::Existing) => " (in baseline)",
        Some(BaselineState::New) => " (new)",
        None => "",
    };
    let _ = writeln!(
        out,
        "{}[{}]: {}{marker}",
        diagnostic.severity, diagnostic.code, diagnostic.title
    );
    let _ = writeln!(
        out,
        "  --> {}:{}:{} (in `{}`, confidence: {})",
        location.file, location.line, location.column, diagnostic.function, diagnostic.confidence
    );
    if let Some(line_text) = sources.line(&location.file, location.line) {
        let gutter = location.line.to_string();
        let pad = " ".repeat(gutter.len());
        let start = location.column.saturating_sub(1) as usize;
        let width = if location.end_line == location.line {
            (location.end_column.saturating_sub(location.column) as usize).max(1)
        } else {
            line_text.chars().count().saturating_sub(start).max(1)
        };
        let prefix: String = line_text
            .chars()
            .take(start)
            .map(|c| if c == '\t' { '\t' } else { ' ' })
            .collect();
        let _ = writeln!(out, "  {pad} |");
        let _ = writeln!(out, "  {gutter} | {line_text}");
        let _ = writeln!(out, "  {pad} | {prefix}{}", "^".repeat(width.min(80)));
    }
    let _ = writeln!(out, "  = {}", diagnostic.message);

    if !diagnostic.failure_sequence.is_empty() {
        let _ = writeln!(out, "  failure sequence:");
        for step in &diagnostic.failure_sequence {
            let _ = writeln!(out, "    {step}");
        }
    }
    if !diagnostic.path_conditions.is_empty() {
        let _ = writeln!(out, "  path conditions:");
        for condition in &diagnostic.path_conditions {
            let _ = writeln!(
                out,
                "    - {} ({})",
                condition.description,
                short_location(&condition.location, &diagnostic.location.file)
            );
        }
    }
    if verbose {
        let _ = writeln!(
            out,
            "  basis: path {}, evidence {}",
            path_certainty(diagnostic.path_certainty),
            diagnostic.evidence_strength.as_str()
        );
    }
    if verbose && !diagnostic.evidence.is_empty() {
        let _ = writeln!(out, "  evidence:");
        for evidence in &diagnostic.evidence {
            let _ = writeln!(out, "    - {evidence}");
        }
    }
    for note in &diagnostic.recovery {
        let _ = writeln!(
            out,
            "  recovery: {} `{}` declared for {} `{}` (provenance: {}, verified: {}; {})",
            match note.scope {
                "function" => "contract",
                _ => "property",
            },
            serde_json::to_value(note.declaration)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
            note.scope,
            note.effect,
            note.source,
            note.verified,
            note.effect_on_finding
        );
    }
    let _ = writeln!(out, "  help: {}", diagnostic.mitigation);
    let general = diagnostic.code.info().limitation;
    for limitation in &diagnostic.limitations {
        if verbose || limitation != general {
            let _ = writeln!(out, "  note: {limitation}");
        }
    }
}

fn path_certainty(certainty: PathCertainty) -> &'static str {
    match certainty {
        PathCertainty::StraightLine => "straight-line",
        PathCertainty::Conditional => "conditional",
        PathCertainty::Approximated => "approximated",
        PathCertainty::Truncated => "truncated",
        PathCertainty::Unknown => "unknown",
    }
}

/// `line:column`, with the file when it differs from the diagnostic's.
fn short_location(location: &SourceLocation, file: &str) -> String {
    if location.file == file {
        format!("{}:{}", location.line, location.column)
    } else {
        format!("{}:{}:{}", location.file, location.line, location.column)
    }
}

#[derive(Default)]
struct SourceCache {
    files: BTreeMap<String, Option<String>>,
}

impl SourceCache {
    fn line(&mut self, file: &str, line: u32) -> Option<String> {
        let source = self
            .files
            .entry(file.to_owned())
            .or_insert_with(|| std::fs::read_to_string(file).ok())
            .as_deref()?;
        LineIndex::new(source).line_text(line).map(str::to_owned)
    }
}

/// The JSON report (see docs/DIAGNOSTICS.md for the schema).
pub fn json(report: &Report) -> String {
    let mut text = serde_json::to_string_pretty(report).expect("reports are serializable");
    text.push('\n');
    text
}

/// SARIF 2.1.0, for code-scanning integrations.
pub fn sarif(report: &Report) -> String {
    let rules: Vec<Value> = DiagnosticCode::ALL
        .iter()
        .map(|code| {
            let info = code.info();
            json!({
                "id": info.code,
                "name": info.code,
                "shortDescription": { "text": info.title },
                "fullDescription": { "text": info.explanation },
                "help": { "text": info.mitigation },
                "defaultConfiguration": { "level": sarif_level(info.severity) },
            })
        })
        .collect();
    let results: Vec<Value> = report
        .diagnostics
        .iter()
        .map(|diagnostic| {
            let related: Vec<Value> = diagnostic
                .effects
                .iter()
                .enumerate()
                .map(|(index, effect)| {
                    json!({
                        "id": index,
                        "message": { "text": format!("{} — {}", effect.label, effect.description) },
                        "physicalLocation": physical_location(&effect.location),
                    })
                })
                .collect();
            let mut text = diagnostic.message.clone();
            for step in &diagnostic.failure_sequence {
                text.push('\n');
                text.push_str(step);
            }
            let mut result = json!({
                "ruleId": diagnostic.code.as_str(),
                "level": sarif_level(diagnostic.severity),
                "message": { "text": text },
                "locations": [{ "physicalLocation": physical_location(&diagnostic.location) }],
                "relatedLocations": related,
                // Keep the versioned key and the shared finding identity stable
                // so code-scanning services can match alerts across runs.
                "partialFingerprints": { "midwrite/v1": diagnostic.fingerprint },
                "properties": {
                    "function": diagnostic.function,
                    "confidence": diagnostic.confidence,
                    "pathCertainty": diagnostic.path_certainty,
                    "evidenceStrength": diagnostic.evidence_strength,
                    "kind": diagnostic.kind,
                    "reproduced": diagnostic.reproduced,
                    "limitations": diagnostic.limitations,
                },
            });
            if !diagnostic.recovery.is_empty() {
                result["properties"]["recovery"] = json!(diagnostic.recovery);
            }
            if let Some(state) = diagnostic.baseline {
                result["baselineState"] = json!(match state {
                    BaselineState::New => "new",
                    BaselineState::Existing => "unchanged",
                });
            }
            result
        })
        .collect();
    let notifications: Vec<Value> = report
        .failures
        .iter()
        .map(|failure| {
            json!({
                "level": "error",
                "message": { "text": format!("{}: {}", failure.file, failure.message) },
            })
        })
        .collect();
    let mut invocation = json!({
        "executionSuccessful": report.failures.is_empty(),
        "toolExecutionNotifications": notifications,
    });
    if let Ok(directory) = std::env::current_dir() {
        let mut uri = artifact_uri(&midwrite_analyzer::display_path(&directory));
        if !uri.ends_with('/') {
            uri.push('/');
        }
        invocation["workingDirectory"] = json!({ "uri": uri });
    }
    let document = json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            // SourceLocation columns count Unicode scalar values, whereas
            // SARIF's default is UTF-16 code units.
            "columnKind": "unicodeCodePoints",
            "tool": { "driver": {
                "name": report.tool.name,
                "version": report.tool.version,
                "rules": rules,
            }},
            "invocations": [invocation],
            "results": results,
        }],
    });
    let mut text = serde_json::to_string_pretty(&document).expect("SARIF is serializable");
    text.push('\n');
    text
}

fn sarif_level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "note",
    }
}

fn physical_location(location: &midwrite_core::location::SourceLocation) -> Value {
    json!({
        "artifactLocation": { "uri": artifact_uri(&location.file) },
        "region": {
            "startLine": location.line,
            "startColumn": location.column,
            "endLine": location.end_line,
            "endColumn": location.end_column,
        },
    })
}

/// Encode a filesystem path as a URI reference. A literal `%`, `#` or `?`
/// belongs to the filename, rather than an escape, fragment or query.
fn artifact_uri(file: &str) -> String {
    let mut uri = if file.starts_with('/') {
        String::from("file://")
    } else {
        String::new()
    };
    for byte in file.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                uri.push(char::from(byte));
            }
            _ => {
                let _ = write!(uri, "%{byte:02X}");
            }
        }
    }
    uri
}

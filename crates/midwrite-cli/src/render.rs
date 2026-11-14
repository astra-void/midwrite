//! Output formats.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::{Value, json};

use midwrite_core::diagnostic::{
    BaselineState, Diagnostic, DiagnosticCode, DiagnosticKind, EffectRole, PathCertainty, Severity,
};
use midwrite_core::location::{LineIndex, SourceLocation};
use midwrite_core::report::Report;

/// Human-readable output.
pub fn text(report: &Report, verbose: bool) -> String {
    let mut out = String::new();
    let mut sources = SourceCache::default();
    // Findings that describe the same effect pair on the same failure path
    // (`Diagnostic::group`) are shown once, at the member whose function
    // contains the operations most directly, with the others listed under
    // it. `--verbose` prints every finding in full.
    let mut groups: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, diagnostic) in report.diagnostics.iter().enumerate() {
        if let Some(group) = &diagnostic.group {
            groups.entry(group).or_default().push(index);
        }
    }
    let call_sites = |diagnostic: &Diagnostic| {
        diagnostic
            .effects
            .iter()
            .filter(|effect| effect.role == EffectRole::CallSite)
            .count()
    };
    let shown: BTreeMap<&str, usize> = groups
        .iter()
        .map(|(group, members)| {
            // A finding that is not in the baseline is never folded under
            // one that is: it is what a CI run is about.
            let shown = members
                .iter()
                .copied()
                .min_by_key(|&index| {
                    let diagnostic = &report.diagnostics[index];
                    (
                        diagnostic.baseline == Some(BaselineState::Existing),
                        call_sites(diagnostic),
                        index,
                    )
                })
                .expect("a group has a member");
            (*group, shown)
        })
        .collect();
    // Analysis gaps (MW9001) by file and function, for cross-references.
    let mut gaps: BTreeMap<(&str, &str), Vec<&Diagnostic>> = BTreeMap::new();
    for diagnostic in &report.diagnostics {
        if diagnostic.kind == DiagnosticKind::AnalysisLimitation {
            gaps.entry((&diagnostic.location.file, &diagnostic.function))
                .or_default()
                .push(diagnostic);
        }
    }
    let mut folded = 0usize;
    for (index, diagnostic) in report.diagnostics.iter().enumerate() {
        let others: Vec<&Diagnostic> = match &diagnostic.group {
            Some(group) => groups[group.as_str()]
                .iter()
                .filter(|&&member| member != index)
                .map(|&member| &report.diagnostics[member])
                .collect(),
            None => Vec::new(),
        };
        if !verbose
            && let Some(group) = &diagnostic.group
            && shown[group.as_str()] != index
        {
            folded += 1;
            continue;
        }
        let context = Context {
            same_pair: &others,
            gaps: gaps
                .get(&(
                    diagnostic.location.file.as_str(),
                    diagnostic.function.as_str(),
                ))
                .map_or(&[], Vec::as_slice),
        };
        write_diagnostic(&mut out, diagnostic, &mut sources, verbose, &context);
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
    if summary.known_gaps > 0 {
        let continuations = match summary.analysis.unmodeled_continuations {
            0 => String::new(),
            count => format!("; {count} promise continuation(s) are not modeled"),
        };
        let _ = writeln!(
            out,
            "midwrite: analysis incomplete at {} reported location(s) (MW9001){continuations}. The absence of a finding there is not a clean result.",
            summary.known_gaps
        );
    }
    if folded > 0 {
        let _ = writeln!(
            out,
            "midwrite: {folded} finding(s) repeat an effect pair and failure path shown above and are listed under it (`--verbose` prints each in full)."
        );
    }
    if !report.diagnostics.is_empty() {
        out.push_str("Findings are potential hazards, not reproduced failures. Run `midwrite explain <CODE>` for details.\n");
    }
    out
}

/// What the rest of the report says about one diagnostic.
struct Context<'r> {
    /// The other findings with the same effect pair and failure path.
    same_pair: &'r [&'r Diagnostic],
    /// The analysis gaps (MW9001) of the same function.
    gaps: &'r [&'r Diagnostic],
}

fn baseline_marker(diagnostic: &Diagnostic) -> &'static str {
    match diagnostic.baseline {
        Some(BaselineState::Existing) => " (in baseline)",
        Some(BaselineState::New) => " (new)",
        None => "",
    }
}

fn role_name(role: EffectRole) -> &'static str {
    match role {
        EffectRole::CompletedFirst => "completed first",
        EffectRole::NotYetPerformed => "not yet performed",
        EffectRole::StagedWrite => "staged write",
        EffectRole::Transaction => "transaction",
        EffectRole::Unanalyzed => "not analyzed",
        EffectRole::Context => "context",
        EffectRole::CallSite => "reached through",
    }
}

fn write_diagnostic(
    out: &mut String,
    diagnostic: &Diagnostic,
    sources: &mut SourceCache,
    verbose: bool,
    context: &Context<'_>,
) {
    let location = &diagnostic.location;
    let marker = baseline_marker(diagnostic);
    let hazard = diagnostic.kind == DiagnosticKind::PotentialHazard;
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
    // Where each operation of the finding is, by role: the durable first
    // effect and the missing second one are locations, not only prose.
    if !diagnostic.effects.is_empty() {
        let _ = writeln!(out, "  operations:");
        for effect in &diagnostic.effects {
            let _ = writeln!(
                out,
                "    - {}: `{}` ({}): {}",
                role_name(effect.role),
                effect.label,
                short_location(&effect.location, &diagnostic.location.file),
                effect.description
            );
        }
    }
    if verbose || hazard {
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
    if hazard {
        if diagnostic.recovery.is_empty() {
            let _ = writeln!(out, "  recovery: none declared");
        }
        if context.gaps.is_empty() {
            let _ = writeln!(
                out,
                "  analysis gaps: none reported in this function (not a statement of complete analysis)"
            );
        } else {
            let places: Vec<String> = context
                .gaps
                .iter()
                .map(|gap| short_location(&gap.location, &diagnostic.location.file))
                .collect();
            let _ = writeln!(
                out,
                "  analysis gaps: this function has unanalyzed operations (MW9001) at {}",
                places.join(", ")
            );
        }
    }
    if !context.same_pair.is_empty() {
        let _ = writeln!(
            out,
            "  same effect pair and failure path, also reported in:"
        );
        for other in context.same_pair {
            let through = other
                .effects
                .iter()
                .find(|effect| effect.role == EffectRole::CallSite)
                .map(|effect| format!(", through `{}`", effect.label))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "    - `{}` at {}{through}{}",
                other.function,
                short_location(&other.location, ""),
                baseline_marker(other)
            );
        }
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
            // Results with the same group are one effect pair and failure
            // path reported in several functions; each stays a result of
            // its own, with its own location and fingerprint.
            if let Some(group) = &diagnostic.group {
                result["properties"]["group"] = json!(group);
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
    let mut notifications = notifications;
    if report.summary.known_gaps > 0 {
        // An incomplete analysis is not a clean run: say so at the run
        // level too, not only as `note` results.
        notifications.push(json!({
            "level": "note",
            "descriptor": { "id": "MW9001" },
            "message": { "text": format!(
                "analysis incomplete at {} reported location(s) (MW9001); the absence of a finding there is not a clean result",
                report.summary.known_gaps
            ) },
        }));
    }
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
            "properties": {
                // Zero known gaps does not mean a complete analysis.
                "knownGaps": report.summary.known_gaps,
                "analysis": report.summary.analysis,
            },
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

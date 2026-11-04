//! `midwrite report --format html`: a self-contained, read-only HTML view
//! of a trace report or a verification report.
//!
//! The page has no script and loads nothing: interaction uses `<details>`,
//! a Content-Security-Policy forbids scripts and remote resources, and every
//! string from a report (source labels, logs, file names) is escaped. Static
//! predictions and runtime observations are drawn differently and labeled.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::Value;

use midwrite_core::trace::{FailureTrace, StepKind, StepRole, StepState, TraceReport};
use midwrite_core::verification::{CaseResult, RunRecord, VerificationLevel, VerificationReport};

/// Escape text for HTML element content and attribute values.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

const STYLE: &str = r#"
body{font:14px/1.45 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;margin:0;background:#f6f7f9;color:#1d232b}
header{background:#1d232b;color:#fff;padding:14px 24px}header h1{margin:0;font-size:18px}header p{margin:4px 0 0;color:#c9d1d9}
main{padding:16px 24px;max-width:1200px}
.card{background:#fff;border:1px solid #d7dce2;border-radius:6px;margin:14px 0;padding:12px 16px}
.static{border-style:dashed;border-color:#8a94a3;background:#fbfcfd}
.runtime{border-left:5px solid #2f6feb}
.badge{display:inline-block;padding:1px 8px;border-radius:10px;font-size:12px;font-weight:600;margin-right:6px;white-space:nowrap}
.STATIC_CANDIDATE{background:#e7eaee;color:#3b4552;border:1px dashed #8a94a3}
.SIMULATED{background:#eef0ff;color:#3b3f8f;border:1px dashed #6b70c9}
.RUNTIME_REPRODUCED{background:#ffe2e0;color:#8c1d18}
.RUNTIME_NOT_REPRODUCED{background:#dcf5e3;color:#185c2d}
.INCONCLUSIVE{background:#f1f1f1;color:#555;border:1px solid #aaa}
table{border-collapse:collapse;width:100%;margin:8px 0}td,th{border-bottom:1px solid #e3e7ec;padding:4px 6px;text-align:left;vertical-align:top}th{font-size:12px;color:#5b6573}
code,.mono{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:12.5px}
tr.boundary td{background:#fff4d6;border-top:2px dashed #d29922;border-bottom:2px dashed #d29922;font-weight:600;color:#7a5200}
.st-committed{color:#185c2d;font-weight:600}.st-performed{color:#1f4fa8;font-weight:600}.st-staged{color:#8c5a00;font-weight:600}
.st-rolledBack{color:#8c1d18}.st-notReached{color:#8a94a3}.st-taken,.st-completed{color:#3b4552}
.role{font-size:11px;color:#5b6573}
.kind{font-size:11px;text-transform:uppercase;letter-spacing:.03em;color:#5b6573}
.ok{color:#185c2d;font-weight:600}.bad{color:#8c1d18;font-weight:600}
details{margin:6px 0}summary{cursor:pointer;color:#2f4b6e}
pre{background:#f3f4f6;padding:8px;overflow:auto;max-height:300px;font-size:12px}
.grid{display:grid;grid-template-columns:1fr 1fr;gap:14px}@media(max-width:900px){.grid{grid-template-columns:1fr}}
.label{font-size:11px;font-weight:700;letter-spacing:.06em;color:#5b6573;text-transform:uppercase}
.muted{color:#5b6573}
"#;

fn page(title: &str, subtitle: &str, body: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src 'none'; script-src 'none'; form-action 'none'; base-uri 'none'\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{}</title><style>{STYLE}</style></head><body><header><h1>{}</h1><p>{}</p></header><main>{body}</main></body></html>\n",
        escape(title),
        escape(title),
        escape(subtitle)
    )
}

fn badge(level: VerificationLevel) -> String {
    let text = match level {
        VerificationLevel::StaticCandidate => "STATIC CANDIDATE — not executed",
        VerificationLevel::Simulated => "SIMULATED — abstract model, not executed",
        VerificationLevel::RuntimeReproduced => "RUNTIME REPRODUCED",
        VerificationLevel::RuntimeNotReproduced => "RUNTIME NOT REPRODUCED",
        VerificationLevel::Inconclusive => "INCONCLUSIVE",
    };
    format!(
        "<span class=\"badge {}\">{}</span>",
        level.as_str(),
        escape(text)
    )
}

fn state_name(state: StepState) -> (&'static str, &'static str) {
    match state {
        StepState::Taken => ("taken", "taken"),
        StepState::Completed => ("completed", "completed"),
        StepState::Committed => ("committed", "committed"),
        StepState::Staged => ("staged", "staged (uncommitted)"),
        StepState::RolledBack => ("rolledBack", "rolled back"),
        StepState::Performed => ("performed", "performed"),
        StepState::NotReached => ("notReached", "not reached"),
    }
}

fn role_name(role: Option<StepRole>) -> &'static str {
    match role {
        Some(StepRole::CompletedFirst) => "completes before the failure",
        Some(StepRole::NotYetPerformed) => "not yet performed at the failure",
        Some(StepRole::StagedWrite) => "staged in the open transaction",
        Some(StepRole::Transaction) => "transaction",
        Some(StepRole::InWindow) => "inside the failure window",
        None => "",
    }
}

fn list(items: &[String]) -> String {
    if items.is_empty() {
        return "<p class=\"muted\">none</p>".to_owned();
    }
    let mut out = String::from("<ul>");
    for item in items {
        let _ = write!(out, "<li>{}</li>", escape(item));
    }
    out.push_str("</ul>");
    out
}

/// The static timeline of one trace.
fn trace_timeline(trace: &FailureTrace) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "<div class=\"card static\"><div class=\"label\">Static prediction</div><p>{}<span class=\"badge\">{:?}</span> <b>{}</b> in <code>{}</code> — <span class=\"mono\">{}:{}</span></p><p class=\"muted\">finding <code>{}</code> · trace <code>{}</code></p>",
        badge(trace.verification),
        trace.completeness,
        escape(&trace.code),
        escape(&trace.function),
        escape(&trace.location.file),
        trace.location.line,
        escape(&trace.finding_id),
        escape(&trace.id)
    );
    out.push_str(
        "<table><tr><th>#</th><th>operation</th><th>state at the failure</th><th>where</th></tr>",
    );
    let mut depth: usize = 0;
    for step in &trace.steps {
        if matches!(step.kind, StepKind::TxCommit | StepKind::TxRollback) {
            depth = depth.saturating_sub(1);
        }
        let (class, state) = state_name(step.at_failure);
        let indent = "&nbsp;&nbsp;&nbsp;&nbsp;".repeat(depth);
        let location = step
            .location
            .as_ref()
            .map(|l| format!("{}:{}", escape(&l.file), l.line))
            .unwrap_or_default();
        let chain = if step.call_chain.is_empty() {
            String::new()
        } else {
            format!(
                "<br><span class=\"role\">via {}</span>",
                step.call_chain
                    .iter()
                    .map(|f| format!("<code>{}</code>", escape(&f.label)))
                    .collect::<Vec<_>>()
                    .join(" → ")
            )
        };
        let _ = write!(
            out,
            "<tr><td>{}</td><td>{indent}<span class=\"kind\">{}</span> <code>{}</code>{}<br><span class=\"role\">{}</span>{chain}</td><td class=\"st-{class}\">{}</td><td class=\"mono\">{location}</td></tr>",
            step.index,
            escape(step.kind.as_str()),
            escape(&step.label),
            step.transaction
                .as_ref()
                .map(|t| format!(" <span class=\"role\">[{}]</span>", escape(t)))
                .unwrap_or_default(),
            escape(role_name(step.role)),
            escape(state)
        );
        if step.kind == StepKind::TxBegin {
            depth += 1;
        }
        if step.index == trace.injection.after_step {
            let checkpoints = trace
                .injection
                .checkpoints
                .iter()
                .map(|c| {
                    format!(
                        "<code>{}</code>{}",
                        escape(&c.name),
                        c.resource
                            .as_ref()
                            .map(|r| format!(" ({})", escape(r)))
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join(" / ");
            let _ = write!(
                out,
                "<tr class=\"boundary\"><td>⚡</td><td colspan=\"3\">failure injection boundary — {checkpoints}<br><span class=\"role\">{}</span></td></tr>",
                escape(&trace.injection.description)
            );
        }
    }
    out.push_str("</table>");
    let _ = write!(
        out,
        "<p>{} {}</p>",
        badge(trace.simulation.verification),
        escape(&trace.simulation.claim)
    );
    let _ = write!(
        out,
        "<details><summary>Assumptions ({})</summary>{}</details><details><summary>Limitations ({})</summary>{}</details><details><summary>Not modeled by the simulation</summary>{}</details></div>",
        trace.assumptions.len(),
        list(&trace.assumptions),
        trace.limitations.len(),
        list(&trace.limitations),
        list(&trace.simulation.not_modeled)
    );
    out
}

pub fn traces_html(report: &TraceReport) -> String {
    let s = report.summary;
    let mut body = format!(
        "<div class=\"card\"><p>{} {} finding(s): {} trace(s) ({} complete, {} approximated, {} incomplete), {} without a trace.</p><p class=\"muted\">Every trace below is a static candidate derived from source code. Nothing was executed; a trace shows that a partial state <i>can</i> arise on the reported path, not that it violates the application's rules.</p></div>",
        badge(VerificationLevel::StaticCandidate),
        s.findings,
        s.traces,
        s.complete,
        s.approximated,
        s.incomplete,
        s.skipped
    );
    for trace in &report.traces {
        body.push_str(&trace_timeline(trace));
    }
    if !report.skipped.is_empty() {
        body.push_str("<div class=\"card\"><details><summary>Findings without a trace</summary><table><tr><th>finding</th><th>code</th><th>where</th><th>reason</th></tr>");
        for skipped in &report.skipped {
            let _ = write!(
                body,
                "<tr><td><code>{}</code></td><td>{}</td><td class=\"mono\">{}:{}</td><td>{}</td></tr>",
                escape(&skipped.finding_id),
                escape(&skipped.code),
                escape(&skipped.location.file),
                skipped.location.line,
                escape(&skipped.reason)
            );
        }
        body.push_str("</table></details></div>");
    }
    page(
        "Midwrite failure traces",
        &format!(
            "{} {} · static analysis only",
            report.tool.name, report.tool.version
        ),
        &body,
    )
}

fn observation_table(run: &RunRecord) -> String {
    let stages: Vec<&String> = run.observations.keys().collect();
    let mut names: Vec<&String> = run.observations.values().flat_map(BTreeMap::keys).collect();
    names.sort();
    names.dedup();
    let order = |stage: &str| match stage {
        "before" => 0,
        "after" => 1,
        "afterRecovery" => 2,
        _ => 3,
    };
    let mut stages = stages;
    stages.sort_by_key(|s| order(s));
    let mut out = String::from("<table><tr><th>observation</th>");
    for stage in &stages {
        let _ = write!(out, "<th>{}</th>", escape(stage));
    }
    out.push_str("</tr>");
    for name in names {
        let _ = write!(out, "<tr><td><code>{}</code></td>", escape(name));
        for stage in &stages {
            let value = run.observations[*stage].get(name).unwrap_or(&Value::Null);
            let _ = write!(
                out,
                "<td class=\"mono\">{}</td>",
                escape(&value.to_string())
            );
        }
        out.push_str("</tr>");
    }
    out.push_str("</table>");
    out
}

fn run_html(run: &RunRecord, control: bool) -> String {
    let (title, verdict) = if control {
        let held = run.verification == VerificationLevel::RuntimeNotReproduced;
        (
            "Runtime observation — control run, nothing injected".to_owned(),
            format!(
                "<span class=\"badge {}\">{}</span>",
                if held {
                    "RUNTIME_NOT_REPRODUCED"
                } else {
                    "INCONCLUSIVE"
                },
                if held {
                    "CONTROL: invariants held"
                } else {
                    "CONTROL FAILED"
                }
            ),
        )
    } else {
        (
            format!("Runtime observation — run {}", run.run),
            badge(run.verification),
        )
    };
    let mut out = format!(
        "<div class=\"card runtime\"><div class=\"label\">{}</div><p>{} {}</p><p class=\"muted\">mode <b>{}</b> · injected: <b>{}</b> · outcome <b>{}</b> · exit {}{} · {} ms</p>",
        escape(&title),
        verdict,
        escape(&run.reason),
        escape(run.failure_mode.as_str()),
        run.injected,
        escape(&run.outcome),
        escape(&run.exit.kind),
        run.exit
            .signal
            .map(|s| format!(" (signal {s})"))
            .or_else(|| run.exit.code.map(|c| format!(" (code {c})")))
            .unwrap_or_default(),
        run.duration_ms
    );
    out.push_str("<table><tr><th>seq</th><th>checkpoint reached</th><th>supervisor</th></tr>");
    for checkpoint in &run.checkpoints {
        let _ = write!(
            out,
            "<tr><td>{}</td><td><code>{}</code>{}</td><td class=\"{}\">{}</td></tr>",
            checkpoint.seq,
            escape(&checkpoint.name),
            checkpoint
                .label
                .as_ref()
                .map(|l| format!(" ({})", escape(l)))
                .unwrap_or_default(),
            if checkpoint.action == "continue" {
                ""
            } else {
                "bad"
            },
            escape(&checkpoint.action)
        );
    }
    out.push_str("</table>");
    for check in &run.pre_injection_checks {
        let _ = write!(
            out,
            "<p>Checked before injecting: <code>{}</code> → <span class=\"{}\">{}</span> (observed <code>{}</code>)</p>",
            escape(&check.description),
            if check.holds { "ok" } else { "bad" },
            if check.holds { "holds" } else { "FAILED" },
            escape(&check.observed.to_string())
        );
    }
    out.push_str(&observation_table(run));
    out.push_str("<table><tr><th>invariant</th><th>stage</th><th>result</th></tr>");
    for invariant in &run.invariants {
        let _ = write!(
            out,
            "<tr><td><code>{}</code></td><td>{}</td><td class=\"{}\">{}</td></tr>",
            escape(&invariant.id),
            escape(&invariant.stage),
            if invariant.holds { "ok" } else { "bad" },
            escape(&invariant.detail)
        );
    }
    out.push_str("</table>");
    if !run.prediction.is_empty() {
        out.push_str(
            "<table><tr><th>predicted by the trace</th><th>expected</th><th>observed</th></tr>",
        );
        for p in &run.prediction {
            let _ = write!(
                out,
                "<tr><td><code>{}</code></td><td class=\"mono\">{}</td><td class=\"mono {}\">{}</td></tr>",
                escape(&p.observation),
                escape(&p.expected.to_string()),
                if p.matches { "ok" } else { "bad" },
                escape(&p.observed.to_string())
            );
        }
        out.push_str("</table>");
    }
    for recovery in &run.recovery {
        let _ = write!(
            out,
            "<p>Recovery scenario <code>{}</code>: {}</p>",
            escape(&recovery.scenario),
            escape(&recovery.outcome)
        );
    }
    let _ = write!(
        out,
        "<details><summary>Harness output{}</summary><pre>{}</pre><pre>{}</pre></details></div>",
        if run.logs.truncated {
            " (truncated)"
        } else {
            ""
        },
        escape(&run.logs.stdout),
        escape(&run.logs.stderr)
    );
    out
}

/// Anchor id of a case card (case ids are `[a-z0-9-]+`).
fn case_anchor(case: &CaseResult) -> String {
    format!(
        "case-{}",
        case.id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            })
            .collect::<String>()
    )
}

/// Which evidence supports the case, by provenance.
fn evidence_html(case: &CaseResult) -> String {
    let reproduced = case
        .runs
        .iter()
        .filter(|r| {
            r.verification == midwrite_core::verification::VerificationLevel::RuntimeReproduced
        })
        .count();
    let statik = match &case.trace {
        Some(trace) => format!(
            "<b>static</b>: STATIC_CANDIDATE, finding <code>{}</code>, trace <code>{}</code> ({:?})",
            escape(&trace.finding_id),
            escape(&trace.id),
            trace.completeness
        ),
        None => "<b>static</b>: no linked finding".to_owned(),
    };
    let predictions: Vec<bool> = case
        .runs
        .iter()
        .flat_map(|r| r.prediction.iter().map(|p| p.matches))
        .collect();
    let simulated = if predictions.is_empty() {
        "<b>simulated</b>: no prediction declared".to_owned()
    } else {
        format!(
            "<b>simulated</b>: the declared prediction matched {} of {} observation(s)",
            predictions.iter().filter(|m| **m).count(),
            predictions.len()
        )
    };
    let runtime = format!(
        "<b>runtime</b>: {} — reproduced in {reproduced} of {} run(s){}",
        case.verification,
        case.runs.len(),
        if case.consistent {
            ""
        } else {
            " (runs disagree)"
        }
    );
    let recovery = case
        .recovery_observation
        .as_ref()
        .map(|o| {
            format!(
                "<br><b>recovery</b> (runtime observation): {}",
                escape(&o.summary)
            )
        })
        .unwrap_or_default();
    format!(
        "<p class=\"muted\">Evidence by provenance — {statik} · {simulated} · {runtime}{recovery}</p>"
    )
}

/// One row per case, grouped by the finding it targets.
fn findings_table(report: &VerificationReport) -> String {
    let mut rows = String::new();
    let mut cases: Vec<&CaseResult> = report.cases.iter().collect();
    cases.sort_by(|a, b| a.finding_id.cmp(&b.finding_id));
    for case in cases {
        let reproduced = case
            .runs
            .iter()
            .filter(|r| {
                r.verification == midwrite_core::verification::VerificationLevel::RuntimeReproduced
            })
            .count();
        let _ = write!(
            rows,
            "<tr><td><code>{}</code></td><td>{}</td><td><a href=\"#{}\"><code>{}</code></a></td><td>{}</td><td>{}</td><td>{reproduced}/{}</td></tr>",
            escape(case.finding_id.as_deref().unwrap_or("—")),
            escape(case.code.as_deref().unwrap_or("—")),
            case_anchor(case),
            escape(&case.id),
            escape(case.failure_mode.as_str()),
            badge(case.verification),
            case.runs.len()
        );
    }
    format!(
        "<div class=\"card\"><h2 style=\"margin:0;font-size:16px\">Findings and cases</h2><table><tr><th>finding</th><th>code</th><th>case</th><th>failure mode</th><th>runtime result</th><th>reproduced runs</th></tr>{rows}</table><p class=\"muted\">A finding is a static candidate; a case is one controlled execution of this harness. Runtime results never change a finding's static confidence.</p></div>"
    )
}

fn case_html(case: &CaseResult) -> String {
    let mut out = format!(
        "<div class=\"card\" id=\"{}\"><h2 style=\"margin:0 0 6px;font-size:16px\">{} <code>{}</code></h2><p>{}</p><p class=\"muted\">failure mode <b>{}</b>{} · static link: {:?} — {} · consistent across runs: {}{}</p>",
        case_anchor(case),
        badge(case.verification),
        escape(&case.id),
        escape(&case.description),
        escape(case.failure_mode.as_str()),
        case.checkpoint
            .as_ref()
            .map(|c| format!(" at <code>{}</code> #{}", escape(&c.name), c.occurrence))
            .unwrap_or_default(),
        case.static_link.status,
        escape(&case.static_link.detail),
        case.consistent,
        match case.matches_expectation {
            Some(true) => " · <span class=\"ok\">as expected</span>",
            Some(false) => " · <span class=\"bad\">NOT as expected</span>",
            None => "",
        }
    );
    out.push_str(&evidence_html(case));
    out.push_str("<div class=\"grid\"><div>");
    match &case.trace {
        Some(trace) => out.push_str(&trace_timeline(trace)),
        None => out.push_str("<div class=\"card static\"><div class=\"label\">Static prediction</div><p class=\"muted\">No static trace is linked to this case.</p></div>"),
    }
    out.push_str("</div><div>");
    if case.runs.is_empty() {
        out.push_str(
            "<div class=\"card runtime\"><p class=\"muted\">No run was executed.</p></div>",
        );
    }
    for run in &case.runs {
        out.push_str(&run_html(run, false));
    }
    out.push_str("</div></div>");
    if !case.limitations.is_empty() {
        let _ = write!(
            out,
            "<details open><summary>Notes</summary>{}</details>",
            list(&case.limitations)
        );
    }
    out.push_str("</div>");
    out
}

pub fn verification_html(report: &VerificationReport) -> String {
    let s = report.summary;
    let env = &report.environment;
    let mut body = format!(
        "<div class=\"card\"><p><b>Harness</b> <code>{}</code> — {}</p><p class=\"muted\">command <code>{}</code> · manifest <code>{}</code> (hash <code>{}</code>) · revision {}</p><p class=\"muted\">{} {} · {} · {} · network: {} · database: {}</p><p>{} case(s): {} reproduced, {} not reproduced, {} inconclusive; {} as expected, {} not as expected.</p>{}</div>",
        escape(&report.harness.id),
        escape(&report.harness.description),
        escape(&report.harness.command.join(" ")),
        escape(&report.harness.manifest),
        escape(&report.harness.manifest_hash),
        escape(
            report
                .source
                .revision
                .as_deref()
                .unwrap_or("unknown (not a committed Git revision)")
        ),
        escape(&env.os),
        escape(&env.arch),
        escape(env.postgres.as_deref().unwrap_or("?")),
        escape(env.runtime.as_deref().unwrap_or("?")),
        escape(&match &env.isolation {
            Some(kind) => format!("[{kind}] {}", env.network_isolation),
            None => env.network_isolation.clone(),
        }),
        escape(&env.database),
        s.cases,
        s.reproduced,
        s.not_reproduced,
        s.inconclusive,
        s.matched_expectations,
        s.mismatched_expectations,
        list(&report.limitations)
    );
    body.push_str(&findings_table(report));
    if let Some(control) = &report.control {
        body.push_str("<div class=\"card\"><h2 style=\"margin:0;font-size:16px\">Control run (no injection)</h2>");
        body.push_str(&run_html(control, true));
        body.push_str("</div>");
    }
    for case in &report.cases {
        body.push_str(&case_html(case));
    }
    page(
        "Midwrite verification report",
        &format!(
            "{} {} · runtime replay of harness {}",
            report.tool.name, report.tool.version, report.harness.id
        ),
        &body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_neutralizes_markup() {
        assert_eq!(
            escape("<script>alert('x')</script>&\""),
            "&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;&amp;&quot;"
        );
    }
}

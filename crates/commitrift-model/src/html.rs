//! A script-free HTML view of a model-checking report: the concurrent
//! timeline of a counterexample, with transaction boundaries, blocking,
//! commits, rollbacks, crashes and retries, and — when the schedule was
//! replayed — the model's prediction beside what PostgreSQL did.
//!
//! The page loads nothing and runs nothing (a Content-Security-Policy
//! forbids scripts and remote resources) and every string from the report is
//! escaped.

use std::fmt::Write as _;

use crate::engine::EventKind;
use crate::report::{EvidenceLevel, ModelCheckReport};
use crate::value::Value;

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
main{padding:16px 24px;max-width:1280px}
.card{background:#fff;border:1px solid #d7dce2;border-radius:6px;margin:14px 0;padding:12px 16px}
.badge{display:inline-block;padding:1px 8px;border-radius:10px;font-size:12px;font-weight:600;margin-right:6px;white-space:nowrap}
.COUNTEREXAMPLE_FOUND{background:#ffe2e0;color:#8c1d18}.NO_COUNTEREXAMPLE_WITHIN_BOUNDS{background:#dcf5e3;color:#185c2d}
.INCONCLUSIVE,.UNSUPPORTED{background:#f1f1f1;color:#555;border:1px solid #aaa}
.ev-model{background:#eef0ff;color:#3b3f8f;border:1px dashed #6b70c9}.ev-pg{background:#ffe2e0;color:#8c1d18}.ev-none{background:#e7eaee;color:#3b4552}
table{border-collapse:collapse;width:100%;margin:8px 0}td,th{border-bottom:1px solid #e3e7ec;padding:4px 8px;text-align:left;vertical-align:top}
th{font-size:12px;color:#5b6573}
code,.mono{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:12.5px}
td.t{width:34px;color:#5b6573;text-align:right}
td.cell{background:#fff}
.k-begin{border-left:4px solid #2f6feb}.k-commit{background:#e6f6ea;border-left:4px solid #1a7f37}
.k-rollback,.k-abort,.k-fail{background:#fdecea;border-left:4px solid #b42318}
.k-blocked{background:#fff4d6;border-left:4px solid #d29922}.k-resumed{color:#7a5200}
.k-crash{background:#2b2b2b;color:#fff}.k-lost{background:#fff1e0;border-left:4px solid #c25e00}.k-retry{background:#e8f1ff;border-left:4px solid #1f6feb}
.k-deadlock{background:#f3e8ff;border-left:4px solid #8250df}
.k-external{background:#eefaff;border-left:4px solid #0969da}
.k-read{border-left:4px solid #8a94a3}.k-write{border-left:4px solid #9a6700}.k-local{color:#5b6573}
.ok{color:#185c2d;font-weight:600}.bad{color:#8c1d18;font-weight:600}
.violation{background:#ffe2e0;border:2px solid #b42318;border-radius:6px;padding:8px 12px;font-weight:600;color:#8c1d18}
.muted{color:#5b6573}.label{font-size:11px;font-weight:700;letter-spacing:.06em;color:#5b6573;text-transform:uppercase}
details{margin:6px 0}summary{cursor:pointer;color:#2f4b6e}pre{background:#f3f4f6;padding:8px;overflow:auto;font-size:12px}
.pg{background:#f7f0ff}.model{background:#eef4ff}
table.lanes td{border-left:1px solid #e3e7ec;min-width:120px}table.lanes td.t{border-left:0}
.lane-op{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:12px}
.lane-crash{background:#2b2b2b;color:#fff;font-weight:700}.lane-retry{background:#e8f1ff;color:#0b3d91;font-weight:700}
.lane-db{background:#fbf7ff}.lane-effect{background:#eefaff}.lane-dup{background:#fff4d6}
.o-extracted-from-source{background:#dcf5e3;color:#185c2d}.o-declared-by-scenario{background:#eef0ff;color:#3b3f8f}
.o-scenario-override{background:#fff4d6;color:#7a5200}.o-not-modeled{background:#f1f1f1;color:#555}
.o-unknown{background:#ffe2e0;color:#8c1d18}.o-confirmed-by-runtime-catalog{background:#dcf5e3;color:#185c2d}
"#;

fn kind_class(k: EventKind) -> &'static str {
    match k {
        EventKind::Begin => "k-begin",
        EventKind::Commit => "k-commit",
        EventKind::Rollback => "k-rollback",
        EventKind::Abort => "k-abort",
        EventKind::Fail => "k-fail",
        EventKind::Blocked => "k-blocked",
        EventKind::Resumed => "k-resumed",
        EventKind::Crash => "k-crash",
        EventKind::AckLost | EventKind::ServerCompletion => "k-lost",
        EventKind::Retry => "k-retry",
        EventKind::Deadlock => "k-deadlock",
        EventKind::External => "k-external",
        EventKind::Read => "k-read",
        EventKind::Write => "k-write",
        EventKind::Local | EventKind::App | EventKind::Spawn => "k-local",
    }
}

fn evidence_class(l: EvidenceLevel) -> &'static str {
    match l {
        EvidenceLevel::ModelCounterexample => "ev-model",
        EvidenceLevel::PostgresConfirmedSchedule
        | EvidenceLevel::OriginalCodeReproduced
        | EvidenceLevel::OriginalCodeCrashReproduced
        | EvidenceLevel::OriginalCodeRecoveryReproduced
        | EvidenceLevel::OriginalCodeInvariantHeld => "ev-pg",
        _ => "ev-none",
    }
}

fn evidence_label(l: EvidenceLevel) -> &'static str {
    match l {
        EvidenceLevel::None => "no counterexample",
        EvidenceLevel::ModelCounterexample => "MODEL COUNTEREXAMPLE (not run on a database)",
        EvidenceLevel::PostgresConfirmedSchedule => "POSTGRESQL-CONFIRMED SCHEDULE",
        EvidenceLevel::PostgresDidNotReproduce => "POSTGRESQL DID NOT REPRODUCE",
        EvidenceLevel::RuntimeInconclusive => "RUNTIME VALIDATION INCONCLUSIVE",
        EvidenceLevel::OriginalCodeReproduced => {
            "ORIGINAL CODE REPRODUCED (application ran on PostgreSQL)"
        }
        EvidenceLevel::OriginalCodeCrashReproduced => {
            "ORIGINAL CODE CRASH REPRODUCED (workers killed and retried on PostgreSQL)"
        }
        EvidenceLevel::OriginalCodeRecoveryReproduced => {
            "ORIGINAL CODE RECOVERY REPRODUCED (invariant held under a real crash and retry; bounded)"
        }
        EvidenceLevel::OriginalCodeInvariantHeld => {
            "ORIGINAL CODE INVARIANT HELD (one model schedule ran on PostgreSQL; bounded)"
        }
    }
}

pub fn model_check_html(r: &ModelCheckReport) -> String {
    let mut b = String::new();
    let verdict = &r.exploration.verdict;
    let _ = write!(
        b,
        "<div class=\"card\"><span class=\"badge {}\">{}</span><span class=\"badge {}\">{}</span> <span class=\"muted\">{} · {} actor(s) · {} state(s), {} transition(s){}</span><p>{}</p></div>",
        escape(verdict),
        escape(verdict),
        evidence_class(r.evidence.level),
        evidence_label(r.evidence.level),
        escape(&r.database.isolation),
        r.actors.len(),
        r.exploration.metrics.states,
        r.exploration.metrics.transitions,
        if r.bounds.partial_order_reduction {
            " · sleep-set reduction"
        } else {
            ""
        },
        escape(&r.evidence.summary)
    );
    if !r.scenario.description.is_empty() {
        let _ = write!(
            b,
            "<div class=\"card\">{}</div>",
            escape(&r.scenario.description)
        );
    }
    // Actors
    b.push_str("<div class=\"card\"><div class=\"label\">Actors</div><table><tr><th>Actor</th><th>Function</th><th>Arguments</th><th>Process</th><th>Origin</th><th>Source</th></tr>");
    for a in &r.actors {
        let args: Vec<String> = a
            .arguments
            .iter()
            .map(|(k, v)| format!("{k}={}", v.render()))
            .collect();
        let loc = a
            .source_refs
            .iter()
            .map(|s| s.label())
            .filter(|l| !l.is_empty())
            .take(3)
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(
            b,
            "<tr><td><b>{}</b></td><td class=\"mono\">{}{}</td><td class=\"mono\">{}</td><td>{}</td><td>{}</td><td class=\"mono\">{}</td></tr>",
            escape(&a.name),
            escape(&a.function),
            a.file
                .as_ref()
                .map(|f| format!(" ({})", escape(f)))
                .unwrap_or_default(),
            escape(&args.join(", ")),
            escape(&a.process),
            escape(&a.origin),
            escape(&loc)
        );
    }
    b.push_str("</table></div>");

    if let Some(w) = &r.counterexample {
        let _ = write!(
            b,
            "<div class=\"card\"><div class=\"label\">Counterexample · {} transitions · {} context switches{}</div>",
            w.steps.len(),
            w.context_switches,
            if w.minimized {
                " · minimized and replayed"
            } else {
                ""
            }
        );
        b.push_str("<table><tr><th>#</th>");
        for a in &r.actors {
            let _ = write!(b, "<th>Request {}</th>", escape(&a.name));
        }
        b.push_str("<th>Environment</th>");
        let runtime = r.runtime.performed;
        if runtime {
            b.push_str("<th class=\"pg\">PostgreSQL observed</th>");
        }
        b.push_str("</tr>");
        let mut shown_obs = std::collections::BTreeSet::new();
        for step in &w.steps {
            for (n, e) in step.events.iter().enumerate() {
                let col = e.actor as usize;
                let _ = write!(
                    b,
                    "<tr><td class=\"t\">{}</td>",
                    if n == 0 {
                        step.index.to_string()
                    } else {
                        String::new()
                    }
                );
                for i in 0..r.actors.len() {
                    if i == col && e.kind != EventKind::Crash {
                        let _ = write!(
                            b,
                            "<td class=\"cell {}\"><code>{}</code></td>",
                            kind_class(e.kind),
                            escape(&e.text)
                        );
                    } else {
                        b.push_str("<td></td>");
                    }
                }
                if e.kind == EventKind::Crash {
                    let _ = write!(
                        b,
                        "<td class=\"cell {}\">{}: {}</td>",
                        kind_class(e.kind),
                        escape(&r.actors[col].name),
                        escape(&e.text)
                    );
                } else {
                    b.push_str("<td></td>");
                }
                if runtime {
                    if n == 0 && shown_obs.insert(step.index) {
                        let obs: Vec<String> = r
                            .runtime
                            .observations
                            .iter()
                            .filter(|o| o.transition == step.index)
                            .map(|o| {
                                format!(
                                    "<span class=\"{}\">{}</span> <code>{}</code>",
                                    if o.agree { "ok" } else { "bad" },
                                    if o.agree { "=" } else { "≠" },
                                    escape(&format!(
                                        "predicted {} · observed {}",
                                        o.predicted, o.observed
                                    ))
                                )
                            })
                            .collect();
                        let _ = write!(b, "<td class=\"pg\">{}</td>", obs.join("<br>"));
                    } else {
                        b.push_str("<td class=\"pg\"></td>");
                    }
                }
                b.push_str("</tr>");
            }
        }
        let span = 2 + r.actors.len() + usize::from(runtime);
        let _ = write!(
            b,
            "<tr><td colspan=\"{span}\"><div class=\"violation\">Invariant <code>{}</code> VIOLATED: {}</div></td></tr>",
            escape(&w.invariant),
            escape(&w.formula)
        );
        b.push_str("</table>");
        // final state
        b.push_str("<details open><summary>Final state</summary><table><tr><th>Table</th><th>Committed rows</th></tr>");
        for (t, rows) in &w.final_state.tables {
            let rendered: Vec<String> = rows
                .iter()
                .map(|row| Value::Row(row.clone()).render())
                .collect();
            let _ = write!(
                b,
                "<tr><td class=\"mono\">{}</td><td class=\"mono\">{}</td></tr>",
                escape(t),
                if rendered.is_empty() {
                    "(no rows)".into()
                } else {
                    escape(&rendered.join("; "))
                }
            );
        }
        b.push_str("</table><table><tr><th>Actor</th><th>Outcome</th></tr>");
        for (a, o) in &w.final_state.actors {
            let _ = write!(b, "<tr><td>{}</td><td>{}</td></tr>", escape(a), escape(o));
        }
        b.push_str("</table>");
        if !w.final_state.external_effects.is_empty() {
            let _ = write!(
                b,
                "<p>External effects: {}</p>",
                escape(&w.final_state.external_effects.join("; "))
            );
        }
        b.push_str("<table><tr><th>Invariant term</th><th>Value</th></tr>");
        for t in &w.final_state.terms {
            let _ = write!(
                b,
                "<tr><td class=\"mono\">{}</td><td class=\"mono\">{}</td></tr>",
                escape(&t.term),
                escape(&t.value.render())
            );
        }
        b.push_str("</table></details></div>");
    }

    // Runtime comparison
    if r.runtime.performed || r.runtime.status == "blocked" {
        let _ = write!(
            b,
            "<div class=\"card pg\"><div class=\"label\">PostgreSQL validation · {}{}</div><p>{}</p>",
            escape(&r.runtime.status),
            r.runtime
                .postgres_version
                .as_ref()
                .map(|v| format!(" · {}", escape(v)))
                .unwrap_or_default(),
            escape(&r.runtime.note)
        );
        if !r.runtime.observations.is_empty() {
            b.push_str("<table><tr><th></th><th>#</th><th>Actor</th><th>Statement</th><th>Model predicted</th><th>PostgreSQL observed</th></tr>");
            for o in &r.runtime.observations {
                let _ = write!(
                    b,
                    "<tr><td class=\"{}\">{}</td><td>{}</td><td>{}</td><td class=\"mono\">{}</td><td>{}</td><td>{}</td></tr>",
                    if o.agree { "ok" } else { "bad" },
                    if o.agree { "=" } else { "≠" },
                    o.transition,
                    escape(&o.actor),
                    escape(&o.sql),
                    escape(&o.predicted),
                    escape(&o.observed)
                );
            }
            b.push_str("</table>");
        }
        if !r.runtime.sqlstates.is_empty() {
            let _ = write!(
                b,
                "<p>SQLSTATEs observed: <code>{}</code></p>",
                escape(&r.runtime.sqlstates.join(", "))
            );
        }
        b.push_str("</div>");
    }

    // Phase 7: extraction facts and the original-code verification.
    for x in &r.extraction {
        let _ = write!(
            b,
            "<div class=\"card\"><div class=\"label\">Extraction · {} · {}</div><p>{} represented, {} assumed, {} unsupported operation(s); declared contract: {}</p><ul>",
            escape(&x.actor),
            escape(&x.entry.selector),
            x.source_coverage.represented_operations,
            x.source_coverage.assumed_operations,
            x.source_coverage.unsupported_operations,
            escape(&x.entry.contract_status)
        );
        for f in &x.source_facts {
            let _ = write!(b, "<li>source: {}</li>", escape(f));
        }
        for a in &x.scenario_assumptions {
            let _ = write!(b, "<li><b>declared</b> (not proven): {}</li>", escape(a));
        }
        b.push_str("</ul></div>");
    }
    if let Some(sv) = &r.source_verification {
        let _ = write!(
            b,
            "<div class=\"card pg\"><div class=\"label\">Original-code verification · {} · harness {}{}</div><p>{}</p>",
            escape(&sv.status),
            escape(&sv.harness),
            sv.postgres_version
                .as_ref()
                .map(|v| format!(" · {}", escape(v)))
                .unwrap_or_default(),
            escape(&sv.note)
        );
        for run in &sv.runs {
            let _ = write!(
                b,
                "<p>run {}: coordination {}, statements {}, final rows {}, outcomes {}, invariant on observed rows: {}</p>",
                run.run,
                escape(&run.coordination),
                if run.statements_agree {
                    "agree"
                } else {
                    "differ"
                },
                if run.tables_agree { "agree" } else { "differ" },
                if run.outcomes_agree {
                    "agree"
                } else {
                    "differ"
                },
                escape(&run.invariant)
            );
        }
        if !sv.statements.is_empty() {
            b.push_str("<table><tr><th></th><th>#</th><th>Actor</th><th>Model operation</th><th>SQL the application sent</th><th>Predicted</th><th>Observed</th></tr>");
            for c in &sv.statements {
                let ok = c.status == "agree";
                let _ = write!(
                    b,
                    "<tr><td class=\"{}\">{}</td><td>{}</td><td>{}</td><td class=\"mono\">{}<br>{}</td><td class=\"mono\">{}<br>{}</td><td>{}</td><td>{}{}</td></tr>",
                    if ok { "ok" } else { "bad" },
                    if ok { "=" } else { "≠" },
                    c.transition,
                    escape(&c.actor),
                    escape(&c.model),
                    escape(&c.model_source),
                    escape(&c.sql),
                    escape(&c.runtime_source),
                    escape(&c.predicted),
                    escape(&c.observed),
                    c.differences
                        .iter()
                        .map(|d| format!("<br><span class=\"bad\">{}</span>", escape(d)))
                        .collect::<String>()
                );
            }
            b.push_str("</table>");
        }
        if let Some(rec) = &sv.recovery {
            recovery_timeline(&mut b, sv, rec);
        }
        if let Some(f) = &sv.schema_fidelity {
            let _ = write!(
                b,
                "<p><b>Schema fidelity</b> (extracted Drizzle schema vs the run database's catalog, {}): <span class=\"{}\">{}</span></p>",
                escape(&f.postgres_version),
                if f.status == "agree" { "ok" } else { "bad" },
                escape(&f.status)
            );
            for t in &f.tables {
                for i in t.items.iter().filter(|i| i.status != "agree") {
                    let _ = write!(
                        b,
                        "<p class=\"bad\">{} {}: {} {}</p>",
                        escape(&t.table),
                        escape(&i.fact),
                        escape(&i.status),
                        escape(&i.detail)
                    );
                }
            }
        }
        for c in &sv.transaction_config {
            let _ = write!(
                b,
                "<p class=\"muted\">transaction configuration released with its BEGIN: <code>{}</code></p>",
                escape(c)
            );
        }
        for d in &sv.driver_internal {
            let _ = write!(
                b,
                "<p class=\"muted\">driver-internal (not held): <code>{}</code></p>",
                escape(d)
            );
        }
        for o in &sv.outside_model {
            let _ = write!(
                b,
                "<p>outside the model (declared preconditions): <code>{}</code></p>",
                escape(o)
            );
        }
        for u in &sv.unexpected {
            let _ = write!(
                b,
                "<p class=\"bad\">unexpected: <code>{}</code></p>",
                escape(u)
            );
        }
        for p in &sv.problems {
            let _ = write!(b, "<p class=\"bad\">problem: {}</p>", escape(p));
        }
        b.push_str("</div>");
    }

    if !r.schema_facts.is_empty() {
        b.push_str(
            "<details class=\"card\"><summary>Schema facts behind the model's tables</summary>",
        );
        for t in &r.schema_facts {
            let _ = write!(
                b,
                "<p><b>{}</b>{}</p><table><tr><th>Origin</th><th>Fact</th><th>Source</th></tr>",
                escape(&t.table),
                t.binding
                    .as_ref()
                    .map(|x| format!(
                        " <span class=\"muted\">(Drizzle binding <code>{}</code>)</span>",
                        escape(x)
                    ))
                    .unwrap_or_default()
            );
            for f in &t.facts {
                let origin = serde_json::to_value(f.origin)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                let _ = write!(
                    b,
                    "<tr><td><span class=\"badge o-{}\">{}</span></td><td>{}</td><td class=\"mono\">{}</td></tr>",
                    escape(&origin),
                    escape(&origin),
                    escape(&f.fact),
                    escape(&f.source)
                );
            }
            b.push_str("</table>");
        }
        b.push_str("</details>");
    }

    // Invariants and bounds
    b.push_str("<div class=\"card\"><div class=\"label\">Invariants</div><table><tr><th>Name</th><th>When</th><th>Assertion</th><th>Status</th></tr>");
    for i in &r.invariants {
        let _ = write!(
            b,
            "<tr><td>{}</td><td>{:?}</td><td class=\"mono\">{}</td><td class=\"{}\">{}</td></tr>",
            escape(&i.name),
            i.when,
            escape(&i.formula),
            if i.status == "violated" { "bad" } else { "" },
            escape(&i.status)
        );
    }
    b.push_str("</table></div>");
    let t = &r.exploration.truncation;
    let _ = write!(
        b,
        "<div class=\"card\"><div class=\"label\">Exploration bounds and coverage</div><pre>{}</pre><p>Resource limits reached: {} · complete within bounds: {}</p></div>",
        escape(&serde_json::to_string_pretty(&r.bounds).unwrap_or_default()),
        if t.resource_limit() { "yes" } else { "none" },
        r.exploration.complete
    );
    if !r.exploration.unsupported.is_empty() {
        b.push_str(
            "<div class=\"card\"><div class=\"label\">Unsupported behavior reached</div><ul>",
        );
        for u in &r.exploration.unsupported {
            let _ = write!(
                b,
                "<li>actor {} at <code>{}</code>: {}</li>",
                escape(&u.actor),
                escape(&u.location),
                escape(&u.detail)
            );
        }
        b.push_str("</ul></div>");
    }
    b.push_str("<details class=\"card\"><summary>Modeling assumptions</summary><ul>");
    for a in &r.assumptions {
        let _ = write!(b, "<li>{}</li>", escape(a));
    }
    b.push_str("</ul></details>");
    for d in &r.diagnostics {
        let _ = write!(
            b,
            "<p><b>{}</b> {}: {}</p>",
            escape(&d.severity),
            escape(&d.code),
            escape(&d.message)
        );
    }
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src 'none'; script-src 'none'; form-action 'none'; base-uri 'none'\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{}</title><style>{STYLE}</style></head><body><header><h1>{}</h1><p>model checking · model {} · report schema {}</p></header><main>{b}</main></body></html>\n",
        escape(&format!("Model check: {}", r.scenario.name)),
        escape(&format!("Model check: {}", r.scenario.name)),
        escape(&r.model_version),
        r.schema_version
    )
}

/// Phase 8: the run as lanes — one per actor execution (`Client#1`,
/// `Client#2` after a restart), PostgreSQL, and each channel test double —
/// with the planned crashes (SIGKILL) and restarts where they happened.
fn recovery_timeline(
    b: &mut String,
    sv: &crate::source_types::SourceVerification,
    rec: &crate::source_types::RecoveryVerification,
) {
    use std::collections::BTreeSet;
    // Which execution a transition of an actor belongs to.
    let attempt = |actor: &str, transition: usize| -> u32 {
        1 + rec
            .restarts
            .iter()
            .filter(|r| r.actor == actor && r.transition <= transition)
            .count() as u32
    };
    let mut lanes: Vec<String> = Vec::new();
    let mut add = |lane: String| {
        if !lanes.contains(&lane) {
            lanes.push(lane);
        }
    };
    let mut channels: BTreeSet<String> = BTreeSet::new();
    for c in &sv.statements {
        add(format!("{}#{}", c.actor, attempt(&c.actor, c.transition)));
        if let Some(ch) = c.sql.strip_prefix("EFFECT ") {
            channels.insert(ch.to_owned());
        }
    }
    for r in &rec.restarts {
        add(format!("{}#{}", r.actor, r.attempt));
    }
    for c in &rec.crashes {
        add(format!("{}#{}", c.actor, c.attempt));
    }
    lanes.sort();
    let columns: Vec<String> = lanes
        .iter()
        .cloned()
        .chain(std::iter::once("PostgreSQL".to_owned()))
        .chain(channels.iter().map(|c| format!("{c} (test double)")))
        .collect();
    let _ = write!(
        b,
        "<p><b>Crash and recovery timeline</b> — one lane per execution; ✕ is a real SIGKILL of the worker's process group, ↻ a new process started by the declared mechanism.</p><table class=\"lanes\"><tr><th>#</th>{}</tr>",
        columns
            .iter()
            .map(|c| format!("<th>{}</th>", escape(c)))
            .collect::<String>()
    );
    // Events by transition: statements/effects, crashes, restarts.
    let mut transitions: BTreeSet<usize> = BTreeSet::new();
    transitions.extend(sv.statements.iter().map(|c| c.transition));
    transitions.extend(rec.crashes.iter().map(|c| c.transition));
    transitions.extend(rec.restarts.iter().map(|r| r.transition));
    let row = |b: &mut String, t: usize, cells: Vec<(usize, String, &str)>| {
        let _ = write!(b, "<tr><td class=\"t\">{t}</td>");
        for (i, _) in columns.iter().enumerate() {
            match cells.iter().find(|(c, _, _)| *c == i) {
                Some((_, html, class)) => {
                    let _ = write!(b, "<td class=\"{class}\">{html}</td>");
                }
                None => b.push_str("<td></td>"),
            }
        }
        b.push_str("</tr>");
    };
    let lane_of = |name: &str| columns.iter().position(|c| c == name).unwrap_or(0);
    let db = lanes.len();
    for t in transitions {
        for c in sv.statements.iter().filter(|c| c.transition == t) {
            let lane = lane_of(&format!("{}#{}", c.actor, attempt(&c.actor, c.transition)));
            let ok = c.status == "agree";
            let mut cells = vec![(
                lane,
                format!(
                    "{} {}<br><span class=\"muted\">{}</span>",
                    if ok { "●" } else { "≠" },
                    escape(&c.model),
                    escape(&c.runtime_source)
                ),
                "lane-op",
            )];
            match c.sql.strip_prefix("EFFECT ") {
                Some(ch) => {
                    let target = lane_of(&format!("{ch} (test double)"));
                    let dup = c.observed.contains("deduplicated");
                    cells.push((
                        target,
                        format!("◀ {}", escape(&c.observed)),
                        if dup { "lane-dup" } else { "lane-effect" },
                    ));
                }
                None => cells.push((db, format!("◀ {}", escape(&c.observed)), "lane-db")),
            }
            row(b, t, cells);
        }
        for c in rec.crashes.iter().filter(|c| c.transition == t) {
            let lane = lane_of(&format!("{}#{}", c.actor, c.attempt));
            row(
                b,
                t,
                vec![
                    (
                        lane,
                        format!(
                            "✕ CRASH — SIGKILL pid {}{}<br><span class=\"muted\">{}</span>",
                            c.pid,
                            if c.killed_by_signal {
                                ""
                            } else {
                                " (NOT confirmed)"
                            },
                            escape(&c.checkpoint)
                        ),
                        "lane-crash",
                    ),
                    (
                        db,
                        format!(
                            "sessions ended: {} ({} ms); committed rows {} the model",
                            c.sessions_ended,
                            c.sessions_ended_ms,
                            if c.status == "agree" {
                                "agree with"
                            } else {
                                "DIFFER from"
                            }
                        ),
                        "lane-db",
                    ),
                ],
            );
        }
        for r in rec.restarts.iter().filter(|r| r.transition == t) {
            let lane = lane_of(&format!("{}#{}", r.actor, r.attempt));
            row(
                b,
                t,
                vec![(
                    lane,
                    format!(
                        "↻ RETRY — new process pid {} (previous pid {}, {})<br><span class=\"muted\">{}</span>",
                        r.pid,
                        r.previous_pid,
                        escape(&r.previous_outcome),
                        escape(&r.mechanism)
                    ),
                    "lane-retry",
                )],
            );
        }
    }
    b.push_str("</table>");
    let _ = write!(
        b,
        "<p>Effects — model: <code>{}</code>; channel double: <code>{}</code> ({}).</p><p>Executions (model / observed): {}.</p><p class=\"muted\">{}</p>",
        escape(&rec.effects.model.join(" · ")),
        escape(&rec.effects.runtime.join(" · ")),
        if rec.effects.agree { "agree" } else { "DIFFER" },
        escape(
            &rec.attempts
                .iter()
                .map(|(a, m, o)| format!("{a} {m}/{o}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        escape(&rec.quiescence)
    );
    for c in &rec.contracts {
        let _ = write!(b, "<p class=\"muted\">declared contract: {}</p>", escape(c));
    }
    for s in &rec.server_settings {
        let _ = write!(
            b,
            "<p class=\"muted\">server setting for this run: <code>{}</code></p>",
            escape(s)
        );
    }
}

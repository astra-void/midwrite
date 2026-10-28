//! The discovery report (`commitrift.discovery`, schema version 1) and its
//! text and HTML renderings.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Serialize;

use crate::candidate::{Candidate, Rejected};
use crate::project::HelperNote;
use crate::synth::{SynthesisReport, Variant};

pub const REPORT_KIND: &str = "commitrift.discovery";
pub const REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    /// The project directory's own name (never an absolute path).
    pub name: String,
    pub modules: usize,
    pub parse_errors: usize,
    pub schema_files: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contracts: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStats {
    pub entry_candidates: usize,
    pub entries_by_kind: BTreeMap<String, usize>,
    /// Entries whose import closure mentions no database receiver and no
    /// declared external: not extracted (they cannot produce database or
    /// declared external effects).
    pub entries_without_database_reach: usize,
    pub entries_extracted: usize,
    pub extraction_failures: usize,
    pub effectful_entries: usize,
    pub read_only_entries: usize,
    pub entries_with_unknown_operations: usize,
    pub unknown_operations: usize,
    pub tables_modeled: usize,
    pub tables_refused: Vec<(String, String)>,
    /// Phase 12: modeled columns whose default generates a fresh identifier
    /// (`table.column`, `database`/`application`, generator).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub generated_columns: Vec<(String, String, String)>,
    /// Distinct written (table, column) pairs and external channels.
    pub resources: usize,
    pub transaction_helpers: Vec<HelperNote>,
    pub cache_hits: usize,
    pub cache_misses: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryReport {
    pub selector: String,
    pub kind: String,
    pub invocation: String,
    /// `effectful`, `read-only`, `no-database-effects`,
    /// `extraction-failed`, `not-extracted`.
    pub status: String,
    pub reads: usize,
    pub writes: usize,
    pub externals: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvariantReport {
    pub name: String,
    pub origin: String,
    pub formula: String,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Validation {
    /// The schedule replays in the model, every transition enabled.
    pub replayed: bool,
    /// The replayed final state violates the reported invariant.
    pub violates: bool,
    /// The initial rows satisfy the modeled constraints (loader) and the
    /// source's foreign keys.
    pub initial_state_valid: bool,
    /// The candidate's operations the counterexample executes.
    pub operations_exercised: Vec<String>,
    /// The candidate's operations it does not execute.
    pub operations_not_exercised: Vec<String>,
    pub minimized: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelResult {
    /// The model checker's verdict.
    pub verdict: String,
    /// The model checker's evidence level (`model-counterexample`, ...).
    pub evidence: String,
    pub algorithm: String,
    pub complete: bool,
    pub states: u64,
    pub transitions: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unsupported: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub violated: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub schedule: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub final_terms: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation: Option<Validation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wall_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioResult {
    pub variant: Variant,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    pub synthesis: SynthesisReport,
    /// The synthesized actors (arguments, invocation contract).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actors: Vec<crate::synth::ActorPlan>,
    /// The synthesized initial rows.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub initial_state: BTreeMap<String, Vec<commitrift_model::value::Row>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub invariants: Vec<InvariantReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub invariant_notes: Vec<String>,
    /// Whether the scenario can be run against the original code
    /// (`commitrift model-check <file> --verify-source --allow-execution`).
    pub source_verification: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelResult>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateReport {
    pub rank: usize,
    #[serde(flatten)]
    pub candidate: Candidate,
    /// `STATIC_CANDIDATE`, `MODEL_COUNTEREXAMPLE`,
    /// `NO_VIOLATION_WITHIN_BOUNDS`, `INCONCLUSIVE`, `UNSUPPORTED`,
    /// `BLOCKED`.
    pub status: String,
    pub finding: String,
    /// Candidates with the same group describe the same underlying issue.
    pub group: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duplicate_of: Option<String>,
    pub ranking: Vec<String>,
    pub scenarios: Vec<ScenarioResult>,
    /// Phase 14: what a developer can do next to verify (or decide) the
    /// candidate, from its status and its property's origin.
    pub next_step: String,
}

/// Phase 14: the next verification step for a candidate.
pub fn next_step(c: &Candidate, status: &str, model_checked: bool) -> String {
    use crate::candidate::Origin;
    let hypothesis = c
        .properties
        .first()
        .is_some_and(|p| p.origin == Origin::HeuristicHypothesis);
    match status {
        "BLOCKED" => "declare the property this candidate needs in commitrift-contracts.json (`invariants`), then rerun `commitrift discover --model-check`".to_owned(),
        "STATIC_CANDIDATE" if !model_checked => "rerun with `--model-check` to explore the synthesized scenario".to_owned(),
        "STATIC_CANDIDATE" => "raise the candidate or state budgets (`--max-candidates`, `--max-total-states`) to build and check its scenario".to_owned(),
        "UNSUPPORTED" => "the first operation the model does not support is named above: declare it in commitrift-contracts.json if a contract applies, otherwise this candidate cannot be verified by CommitRift (docs/reference/limitations.md)".to_owned(),
        "INCONCLUSIVE" => "no valid initial state or property instance was found: declare argument `values` or an `initialState` for the entry in commitrift-contracts.json (docs/user-guide/contracts.md)".to_owned(),
        "MODEL_COUNTEREXAMPLE" if hypothesis => "decide whether the application requires this property; if it does, declare it in commitrift-contracts.json. Write the scenario with `--emit-scenarios <DIR>` and replay the counterexample with `commitrift model-check <DIR>/<file>`".to_owned(),
        "MODEL_COUNTEREXAMPLE" => "write the scenario with `--emit-scenarios <DIR>`, read the counterexample with `commitrift model-check <DIR>/<file>`; with an authorized harness, reproduce it on the original code with `--verify-source --allow-execution` (docs/user-guide/source-verification.md)".to_owned(),
        "NO_VIOLATION_WITHIN_BOUNDS" => "nothing to do within these bounds; a bounded search is not a proof (raise `--max-states` for a larger search)".to_owned(),
        _ => String::new(),
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Budgets {
    pub max_candidates: usize,
    pub max_partners: usize,
    pub max_states_per_scenario: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_wall_ms_per_scenario: Option<u64>,
    pub max_total_states: u64,
    pub candidates_found: usize,
    pub candidates_reported: usize,
    pub candidates_over_budget: usize,
    pub partners_truncated: usize,
    pub scenarios_built: usize,
    pub scenarios_model_checked: usize,
    pub scenarios_skipped_for_budget: usize,
    pub total_states_explored: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub candidates: usize,
    pub by_class: BTreeMap<String, usize>,
    pub by_status: BTreeMap<String, usize>,
    pub by_property_origin: BTreeMap<String, usize>,
    pub with_declared_property: usize,
    pub requiring_assumptions: usize,
    pub model_checkable: usize,
    pub model_counterexamples: usize,
    pub counterexamples_against_hypotheses: usize,
    pub deduplicated_findings: usize,
    /// Why candidates could not be verified, by category.
    pub unsupported_breakdown: BTreeMap<String, usize>,
    /// Operations the extractor did not understand in extracted entries, by
    /// category (what blocks verification of the project).
    pub unknown_operation_breakdown: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Timings {
    pub index_ms: u64,
    pub extraction_ms: u64,
    pub detection_ms: u64,
    pub synthesis_ms: u64,
    pub model_checking_ms: u64,
    pub total_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryReport {
    pub kind: String,
    pub schema_version: u32,
    pub tool: Tool,
    pub project: ProjectInfo,
    /// `discover`, `discover+model-check`.
    pub mode: String,
    pub algorithm: String,
    pub executes_application_code: bool,
    pub index: IndexStats,
    pub summary: Summary,
    pub budgets: Budgets,
    pub candidates: Vec<CandidateReport>,
    pub rejected: Vec<Rejected>,
    pub entries: Vec<EntryReport>,
    pub notes: Vec<String>,
    /// Scoring against labeled entries (`--labels`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evaluation: Option<crate::eval::Evaluation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timings: Option<Timings>,
}

impl DiscoveryReport {
    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("report serializes");
        s.push('\n');
        s
    }
}

pub fn render_text(r: &DiscoveryReport, verbose: bool) -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "commitrift discover: {} ({} modules, {} entry candidates, {} extracted, {} effectful)",
        r.project.name,
        r.project.modules,
        r.index.entry_candidates,
        r.index.entries_extracted,
        r.index.effectful_entries
    );
    let _ = writeln!(
        o,
        "mode: {}; application code executed: no; search: {}",
        r.mode, r.algorithm
    );
    let _ = writeln!(
        o,
        "candidates: {} ({} model counterexamples, {} against hypotheses; {} deduplicated findings)",
        r.summary.candidates,
        r.summary.model_counterexamples,
        r.summary.counterexamples_against_hypotheses,
        r.summary.deduplicated_findings
    );
    for c in &r.candidates {
        let k = &c.candidate;
        let _ = writeln!(o);
        let _ = writeln!(o, "Candidate {}  [{}]", k.id, c.status);
        let _ = writeln!(o, "  {}", k.title);
        if let Some(d) = &c.duplicate_of {
            let _ = writeln!(o, "  same underlying issue as {d}");
        }
        let _ = writeln!(o, "  Entry:");
        for e in &k.entries {
            let _ = writeln!(o, "    {} ({}, {})", e.selector, e.kind, e.invocation);
        }
        let _ = writeln!(o, "  Conflict:");
        for op in &k.conflict {
            let _ = writeln!(o, "    [{}] {}  {}", op.role, op.operation, op.location);
        }
        let _ = writeln!(o, "  Resource: {}", k.resource.text);
        for p in &k.properties {
            let _ = writeln!(o, "  Property: {}", p.description);
            let _ = writeln!(o, "    origin: {}", p.origin.as_str());
            let _ = writeln!(o, "    why: {}", p.rationale);
        }
        for p in &k.protections {
            let _ = writeln!(o, "  Protection: {p}");
        }
        if let Some(b) = &k.blocked {
            let _ = writeln!(o, "  Blocked: {b}");
        }
        if !k.unsupported.is_empty() {
            let _ = writeln!(
                o,
                "  Unsupported{}: {}",
                if k.unsupported_on_path {
                    " (on the path)"
                } else {
                    ""
                },
                k.unsupported.join("; ")
            );
        }
        if verbose {
            for a in &k.assumptions {
                let _ = writeln!(o, "  Assumption ({}, {}): {}", a.kind, a.origin, a.text);
            }
        }
        for s in &c.scenarios {
            let model = s.model.as_ref().map_or_else(
                || "not model-checked".to_owned(),
                |m| {
                    format!(
                        "{} ({} states{})",
                        m.verdict,
                        m.states,
                        m.violated
                            .as_ref()
                            .map(|v| format!(", violates `{v}`"))
                            .unwrap_or_default()
                    )
                },
            );
            let _ = writeln!(
                o,
                "  Scenario {} [{:?}]: synthesis {} — {}",
                s.variant.name, s.variant.kind, s.synthesis.status, model
            );
            if s.synthesis.status != "satisfied" {
                let _ = writeln!(o, "    {}", s.synthesis.detail);
            }
            if let Some(f) = &s.file {
                let _ = writeln!(o, "    file: {f}");
            }
        }
        let _ = writeln!(o, "  Status: {}", c.status);
        let _ = writeln!(o, "  {}", c.finding);
        if !c.next_step.is_empty() {
            let _ = writeln!(o, "  Next step: {}", c.next_step);
        }
    }
    if verbose && !r.rejected.is_empty() {
        let _ = writeln!(o);
        let _ = writeln!(o, "Rejected combinations:");
        for x in &r.rejected {
            let _ = writeln!(
                o,
                "  [{}] {} — {} ({})",
                x.class.as_str(),
                x.entries.join(" × "),
                x.reason,
                x.location
            );
        }
    }
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "Important: a model counterexample against a HEURISTIC_HYPOTHESIS is not a confirmed defect; a bounded run without counterexample is not a proof."
    );
    o
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A self-contained, script-free HTML page.
pub fn render_html(r: &DiscoveryReport) -> String {
    let mut o = String::new();
    o.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    o.push_str("<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'\">");
    let _ = write!(
        o,
        "<title>CommitRift discovery: {}</title>",
        esc(&r.project.name)
    );
    o.push_str("<style>body{font:14px/1.5 system-ui,sans-serif;margin:24px;max-width:1100px;color:#1d2330;background:#fff}h1{font-size:20px}h2{font-size:16px;margin-top:28px}.c{border:1px solid #d5d9e0;border-radius:6px;padding:12px 16px;margin:12px 0}.s{display:inline-block;padding:1px 8px;border-radius:10px;background:#eef1f5;font-size:12px}.ce{background:#fde8e8}.ok{background:#e6f4ea}code{font-size:12px}table{border-collapse:collapse}td,th{padding:2px 8px;border-bottom:1px solid #eee;text-align:left;vertical-align:top}@media (prefers-color-scheme:dark){body{background:#14171c;color:#e4e7ec}.c{border-color:#333a45}.s{background:#262c36}.ce{background:#4a2224}.ok{background:#1f3a28}td,th{border-color:#262c36}}</style></head><body>");
    let _ = write!(
        o,
        "<h1>CommitRift discovery: {}</h1><p>{} candidates, {} model counterexamples ({} against hypotheses), {} deduplicated findings. Mode: {}. Application code executed: <b>no</b>.</p>",
        esc(&r.project.name),
        r.summary.candidates,
        r.summary.model_counterexamples,
        r.summary.counterexamples_against_hypotheses,
        r.summary.deduplicated_findings,
        esc(&r.mode)
    );
    o.push_str("<p><b>A counterexample against a HEURISTIC_HYPOTHESIS is not a confirmed defect. No counterexample within bounds is not a proof.</b></p>");
    for c in &r.candidates {
        let k = &c.candidate;
        let class = match c.status.as_str() {
            "MODEL_COUNTEREXAMPLE" => "s ce",
            "NO_VIOLATION_WITHIN_BOUNDS" => "s ok",
            _ => "s",
        };
        let _ = write!(
            o,
            "<div class=\"c\"><h2>{} <span class=\"{class}\">{}</span></h2><p>{}</p><table>",
            esc(&k.id),
            esc(&c.status),
            esc(&k.title)
        );
        for e in &k.entries {
            let _ = write!(
                o,
                "<tr><th>entry</th><td><code>{}</code> ({}, {})</td></tr>",
                esc(&e.selector),
                esc(&e.kind),
                esc(&e.invocation)
            );
        }
        for op in &k.conflict {
            let _ = write!(
                o,
                "<tr><th>{}</th><td><code>{}</code> {}</td></tr>",
                esc(&op.role),
                esc(&op.operation),
                esc(&op.location)
            );
        }
        let _ = write!(
            o,
            "<tr><th>resource</th><td>{}</td></tr>",
            esc(&k.resource.text)
        );
        for p in &k.properties {
            let _ = write!(
                o,
                "<tr><th>property</th><td>{} <span class=\"s\">{}</span><br>{}</td></tr>",
                esc(&p.description),
                esc(p.origin.as_str()),
                esc(&p.rationale)
            );
        }
        for p in &k.protections {
            let _ = write!(o, "<tr><th>protection</th><td>{}</td></tr>", esc(p));
        }
        for a in &k.assumptions {
            let _ = write!(
                o,
                "<tr><th>assumption</th><td>{} ({}, {})</td></tr>",
                esc(&a.text),
                esc(&a.kind),
                esc(&a.origin)
            );
        }
        if !k.unsupported.is_empty() {
            let _ = write!(
                o,
                "<tr><th>unsupported</th><td>{}</td></tr>",
                esc(&k.unsupported.join("; "))
            );
        }
        for s in &c.scenarios {
            let model = s.model.as_ref().map_or_else(
                || "not model-checked".to_owned(),
                |m| format!("{} ({} states)", m.verdict, m.states),
            );
            let _ = write!(
                o,
                "<tr><th>scenario {}</th><td>{} — synthesis {}: {}<br>{}</td></tr>",
                esc(&s.variant.name),
                esc(&s.variant.description),
                esc(&s.synthesis.status),
                esc(&s.synthesis.detail),
                esc(&model)
            );
            if let Some(m) = &s.model {
                for line in &m.schedule {
                    let _ = write!(o, "<tr><td></td><td><code>{}</code></td></tr>", esc(line));
                }
            }
        }
        let _ = write!(o, "</table><p>{}</p>", esc(&c.finding));
        if !c.next_step.is_empty() {
            let _ = write!(o, "<p><b>Next step:</b> {}</p>", esc(&c.next_step));
        }
        let _ = write!(o, "</div>");
    }
    o.push_str("</body></html>\n");
    o
}

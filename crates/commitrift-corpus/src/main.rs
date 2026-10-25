//! `commitrift-corpus`: evaluate CommitRift on a corpus of projects.
//!
//! ```text
//! commitrift-corpus run --manifest corpora/manifest.json --output reports/phase2/current.json
//! commitrift-corpus compare reports/phase2/before.json reports/phase2/after.json
//! commitrift-corpus precision --report reports/phase2/current.json --labels corpora/labels
//! ```
//!
//! `run` analyzes every project of the manifests (never executing or
//! modifying them) and writes a deterministic JSON report: measured counts
//! and every finding with its stable id (the diagnostic fingerprint). Wall
//! time is included only with `--timings`, because it differs between runs.
//! `compare` lists findings added, removed and changed between two reports.
//! `precision` joins a report with manually written labels; precision is
//! computed only over reviewed findings, and nothing is labeled
//! automatically.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

use commitrift_analyzer::Analyzer;
use commitrift_core::analysis::AnalysisOptions;
use commitrift_core::config::Config;
use commitrift_core::diagnostic::{Policy, Severity};
use commitrift_core::report::AnalysisCounts;

#[derive(Debug, Parser)]
#[command(
    name = "commitrift-corpus",
    about = "Evaluate CommitRift on a corpus of projects"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Analyze every project of the manifests and write a report.
    Run {
        /// Corpus manifest(s) (see corpora/manifest.json).
        #[arg(long, required = true)]
        manifest: Vec<PathBuf>,
        /// Report file to write (stdout when omitted).
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = PolicyArg::Recommended)]
        policy: PolicyArg,
        #[arg(long, default_value_t = AnalysisOptions::default().max_paths)]
        max_paths: usize,
        #[arg(long, default_value_t = AnalysisOptions::default().max_call_depth)]
        max_call_depth: usize,
        /// Include wall-clock timings (makes the report non-deterministic).
        #[arg(long)]
        timings: bool,
    },
    /// Compare two reports by finding id.
    Compare { before: PathBuf, after: PathBuf },
    /// Measure live heap memory at each analysis phase (counting
    /// allocator; dev tool only). Set COMMITRIFT_THREADS to control
    /// parallelism.
    Memory {
        /// Corpus manifest(s).
        #[arg(long, required = true)]
        manifest: Vec<PathBuf>,
        #[arg(long, default_value_t = AnalysisOptions::default().max_call_depth)]
        max_call_depth: usize,
        /// Also build failure traces (as `commitrift trace` does).
        #[arg(long)]
        traces: bool,
    },
    /// Build the failure trace of every finding (as `commitrift trace`
    /// does), validate each against the analysis model, and print coverage
    /// and incompleteness causes per project. Labels of private projects
    /// are not printed.
    Traces {
        /// Corpus manifest(s).
        #[arg(long, required = true)]
        manifest: Vec<PathBuf>,
        /// Also write every unknown call inside a failure window of an
        /// incomplete trace (project, location, label, unknown reason and
        /// detail, automatic category) as TSV to this file. It contains
        /// source labels of every project, private ones included: keep it
        /// local (for example under `.corpora/`).
        #[arg(long)]
        unknowns: Option<PathBuf>,
    },
    /// Write blind review packets for a sample of findings of one project:
    /// the diagnostic, its failure sequence and the source around every
    /// operation, but never a label or an earlier verdict. Also writes an
    /// empty review form. Nothing is executed.
    ReviewPacket {
        /// Corpus manifest(s).
        #[arg(long, required = true)]
        manifest: Vec<PathBuf>,
        #[arg(long)]
        project: String,
        /// Findings to include (fingerprints); repeatable.
        #[arg(long = "finding")]
        findings: Vec<String>,
        /// Or: the first N warnings by fingerprint (deterministic).
        #[arg(long)]
        sample: Option<usize>,
        /// Directory to create.
        #[arg(long)]
        output: PathBuf,
    },
    /// Validate a completed review form and, only then, compare it with a
    /// label file.
    ReviewCheck {
        /// The completed `reviews.json`.
        reviews: PathBuf,
        /// A label file to compare with (optional).
        #[arg(long)]
        labels: Option<PathBuf>,
    },
    /// Precision over manually reviewed findings.
    Precision {
        #[arg(long)]
        report: PathBuf,
        /// Directory of `<project>.json` label files.
        #[arg(long)]
        labels: PathBuf,
    },
}

/// Counts live heap bytes and their peak, only while [`COUNTING`] is set
/// (the `memory` command). Otherwise it only adds one uncontended load per
/// allocation, so `run --timings` is not distorted by contention on the
/// counters.
struct Counting;

static COUNTING: AtomicBool = AtomicBool::new(false);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every method forwards to `System` with the same arguments and only
// adds bookkeeping on atomics.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() && COUNTING.load(Ordering::Relaxed) {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        if COUNTING.load(Ordering::Relaxed) {
            // Saturating: blocks allocated before counting started.
            saturating_release(layout.size());
        }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new = unsafe { System.realloc(pointer, layout, new_size) };
        if !new.is_null() && COUNTING.load(Ordering::Relaxed) {
            saturating_release(layout.size());
            let live = LIVE.fetch_add(new_size, Ordering::Relaxed) + new_size;
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        new
    }
}

/// Subtract from LIVE without wrapping below zero (blocks allocated before
/// counting started). A compare-exchange loop: `fetch_update` is deprecated
/// on newer toolchains, and its replacement is newer than the MSRV.
fn saturating_release(size: usize) {
    let mut current = LIVE.load(Ordering::Relaxed);
    loop {
        let next = current.saturating_sub(size);
        match LIVE.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(actual) => current = actual,
        }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn live_bytes() -> usize {
    LIVE.load(Ordering::Relaxed)
}

fn memory(manifests: &[PathBuf], max_call_depth: usize, traces: bool) -> Result<(), String> {
    COUNTING.store(true, Ordering::SeqCst);
    let _ = commitrift_analyzer::MEMORY_PROBE.set(live_bytes);
    let mib = |bytes: usize| bytes as f64 / 1_048_576.0;
    println!(
        "{:<20} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "project", "lowered", "resolved", "summary", "analyzed", "peak", "nodes"
    );
    let start_dir = std::env::current_dir().map_err(|e| e.to_string())?;
    for manifest_path in manifests {
        let manifest_path = start_dir.join(manifest_path);
        let base = manifest_path.parent().ok_or("no parent")?.to_path_buf();
        let manifest: Manifest = serde_json::from_str(
            &std::fs::read_to_string(&manifest_path).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        for project in manifest.projects {
            let root = base.join(&project.path);
            if !root.is_dir() {
                continue;
            }
            let config = Config::load(&base.join(&project.config)).map_err(|e| e.to_string())?;
            let analyzer = Analyzer::new(config).with_options(AnalysisOptions {
                max_call_depth,
                ..AnalysisOptions::default()
            });
            std::env::set_current_dir(&root).map_err(|e| e.to_string())?;
            PEAK.store(live_bytes(), Ordering::Relaxed);
            let analysis = if traces {
                analyzer.trace_project(&[PathBuf::from(".")])
            } else {
                analyzer.analyze_project(&[PathBuf::from(".")])
            }
            .map_err(|e| e.to_string())?;
            std::env::set_current_dir(&start_dir).map_err(|e| e.to_string())?;
            let heap = analysis.timings.heap.unwrap_or_default();
            let peak = PEAK.load(Ordering::Relaxed);
            fn count(nodes: &[commitrift_core::ir::Node]) -> usize {
                use commitrift_core::ir::Node;
                nodes
                    .iter()
                    .map(|node| {
                        1 + match node {
                            Node::Effect(_) | Node::Exit(_) => 0,
                            Node::Transaction(scope) => count(&scope.body),
                            Node::Branch(branch) => {
                                branch.arms.iter().map(|a| count(&a.body)).sum()
                            }
                            Node::Switch(switch) => {
                                switch.cases.iter().map(|c| count(&c.body)).sum()
                            }
                            Node::Loop(lp) => count(&lp.body),
                            Node::Call(call) => count(&call.body),
                            Node::Try(t) => {
                                count(&t.block)
                                    + t.handler.as_ref().map_or(0, |h| count(&h.body))
                                    + t.finalizer.as_deref().map_or(0, count)
                            }
                        }
                    })
                    .sum()
            }
            let nodes: usize = analysis
                .files
                .iter()
                .flat_map(|f| &f.ir.functions)
                .map(|f| count(&f.body))
                .sum();
            println!(
                "{:<20} {:>8.1}M {:>8.1}M {:>8.1}M {:>8.1}M {:>8.1}M {:>9}",
                project.name,
                mib(heap.lowered.saturating_sub(heap.start)),
                mib(heap.resolved.saturating_sub(heap.start)),
                mib(heap.summarized.saturating_sub(heap.start)),
                mib(heap.analyzed.saturating_sub(heap.start)),
                mib(peak.saturating_sub(heap.start)),
                nodes
            );
            drop(analysis);
        }
    }
    println!(
        "Live heap bytes above the start of each analysis, after each phase, and the peak; nodes = IR nodes retained after summary expansion (size_of::<Node>() = {} bytes).",
        std::mem::size_of::<commitrift_core::ir::Node>()
    );
    Ok(())
}

/// An automatic, purely descriptive category for an unknown call, from the
/// unknown effect's reason and detail and the shape of the call as written.
/// It says why the call is unknown, not what it does.
fn unknown_category(reason: &str, detail: &str, label: &str) -> &'static str {
    if reason != "unrecognized call" {
        return match reason {
            "raw SQL" => "raw SQL",
            "unmodeled database method" => "unmodeled database method",
            "unsupported receiver expression" => "unconfigured receiver path",
            "database handle argument not recognized" => "database handle argument not recognized",
            "transaction body not analyzable" => "opaque transaction",
            _ => "other",
        };
    }
    if detail.contains("imported from the package") {
        return "third-party package (no source)";
    }
    if detail.contains("is declared in") {
        if detail.contains("performs no recognized") {
            return "resolved function without recognized effects";
        }
        if detail.contains("recursive") {
            return "resolved function, recursive";
        }
        if detail.contains("call(s) deep") {
            return "resolved function beyond the depth bound";
        }
        if detail.contains("control flow") {
            return "resolved function with unsupported control flow";
        }
        return "resolved function, other reason";
    }
    if detail.contains("is imported from") {
        return "unresolved import";
    }
    if detail.contains("collection that is not an array literal") {
        return "promise combinator over a non-literal collection";
    }
    let callee = label.split('(').next().unwrap_or(label);
    if !callee.is_empty() && !callee.contains('.') && label.contains(").") {
        return "method chain on a call result";
    }
    if callee.is_empty() {
        "call through an expression"
    } else if callee.starts_with("this.") {
        "method call on `this`"
    } else if callee.contains('.') {
        "method call on a value"
    } else if callee
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
    {
        "plain call of an unresolved binding"
    } else {
        "call through an expression"
    }
}

fn traces(manifests: &[PathBuf], unknowns_out: Option<&Path>) -> Result<bool, String> {
    use commitrift_core::ir::EffectKind;
    use commitrift_core::trace::{Completeness, StepKind};
    let start_dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let mut all_valid = true;
    let mut total = 0;
    let mut rows: Vec<String> = Vec::new();
    for manifest_path in manifests {
        let manifest_path = start_dir.join(manifest_path);
        let base = manifest_path.parent().ok_or("no parent")?.to_path_buf();
        let manifest: Manifest = serde_json::from_str(
            &std::fs::read_to_string(&manifest_path).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        for project in manifest.projects {
            let root = base.join(&project.path);
            if !root.is_dir() {
                println!("{:<20} not checked out", project.name);
                continue;
            }
            let config = Config::load(&base.join(&project.config)).map_err(|e| e.to_string())?;
            let analysis = Analyzer::new(config)
                .trace_project_at(&root, &[PathBuf::from(".")])
                .map_err(|e| e.to_string())?;
            // Unknown effects by (file, line, column, label): reason, detail.
            let mut details: BTreeMap<(String, u32, u32, String), (String, String)> =
                BTreeMap::new();
            if unknowns_out.is_some() {
                for file in &analysis.files {
                    for function in &file.ir.functions {
                        for effect in function.effects() {
                            if let EffectKind::Unknown { reason, detail } = &effect.kind {
                                let at = effect.location.resolve(&file.ir.path, &file.ir.sources);
                                details.insert(
                                    (at.file, at.line, at.column, effect.label.clone()),
                                    (reason.to_string(), detail.clone()),
                                );
                            }
                        }
                    }
                }
            }
            let report = analysis.into_trace_report(None);
            let invalid: Vec<String> = report
                .traces
                .iter()
                .filter(|t| !t.validate().is_empty() || t.compute_id() != t.id)
                .map(|t| t.id.clone())
                .collect();
            all_valid &= invalid.is_empty();
            total += report.traces.len();
            let s = report.summary;
            println!(
                "{:<20} findings {} traces {} (complete {}, approximated {}, incomplete {}), without trace {}; model validation failures {}",
                project.name,
                s.findings,
                s.traces,
                s.complete,
                s.approximated,
                s.incomplete,
                s.skipped,
                invalid.len()
            );
            let mut causes: BTreeMap<&str, usize> = BTreeMap::new();
            let mut labels: BTreeMap<String, usize> = BTreeMap::new();
            let mut categories: BTreeMap<&'static str, usize> = BTreeMap::new();
            for trace in report
                .traces
                .iter()
                .filter(|t| t.completeness == Completeness::Incomplete)
            {
                if unknowns_out.is_some() {
                    for step in trace.steps.iter().filter(|step| {
                        step.kind == StepKind::Unknown
                            && trace.injection.window.contains(&step.index)
                    }) {
                        let Some(at) = &step.location else { continue };
                        let (reason, detail) = details
                            .get(&(at.file.clone(), at.line, at.column, step.label.clone()))
                            .cloned()
                            .unwrap_or_else(|| ("?".to_owned(), "?".to_owned()));
                        let category = unknown_category(&reason, &detail, &step.label);
                        *categories.entry(category).or_default() += 1;
                        rows.push(format!(
                            "{}\t{}\t{}\t{}:{}:{}\t{}\t{}\t{}\t{}",
                            project.name,
                            trace.finding_id,
                            trace.code,
                            at.file,
                            at.line,
                            at.column,
                            step.label.replace('\t', " "),
                            category,
                            reason,
                            detail.replace(['\t', '\n'], " ")
                        ));
                    }
                }
                let unknown: Vec<&str> = trace
                    .steps
                    .iter()
                    .filter(|step| {
                        step.kind == StepKind::Unknown
                            && trace.injection.window.contains(&step.index)
                    })
                    .map(|step| step.label.as_str())
                    .collect();
                if !unknown.is_empty() {
                    *causes
                        .entry("unknown call in the failure window")
                        .or_default() += 1;
                }
                if trace.steps.iter().any(|s| s.kind == StepKind::Unsupported) {
                    *causes
                        .entry("unsupported control flow on the path")
                        .or_default() += 1;
                }
                for label in unknown {
                    let short = label.split('(').next().unwrap_or(label).to_owned();
                    *labels.entry(short).or_default() += 1;
                }
            }
            if !causes.is_empty() {
                println!("{:<20}   incomplete by cause: {causes:?}", "");
                if project.origin == "external" {
                    let mut top: Vec<(String, usize)> = labels.into_iter().collect();
                    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                    top.truncate(8);
                    println!(
                        "{:<20}   most frequent unknown calls in windows: {top:?}",
                        ""
                    );
                } else {
                    println!(
                        "{:<20}   {} distinct unknown calls in windows (private project: labels not printed)",
                        "",
                        labels.len()
                    );
                }
                if !categories.is_empty() {
                    println!(
                        "{:<20}   unknown calls in windows by category: {categories:?}",
                        ""
                    );
                }
            }
        }
    }
    if let Some(path) = unknowns_out {
        let mut text =
            String::from("project\tfinding\tcode\tlocation\tlabel\tcategory\treason\tdetail\n");
        for row in &rows {
            text.push_str(row);
            text.push('\n');
        }
        std::fs::write(path, text).map_err(|e| e.to_string())?;
    }
    println!("total traces: {total}; all valid against the analysis model: {all_valid}");
    Ok(all_valid)
}

/// The classifications a reviewer may give (docs/phase4/REAL-WORLD-EVALUATION.md).
const REVIEW_CLASSES: &[&str] = &[
    "confirmed-bug",
    "intended-risk",
    "false-positive",
    "unclear",
    "not-reviewed",
];

fn excerpt(root: &Path, file: &str, line: usize, radius: usize) -> String {
    let Ok(text) = std::fs::read_to_string(root.join(file)) else {
        return format!("(cannot read {file})\n");
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = line.saturating_sub(radius + 1);
    let end = (line + radius).min(lines.len());
    let mut out = String::new();
    for (index, content) in lines.iter().enumerate().take(end).skip(start) {
        let number = index + 1;
        let marker = if number == line { ">" } else { " " };
        out.push_str(&format!("{marker}{number:>5} | {content}\n"));
    }
    out
}

fn review_packet(
    manifests: &[PathBuf],
    project_name: &str,
    wanted: &[String],
    sample: Option<usize>,
    output: &Path,
) -> Result<(), String> {
    if wanted.is_empty() == sample.is_none() {
        return Err("pass either --finding (repeatable) or --sample N".to_owned());
    }
    if output.exists() {
        return Err(format!(
            "`{}` exists; choose a new directory",
            output.display()
        ));
    }
    let start_dir = std::env::current_dir().map_err(|e| e.to_string())?;
    for manifest_path in manifests {
        let manifest_path = start_dir.join(manifest_path);
        let base = manifest_path.parent().ok_or("no parent")?.to_path_buf();
        let manifest: Manifest = serde_json::from_str(
            &std::fs::read_to_string(&manifest_path).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let Some(project) = manifest
            .projects
            .into_iter()
            .find(|p| p.name == project_name)
        else {
            continue;
        };
        let root = base.join(&project.path);
        let config = Config::load(&base.join(&project.config)).map_err(|e| e.to_string())?;
        let report = Analyzer::new(config)
            .trace_project_at(&root, &[PathBuf::from(".")])
            .map_err(|e| e.to_string())?;
        let traces = report.clone().into_trace_report(None);
        let mut diagnostics = report.into_report(None).diagnostics;
        diagnostics.sort_by(|a, b| a.fingerprint.cmp(&b.fingerprint));
        let selected: Vec<_> = match sample {
            Some(n) => diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Warning)
                .take(n)
                .collect(),
            None => {
                let found: Vec<_> = diagnostics
                    .iter()
                    .filter(|d| wanted.contains(&d.fingerprint))
                    .collect();
                if found.len() != wanted.len() {
                    return Err("some --finding ids are not findings of this project".to_owned());
                }
                found
            }
        };
        std::fs::create_dir_all(output.join("packets")).map_err(|e| e.to_string())?;
        let mut form = Vec::new();
        for (index, diagnostic) in selected.iter().enumerate() {
            let mut text = format!(
                "# Review packet {} — finding `{}`\n\nProject `{}` at revision `{}`. Classify this finding from the source; you are not shown any earlier verdict.\n\n**{} {}** in `{}` at `{}:{}`\n\n{}\n\n## Failure sequence (static analysis)\n\n",
                index + 1,
                diagnostic.fingerprint,
                project.name,
                project.revision.as_deref().unwrap_or("unknown"),
                diagnostic.code.as_str(),
                diagnostic.title,
                diagnostic.function,
                diagnostic.location.file,
                diagnostic.location.line,
                diagnostic.message
            );
            for step in &diagnostic.failure_sequence {
                text.push_str(&format!("- {step}\n"));
            }
            if let Some(trace) = traces
                .traces
                .iter()
                .find(|t| t.finding_id == diagnostic.fingerprint)
            {
                text.push_str(&format!(
                    "\n## Failure trace\n\n```text\n{}```\n",
                    trace.to_text()
                ));
            }
            let mut spots: Vec<(String, u32)> =
                vec![(diagnostic.location.file.clone(), diagnostic.location.line)];
            for effect in &diagnostic.effects {
                let location = (effect.location.file.clone(), effect.location.line);
                if !spots.contains(&location) {
                    spots.push(location);
                }
            }
            text.push_str("\n## Source\n");
            for (file, line) in spots {
                text.push_str(&format!(
                    "\n`{file}:{line}`\n\n```ts\n{}```\n",
                    excerpt(&root, &file, line as usize, 12)
                ));
            }
            text.push_str("\n## Questions\n\n1. Can the failure window occur in production (is the path executable, are the operations related)?\n2. If it occurs, what state is left, and does anything repair it (retries, redelivery, reconciliation, a user-visible repair action)? Cite where.\n3. Classification: confirmed-bug, intended-risk, false-positive or unclear.\n");
            let name = format!("{:02}-{}.md", index + 1, diagnostic.fingerprint);
            std::fs::write(output.join("packets").join(&name), text).map_err(|e| e.to_string())?;
            form.push(serde_json::json!({
                "findingId": diagnostic.fingerprint,
                "packet": format!("packets/{name}"),
                "classification": "not-reviewed",
                "reviewer": "",
                "role": "",
                "date": "",
                "notes": "",
                "evidence": []
            }));
        }
        let form = serde_json::json!({
            "kind": "commitrift.review",
            "schemaVersion": 1,
            "project": project.name,
            "revision": project.revision,
            "instructions": "Fill one entry per finding from its packet only. classification: confirmed-bug | intended-risk | false-positive | unclear | not-reviewed. reviewer: your name or handle; role: e.g. maintainer, external reviewer; date: YYYY-MM-DD; evidence: file:line references. Do not open label files or earlier reports before finishing.",
            "reviews": form
        });
        std::fs::write(
            output.join("reviews.json"),
            serde_json::to_string_pretty(&form).map_err(|e| e.to_string())? + "\n",
        )
        .map_err(|e| e.to_string())?;
        println!(
            "wrote {} review packet(s) and an empty form to {}",
            selected.len(),
            output.display()
        );
        return Ok(());
    }
    Err(format!("no project `{project_name}` in the manifests"))
}

fn review_check(path: &Path, labels: Option<&Path>) -> Result<(), String> {
    let form: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?,
    )
    .map_err(|e| e.to_string())?;
    if form["kind"] != "commitrift.review" {
        return Err("not a review form (`kind` must be `commitrift.review`)".to_owned());
    }
    let reviews = form["reviews"].as_array().ok_or("no `reviews`")?;
    let mut problems = Vec::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (index, review) in reviews.iter().enumerate() {
        let class = review["classification"].as_str().unwrap_or("");
        if !REVIEW_CLASSES.contains(&class) {
            problems.push(format!(
                "reviews[{index}]: unknown classification `{class}`"
            ));
        }
        if class != "not-reviewed" {
            for field in ["reviewer", "role", "date", "notes"] {
                if review[field].as_str().is_none_or(str::is_empty) {
                    problems.push(format!(
                        "reviews[{index}]: `{field}` is required for a review"
                    ));
                }
            }
            let date = review["date"].as_str().unwrap_or("");
            let shape = date.len() == 10
                && date.chars().enumerate().all(|(i, c)| {
                    if i == 4 || i == 7 {
                        c == '-'
                    } else {
                        c.is_ascii_digit()
                    }
                });
            if !date.is_empty() && !shape {
                problems.push(format!("reviews[{index}]: `date` must be YYYY-MM-DD"));
            }
        }
        *counts.entry(class.to_owned()).or_default() += 1;
    }
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    println!("{} entries: {counts:?}", reviews.len());
    if let Some(labels) = labels {
        let labels: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(labels).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let mut agree = 0;
        let mut compared = 0;
        for review in reviews
            .iter()
            .filter(|r| r["classification"] != "not-reviewed")
        {
            let Some(label) = labels
                .iter()
                .find(|l| l["findingId"] == review["findingId"] && l["reviewed"] == true)
            else {
                continue;
            };
            compared += 1;
            let mapped = match review["classification"].as_str().unwrap_or("") {
                "confirmed-bug" => "true-positive",
                other => other,
            };
            if label["classification"].as_str() == Some(mapped) {
                agree += 1;
            }
        }
        println!("agreement with the label file: {agree} of {compared} compared finding(s)");
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum PolicyArg {
    Recommended,
    Strict,
    Audit,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    projects: Vec<ManifestProject>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestProject {
    name: String,
    category: String,
    origin: String,
    #[serde(default)]
    repository: Option<String>,
    #[serde(default)]
    revision: Option<String>,
    path: String,
    config: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CorpusReport {
    schema_version: u32,
    tool: String,
    policy: String,
    max_paths: usize,
    max_call_depth: usize,
    projects: Vec<ProjectReport>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectReport {
    name: String,
    category: String,
    origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repository: Option<String>,
    /// Revision named by the manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revision: Option<String>,
    /// `git rev-parse HEAD` of the checkout, when it is a git repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checked_out: Option<String>,
    /// Number of locally modified or untracked files in the checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local_changes: Option<usize>,
    config: String,
    stats: Stats,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timings: Option<Timings>,
    findings: Vec<Finding>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stats {
    files_discovered: usize,
    files_failed: usize,
    functions: usize,
    #[serde(flatten)]
    analysis: Counts,
    diagnostics_by_code: BTreeMap<String, usize>,
    warnings: usize,
    infos: usize,
    /// Phase 5 counters; absent in earlier reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    values: Option<ValueCounters>,
    /// Findings by value-relation status (Phase 5).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    value_relations: BTreeMap<String, usize>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Counts {
    functions_with_effects: usize,
    truncated_functions: usize,
    pruned_paths: usize,
    merged_paths: usize,
    same_file_calls: usize,
    cross_file_calls_resolved: usize,
    cross_file_calls_unresolved: usize,
    package_calls: usize,
    calls_summarized: usize,
}

impl From<AnalysisCounts> for Counts {
    fn from(c: AnalysisCounts) -> Self {
        Self {
            functions_with_effects: c.functions_with_effects,
            truncated_functions: c.truncated_functions,
            pruned_paths: c.pruned_paths,
            merged_paths: c.merged_paths,
            same_file_calls: c.same_file_calls,
            cross_file_calls_resolved: c.cross_file_calls_resolved,
            cross_file_calls_unresolved: c.cross_file_calls_unresolved,
            package_calls: c.package_calls,
            calls_summarized: c.calls_summarized,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Timings {
    total_ms: f64,
    lower_ms: f64,
    resolve_ms: f64,
    summaries_ms: f64,
    analysis_ms: f64,
    summary_expansions: usize,
    summary_reuses: usize,
    /// Of `resolve_ms`: the effect-summary fixpoint (Phase 5).
    #[serde(default)]
    solve_ms: f64,
}

/// Phase 5 analysis counters (deterministic).
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ValueCounters {
    /// Abstract states created at control-flow forks (all functions).
    abstract_states: usize,
    joins: usize,
    widenings: usize,
    loop_iterations: usize,
    loops_widened: usize,
    loops_havocked: usize,
    dry_runs: usize,
    /// Effect summaries: functions, inert functions, SCCs, recursive SCCs,
    /// largest SCC, fixpoint rounds, exhausted budgets.
    summary_functions: usize,
    inert_functions: usize,
    sccs: usize,
    recursive_sccs: usize,
    largest_scc: usize,
    fixpoint_rounds: usize,
    fixpoint_budget_exhausted: usize,
    /// Inlining: expansions computed, memo hits, callback specializations,
    /// calls resolved as inert, concurrent calls summarized, callbacks
    /// linked, argument placeholders kept and dropped, calls refused by
    /// the specialization limit.
    expansions: usize,
    memo_hits: usize,
    specializations: usize,
    specialization_limit: usize,
    inert_calls: usize,
    concurrent_calls: usize,
    callbacks_linked: usize,
    argument_effects_kept: usize,
    argument_effects_dropped: usize,
    /// Call resolution: classes, methods resolved / unresolved / assumed,
    /// immediately invoked functions, callback sites.
    classes: usize,
    methods_resolved: usize,
    methods_unresolved: usize,
    methods_assumed: usize,
    immediate_calls: usize,
    callback_sites: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Finding {
    id: String,
    code: String,
    severity: String,
    confidence: String,
    path_certainty: String,
    evidence_strength: String,
    file: String,
    line: u32,
    function: String,
    message: String,
    /// Files other than `file` that hold related operations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    other_files: Vec<String>,
    /// Value-relation status of the two operations (Phase 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value_relation: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Label {
    finding_id: String,
    classification: Classification,
    reviewed: bool,
    #[serde(default)]
    #[allow(dead_code)]
    notes: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Classification {
    TruePositive,
    FalsePositive,
    Unclear,
    IntendedRisk,
    NotReviewed,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Err(message) = commitrift_core::analysis::path_mode_from_env() {
        eprintln!("error: {message}");
        return ExitCode::from(2);
    }
    let differential = std::env::var_os("COMMITRIFT_PATHS_DIFFERENTIAL").is_some();
    commitrift_core::analysis::enable_path_differential(differential);
    let result = match cli.command {
        Commands::Traces { manifest, unknowns } => match traces(&manifest, unknowns.as_deref()) {
            Ok(true) => return ExitCode::SUCCESS,
            Ok(false) => return ExitCode::from(1),
            Err(error) => Err(error),
        },
        Commands::ReviewPacket {
            manifest,
            project,
            findings,
            sample,
            output,
        } => review_packet(&manifest, &project, &findings, sample, &output),
        Commands::ReviewCheck { reviews, labels } => review_check(&reviews, labels.as_deref()),
        Commands::Run {
            manifest,
            output,
            policy,
            max_paths,
            max_call_depth,
            timings,
        } => run(
            &manifest,
            output.as_deref(),
            policy,
            max_paths,
            max_call_depth,
            timings,
        ),
        Commands::Compare { before, after } => compare(&before, &after),
        Commands::Precision { report, labels } => precision(&report, &labels),
        Commands::Memory {
            manifest,
            max_call_depth,
            traces,
        } => memory(&manifest, max_call_depth, traces),
    };
    if differential {
        let (functions, stats, differences) = commitrift_core::analysis::take_path_differential();
        eprintln!(
            "path differential: {functions} function analyses compared (legacy vs compact), {} difference(s); compact summaries built {}, incoming traces spliced {}, calls with shared facts {}",
            differences.len(),
            stats.built,
            stats.reused,
            stats.shared_facts
        );
        for d in differences.iter().take(20) {
            eprintln!("  {d}");
        }
        if !differences.is_empty() {
            return ExitCode::from(1);
        }
    }
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

fn run(
    manifests: &[PathBuf],
    output: Option<&Path>,
    policy: PolicyArg,
    max_paths: usize,
    max_call_depth: usize,
    timings: bool,
) -> Result<(), String> {
    let policy = match policy {
        PolicyArg::Recommended => Policy::Recommended,
        PolicyArg::Strict => Policy::Strict,
        PolicyArg::Audit => Policy::Audit,
    };
    let start_dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let mut projects = Vec::new();
    for manifest_path in manifests {
        let manifest_path = start_dir.join(manifest_path);
        let base = manifest_path
            .parent()
            .ok_or("manifest has no parent directory")?
            .to_path_buf();
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
        let manifest: Manifest =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", manifest_path.display()))?;
        for project in manifest.projects {
            let root = base.join(&project.path);
            if !root.is_dir() {
                eprintln!("skip {}: {} not found", project.name, root.display());
                continue;
            }
            let config_path = base.join(&project.config);
            let config = Config::load(&config_path).map_err(|e| e.to_string())?;
            let analyzer = Analyzer::new(config).with_options(AnalysisOptions {
                max_paths,
                max_call_depth,
                policy,
            });
            // Paths in the report are relative to the project root.
            std::env::set_current_dir(&root).map_err(|e| e.to_string())?;
            let started = Instant::now();
            let analysis = analyzer
                .analyze_project(&[PathBuf::from(".")])
                .map_err(|e| e.to_string())?;
            let elapsed = started.elapsed();
            let (checked_out, local_changes) = git_state(&root);
            std::env::set_current_dir(&start_dir).map_err(|e| e.to_string())?;
            eprintln!(
                "{}: {} files, {} functions, {:.3}s",
                project.name,
                analysis.files_discovered,
                analysis.functions(),
                elapsed.as_secs_f64()
            );
            let phase = analysis.timings;
            let timing = timings.then_some(Timings {
                total_ms: elapsed.as_secs_f64() * 1000.0,
                lower_ms: phase.lower.as_secs_f64() * 1000.0,
                resolve_ms: phase.resolve.as_secs_f64() * 1000.0,
                summaries_ms: phase.summaries.as_secs_f64() * 1000.0,
                analysis_ms: phase.analysis.as_secs_f64() * 1000.0,
                summary_expansions: phase.inline.expansions,
                summary_reuses: phase.inline.reused,
                solve_ms: phase.solve.as_secs_f64() * 1000.0,
            });
            let values = ValueCounters {
                abstract_states: phase.values.states,
                joins: phase.values.joins,
                widenings: phase.values.widenings,
                loop_iterations: phase.values.loop_iterations,
                loops_widened: phase.values.loops_widened,
                loops_havocked: phase.values.loops_havocked,
                dry_runs: phase.values.dry_runs,
                summary_functions: phase.solve_stats.functions,
                inert_functions: phase.solve_stats.inert,
                sccs: phase.solve_stats.sccs,
                recursive_sccs: phase.solve_stats.recursive_sccs,
                largest_scc: phase.solve_stats.largest_scc,
                fixpoint_rounds: phase.solve_stats.iterations,
                fixpoint_budget_exhausted: phase.solve_stats.budget_exhausted,
                expansions: phase.inline.expansions,
                memo_hits: phase.inline.reused,
                specializations: phase.inline.specializations,
                specialization_limit: phase.inline.specialization_limit,
                inert_calls: phase.inline.inert,
                concurrent_calls: phase.inline.concurrent,
                callbacks_linked: phase.inline.callbacks,
                argument_effects_kept: phase.inline.argument_effects_kept,
                argument_effects_dropped: phase.inline.argument_effects_dropped,
                classes: phase.resolution.classes,
                methods_resolved: phase.resolution.methods_resolved,
                methods_unresolved: phase.resolution.methods_unresolved,
                methods_assumed: phase.resolution.methods_assumed,
                immediate_calls: phase.resolution.immediate_calls,
                callback_sites: phase.resolution.callback_sites,
            };
            let functions = analysis.functions();
            let failed = analysis.failures.len();
            let discovered = analysis.files_discovered;
            let counts = analysis.counts;
            let report = analysis.into_report(None);
            let mut by_code: BTreeMap<String, usize> = BTreeMap::new();
            let mut relations: BTreeMap<String, usize> = BTreeMap::new();
            let findings: Vec<Finding> = report
                .diagnostics
                .iter()
                .map(|d| {
                    *by_code.entry(d.code.as_str().to_owned()).or_default() += 1;
                    let value_relation = d.value_relation.as_ref().and_then(|r| {
                        serde_json::to_value(r.status)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                    });
                    if let Some(status) = &value_relation {
                        *relations.entry(status.clone()).or_default() += 1;
                    }
                    let other_files: BTreeSet<String> = d
                        .effects
                        .iter()
                        .map(|e| e.location.file.clone())
                        .filter(|file| *file != d.location.file)
                        .collect();
                    Finding {
                        id: d.fingerprint.clone(),
                        code: d.code.as_str().to_owned(),
                        severity: d.severity.as_str().to_owned(),
                        confidence: d.confidence.to_string(),
                        path_certainty: serde_json::to_value(d.path_certainty)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_default(),
                        evidence_strength: d.evidence_strength.as_str().to_owned(),
                        file: d.location.file.clone(),
                        line: d.location.line,
                        function: d.function.clone(),
                        message: d.message.clone(),
                        other_files: other_files.into_iter().collect(),
                        value_relation,
                    }
                })
                .collect();
            projects.push(ProjectReport {
                name: project.name,
                category: project.category,
                origin: project.origin,
                repository: project.repository,
                revision: project.revision,
                checked_out,
                local_changes,
                config: project.config,
                stats: Stats {
                    files_discovered: discovered,
                    files_failed: failed,
                    functions,
                    analysis: counts.into(),
                    diagnostics_by_code: by_code,
                    warnings: report.count_at_or_above(Severity::Warning),
                    infos: report.summary.diagnostics.info,
                    values: Some(values),
                    value_relations: relations,
                },
                timings: timing,
                findings,
            });
        }
    }
    let report = CorpusReport {
        schema_version: 1,
        tool: format!("commitrift {}", env!("CARGO_PKG_VERSION")),
        policy: policy.as_str().to_owned(),
        max_paths,
        max_call_depth,
        projects,
    };
    let mut text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    text.push('\n');
    match output {
        Some(path) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
        }
        None => {
            print!("{text}");
            Ok(())
        }
    }
}

/// The checked-out revision and the number of locally changed files.
fn git_state(root: &Path) -> (Option<String>, Option<usize>) {
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
    let changes = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().count());
    (head, changes)
}

fn load(path: &Path) -> Result<CorpusReport, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn compare(before: &Path, after: &Path) -> Result<(), String> {
    let before = load(before)?;
    let after = load(after)?;
    let index = |report: &CorpusReport| -> BTreeMap<(String, String), Finding> {
        report
            .projects
            .iter()
            .flat_map(|p| {
                p.findings
                    .iter()
                    .map(move |f| ((p.name.clone(), f.id.clone()), f.clone()))
            })
            .collect()
    };
    let old = index(&before);
    let new = index(&after);
    let mut added = 0;
    let mut removed = 0;
    let mut changed = 0;
    for (key, finding) in &new {
        match old.get(key) {
            None => {
                added += 1;
                println!(
                    "+ {} {} {} {}:{} {} — {}",
                    key.0,
                    finding.code,
                    finding.severity,
                    finding.file,
                    finding.line,
                    finding.function,
                    finding.message
                );
            }
            Some(previous)
                if previous.severity != finding.severity
                    || previous.confidence != finding.confidence =>
            {
                changed += 1;
                println!(
                    "~ {} {} {}:{} severity {} -> {}, confidence {} -> {}",
                    key.0,
                    finding.code,
                    finding.file,
                    finding.line,
                    previous.severity,
                    finding.severity,
                    previous.confidence,
                    finding.confidence
                );
            }
            Some(_) => {}
        }
    }
    for (key, finding) in &old {
        if !new.contains_key(key) {
            removed += 1;
            println!(
                "- {} {} {} {}:{} {} — {}",
                key.0,
                finding.code,
                finding.severity,
                finding.file,
                finding.line,
                finding.function,
                finding.message
            );
        }
    }
    println!("{added} added, {removed} removed, {changed} changed");
    Ok(())
}

fn precision(report: &Path, labels: &Path) -> Result<(), String> {
    let report = load(report)?;
    println!(
        "{:<20} {:<7} {:>6} {:>8} {:>4} {:>4} {:>4} {:>7}  {:>22}  {:>22}",
        "project",
        "code",
        "found",
        "reviewed",
        "tp",
        "ir",
        "fp",
        "unclear",
        "hazard prec. (95% CI)",
        "bug prec. (95% CI)"
    );
    let mut totals: BTreeMap<String, [usize; 6]> = BTreeMap::new();
    for project in &report.projects {
        let path = labels.join(format!("{}.json", project.name));
        let labels: Vec<Label> = match std::fs::read_to_string(&path) {
            Ok(text) => {
                serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?
            }
            Err(_) => Vec::new(),
        };
        let by_id: BTreeMap<&str, &Label> =
            labels.iter().map(|l| (l.finding_id.as_str(), l)).collect();
        let stale = labels
            .iter()
            .filter(|l| !project.findings.iter().any(|f| f.id == l.finding_id))
            .count();
        if stale > 0 {
            eprintln!(
                "{}: {stale} label(s) refer to findings not in this report",
                project.name
            );
        }
        let mut per_code: BTreeMap<String, [usize; 6]> = BTreeMap::new();
        for finding in &project.findings {
            let counts = per_code.entry(finding.code.clone()).or_default();
            counts[0] += 1;
            if let Some(label) = by_id.get(finding.id.as_str())
                && label.reviewed
            {
                match label.classification {
                    Classification::TruePositive => counts[2] += 1,
                    Classification::IntendedRisk => counts[3] += 1,
                    Classification::FalsePositive => counts[4] += 1,
                    Classification::Unclear => counts[5] += 1,
                    Classification::NotReviewed => continue,
                }
                counts[1] += 1;
            }
        }
        for (code, counts) in &per_code {
            print_row(&project.name, code, counts);
            let total = totals.entry(code.clone()).or_default();
            for (t, c) in total.iter_mut().zip(counts) {
                *t += c;
            }
        }
    }
    for (code, counts) in &totals {
        print_row("ALL", code, counts);
    }
    println!(
        "hazard precision = (tp + ir) / (tp + ir + fp): the reported partial-completion window exists. bug precision = tp / (tp + ir + fp): it is also not an accepted design tradeoff. Over reviewed findings with a decided label (unclear excluded); ir = intended risk. 95% Wilson score intervals."
    );
    Ok(())
}

fn print_row(project: &str, code: &str, counts: &[usize; 6]) {
    let [found, reviewed, tp, ir, fp, unclear] = *counts;
    let decided = tp + ir + fp;
    let ratio = |successes: usize| {
        if decided == 0 {
            "n/a".to_owned()
        } else {
            let (low, high) = wilson(successes, decided);
            format!(
                "{:.2} ({:.2}–{:.2})",
                successes as f64 / decided as f64,
                low,
                high
            )
        }
    };
    let hazard = ratio(tp + ir);
    let bug = ratio(tp);
    println!(
        "{project:<20} {code:<7} {found:>6} {reviewed:>8} {tp:>4} {ir:>4} {fp:>4} {unclear:>7}  {hazard:>22}  {bug:>22}"
    );
}

/// 95% Wilson score interval for `successes` out of `trials`.
fn wilson(successes: usize, trials: usize) -> (f64, f64) {
    let n = trials as f64;
    let p = successes as f64 / n;
    let z = 1.96_f64;
    let denominator = 1.0 + z * z / n;
    let center = (p + z * z / (2.0 * n)) / denominator;
    let margin = z * ((p * (1.0 - p) / n) + z * z / (4.0 * n * n)).sqrt() / denominator;
    ((center - margin).max(0.0), (center + margin).min(1.0))
}

#[cfg(test)]
mod tests {
    use super::wilson;

    #[test]
    fn wilson_interval_is_bounded_and_contains_the_estimate() {
        let (low, high) = wilson(8, 10);
        assert!(low > 0.4 && low < 0.8, "{low}");
        assert!(high > 0.8 && high <= 1.0, "{high}");
        let (low, high) = wilson(0, 5);
        assert_eq!(low, 0.0);
        assert!(high > 0.0);
    }
}

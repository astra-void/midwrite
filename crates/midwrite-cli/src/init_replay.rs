//! `midwrite init-replay`: an editable replay harness skeleton generated
//! from one failure trace.
//!
//! The generator never executes anything, never installs packages and
//! writes only inside the requested output directory (which must lie inside
//! the analyzed project). What it cannot know — how to start the scenario,
//! how to observe the state, what the application's contract is — is left
//! as `MIDWRITE-TODO` placeholders that `midwrite replay` refuses to
//! run. See `docs/phase4/HARNESS-GENERATION.md`.

use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use serde_json::{Value, json};

use midwrite_analyzer::{Analyzer, display_path};
use midwrite_core::config::{Config, discover_config};
use midwrite_core::trace::{
    Completeness, FailureTrace, StepKind, TRACE_REPORT_KIND, TraceReport, TraceStep,
};
use midwrite_replay::manifest::PLACEHOLDER_MARKER as TODO;

use crate::{EXIT_OK, InitReplayArgs, RuntimeArg};

/// The replay client, vendored into every generated harness so that it is
/// self-contained and matches this binary's protocol version.
const RUNTIME_FILES: &[(&str, &str)] = &[
    ("index.js", include_str!("../../../runtime/node/index.js")),
    (
        "index.d.ts",
        include_str!("../../../runtime/node/index.d.ts"),
    ),
    ("pg.js", include_str!("../../../runtime/node/pg.js")),
    ("pg.d.ts", include_str!("../../../runtime/node/pg.d.ts")),
    (
        "event-log.js",
        include_str!("../../../runtime/node/event-log.js"),
    ),
    (
        "event-log.d.ts",
        include_str!("../../../runtime/node/event-log.d.ts"),
    ),
    (
        "package.json",
        include_str!("../../../runtime/node/package.json"),
    ),
];

/// `text` if it is already an identifier, else a lower camel case
/// identifier derived from it.
fn ident(text: &str, fallback: &str) -> String {
    if midwrite_core::config::is_identifier(text) {
        return text.to_owned();
    }
    let mut out = String::new();
    for (index, word) in text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .enumerate()
    {
        if index == 0 {
            out.push_str(&word.to_ascii_lowercase());
        } else {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                out.push(first.to_ascii_uppercase());
                out.push_str(chars.as_str());
            }
        }
    }
    if out.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        out
    } else {
        fallback.to_owned()
    }
}

/// `text` made safe for a single-line comment or string in generated code:
/// control characters (newlines included) become spaces, so a label from a
/// trace file can never end the comment and inject code or SQL.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn slug(text: &str) -> String {
    let mut out: String = text
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches('-');
    let out: String = out.chars().take(60).collect();
    if out.is_empty() {
        "harness".to_owned()
    } else {
        out
    }
}

fn location(step: &TraceStep) -> String {
    step.location
        .as_ref()
        .map(|l| format!("{}:{}:{}", l.file, l.line, l.column))
        .unwrap_or_else(|| "unknown location".to_owned())
}

/// A checkpoint the first-party adapters emit: name and label.
type Checkpoint = (&'static str, Option<String>);

/// The checkpoints the runtime adapters emit right before and right after
/// the operation of `step` (see runtime/node/pg.js and event-log.js).
fn checkpoints_of(step: &TraceStep) -> (Option<Checkpoint>, Option<Checkpoint>) {
    match step.kind {
        StepKind::DbWrite => (
            Some(("before-db-write", step.table.clone())),
            Some((
                if step.transaction.is_some() {
                    "after-tx-write"
                } else {
                    "after-db-commit"
                },
                step.table.clone(),
            )),
        ),
        StepKind::TxCommit => (
            Some(("before-tx-commit", Some("transaction".to_owned()))),
            Some(("after-db-commit", Some("transaction".to_owned()))),
        ),
        StepKind::External => (
            Some(("before-event-publish", None)),
            Some(("after-event-publish", None)),
        ),
        _ => (None, None),
    }
}

struct Observation {
    name: String,
    spec: ObservationSpec,
}

// Ordered mirrors of the manifest, so that the generated file reads top
// down (serde_json maps are sorted).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestOut {
    schema_version: u32,
    harness: String,
    description: String,
    runtime: &'static str,
    command: Vec<String>,
    timeout_seconds: u32,
    database: DatabaseOut,
    analysis: AnalysisOut,
    observations: Vec<ObservationSpec>,
    invariants: Vec<InvariantOut>,
    control: ScenarioOut,
    cases: Vec<CaseOut>,
}

#[derive(Serialize)]
struct DatabaseOut {
    provider: &'static str,
    setup: Vec<&'static str>,
}

#[derive(Serialize)]
struct AnalysisOut {
    root: String,
    paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    config: Option<String>,
}

#[derive(Clone, Serialize)]
struct ObservationSpec {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sql: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    events: Option<EventsOut>,
}

#[derive(Clone, Serialize)]
struct EventsOut {
    topic: String,
    measure: &'static str,
}

#[derive(Clone, Serialize)]
struct ConditionOut {
    observation: String,
    equals: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InvariantOut {
    id: &'static str,
    description: String,
    when: Vec<ConditionOut>,
    require: Vec<ConditionOut>,
}

#[derive(Serialize)]
struct ScenarioOut {
    scenario: String,
    input: Value,
}

#[derive(Clone, Serialize)]
struct InjectOut {
    checkpoint: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    mode: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CaseOut {
    id: String,
    description: String,
    finding: Value,
    scenario: String,
    input: Value,
    inject: InjectOut,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    verify_before_injection: Vec<ConditionOut>,
}

/// One placeholder observation per effect the invariant will talk about.
fn observation_for(step: &TraceStep, steps: &[TraceStep], taken: &[Observation]) -> Observation {
    let base = match step.kind {
        StepKind::DbWrite => format!(
            "{}State",
            ident(step.table.as_deref().unwrap_or("table"), "table")
        ),
        StepKind::TxCommit => {
            let table = steps
                .iter()
                .find(|s| s.kind == StepKind::DbWrite && s.transaction == step.transaction)
                .and_then(|s| s.table.as_deref())
                .unwrap_or("transaction");
            format!("{}State", ident(table, "transaction"))
        }
        _ => format!(
            "{}Events",
            ident(
                step.label.split('(').next().unwrap_or("published"),
                "published"
            )
        ),
    };
    let mut name = base.clone();
    let mut n = 2;
    while taken.iter().any(|o| o.name == name) {
        name = format!("{base}{n}");
        n += 1;
    }
    let spec = match step.kind {
        StepKind::External => ObservationSpec {
            name: name.clone(),
            sql: None,
            events: Some(EventsOut {
                topic: format!(
                    "{TODO}: the topic your event-log adapter records for `{}` ({})",
                    step.label,
                    location(step)
                ),
                measure: "count",
            }),
        },
        _ => ObservationSpec {
            name: name.clone(),
            sql: Some(format!(
                "{TODO}: a read-only SQL query returning one value (one row, one column) that shows whether `{}` ({}) took effect for the record the scenario changes",
                step.label,
                location(step)
            )),
            events: None,
        },
    };
    Observation { name, spec }
}

fn relative_root(depth: usize) -> String {
    if depth == 0 {
        ".".to_owned()
    } else {
        vec![".."; depth].join("/")
    }
}

/// Resolve the output directory: the canonical deepest existing ancestor
/// plus the remaining normal components.
fn resolve_output(output: &Path) -> Result<PathBuf, String> {
    let absolute = if output.is_absolute() {
        output.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(output)
    };
    let mut existing = absolute.as_path();
    let mut rest: Vec<&std::ffi::OsStr> = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent;
            }
            _ => return Err(format!("cannot resolve `{}`", output.display())),
        }
    }
    if std::fs::symlink_metadata(&absolute).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!(
            "`{}` is a symbolic link; choose a real directory",
            output.display()
        ));
    }
    let mut resolved = existing
        .canonicalize()
        .map_err(|e| format!("cannot resolve `{}`: {e}", existing.display()))?;
    for name in rest.iter().rev() {
        if Path::new(name)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(format!("`{}` is not a plain path", output.display()));
        }
        resolved.push(name);
    }
    Ok(resolved)
}

/// What the generator decided, shared by the file renderers.
struct Plan<'t> {
    trace: &'t FailureTrace,
    runtime: RuntimeArg,
    scenario: String,
    /// Output directory relative to the project, `/`-separated.
    harness_rel: String,
    /// From the harness directory back to the project root (`..`).
    root_rel: String,
    analyze: Vec<String>,
    config: Option<String>,
    observations: Vec<Observation>,
    linked: Result<(), String>,
}

pub(crate) fn init_replay(args: &InitReplayArgs) -> Result<u8, String> {
    let text = std::fs::read_to_string(&args.trace)
        .map_err(|e| format!("cannot read `{}`: {e}", args.trace.display()))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| format!("`{}` is not JSON: {e}", args.trace.display()))?;
    if value["kind"].as_str() != Some(TRACE_REPORT_KIND) {
        return Err(format!(
            "`{}` is not a trace report; create one with `midwrite trace --format json`",
            args.trace.display()
        ));
    }
    let report: TraceReport =
        serde_json::from_value(value).map_err(|e| format!("invalid trace report: {e}"))?;
    let Some(trace) = report
        .traces
        .iter()
        .find(|t| t.finding_id == args.finding || t.id == args.finding)
    else {
        if let Some(skipped) = report.skipped.iter().find(|s| s.finding_id == args.finding) {
            return Err(format!(
                "finding {} has no failure trace ({}), so there is no failure window to replay",
                skipped.finding_id, skipped.reason
            ));
        }
        return Err(format!(
            "no finding or trace `{}` in `{}`",
            args.finding,
            args.trace.display()
        ));
    };
    if trace.completeness == Completeness::Incomplete && !args.allow_incomplete {
        return Err(format!(
            "the trace of finding {} is incomplete (it contains operations with unknown semantics or unsupported control flow), so it is not a basis for a reproduction plan; pass --allow-incomplete to generate a skeleton anyway",
            trace.finding_id
        ));
    }

    let project = args
        .project
        .canonicalize()
        .map_err(|e| format!("cannot resolve --project `{}`: {e}", args.project.display()))?;
    if !project.join(&trace.location.file).is_file() {
        return Err(format!(
            "the finding's file `{}` is not under --project `{}`; pass the directory `midwrite trace` ran in",
            trace.location.file,
            display_path(&args.project)
        ));
    }
    let output = resolve_output(&args.output)?;
    let Ok(harness_rel_path) = output.strip_prefix(&project) else {
        return Err(format!(
            "the output directory must lie inside the project `{}` (it analyzes the project from there)",
            display_path(&args.project)
        ));
    };
    if harness_rel_path.as_os_str().is_empty() {
        return Err(
            "the output directory must be a subdirectory of the project, not the project itself"
                .to_owned(),
        );
    }
    let harness_rel = harness_rel_path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    let root_rel = relative_root(harness_rel_path.components().count());

    let analyze: Vec<String> = if args.analyze.is_empty() {
        let first = Path::new(&trace.location.file)
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_else(|| trace.location.file.clone());
        vec![first]
    } else {
        args.analyze.clone()
    };
    for path in &analyze {
        if Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err(format!(
                "--analyze `{path}` must be relative to the project without `..`"
            ));
        }
    }
    let config = match &args.config {
        Some(config) => Some(config.clone()),
        None => discover_config(&project)
            .and_then(|found| found.path.strip_prefix(&project).ok().map(display_path)),
    };

    // Re-run the static analysis exactly as the generated manifest will
    // (read-only) and check that it reports the same finding id.
    let linked = {
        let loaded = match &config {
            Some(c) => Config::load(&project.join(c)).map_err(|e| e.to_string()),
            None => Ok(Config::default()),
        };
        loaded.and_then(|config| {
            let paths: Vec<PathBuf> = analyze.iter().map(PathBuf::from).collect();
            let project_report = Analyzer::new(config)
                .trace_project_at(&project, &paths)
                .map_err(|e| e.to_string())?
                .into_trace_report(None);
            if project_report
                .traces
                .iter()
                .any(|t| t.finding_id == trace.finding_id)
            {
                Ok(())
            } else {
                Err(format!(
                    "analyzing {} from the project with this configuration does not report finding {}; the trace report was produced from another directory, with other paths, a different configuration, or older sources",
                    analyze.join(", "),
                    trace.finding_id
                ))
            }
        })
    };

    let first = &trace.steps[trace.injection.after_step];
    let second = trace.injection.before_step.map(|i| &trace.steps[i]);
    let mut observations: Vec<Observation> = Vec::new();
    for step in std::iter::once(first).chain(second) {
        let observation = observation_for(step, &trace.steps, &observations);
        observations.push(observation);
    }
    let plan = Plan {
        trace,
        runtime: args.runtime,
        scenario: ident(&trace.function, "scenario"),
        harness_rel,
        root_rel,
        analyze,
        config,
        observations,
        linked,
    };

    let mut files: Vec<(PathBuf, String)> = vec![
        (PathBuf::from("midwrite-replay.json"), manifest(&plan)),
        (PathBuf::from("harness.ts"), harness(&plan)),
        (PathBuf::from("schema.sql"), schema(&plan)),
        (PathBuf::from("package.json"), package_json(&plan)),
        (PathBuf::from("README.md"), readme(&plan)),
        (PathBuf::from("finding.json"), {
            let mut json = serde_json::to_string_pretty(trace).map_err(|e| e.to_string())?;
            json.push('\n');
            json
        }),
    ];
    if plan.runtime == RuntimeArg::Node {
        files.push((PathBuf::from("tsconfig.json"), tsconfig(&plan)));
    }
    for (name, content) in RUNTIME_FILES {
        files.push((
            Path::new("midwrite-runtime").join(name),
            (*content).to_owned(),
        ));
    }

    if !args.force {
        let existing: Vec<String> = files
            .iter()
            .filter(|(path, _)| std::fs::symlink_metadata(output.join(path)).is_ok())
            .map(|(path, _)| display_path(path))
            .collect();
        if !existing.is_empty() {
            return Err(format!(
                "refusing to overwrite existing files in `{}`: {} (pass --force to overwrite exactly these files)",
                display_path(&args.output),
                existing.join(", ")
            ));
        }
    }
    std::fs::create_dir_all(&output)
        .map_err(|e| format!("cannot create `{}`: {e}", output.display()))?;
    let canonical_output = output
        .canonicalize()
        .map_err(|e| format!("cannot resolve `{}`: {e}", output.display()))?;
    // First pass: every target directory exists and resolves inside the
    // output directory, so that a refusal leaves no partial skeleton.
    for (path, _) in &files {
        let target = output.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create `{}`: {e}", parent.display()))?;
            // A pre-existing symbolic link (for example `midwrite-runtime`
            // pointing elsewhere) must not redirect writes out of --output.
            let resolved = parent
                .canonicalize()
                .map_err(|e| format!("cannot resolve `{}`: {e}", parent.display()))?;
            if !resolved.starts_with(&canonical_output) {
                return Err(format!(
                    "refusing to write `{}`: its directory resolves outside the output directory",
                    display_path(path)
                ));
            }
        }
        if std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!(
                "refusing to write through the symbolic link `{}`",
                target.display()
            ));
        }
    }
    for (path, content) in &files {
        let target = output.join(path);
        let mut options = std::fs::OpenOptions::new();
        options.write(true);
        if args.force {
            options.create(true).truncate(true);
        } else {
            options.create_new(true);
        }
        let mut file = options
            .open(&target)
            .map_err(|e| format!("cannot write `{}`: {e}", target.display()))?;
        std::io::Write::write_all(&mut file, content.as_bytes())
            .map_err(|e| format!("cannot write `{}`: {e}", target.display()))?;
    }

    let mut summary = format!(
        "midwrite init-replay: wrote a {} replay skeleton for finding {} ({} in `{}`, {}:{}) to {}\n",
        match plan.runtime {
            RuntimeArg::Node => "Node.js",
            RuntimeArg::Bun => "Bun",
        },
        trace.finding_id,
        trace.code,
        trace.function,
        trace.location.file,
        trace.location.line,
        display_path(&args.output)
    );
    for (path, _) in files
        .iter()
        .filter(|(p, _)| !p.starts_with("midwrite-runtime"))
    {
        let _ = writeln!(summary, "  {}", display_path(path));
    }
    let _ = writeln!(
        summary,
        "  midwrite-runtime/ (the replay client, vendored)\nNothing was executed or installed. Complete every {TODO} marker (README.md lists them); `midwrite replay` refuses to run until then."
    );
    if let Err(problem) = &plan.linked {
        let _ = writeln!(summary, "warning: {problem}");
    }
    eprint!("{summary}");
    Ok(EXIT_OK)
}

fn finding_selector(plan: &Plan<'_>) -> Value {
    json!({ "id": plan.trace.finding_id })
}

fn manifest(plan: &Plan<'_>) -> String {
    let trace = plan.trace;
    let first = &trace.steps[trace.injection.after_step];
    let second = trace.injection.before_step.map(|i| &trace.steps[i]);
    let (first_before, first_after) = checkpoints_of(first);
    let (second_before, second_after) = second.map_or((None, None), checkpoints_of);
    let input = json!({
        TODO.to_string(): format!("the arguments for `{}` as JSON (the harness passes this object to your scenario)", trace.function)
    });
    let first_obs = plan.observations[0].name.clone();
    let inject = |checkpoint: &Checkpoint, mode: &'static str| InjectOut {
        checkpoint: checkpoint.0,
        label: checkpoint.1.clone(),
        mode,
    };
    let verify = vec![ConditionOut {
        observation: first_obs.clone(),
        equals: format!(
            "{TODO}: the value of `{first_obs}` once `{}` has completed (checked from a separate database session before injecting)",
            first.label
        ),
    }];
    let case =
        |id: String, description: String, inject: InjectOut, verify: Vec<ConditionOut>| CaseOut {
            id,
            description,
            finding: finding_selector(plan),
            scenario: plan.scenario.clone(),
            input: input.clone(),
            inject,
            verify_before_injection: verify,
        };
    let mut cases: Vec<CaseOut> = Vec::new();
    if let Some(boundary) = &first_after {
        for (mode, prefix) in [("processKill", "kill"), ("exception", "exception")] {
            cases.push(case(
                slug(&format!("{prefix}-{}", boundary.0)),
                format!(
                    "{}: fail ({mode}) right after `{}` completed ({}), before `{}`. This is the trace's failure window.",
                    trace.code,
                    first.label,
                    location(first),
                    second.map_or("the end of the function", |s| s.label.as_str())
                ),
                inject(boundary, mode),
                verify.clone(),
            ));
        }
    }
    if let (Some(checkpoint), Some(second)) = (&second_before, second)
        && first_after.as_ref().map(|c| c.0) != Some(checkpoint.0)
    {
        cases.push(case(
            slug(&format!("kill-{}", checkpoint.0)),
            format!(
                "{}: kill right before `{}` ({}) starts; the same failure window seen from its other end.",
                trace.code,
                second.label,
                location(second)
            ),
            inject(checkpoint, "processKill"),
            Vec::new(),
        ));
    }
    if let Some(checkpoint) = &first_before {
        cases.push(case(
            slug(&format!("negative-kill-{}", checkpoint.0)),
            format!(
                "Negative case: kill before `{}` ({}) starts, outside the failure window.",
                first.label,
                location(first)
            ),
            inject(checkpoint, "processKill"),
            Vec::new(),
        ));
    }
    if let (Some(checkpoint), Some(second)) = (&second_after, second) {
        cases.push(case(
            slug(&format!("negative-kill-{}", checkpoint.0)),
            format!(
                "Negative case: kill after `{}` ({}) completed, outside the failure window.",
                second.label,
                location(second)
            ),
            inject(checkpoint, "processKill"),
            Vec::new(),
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    cases.retain(|c| seen.insert(c.id.clone()));

    let second_obs = plan
        .observations
        .get(1)
        .map_or(first_obs.clone(), |o| o.name.clone());
    let runtime = match plan.runtime {
        RuntimeArg::Node => "node",
        RuntimeArg::Bun => "bun",
    };
    let entry = match plan.runtime {
        RuntimeArg::Node => format!("dist/{}/harness.js", plan.harness_rel),
        RuntimeArg::Bun => "harness.ts".to_owned(),
    };
    let value = ManifestOut {
        schema_version: 1,
        harness: slug(&format!("{}-{}", trace.function, trace.code)),
        description: format!(
            "Generated by `midwrite init-replay` from finding {} ({} in `{}`, {}:{}; trace {}). Edit freely.",
            trace.finding_id,
            trace.code,
            trace.function,
            trace.location.file,
            trace.location.line,
            trace.id
        ),
        runtime,
        command: vec![runtime.to_owned(), entry],
        timeout_seconds: 30,
        database: DatabaseOut {
            provider: "ephemeral-postgres",
            setup: vec!["schema.sql"],
        },
        analysis: AnalysisOut {
            root: plan.root_rel.clone(),
            paths: plan.analyze.clone(),
            config: plan.config.clone(),
        },
        observations: plan.observations.iter().map(|o| o.spec.clone()).collect(),
        invariants: vec![InvariantOut {
            id: "application-contract",
            description: format!(
                "{TODO}: the application contract this finding may break, in your own words; e.g. \"when `{}` took effect, `{}` must have happened too (or be durably owed)\". Midwrite does not know your business rules.",
                first.label,
                second.map_or("the rest of the operation", |s| s.label.as_str())
            ),
            when: vec![ConditionOut {
                observation: first_obs.clone(),
                equals: format!("{TODO}: the value of `{first_obs}` after `{}`", first.label),
            }],
            require: vec![ConditionOut {
                observation: second_obs.clone(),
                equals: format!("{TODO}: the value `{second_obs}` must have then"),
            }],
        }],
        control: ScenarioOut {
            scenario: plan.scenario.clone(),
            input: input.clone(),
        },
        cases,
    };
    let mut text = serde_json::to_string_pretty(&value).unwrap_or_default();
    text.push('\n');
    text
}

fn import_path(plan: &Plan<'_>) -> String {
    let file = &plan.trace.location.file;
    let file = match plan.runtime {
        RuntimeArg::Node => {
            let path = Path::new(file);
            match path.extension().and_then(|e| e.to_str()) {
                Some("ts") | Some("tsx") | Some("mts") => {
                    path.with_extension("js").display().to_string()
                }
                _ => file.clone(),
            }
        }
        RuntimeArg::Bun => file.clone(),
    };
    format!("{}/{}", plan.root_rel, file)
}

fn effect_lines(plan: &Plan<'_>, kind: StepKind) -> String {
    let mut out = String::new();
    for step in plan.trace.steps.iter().filter(|s| s.kind == kind) {
        let _ = writeln!(
            out,
            "//   - {} ({}{})",
            one_line(&step.label),
            one_line(&location(step)),
            step.category
                .as_deref()
                .map(|c| one_line(&format!(", {c}")))
                .unwrap_or_default()
        );
    }
    if out.is_empty() {
        out.push_str("//   (none on this trace)\n");
    }
    out
}

fn harness(plan: &Plan<'_>) -> String {
    let trace = plan.trace;
    format!(
        r#"// Midwrite replay harness skeleton, generated by `midwrite init-replay`
// from finding {finding} ({code} in `{function}`, {file}:{line}:{column}; trace {trace_id}).
//
// The TODO markers below (with the MIDWRITE prefix) are project-specific
// integration only you can write. `midwrite replay` refuses to run while
// one remains here (in the compiled entry point), in midwrite-replay.json
// or in schema.sql. Comments that mention the marker count too.
//
// Checkpoints are emitted by the adapters below at the boundaries of your
// real driver calls, so your application code stays unchanged.
import pg from "pg";
import {{ runScenarios }} from "@midwrite/replay";
import {{ instrumentPgPool, replayPoolConfig }} from "@midwrite/replay/pg";
import {{ publishToEventLog }} from "@midwrite/replay/event-log";

// Database: a node-postgres pool for the run's disposable database (Unix
// socket inside the replay directory), instrumented to emit before-db-write,
// after-db-commit, after-tx-write, before-tx-commit and after-tx-rollback.
// Applications on another driver need an equivalent adapter.
// Database writes on this trace:
{writes}const pool = instrumentPgPool(new pg.Pool(replayPoolConfig()));

// {TODO}(database): hand `pool` to your application before it creates its
// database handle, for example `usePool(pool)` or `drizzle(pool)`.

// External effects on this trace:
{externals}// {TODO}(effects): route each of them to `publishToEventLog(topic, payload,
// {{ id }})`, or to your own adapter that emits before-event-publish and
// after-event-publish around the durable effect, instead of the real service.

// Load your application only after the wiring above, for example:
//   const {{ {function} }} = await import("{import}");

await runScenarios(
  {{
    {scenario}: async (input: unknown) => {{
      // {TODO}(scenario): call `{function}` ({file}) with `input`.
      throw new Error("{TODO}: scenario `{scenario}` is not implemented; see README.md");
    }},
  }},
  () => pool.end(),
);
"#,
        finding = one_line(&trace.finding_id),
        code = one_line(&trace.code),
        function = one_line(&trace.function).replace('"', "'"),
        file = one_line(&trace.location.file).replace('"', "'"),
        line = trace.location.line,
        column = trace.location.column,
        trace_id = one_line(&trace.id),
        writes = effect_lines(plan, StepKind::DbWrite),
        externals = effect_lines(plan, StepKind::External),
        import = one_line(&import_path(plan)),
        scenario = plan.scenario,
    )
}

fn schema(plan: &Plan<'_>) -> String {
    let mut out = format!(
        "-- {TODO}: the schema and initial rows your scenario needs (CREATE TABLE ...;\n-- INSERT ...). Applied to a fresh, empty database before every run.\n"
    );
    let tables: std::collections::BTreeSet<&str> = plan
        .trace
        .steps
        .iter()
        .filter_map(|s| s.table.as_deref())
        .collect();
    if !tables.is_empty() {
        let _ = writeln!(
            out,
            "-- Tables this trace reads or writes: {}.",
            one_line(&tables.into_iter().collect::<Vec<_>>().join(", "))
        );
    }
    out
}

fn package_json(plan: &Plan<'_>) -> String {
    let mut value = json!({
        "name": format!("midwrite-replay-{}", slug(&plan.trace.function)),
        "private": true,
        "type": "module",
        "dependencies": { "@midwrite/replay": "file:./midwrite-runtime" }
    });
    if plan.runtime == RuntimeArg::Node {
        value["scripts"] = json!({ "build": "tsc -p tsconfig.json" });
    }
    let mut text = serde_json::to_string_pretty(&value).unwrap_or_default();
    text.push('\n');
    text
}

fn tsconfig(plan: &Plan<'_>) -> String {
    let mut include = vec!["harness.ts".to_owned()];
    include.extend(
        plan.analyze
            .iter()
            .map(|p| format!("{}/{}", plan.root_rel, p)),
    );
    let value = json!({
        "compilerOptions": {
            "target": "ES2022",
            "module": "NodeNext",
            "moduleResolution": "NodeNext",
            "strict": true,
            "skipLibCheck": true,
            "rootDir": plan.root_rel,
            "outDir": "dist"
        },
        "include": include
    });
    let mut text = serde_json::to_string_pretty(&value).unwrap_or_default();
    text.push('\n');
    text
}

fn readme(plan: &Plan<'_>) -> String {
    let trace = plan.trace;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Replay harness for finding {} ({} in `{}`)\n",
        trace.finding_id, trace.code, trace.function
    );
    let _ = writeln!(
        out,
        "Generated by `midwrite init-replay` (Midwrite {}) from trace `{}` of finding `{}`, reported at `{}:{}:{}` (trace completeness: {:?}). Nothing was executed or installed to create it.\n",
        env!("CARGO_PKG_VERSION"),
        trace.id,
        trace.finding_id,
        trace.location.file,
        trace.location.line,
        trace.location.column,
        trace.completeness
    );
    match &plan.linked {
        Ok(()) => {
            let _ = writeln!(
                out,
                "The manifest analyzes `{}` from the project root (`analysis.root` = `{}`), which reproduces finding `{}` with the same identity, so every case links to it by fingerprint.\n",
                plan.analyze.join("`, `"),
                plan.root_rel,
                trace.finding_id
            );
        }
        Err(problem) => {
            let _ = writeln!(
                out,
                "**Warning:** {problem}. Adjust `analysis` in `midwrite-replay.json` until `midwrite replay .` shows the cases as `Matched`.\n"
            );
        }
    }
    let _ = writeln!(out, "## The failure trace (static candidate)\n\n```text");
    out.push_str(&trace.to_text());
    let _ = writeln!(out, "```\n");
    let _ = writeln!(
        out,
        "The trace is a static candidate: it says such a window *may* exist. The harness turns it into an observation of what your application actually does when it fails there.\n"
    );
    let _ = writeln!(
        out,
        "## Checkpoints\n\nThe vendored adapters emit the protocol's checkpoints around real driver calls; no change to your source is needed. For this trace:\n\n| Step | Operation | Source | Checkpoints |\n| --- | --- | --- | --- |"
    );
    for step in &trace.steps {
        let (before, after) = checkpoints_of(step);
        if before.is_none() && after.is_none() {
            continue;
        }
        let names = [before, after]
            .into_iter()
            .flatten()
            .map(|(name, label)| match label {
                Some(label) => format!("`{name}` ({label})"),
                None => format!("`{name}`"),
            })
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "| {} | `{}` | `{}` | {} |",
            step.index,
            step.label.replace('|', "\\|"),
            location(step),
            names
        );
    }
    let boundary = trace
        .injection
        .checkpoints
        .iter()
        .map(|c| format!("`{}`", c.name))
        .collect::<Vec<_>>()
        .join(" / ");
    let _ = writeln!(
        out,
        "\nThe failure window lies at {boundary}. External effects only emit their checkpoints if you route them through `publishToEventLog` (or your own adapter).\n"
    );
    let build = match plan.runtime {
        RuntimeArg::Node => {
            "npm install          # links the vendored client; pg and typescript resolve from the project\nnpm run build        # compiles harness.ts and the analyzed sources to dist/\n"
        }
        RuntimeArg::Bun => "bun install          # links the vendored client\n",
    };
    let _ = writeln!(
        out,
        r#"## What you must do (manual steps)

Search for `{TODO}`:

1. `harness.ts`: give the application the instrumented pool, route its external effects to the event log, and call `{function}` in the scenario `{scenario}`.
2. `schema.sql`: the tables and rows the scenario needs.
3. `midwrite-replay.json`:
   * `observations`: one read-only SQL query per database effect (one row, one column) and the event-log topic of each external effect;
   * `invariants`: the contract your application must keep. Midwrite cannot know it, and a runtime result is classified only against it;
   * `input` of the control run and of every case;
   * `verifyBeforeInjection`: the value that proves the first effect completed (for example, is committed) before the failure is injected.
4. After the first run, add `"expect"` to the cases you want to keep as regression tests.

`midwrite replay` refuses to start while any marker remains.

## Run it

```bash
{build}midwrite replay .                     # prints the plan; executes nothing
midwrite replay . --allow-execution    # control run, then every case, isolated
midwrite replay . --allow-execution --format json --output verification.json
midwrite report verification.json -o verification.html
```

Requirements: PostgreSQL server binaries, {runtime_req}, and OS isolation (macOS sandbox-exec, or Linux bubblewrap with unprivileged user namespaces). See Midwrite's `docs/phase4/HARNESS-GENERATION.md`.

## Files

| File | Purpose |
| --- | --- |
| `midwrite-replay.json` | The manifest: command, database, observations, invariants, cases. |
| `harness.ts` | Entry point: wiring and scenarios. |
| `schema.sql` | Initial database state of every run. |
| `finding.json` | The trace this skeleton was generated from (provenance; not read by replay). |
| `midwrite-runtime/` | The replay client and adapters, vendored from this Midwrite version. |
"#,
        function = trace.function,
        scenario = plan.scenario,
        runtime_req = match plan.runtime {
            RuntimeArg::Node => "Node.js 22 or newer",
            RuntimeArg::Bun => "Bun 1.2 or newer",
        },
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_and_slugs() {
        assert_eq!(ident("orders", "x"), "orders");
        assert_eq!(ident("completeOrder", "x"), "completeOrder");
        assert_eq!(ident("order_items", "x"), "order_items");
        assert_eq!(ident("order-items", "x"), "orderItems");
        assert_eq!(ident("queue.publish", "x"), "queuePublish");
        assert_eq!(ident("1abc", "x"), "x");
        assert_eq!(slug("completeOrder-MW1001"), "completeorder-mw1001");
        assert_eq!(slug("a  b__c"), "a-b-c");
        assert_eq!(relative_root(2), "../..");
    }

    #[test]
    fn interpolated_text_cannot_leave_a_comment() {
        let hostile = "x)\nawait fetch(\"https://evil\");\r// ";
        let safe = one_line(hostile);
        assert!(!safe.contains('\n') && !safe.contains('\r'));
        assert!(safe.starts_with("x) await fetch"));
    }
}

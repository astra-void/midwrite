//! CommitRift runtime replay: turns a selected failure window into
//! observed runtime evidence, for explicitly authorized harnesses only.
//!
//! A *harness* is a directory with a `commitrift-replay.json` manifest
//! ([`manifest`]), an instrumented test application that speaks the
//! checkpoint protocol ([`supervisor`]), and SQL files describing its initial
//! state. [`run`] creates a disposable PostgreSQL cluster ([`postgres`]),
//! runs a control scenario and every selected case from a fresh database,
//! injects the failure at the selected checkpoint, observes the surviving
//! state ([`observe`]) and classifies each case against the invariants the
//! harness declares.
//!
//! Static analysis (`commitrift check`) never calls into this crate.

pub mod isolation;
pub mod manifest;
pub mod observe;
pub mod postgres;
pub mod safety;
pub mod supervisor;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;

use commitrift_core::trace::{FailureTrace, ToolStamp, TraceReport};
use commitrift_core::verification::{
    BoundaryRelation, CaseResult, CheckpointSelector, EnvironmentInfo, FailureMode, HarnessInfo,
    Logs, RecoveryRecord, RunRecord, SourceInfo, StaticLink, StaticLinkStatus,
    VERIFICATION_REPORT_KIND, VERIFICATION_SCHEMA_VERSION, VerificationLevel, VerificationReport,
    VerificationSummary,
};

pub use manifest::{CaseSpec, LoadedHarness, MANIFEST_FILE_NAME, Manifest};

use isolation::{Isolation, IsolationRequest};
use manifest::{InjectSpec, NetworkPolicy, ScenarioCall};
use observe::Observations;
use postgres::{Binaries, Cluster};
use safety::ReplayRoot;
use supervisor::{Injection, Process};

/// Largest number of bytes of each output stream kept in a report.
const LOG_LIMIT: usize = 16 * 1024;
const MAX_REPEAT: u32 = 50;

#[derive(Clone, Debug)]
pub struct ReplayOptions {
    /// How many times each case runs, each from a fresh database.
    pub repeat: u32,
    /// Allow running without network isolation (requires the manifest's
    /// `"network": "allow"` as well).
    pub allow_network: bool,
    /// Keep the temporary directory (database, logs) for inspection.
    pub keep_artifacts: bool,
}

impl Default for ReplayOptions {
    fn default() -> Self {
        Self {
            repeat: 1,
            allow_network: false,
            keep_artifacts: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    /// A precondition of the environment is missing (PostgreSQL, network
    /// isolation). Nothing was executed.
    #[error("replay blocked: {0}")]
    Blocked(String),
    /// The request is invalid or unsafe. Nothing was executed.
    #[error("{0}")]
    Refused(String),
}

/// Pick the cases to run. `selector` is a case id or a diagnostic code; a
/// code must identify exactly one case. `finding` is a diagnostic
/// fingerprint. Without either, every case of the harness runs.
pub fn select_cases<'m>(
    manifest: &'m Manifest,
    selector: Option<&str>,
    finding: Option<&str>,
    traces: Option<&TraceReport>,
) -> Result<Vec<&'m CaseSpec>, String> {
    let mut cases: Vec<&CaseSpec> = manifest.cases.iter().collect();
    if let Some(finding) = finding {
        cases.retain(|case| link(case, traces).finding_id.as_deref() == Some(finding));
        if cases.is_empty() {
            return Err(format!(
                "no case of this harness targets finding `{finding}`"
            ));
        }
    }
    let Some(selector) = selector else {
        return Ok(cases);
    };
    if let Some(case) = cases.iter().find(|case| case.id == selector) {
        return Ok(vec![case]);
    }
    let by_code: Vec<&CaseSpec> = cases
        .iter()
        .copied()
        .filter(|case| {
            let linked = link(case, traces);
            let declared = case.finding.as_ref().and_then(|f| f.code.as_deref());
            linked.code.as_deref().or(declared) == Some(selector)
        })
        .collect();
    match by_code.len() {
        0 => Err(format!(
            "no case `{selector}`; cases: {}",
            cases
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
        1 => Ok(by_code),
        _ => Err(format!(
            "`{selector}` matches several cases ({}); a diagnostic code does not identify a unique trace: pass a case id or --finding",
            by_code
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The static finding a case targets.
#[derive(Clone, Debug)]
pub struct Link<'t> {
    pub status: StaticLink,
    pub trace: Option<&'t FailureTrace>,
    pub finding_id: Option<String>,
    pub code: Option<String>,
}

pub fn link<'t>(case: &CaseSpec, traces: Option<&'t TraceReport>) -> Link<'t> {
    let Some(selector) = &case.finding else {
        return Link {
            status: StaticLink {
                status: StaticLinkStatus::None,
                detail: "the case names no finding".to_owned(),
            },
            trace: None,
            finding_id: None,
            code: None,
        };
    };
    let Some(report) = traces else {
        return Link {
            status: StaticLink {
                status: StaticLinkStatus::NotFound,
                detail: "no static analysis was run for this harness (manifest has no `analysis`)"
                    .to_owned(),
            },
            trace: None,
            finding_id: selector.id.clone(),
            code: selector.code.clone(),
        };
    };
    let matches_selector = |id: &str, code: &str, function: &str, file: &str| {
        selector.id.as_deref().is_none_or(|x| x == id)
            && selector.code.as_deref().is_none_or(|x| x == code)
            && selector.function.as_deref().is_none_or(|x| x == function)
            && selector.file.as_deref().is_none_or(|x| x == file)
    };
    let traced: Vec<&FailureTrace> = report
        .traces
        .iter()
        .filter(|t| matches_selector(&t.finding_id, &t.code, &t.function, &t.location.file))
        .collect();
    let untraced = report
        .skipped
        .iter()
        .filter(|s| matches_selector(&s.finding_id, &s.code, &s.function, &s.location.file))
        .count();
    match (traced.as_slice(), untraced) {
        ([trace], 0) => Link {
            status: StaticLink {
                status: StaticLinkStatus::Matched,
                detail: format!(
                    "finding {} ({} in `{}`, {}:{})",
                    trace.finding_id,
                    trace.code,
                    trace.function,
                    trace.location.file,
                    trace.location.line
                ),
            },
            trace: Some(trace),
            finding_id: Some(trace.finding_id.clone()),
            code: Some(trace.code.clone()),
        },
        ([], 0) => Link {
            status: StaticLink {
                status: StaticLinkStatus::NotFound,
                detail: "the static analysis reports no finding matching the case's selector"
                    .to_owned(),
            },
            trace: None,
            finding_id: None,
            code: selector.code.clone(),
        },
        ([], _) => Link {
            status: StaticLink {
                status: StaticLinkStatus::NotFound,
                detail: "the matching finding has no failure trace".to_owned(),
            },
            trace: None,
            finding_id: None,
            code: selector.code.clone(),
        },
        _ => Link {
            status: StaticLink {
                status: StaticLinkStatus::Ambiguous,
                detail: format!(
                    "{} findings match the case's selector; add `id`",
                    traced.len() + untraced
                ),
            },
            trace: None,
            finding_id: None,
            code: selector.code.clone(),
        },
    }
}

/// What `commitrift replay` would execute, without executing it.
pub fn plan(harness: &LoadedHarness, cases: &[&CaseSpec], traces: Option<&TraceReport>) -> String {
    let manifest = &harness.manifest;
    let mut out = format!(
        "Replay harness `{}` ({})\n  manifest: {} (hash {})\n  command:  {} (run from {}, without a shell)\n  database: disposable PostgreSQL cluster created in a private temporary directory, Unix socket only\n  network:  {}\n  control:  scenario `{}`\n  cases:\n",
        manifest.harness,
        manifest.description,
        harness.manifest_path.display(),
        harness.manifest_hash,
        manifest.command.join(" "),
        harness.dir.display(),
        match manifest.network {
            NetworkPolicy::Deny => "denied",
            NetworkPolicy::Allow => "allowed by the manifest (also needs --allow-network)",
        },
        manifest.control.scenario
    );
    for case in cases {
        let linked = link(case, traces);
        let inject = case.inject.as_ref().map_or_else(
            || "no injection".to_owned(),
            |i| format!("{} at `{}` #{}", i.mode, i.checkpoint, i.occurrence),
        );
        out.push_str(&format!(
            "    {:<32} scenario `{}`, {inject}; finding: {:?} {}\n",
            case.id, case.scenario, linked.status.status, linked.status.detail
        ));
    }
    out
}

/// The run context shared by all runs of one invocation.
struct Context<'a> {
    harness: &'a LoadedHarness,
    root: &'a Path,
    cluster: &'a Cluster,
    command: Vec<String>,
    base_env: Vec<(String, String)>,
    keep: bool,
}

/// Execute the control run and `cases` of an authorized harness.
pub fn run(
    harness: &LoadedHarness,
    cases: &[&CaseSpec],
    traces: Option<&TraceReport>,
    options: &ReplayOptions,
) -> Result<VerificationReport, ReplayError> {
    let manifest = &harness.manifest;
    if options.repeat == 0 || options.repeat > MAX_REPEAT {
        return Err(ReplayError::Refused(format!(
            "--repeat must be between 1 and {MAX_REPEAT}"
        )));
    }
    if options.allow_network && manifest.network != NetworkPolicy::Allow {
        return Err(ReplayError::Refused(
            "--allow-network was given, but the manifest does not declare `\"network\": \"allow\"`"
                .to_owned(),
        ));
    }
    preflight_runtime(harness)?;
    let bins = Binaries::discover().map_err(ReplayError::Blocked)?;
    let root = ReplayRoot::create(options.keep_artifacts).map_err(ReplayError::Blocked)?;
    let isolation = isolation::prepare(&IsolationRequest {
        root: &root.path,
        harness_dir: &harness.dir,
        program: &manifest.command[0],
        allow_unisolated: options.allow_network && manifest.network == NetworkPolicy::Allow,
    })
    .map_err(ReplayError::Blocked)?;
    let cluster = Cluster::start(bins, &root.path.join("db")).map_err(|e| {
        ReplayError::Blocked(format!("the disposable PostgreSQL cluster failed: {e}"))
    })?;
    safety::check_database_target(&root.path, &cluster.socket_path())
        .map_err(ReplayError::Refused)?;

    let mut base_env: Vec<(String, String)> = vec![
        (
            "PATH".to_owned(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_owned()),
        ),
        (
            "HOME".to_owned(),
            root.path.join("home").display().to_string(),
        ),
        (
            "TMPDIR".to_owned(),
            root.path.join("tmp").display().to_string(),
        ),
        ("LANG".to_owned(), "C".to_owned()),
        ("NO_COLOR".to_owned(), "1".to_owned()),
        ("COMMITRIFT_REPLAY".to_owned(), "1".to_owned()),
        (
            "COMMITRIFT_REPLAY_PROTOCOL".to_owned(),
            supervisor::PROTOCOL_VERSION.to_string(),
        ),
        (
            "COMMITRIFT_REPLAY_ROOT".to_owned(),
            root.path.display().to_string(),
        ),
        (
            "COMMITRIFT_REPLAY_PG_SOCKET".to_owned(),
            cluster.socket_path().display().to_string(),
        ),
        (
            "COMMITRIFT_REPLAY_PG_USER".to_owned(),
            postgres::USER.to_owned(),
        ),
    ];
    for dir in ["home", "tmp", "runs"] {
        safety::create_private_dir(&root.path.join(dir)).map_err(ReplayError::Blocked)?;
    }
    base_env.extend(manifest.env.iter().map(|(k, v)| (k.clone(), v.clone())));

    let runtime = runtime_version(&manifest.command[0], &base_env);
    let context = Context {
        harness,
        root: &root.path,
        cluster: &cluster,
        command: isolation.wrap(&manifest.command),
        base_env,
        keep: options.keep_artifacts,
    };

    let mut limitations = vec![
        "Runtime results hold for this harness, its declared invariants and the recorded environment only; they are not a proof that the application is safe or unsafe in production.".to_owned(),
        "The harness is trusted code: replay isolates it from external networks and other databases where the platform allows it, but it is not a security sandbox for malicious code.".to_owned(),
    ];
    if isolation == Isolation::None {
        limitations
            .push("The harness ran without network isolation (explicitly allowed).".to_owned());
    }

    let control = run_once(
        &context,
        "control",
        &manifest.control,
        None,
        &[],
        &[],
        &BTreeMap::new(),
    );
    let control_ok = control.verification == VerificationLevel::RuntimeNotReproduced
        && control.outcome == "returned";

    let mut results = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let linked = link(case, traces);
        let mut runs = Vec::new();
        if control_ok {
            for repetition in 1..=options.repeat {
                let label = format!("c{index}-r{repetition}");
                let mut record = run_once(
                    &context,
                    &label,
                    &ScenarioCall {
                        scenario: case.scenario.clone(),
                        input: case.input.clone(),
                    },
                    case.inject.as_ref(),
                    &case.verify_before_injection,
                    &case.recovery,
                    &case.predict,
                );
                record.run = repetition;
                runs.push(record);
            }
        }
        results.push(case_result(
            &manifest.harness,
            case,
            &linked,
            runs,
            &control,
            control_ok,
            options.repeat,
        ));
    }

    let source = source_info(&harness.dir);
    let environment = EnvironmentInfo {
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        postgres: Some(cluster.version().to_owned()),
        runtime,
        network_isolation: isolation.describe(),
        isolation: Some(isolation.kind().to_owned()),
        database: "ephemeral-postgres: private cluster, Unix socket only (listen_addresses=''), trust authentication for a local test user, fresh database per run, deleted afterwards".to_owned(),
        environment_variables: {
            let mut names: Vec<String> = context
                .base_env
                .iter()
                .map(|(k, _)| k.clone())
                .chain(
                    [
                        "COMMITRIFT_REPLAY_PG_DATABASE",
                        "COMMITRIFT_REPLAY_EVENT_LOG",
                        "COMMITRIFT_REPLAY_SOCKET",
                        "COMMITRIFT_REPLAY_SCENARIO",
                        "COMMITRIFT_REPLAY_INPUT",
                    ]
                    .map(str::to_owned),
                )
                .collect();
            names.sort();
            names.dedup();
            names
        },
    };
    let summary = VerificationSummary::of(&results);
    Ok(VerificationReport {
        schema_version: VERIFICATION_SCHEMA_VERSION,
        kind: VERIFICATION_REPORT_KIND.to_owned(),
        tool: ToolStamp::default(),
        harness: HarnessInfo {
            id: manifest.harness.clone(),
            description: manifest.description.clone(),
            manifest: commitrift_core_display(&harness.manifest_path),
            manifest_hash: harness.manifest_hash.clone(),
            command: manifest.command.clone(),
        },
        source,
        environment,
        control: Some(control),
        cases: results,
        summary,
        limitations,
    })
}

fn commitrift_core_display(path: &Path) -> String {
    let text = path.display().to_string();
    text.strip_prefix("./").unwrap_or(&text).to_owned()
}

fn case_result(
    harness: &str,
    case: &CaseSpec,
    linked: &Link<'_>,
    runs: Vec<RunRecord>,
    control: &RunRecord,
    control_ok: bool,
    repeat: u32,
) -> CaseResult {
    let mut limitations = Vec::new();
    let (verification, consistent) = if !control_ok {
        limitations.push(format!(
            "The control run (no injection) did not establish the invariants: {}. The harness or its declared contract is wrong, so no case was run.",
            control.reason
        ));
        (VerificationLevel::Inconclusive, true)
    } else {
        let first = runs.first().map(|r| r.verification);
        let consistent = runs.iter().all(|r| Some(r.verification) == first);
        if consistent {
            (first.unwrap_or(VerificationLevel::Inconclusive), true)
        } else {
            limitations.push(format!(
                "The {repeat} runs did not agree: {}. The outcome depends on something the harness does not control.",
                runs.iter().map(|r| r.verification.as_str()).collect::<Vec<_>>().join(", ")
            ));
            (VerificationLevel::Inconclusive, false)
        }
    };
    if linked.status.status != StaticLinkStatus::Matched && case.finding.is_some() {
        limitations.push(format!(
            "The case is not linked to a static finding: {}.",
            linked.status.detail
        ));
    }
    let boundary = match (linked.trace, &case.inject) {
        (Some(trace), Some(inject)) => {
            if trace
                .injection
                .checkpoints
                .iter()
                .any(|c| c.name == inject.checkpoint)
            {
                BoundaryRelation::OnTraceBoundary
            } else {
                BoundaryRelation::OutsideTraceBoundary
            }
        }
        _ => BoundaryRelation::NotApplicable,
    };
    if let Some(trace) = linked.trace
        && trace.completeness != commitrift_core::trace::Completeness::Complete
    {
        limitations.push(format!(
            "The static trace is {:?}; the harness, not the trace, defines what was executed.",
            trace.completeness
        ));
    }
    if case
        .inject
        .as_ref()
        .is_some_and(|i| i.mode == FailureMode::Exception)
    {
        limitations.push("Exception mode: the application's catch/finally handlers and transaction rollbacks ran; this is not the same as a process crash.".to_owned());
    }
    let recovery_observation = commitrift_core::verification::RecoveryObservation::of(
        &case
            .recovery
            .iter()
            .map(|r| r.scenario.clone())
            .collect::<Vec<_>>(),
        &runs,
    );
    let initial_state = "a fresh database created from the manifest's setup files before every run; an empty event log".to_owned();
    CaseResult {
        id: case.id.clone(),
        key: Some(format!("{harness}/{}", case.id)),
        description: case.description.clone(),
        static_link: linked.status.clone(),
        finding_id: linked.finding_id.clone(),
        trace_id: linked.trace.map(|t| t.id.clone()),
        code: linked.code.clone(),
        trace: linked.trace.cloned(),
        failure_mode: case.inject.as_ref().map_or(FailureMode::None, |i| i.mode),
        checkpoint: case.inject.as_ref().map(|i| CheckpointSelector {
            name: i.checkpoint.clone(),
            occurrence: i.occurrence,
            label: i.label.clone(),
        }),
        boundary,
        initial_state,
        runs,
        verification,
        consistent,
        expected: case.expect,
        matches_expectation: case.expect.map(|e| e == verification),
        recovery_observation,
        limitations,
    }
}

#[allow(clippy::too_many_arguments)]
fn run_once(
    context: &Context<'_>,
    label: &str,
    call: &ScenarioCall,
    inject: Option<&InjectSpec>,
    verify: &[manifest::ConditionSpec],
    recovery: &[ScenarioCall],
    predict: &BTreeMap<String, Value>,
) -> RunRecord {
    let start = Instant::now();
    let manifest = &context.harness.manifest;
    let mut record = RunRecord {
        run: 0,
        scenario: call.scenario.clone(),
        failure_mode: inject.map_or(FailureMode::None, |i| i.mode),
        injected: false,
        checkpoints: Vec::new(),
        pre_injection_checks: Vec::new(),
        exit: commitrift_core::verification::ExitInfo {
            kind: "notStarted".to_owned(),
            code: None,
            signal: None,
        },
        outcome: "notStarted".to_owned(),
        error: None,
        injected_fault_surfaced: None,
        observations: BTreeMap::new(),
        invariants: Vec::new(),
        prediction: Vec::new(),
        recovery: Vec::new(),
        duration_ms: 0,
        verification: VerificationLevel::Inconclusive,
        reason: String::new(),
        logs: Logs::default(),
    };
    let database = format!("cr_{}", label.replace('-', "_"));
    let run_dir = context.root.join("runs").join(label);
    let event_log = run_dir.join("events.jsonl");
    let finish = |mut record: RunRecord, reason: String| {
        record.reason = reason;
        record.duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        if !context.keep {
            let _ = context.cluster.drop_database(&database);
        }
        record
    };
    if let Err(error) = safety::create_private_dir(&run_dir) {
        return finish(record, error);
    }
    if let Err(error) = context.cluster.create_database(&database) {
        return finish(record, format!("database setup failed: {error}"));
    }
    for file in &manifest.database.setup {
        if let Err(error) = context
            .cluster
            .apply_file(&database, &context.harness.dir.join(file))
        {
            let error = safety::redact(&error, context.root);
            return finish(
                record,
                format!("database setup failed in `{file}`: {error}"),
            );
        }
    }
    let observe = || {
        observe::observe(
            &manifest.observations,
            context.cluster,
            &database,
            &event_log,
        )
    };
    match observe() {
        Ok(before) => {
            record.observations.insert("before".to_owned(), before);
        }
        Err(error) => {
            return finish(
                record,
                format!("observation failed before the run: {error}"),
            );
        }
    }

    let mut env = context.base_env.clone();
    env.push(("COMMITRIFT_REPLAY_PG_DATABASE".to_owned(), database.clone()));
    env.push((
        "COMMITRIFT_REPLAY_EVENT_LOG".to_owned(),
        event_log.display().to_string(),
    ));
    let process = Process {
        harness_dir: &context.harness.dir,
        command: &context.command,
        env: &env,
        run_dir: &run_dir,
        timeout: Duration::from_secs(manifest.timeout_seconds),
    };
    let mut verify_now =
        || -> Result<Vec<commitrift_core::verification::ConditionResult>, String> {
            let observations = observe()?;
            Ok(observe::check(verify, &observations))
        };
    let injection = inject.map(|spec| Injection {
        spec,
        verify: &mut verify_now,
    });
    let execution = supervisor::execute(&process, "main", &call.scenario, &call.input, injection);
    record.injected = execution.injected;
    record.checkpoints = execution.checkpoints;
    record.pre_injection_checks = execution.pre_checks;
    record.outcome = execution.outcome;
    record.error = execution.error.map(|e| safety::redact(&e, context.root));
    if record.injected && record.failure_mode == FailureMode::Exception {
        record.injected_fault_surfaced = Some(execution.fault_surfaced.unwrap_or(false));
    }
    if let Some(exit) = execution.exit {
        record.exit = exit;
    }
    let mut stdout = execution.stdout;
    let mut stderr = execution.stderr;
    let timed_out = execution.timed_out;
    let problem = execution.problem;

    let idle = context
        .cluster
        .wait_for_idle(&database, Duration::from_secs(10));
    let mut stage_observations: Vec<(&str, Observations)> = Vec::new();
    match observe() {
        Ok(after) => stage_observations.push(("after", after)),
        Err(error) => {
            record.logs = logs(&stdout, &stderr, context.root);
            return finish(record, format!("observation failed after the run: {error}"));
        }
    }
    let mut recovery_problem = None;
    if problem.is_none() && !timed_out {
        for (index, call) in recovery.iter().enumerate() {
            let execution = supervisor::execute(
                &process,
                &format!("recovery-{index}"),
                &call.scenario,
                &call.input,
                None,
            );
            stdout.push_str(&execution.stdout);
            stderr.push_str(&execution.stderr);
            if execution.outcome != "returned" && recovery_problem.is_none() {
                recovery_problem = Some(format!(
                    "the recovery scenario `{}` did not complete normally ({})",
                    call.scenario, execution.outcome
                ));
            }
            record.recovery.push(RecoveryRecord {
                scenario: call.scenario.clone(),
                outcome: execution.outcome,
                exit: execution
                    .exit
                    .unwrap_or(commitrift_core::verification::ExitInfo {
                        kind: "notStarted".to_owned(),
                        code: None,
                        signal: None,
                    }),
            });
        }
        if !recovery.is_empty() {
            let _ = context
                .cluster
                .wait_for_idle(&database, Duration::from_secs(10));
            match observe() {
                Ok(after) => stage_observations.push(("afterRecovery", after)),
                Err(error) => {
                    record.logs = logs(&stdout, &stderr, context.root);
                    return finish(
                        record,
                        format!("observation failed after recovery: {error}"),
                    );
                }
            }
        }
    }
    for (stage, observations) in &stage_observations {
        record.invariants.extend(observe::invariants(
            &manifest.invariants,
            stage,
            observations,
        ));
    }
    if let Some((_, after)) = stage_observations.first() {
        for (name, expected) in predict {
            let observed = after.get(name).cloned().unwrap_or(Value::Null);
            let matches = observe::holds(
                &manifest::ConditionSpec {
                    observation: name.clone(),
                    equals: Some(expected.clone()),
                    not_equals: None,
                    at_least: None,
                    at_most: None,
                },
                after,
            );
            record
                .prediction
                .push(commitrift_core::verification::PredictionResult {
                    observation: name.clone(),
                    expected: expected.clone(),
                    observed,
                    matches,
                });
        }
    }
    for (stage, observations) in stage_observations {
        record.observations.insert(stage.to_owned(), observations);
    }
    record.logs = logs(&stdout, &stderr, context.root);

    let violated: Vec<String> = record
        .invariants
        .iter()
        .filter(|i| !i.holds)
        .map(|i| format!("`{}` at {}: {}", i.id, i.stage, i.detail))
        .collect();
    let (level, reason) = if let Some(problem) = problem {
        (
            VerificationLevel::Inconclusive,
            safety::redact(&problem, context.root),
        )
    } else if timed_out {
        (
            VerificationLevel::Inconclusive,
            format!(
                "the harness did not finish within {} s and was killed",
                manifest.timeout_seconds
            ),
        )
    } else if let Some(spec) = inject.filter(|_| !record.injected) {
        (
            VerificationLevel::Inconclusive,
            format!(
                "checkpoint `{}` (occurrence {}) was never reached, so nothing was injected",
                spec.checkpoint, spec.occurrence
            ),
        )
    } else if inject.is_some_and(|i| i.mode == FailureMode::ProcessKill)
        && record.exit.signal != Some(9)
    {
        (
            VerificationLevel::Inconclusive,
            "the process was not terminated by the injected kill".to_owned(),
        )
    } else if inject.is_none() && record.outcome != "returned" {
        (
            VerificationLevel::Inconclusive,
            format!(
                "without injection the scenario did not complete normally ({}{})",
                record.outcome,
                record
                    .error
                    .as_ref()
                    .map(|e| format!(": {e}"))
                    .unwrap_or_default()
            ),
        )
    } else if let Err(error) = idle {
        (VerificationLevel::Inconclusive, error)
    } else if let Some(problem) = recovery_problem {
        (VerificationLevel::Inconclusive, problem)
    } else if !violated.is_empty() {
        (
            VerificationLevel::RuntimeReproduced,
            format!("declared invariant violated: {}", violated.join("; ")),
        )
    } else {
        (
            VerificationLevel::RuntimeNotReproduced,
            format!(
                "all {} declared invariant check(s) held",
                record.invariants.len()
            ),
        )
    };
    record.verification = level;
    finish(record, reason)
}

fn logs(stdout: &str, stderr: &str, root: &Path) -> Logs {
    let cut = |text: &str| -> (String, bool) {
        let text = safety::redact(text, root);
        if text.len() <= LOG_LIMIT {
            (text, false)
        } else {
            let mut end = LOG_LIMIT;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            (text[..end].to_owned(), true)
        }
    };
    let (stdout, a) = cut(stdout);
    let (stderr, b) = cut(stderr);
    Logs {
        stdout,
        stderr,
        truncated: a || b,
    }
}

/// For a manifest that declares its runtime: the entry point exists and the
/// runtime is installed in a supported version. Nothing of the harness runs.
fn preflight_runtime(harness: &LoadedHarness) -> Result<(), ReplayError> {
    let manifest = &harness.manifest;
    let Some(runtime) = manifest.runtime else {
        return Ok(());
    };
    let entry = manifest.entry_point().unwrap_or_default();
    if !harness.dir.join(entry).is_file() {
        return Err(ReplayError::Blocked(format!(
            "the harness entry point `{entry}` does not exist: compile the harness first (for example `npm ci && npm run build` in the harness directory); replay never installs packages or compiles code"
        )));
    }
    if std::fs::read_to_string(harness.dir.join(entry))
        .is_ok_and(|code| code.contains(manifest::PLACEHOLDER_MARKER))
    {
        return Err(ReplayError::Refused(format!(
            "the harness entry point `{entry}` still contains {} placeholders; implement the marked integration points (see the harness README) and rebuild",
            manifest::PLACEHOLDER_MARKER
        )));
    }
    let path = [(
        "PATH".to_owned(),
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_owned()),
    )];
    let Some(reported) = runtime_version(&manifest.command[0], &path) else {
        return Err(ReplayError::Blocked(format!(
            "the harness runtime `{}` was not found or did not report its version",
            manifest.command[0]
        )));
    };
    let (major, minor) = runtime.minimum();
    let line = reported
        .strip_prefix(manifest.command[0].as_str())
        .unwrap_or(&reported);
    match parse_version(line) {
        Some(found) if found >= (major, minor) => Ok(()),
        _ => Err(ReplayError::Blocked(format!(
            "{reported} is not supported; replay harnesses need {} {major}.{minor} or newer",
            runtime.program()
        ))),
    }
}

/// `(major, minor)` of the first version number in `text` (`node v22.23.2`,
/// `bun 1.3.3`).
pub fn parse_version(text: &str) -> Option<(u64, u64)> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let mut parts = text[start..]
        .split(|c: char| !c.is_ascii_digit())
        .map(str::parse::<u64>);
    let major = parts.next()?.ok()?;
    let minor = parts.next().and_then(Result::ok).unwrap_or(0);
    Some((major, minor))
}

fn runtime_version(program: &str, env: &[(String, String)]) -> Option<String> {
    let mut child = Command::new(program)
        .arg("--version")
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut text = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut text).ok()?;
    let line = text.lines().next()?.trim().to_owned();
    Some(format!("{program} {line}"))
}

fn source_info(dir: &Path) -> SourceInfo {
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    let revision = git(&["rev-parse", "HEAD"]);
    let dirty = revision
        .as_ref()
        .and_then(|_| git(&["status", "--porcelain", "--", "."]))
        .map(|status| !status.is_empty());
    SourceInfo { revision, dirty }
}

/// The harness directory as a `PathBuf`, for callers that only have the
/// manifest.
pub fn harness_dir(harness: &LoadedHarness) -> PathBuf {
    harness.dir.clone()
}

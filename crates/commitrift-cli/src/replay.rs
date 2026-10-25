//! `commitrift replay`: runs an authorized replay harness.

use std::fmt::Write as _;

use commitrift_analyzer::{Analyzer, display_path};
use commitrift_core::config::{Config, discover_config};
use commitrift_core::trace::TraceReport;
use commitrift_core::verification::{
    RegressionReport, VerificationLevel, VerificationReport, compare_with_baseline,
};
use commitrift_replay::{LoadedHarness, ReplayError, ReplayOptions};

use crate::{
    EXIT_FINDINGS, EXIT_INCOMPLETE, EXIT_OK, EXIT_USAGE, IrFormat, ReplayArgs, write_stdout,
};

/// Static analysis of the harness application (never executes it).
fn static_traces(harness: &LoadedHarness) -> Result<Option<TraceReport>, String> {
    let Some(analysis) = &harness.manifest.analysis else {
        return Ok(None);
    };
    let root = analysis.root_dir(&harness.dir)?;
    let config_path = match &analysis.config {
        Some(config) => Some(root.join(config)),
        None => discover_config(&root).map(|found| found.path),
    };
    let (config, label) = match &config_path {
        Some(path) => (
            Config::load(path).map_err(|e| e.to_string())?,
            path.strip_prefix(&root).ok().map(display_path),
        ),
        None => (Config::default(), None),
    };
    let paths: Vec<std::path::PathBuf> = analysis.paths.iter().map(Into::into).collect();
    let project = Analyzer::new(config)
        .trace_project_at(&root, &paths)
        .map_err(|e| e.to_string())?;
    if !project.failures.is_empty() {
        return Err(format!(
            "the harness application could not be analyzed: {}",
            project
                .failures
                .iter()
                .map(|f| format!("{}: {}", f.file, f.message))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    Ok(Some(project.into_trace_report(label)))
}

/// Read a verification report to compare with (schema version 1 or 2).
fn load_baseline(path: &std::path::Path) -> Result<VerificationReport, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read the baseline `{}`: {e}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("the baseline `{}` is not JSON: {e}", path.display()))?;
    if value["kind"].as_str() != Some(commitrift_core::verification::VERIFICATION_REPORT_KIND) {
        return Err(format!(
            "the baseline `{}` is not a verification report (`commitrift replay --format json`)",
            path.display()
        ));
    }
    serde_json::from_value(value).map_err(|e| format!("invalid baseline report: {e}"))
}

pub(crate) fn regression_text(report: &RegressionReport) -> String {
    let mut out = format!(
        "\nregression against the baseline (harness `{}`):\n",
        report.harness
    );
    for entry in &report.entries {
        let _ = writeln!(
            out,
            "  {:<22} {}  (baseline {}, now {}; {} of {} run(s) reproduced)",
            entry.status.as_str(),
            entry.key,
            entry.baseline.map_or("-", |l| l.as_str()),
            entry.current.map_or("-", |l| l.as_str()),
            entry.reproduced_runs,
            entry.runs
        );
    }
    let s = report.summary;
    let _ = writeln!(
        out,
        "{} case(s): {} still reproduced, {} no longer reproduced, {} newly reproduced, {} still not reproduced, {} inconclusive, {} not executed, {} not in the baseline.",
        s.cases,
        s.still_reproduced,
        s.no_longer_reproduced,
        s.newly_reproduced,
        s.still_not_reproduced,
        s.inconclusive,
        s.not_executed,
        s.not_in_baseline
    );
    out
}

pub(crate) fn replay(args: &ReplayArgs) -> Result<u8, String> {
    let baseline = args.baseline.as_deref().map(load_baseline).transpose()?;
    let harness = LoadedHarness::load(&args.harness)?;
    if let Some(baseline) = &baseline
        && baseline.harness.id != harness.manifest.harness
    {
        return Err(format!(
            "the baseline is a report of harness `{}`, not `{}`",
            baseline.harness.id, harness.manifest.harness
        ));
    }
    let traces = static_traces(&harness)?;
    let cases = commitrift_replay::select_cases(
        &harness.manifest,
        args.case.as_deref(),
        args.finding.as_deref(),
        traces.as_ref(),
    )?;
    if !args.allow_execution {
        let mut text = commitrift_replay::plan(&harness, &cases, traces.as_ref());
        text.push_str(
            "\nNothing was executed. Replay runs the harness's own code; review it, then re-run with --allow-execution.\n",
        );
        eprint!("{text}");
        return Ok(EXIT_USAGE);
    }
    let options = ReplayOptions {
        repeat: args.repeat,
        allow_network: args.allow_network,
        keep_artifacts: args.keep_artifacts,
    };
    let report = match commitrift_replay::run(&harness, &cases, traces.as_ref(), &options) {
        Ok(report) => report,
        Err(ReplayError::Blocked(reason)) => {
            eprintln!("error: replay blocked: {reason}");
            return Ok(EXIT_INCOMPLETE);
        }
        Err(ReplayError::Refused(reason)) => return Err(reason),
    };
    let json = {
        let mut json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
        json.push('\n');
        json
    };
    if let Some(path) = &args.output {
        std::fs::write(path, &json)
            .map_err(|e| format!("cannot write `{}`: {e}", path.display()))?;
    }
    let regression = baseline
        .as_ref()
        .map(|baseline| compare_with_baseline(baseline, &report));
    if let (Some(regression), Some(path)) = (&regression, &args.regression_output) {
        let mut json = serde_json::to_string_pretty(regression).map_err(|e| e.to_string())?;
        json.push('\n');
        std::fs::write(path, json)
            .map_err(|e| format!("cannot write `{}`: {e}", path.display()))?;
    }
    match args.format {
        IrFormat::Json => write_stdout(&json)?,
        IrFormat::Text => {
            let mut out = text(&report);
            if let Some(regression) = &regression {
                out.push_str(&regression_text(regression));
            }
            write_stdout(&out)?;
        }
    }
    if let Some(regression) = &regression {
        if regression.entries.iter().any(|e| e.status.is_change()) {
            return Ok(EXIT_FINDINGS);
        }
        if regression.entries.iter().any(|e| e.status.is_unverified()) {
            return Ok(EXIT_INCOMPLETE);
        }
    }
    let summary = report.summary;
    Ok(if summary.mismatched_expectations > 0 {
        EXIT_FINDINGS
    } else if report
        .cases
        .iter()
        .any(|c| c.verification == VerificationLevel::Inconclusive && c.expected.is_none())
    {
        EXIT_INCOMPLETE
    } else {
        EXIT_OK
    })
}

fn values(map: Option<&std::collections::BTreeMap<String, serde_json::Value>>) -> String {
    map.map(|m| {
        m.iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ")
    })
    .unwrap_or_else(|| "-".to_owned())
}

pub(crate) fn text(report: &VerificationReport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "commitrift replay: harness `{}` — {}",
        report.harness.id, report.harness.description
    );
    let _ = writeln!(
        out,
        "  environment: {} {}, {}, {}; network: {}",
        report.environment.os,
        report.environment.arch,
        report.environment.postgres.as_deref().unwrap_or("?"),
        report.environment.runtime.as_deref().unwrap_or("?"),
        report.environment.network_isolation
    );
    if let Some(control) = &report.control {
        let _ = writeln!(
            out,
            "  control run (no injection): {} — {}; state: {}",
            control.verification,
            control.reason,
            values(control.observations.get("after"))
        );
    }
    for case in &report.cases {
        let _ = writeln!(out);
        let checkpoint = case
            .checkpoint
            .as_ref()
            .map(|c| format!("{} at `{}`", case.failure_mode, c.name))
            .unwrap_or_else(|| "no injection".to_owned());
        let _ = writeln!(
            out,
            "case {}: {}  [{}{}]",
            case.id,
            checkpoint,
            case.verification,
            match case.matches_expectation {
                Some(true) => ", as expected",
                Some(false) => ", NOT AS EXPECTED",
                None => "",
            }
        );
        if !case.description.is_empty() {
            let _ = writeln!(out, "  {}", case.description);
        }
        let _ = writeln!(
            out,
            "  static link: {:?} — {}{}",
            case.static_link.status,
            case.static_link.detail,
            case.trace_id
                .as_ref()
                .map(|t| format!("; trace {t} ({:?})", case.boundary))
                .unwrap_or_default()
        );
        for run in &case.runs {
            let _ = writeln!(
                out,
                "  run {}: {} — {}",
                run.run, run.verification, run.reason
            );
            let _ = writeln!(
                out,
                "    injected={} outcome={} exit={}{}",
                run.injected,
                run.outcome,
                run.exit.kind,
                run.exit
                    .signal
                    .map(|s| format!(" (signal {s})"))
                    .or_else(|| run.exit.code.map(|c| format!(" (code {c})")))
                    .unwrap_or_default()
            );
            for check in &run.pre_injection_checks {
                let _ = writeln!(
                    out,
                    "    before injecting: {} -> {} (observed {})",
                    check.description,
                    if check.holds { "ok" } else { "FAILED" },
                    check.observed
                );
            }
            let _ = writeln!(
                out,
                "    before:        {}",
                values(run.observations.get("before"))
            );
            let _ = writeln!(
                out,
                "    after:         {}",
                values(run.observations.get("after"))
            );
            if run.observations.contains_key("afterRecovery") {
                let _ = writeln!(
                    out,
                    "    afterRecovery: {}",
                    values(run.observations.get("afterRecovery"))
                );
            }
            let mismatched: Vec<String> = run
                .prediction
                .iter()
                .filter(|p| !p.matches)
                .map(|p| {
                    format!(
                        "{} predicted {} observed {}",
                        p.observation, p.expected, p.observed
                    )
                })
                .collect();
            if !run.prediction.is_empty() {
                let _ = writeln!(
                    out,
                    "    prediction: {}",
                    if mismatched.is_empty() {
                        "matches the observed state".to_owned()
                    } else {
                        format!("differs: {}", mismatched.join(", "))
                    }
                );
            }
        }
        if let Some(observation) = &case.recovery_observation {
            let _ = writeln!(
                out,
                "  recovery (runtime observation): {}",
                observation.summary
            );
        }
        for limitation in &case.limitations {
            let _ = writeln!(out, "  note: {limitation}");
        }
    }
    let s = report.summary;
    let _ = writeln!(
        out,
        "\n{} case(s): {} RUNTIME_REPRODUCED, {} RUNTIME_NOT_REPRODUCED, {} INCONCLUSIVE; {} as expected, {} not as expected.",
        s.cases,
        s.reproduced,
        s.not_reproduced,
        s.inconclusive,
        s.matched_expectations,
        s.mismatched_expectations
    );
    let _ = writeln!(
        out,
        "Results describe this harness under the recorded conditions; RUNTIME_NOT_REPRODUCED does not prove the application safe."
    );
    out
}

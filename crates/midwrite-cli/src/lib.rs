//! The `midwrite` command-line interface.

mod baseline;
mod discover;
mod html;
#[cfg(feature = "replay")]
mod init_replay;
mod model_check;
mod render;
#[cfg(feature = "replay")]
mod replay;
mod schema;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

use midwrite_analyzer::{Analyzer, display_path};
use midwrite_core::analysis::AnalysisOptions;
use midwrite_core::config::{Config, discover_config};
use midwrite_core::diagnostic::{DiagnosticCode, Policy, Severity};

use baseline::Baseline;

/// Exit status when no diagnostic reaches the threshold and every file was analyzed.
const EXIT_OK: u8 = 0;
/// Exit status when at least one diagnostic is at or above `--fail-on`.
const EXIT_FINDINGS: u8 = 1;
/// Exit status for usage, configuration and input errors.
const EXIT_USAGE: u8 = 2;
/// Exit status when some files could not be read or parsed (and no
/// diagnostic reached the threshold).
const EXIT_INCOMPLETE: u8 = 3;

#[derive(Debug, Parser)]
#[command(
    name = "midwrite",
    version,
    about = "Failure-path analysis for TypeScript: finds partial-completion hazards between database commits and external side effects.",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Analyze TypeScript sources and report potential partial-failure hazards.
    Check(CheckArgs),
    /// Show documentation for a diagnostic code (or list all codes).
    Explain {
        /// Diagnostic code such as MW1001.
        code: Option<String>,
    },
    /// Print the Effect IR that Midwrite derives from TypeScript sources.
    Ir(IrArgs),
    /// Print the failure trace behind each finding: the ordered operations
    /// on the reported path and the failure-injection boundary. Static only:
    /// nothing is executed.
    Trace(TraceArgs),
    /// Render a trace report (`midwrite trace --format json`) or a
    /// verification report (`midwrite replay --format json`) as a
    /// self-contained, read-only HTML page.
    Report(ReportArgs),
    /// Bounded model checking of concurrent executions: explore the
    /// schedules of the actors a scenario declares (against a modeled
    /// PostgreSQL isolation level, with optional crashes and retries),
    /// report a counterexample to a declared invariant, and optionally replay
    /// it on a real PostgreSQL. Opt-in; `midwrite check` never does this.
    ModelCheck(model_check::ModelCheckArgs),
    /// Discover concurrency and failure-verification candidates in the
    /// original TypeScript: an entry and effect index, conflicting effects,
    /// property candidates with their origin, synthesized initial states and
    /// scenarios. Static: application code is never executed; model
    /// checking only with --model-check.
    Discover(discover::DiscoverArgs),
    /// Print the PostgreSQL schema facts read statically from a Drizzle
    /// schema module; with --migrations and --allow-execution, compare them
    /// with the catalog of a disposable database (schema drift).
    Schema(schema::SchemaArgs),
    /// Run an explicitly authorized replay harness: inject a failure at a
    /// named checkpoint of an instrumented test application, against a
    /// disposable PostgreSQL database, and report the observed state.
    /// Without --allow-execution only the plan is printed.
    #[cfg(feature = "replay")]
    Replay(ReplayArgs),
    /// Generate an editable replay harness skeleton for one finding from a
    /// trace report (`midwrite trace --format json`). Writes only into
    /// --output; executes and installs nothing. Project-specific parts are
    /// left as MIDWRITE-TODO markers that `midwrite replay` refuses to
    /// run.
    #[cfg(feature = "replay")]
    InitReplay(InitReplayArgs),
}

#[cfg(feature = "replay")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum RuntimeArg {
    /// Node.js 22+ running the harness compiled with tsc.
    Node,
    /// Bun 1.2+ running the TypeScript harness directly.
    Bun,
}

#[cfg(feature = "replay")]
#[derive(Debug, Args)]
struct InitReplayArgs {
    /// A trace report (`midwrite trace --format json`).
    #[arg(long, value_name = "FILE")]
    trace: PathBuf,
    /// The finding to replay: its diagnostic fingerprint or trace id.
    #[arg(long, value_name = "ID")]
    finding: String,
    /// The harness runtime.
    #[arg(long, value_enum, default_value_t = RuntimeArg::Node)]
    runtime: RuntimeArg,
    /// The directory to create, inside --project. Existing files are never
    /// overwritten without --force.
    #[arg(long, short, value_name = "DIR")]
    output: PathBuf,
    /// The directory `midwrite trace` ran in: the analysis root that the
    /// trace's file paths and fingerprints are relative to.
    #[arg(long, value_name = "DIR", default_value = ".")]
    project: PathBuf,
    /// Paths to analyze, relative to --project (repeatable). Default: the
    /// top-level directory of the finding's file.
    #[arg(long = "analyze", value_name = "PATH")]
    analyze: Vec<String>,
    /// Configuration file relative to --project. Default: the project's
    /// midwrite.json, if any.
    #[arg(long, value_name = "FILE")]
    config: Option<String>,
    /// Overwrite the generated files if they exist (other files in the
    /// directory are never touched).
    #[arg(long)]
    force: bool,
    /// Generate a skeleton even for an incomplete trace.
    #[arg(long)]
    allow_incomplete: bool,
}

#[cfg(feature = "replay")]
#[derive(Debug, Args)]
struct ReplayArgs {
    /// Harness directory containing midwrite-replay.json.
    harness: PathBuf,
    /// A case id, or a diagnostic code when exactly one case targets it.
    #[arg(long)]
    case: Option<String>,
    /// Only cases that target this finding (diagnostic fingerprint).
    #[arg(long, value_name = "ID")]
    finding: Option<String>,
    /// Execute the harness's own code. Required: replay never runs code
    /// unless asked to.
    #[arg(long)]
    allow_execution: bool,
    /// Run each case this many times, each from a fresh database.
    #[arg(long, default_value_t = 1)]
    repeat: u32,
    /// Run without network isolation (the manifest must also declare
    /// `"network": "allow"`).
    #[arg(long)]
    allow_network: bool,
    /// Keep the temporary database and logs for inspection.
    #[arg(long)]
    keep_artifacts: bool,
    /// Output format.
    #[arg(long, value_enum, default_value_t = IrFormat::Text)]
    format: IrFormat,
    /// Also write the JSON report to this file.
    #[arg(long, value_name = "FILE")]
    output: Option<PathBuf>,
    /// Compare every case with a previous verification report of the same
    /// harness. Exit status 1 when a case changed between reproduced and
    /// not reproduced, 3 when a case is inconclusive or was not executed.
    #[arg(long, value_name = "FILE")]
    baseline: Option<PathBuf>,
    /// Write the comparison with --baseline as JSON to this file.
    #[arg(long, value_name = "FILE", requires = "baseline")]
    regression_output: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ReportArgs {
    /// A JSON trace or verification report.
    input: PathBuf,
    /// Output format.
    #[arg(long, value_enum, default_value_t = ReportFormat::Html)]
    format: ReportFormat,
    /// Write to this file instead of standard output.
    #[arg(long, short, value_name = "FILE")]
    output: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ReportFormat {
    Html,
}

#[derive(Debug, Args)]
struct TraceArgs {
    /// Files or directories to analyze.
    #[arg(default_value = ".")]
    paths: Vec<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = IrFormat::Text)]
    format: IrFormat,
    /// Configuration file. Defaults to ./midwrite.json if it exists.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Only the trace of this finding (diagnostic fingerprint) or trace id.
    #[arg(long, value_name = "ID")]
    finding: Option<String>,
    #[arg(long, default_value_t = AnalysisOptions::default().max_paths)]
    max_paths: usize,
    #[arg(long, default_value_t = AnalysisOptions::default().max_call_depth)]
    max_call_depth: usize,
    #[arg(long, value_enum, default_value_t = PolicyArg::Recommended)]
    policy: PolicyArg,
}

#[derive(Debug, Args)]
struct CheckArgs {
    /// Files or directories to analyze.
    #[arg(default_value = ".")]
    paths: Vec<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
    /// Configuration file. Defaults to ./midwrite.json if it exists.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Lowest severity that makes the command exit with status 1.
    #[arg(long, value_enum, default_value_t = FailOn::Warning)]
    fail_on: FailOn,
    /// Maximum number of paths analyzed per function.
    #[arg(long, default_value_t = AnalysisOptions::default().max_paths)]
    max_paths: usize,
    /// How many levels of calls (to functions declared in the same file or
    /// imported from analyzed files) are inlined from their summaries
    /// (0 disables summaries).
    #[arg(long, default_value_t = AnalysisOptions::default().max_call_depth)]
    max_call_depth: usize,
    /// Which findings are reported and at which severity: `recommended`
    /// (cross-resource hazards and transaction bypasses are warnings; broad
    /// atomicity checks are info), `strict` (every hazard is a warning), or
    /// `audit` (strict, plus unknown calls before the first recognized
    /// operation).
    #[arg(long, value_enum, default_value_t = PolicyArg::Recommended)]
    policy: PolicyArg,
    /// Compare with a baseline file: findings recorded in it are still
    /// reported but do not count toward `--fail-on`.
    #[arg(long, value_name = "FILE")]
    baseline: Option<PathBuf>,
    /// Write every current finding to a baseline file. The run then exits
    /// with status 0 unless some file could not be analyzed.
    #[arg(long, value_name = "FILE")]
    write_baseline: Option<PathBuf>,
    /// Text output: also print evidence and general limitations.
    #[arg(long, short)]
    verbose: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum PolicyArg {
    Recommended,
    Strict,
    Audit,
}

impl From<PolicyArg> for Policy {
    fn from(value: PolicyArg) -> Self {
        match value {
            PolicyArg::Recommended => Self::Recommended,
            PolicyArg::Strict => Self::Strict,
            PolicyArg::Audit => Self::Audit,
        }
    }
}

#[derive(Debug, Args)]
struct IrArgs {
    /// Files or directories to lower.
    #[arg(default_value = ".")]
    paths: Vec<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = IrFormat::Text)]
    format: IrFormat,
    /// Configuration file. Defaults to ./midwrite.json if it exists.
    #[arg(long)]
    config: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Format {
    Text,
    Json,
    Sarif,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum IrFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum FailOn {
    Error,
    Warning,
    Info,
    /// Never fail because of diagnostics.
    Never,
}

impl FailOn {
    fn threshold(self) -> Option<Severity> {
        match self {
            Self::Error => Some(Severity::Error),
            Self::Warning => Some(Severity::Warning),
            Self::Info => Some(Severity::Info),
            Self::Never => None,
        }
    }
}

/// Parse the command line and run the selected command.
pub fn run() -> ExitCode {
    let cli = Cli::parse();
    if let Err(message) = midwrite_core::analysis::path_mode_from_env() {
        eprintln!("error: {message}");
        return ExitCode::from(EXIT_USAGE);
    }
    let result = match cli.command {
        Command::Check(args) => check(&args),
        Command::Explain { code } => explain(code.as_deref()),
        Command::Ir(args) => ir(&args),
        Command::Trace(args) => trace(&args),
        Command::Report(args) => report(&args),
        Command::ModelCheck(args) => model_check::model_check(&args),
        Command::Discover(args) => discover::discover(&args),
        Command::Schema(args) => schema::schema(&args),
        #[cfg(feature = "replay")]
        Command::Replay(args) => replay::replay(&args),
        #[cfg(feature = "replay")]
        Command::InitReplay(args) => init_replay::init_replay(&args),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

/// Load `--config`, else `./midwrite.json`, else defaults.
fn load_config(explicit: Option<&Path>) -> Result<(Config, Option<String>), String> {
    let path = match explicit {
        Some(path) => Some(path.to_path_buf()),
        None => discover_config(Path::new("")),
    };
    match path {
        Some(path) => {
            let config = Config::load(&path).map_err(|error| error.to_string())?;
            Ok((config, Some(display_path(&path))))
        }
        None => Ok((Config::default(), None)),
    }
}

fn check(args: &CheckArgs) -> Result<u8, String> {
    let (config, config_label) = load_config(args.config.as_deref())?;
    let analyzer = Analyzer::new(config).with_options(AnalysisOptions {
        max_paths: args.max_paths,
        max_call_depth: args.max_call_depth,
        policy: args.policy.into(),
    });
    let baseline = args.baseline.as_deref().map(Baseline::load).transpose()?;
    let mut report = analyzer
        .analyze_paths(&args.paths, config_label)
        .map_err(|error| error.to_string())?;
    if let Some(baseline) = &baseline {
        baseline.apply(&mut report);
    }
    if let Some(path) = &args.write_baseline {
        Baseline::from_report(&report).write(path)?;
    }

    let output = match args.format {
        Format::Text => render::text(&report, args.verbose),
        Format::Json => render::json(&report),
        Format::Sarif => render::sarif(&report),
    };
    write_stdout(&output)?;

    let failing = args.write_baseline.is_none()
        && args
            .fail_on
            .threshold()
            .is_some_and(|threshold| report.count_new_at_or_above(threshold) > 0);
    Ok(if failing {
        EXIT_FINDINGS
    } else if !report.failures.is_empty() {
        EXIT_INCOMPLETE
    } else {
        EXIT_OK
    })
}

fn explain(code: Option<&str>) -> Result<u8, String> {
    let text = match code.map(str::trim) {
        Some(model_code) if midwrite_model::report::explain(model_code).is_some() => {
            format!(
                "{}\n",
                midwrite_model::report::explain(model_code).unwrap_or_default()
            )
        }
        Some(code) => {
            let code: DiagnosticCode = code.parse().map_err(|error| {
                format!("{error}; run `midwrite explain` to list the known codes")
            })?;
            let info = code.info();
            format!(
                "{}\n\nDefault severity: {}\n",
                info.explanation, info.severity
            )
        }
        None => {
            let mut text = String::from("Diagnostic codes:\n");
            for code in DiagnosticCode::ALL {
                let info = code.info();
                text.push_str(&format!(
                    "  {}  {:<7}  {}\n",
                    info.code,
                    info.severity.as_str(),
                    info.title
                ));
            }
            text.push_str("\nConcurrency model checking (`midwrite model-check`):\n");
            for code in ["MW7001", "MW7002", "MW7003", "MW7101", "MW7102", "MW7103"] {
                text.push_str(&format!("  {code}\n"));
            }
            text.push_str("\nRun `midwrite explain <CODE>` for details.\n");
            text
        }
    };
    write_stdout(&text)?;
    Ok(EXIT_OK)
}

fn ir(args: &IrArgs) -> Result<u8, String> {
    let (config, _) = load_config(args.config.as_deref())?;
    let analyzer = Analyzer::new(config);
    let project = analyzer
        .analyze_project(&args.paths)
        .map_err(|error| error.to_string())?;
    let failed = !project.failures.is_empty();
    for failure in &project.failures {
        let position = match (failure.line, failure.column) {
            (Some(line), Some(column)) => format!(":{line}:{column}"),
            _ => String::new(),
        };
        let what = match failure.kind {
            midwrite_core::report::FailureKind::Read => "read",
            midwrite_core::report::FailureKind::Parse => "parse",
        };
        eprintln!(
            "error: failed to {what} {}{position}: {}",
            failure.file, failure.message
        );
    }
    let lowered: Vec<_> = project.files.into_iter().map(|file| file.ir).collect();
    let output = match args.format {
        IrFormat::Text => lowered
            .iter()
            .map(|file| file.to_text())
            .collect::<Vec<_>>()
            .join("\n"),
        IrFormat::Json => {
            let mut json = serde_json::to_string_pretty(&lowered).map_err(|e| e.to_string())?;
            json.push('\n');
            json
        }
    };
    write_stdout(&output)?;
    Ok(if failed { EXIT_INCOMPLETE } else { EXIT_OK })
}

fn trace(args: &TraceArgs) -> Result<u8, String> {
    let (config, config_label) = load_config(args.config.as_deref())?;
    let analyzer = Analyzer::new(config).with_options(AnalysisOptions {
        max_paths: args.max_paths,
        max_call_depth: args.max_call_depth,
        policy: args.policy.into(),
    });
    let project = analyzer
        .trace_project(&args.paths)
        .map_err(|error| error.to_string())?;
    let failed = !project.failures.is_empty();
    let mut report = project.into_trace_report(config_label);
    if let Some(id) = &args.finding {
        report
            .traces
            .retain(|trace| &trace.finding_id == id || &trace.id == id);
        report.skipped.retain(|skipped| &skipped.finding_id == id);
        if report.traces.is_empty() && report.skipped.is_empty() {
            return Err(format!("no finding or trace with id `{id}`"));
        }
        report =
            midwrite_core::trace::TraceReport::new(report.config, report.traces, report.skipped);
    }
    let output = match args.format {
        IrFormat::Json => {
            let mut json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
            json.push('\n');
            json
        }
        IrFormat::Text => {
            let mut text = String::new();
            for trace in &report.traces {
                text.push_str(&trace.to_text());
                text.push('\n');
            }
            for skipped in &report.skipped {
                text.push_str(&format!(
                    "{} {} {} in `{}` ({}): no trace: {}\n",
                    skipped.finding_id,
                    skipped.code,
                    skipped.location,
                    skipped.function,
                    skipped.location.file,
                    skipped.reason
                ));
            }
            let s = report.summary;
            text.push_str(&format!(
                "midwrite: {} finding(s): {} trace(s) ({} complete, {} approximated, {} incomplete), {} without a trace. Traces are static candidates, not reproduced failures.\n",
                s.findings, s.traces, s.complete, s.approximated, s.incomplete, s.skipped
            ));
            text
        }
    };
    write_stdout(&output)?;
    Ok(if failed { EXIT_INCOMPLETE } else { EXIT_OK })
}

fn report(args: &ReportArgs) -> Result<u8, String> {
    let text = std::fs::read_to_string(&args.input)
        .map_err(|e| format!("cannot read `{}`: {e}", args.input.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("`{}` is not JSON: {e}", args.input.display()))?;
    let html = match (args.format, value["kind"].as_str()) {
        (ReportFormat::Html, Some(midwrite_core::trace::TRACE_REPORT_KIND)) => {
            let report: midwrite_core::trace::TraceReport =
                serde_json::from_value(value).map_err(|e| format!("invalid trace report: {e}"))?;
            html::traces_html(&report)
        }
        (ReportFormat::Html, Some(midwrite_core::verification::VERIFICATION_REPORT_KIND)) => {
            let report: midwrite_core::verification::VerificationReport =
                serde_json::from_value(value)
                    .map_err(|e| format!("invalid verification report: {e}"))?;
            html::verification_html(&report)
        }
        (ReportFormat::Html, Some(midwrite_model::report::REPORT_KIND)) => {
            let report: midwrite_model::report::ModelCheckReport = serde_json::from_value(value)
                .map_err(|e| format!("invalid model-check report: {e}"))?;
            midwrite_model::html::model_check_html(&report)
        }
        _ => {
            return Err(format!(
                "`{}` is neither a trace report (`midwrite trace --format json`), a verification report (`midwrite replay --format json`) nor a model-check report (`midwrite model-check --format json`)",
                args.input.display()
            ));
        }
    };
    match &args.output {
        Some(path) => std::fs::write(path, html)
            .map_err(|e| format!("cannot write `{}`: {e}", path.display()))?,
        None => write_stdout(&html)?,
    }
    Ok(EXIT_OK)
}

fn write_stdout(text: &str) -> Result<(), String> {
    let mut stdout = io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => Ok(()),
        // A closed pipe (e.g. `| head`) is not an error worth reporting.
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(format!("failed to write output: {error}")),
    }
}

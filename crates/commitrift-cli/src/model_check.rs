//! `commitrift model-check`: bounded model checking of a scenario.

use std::io::Write as _;
use std::path::PathBuf;

use clap::{Args, ValueEnum};

use commitrift_analyzer::TsActorProvider;
use commitrift_model::model::Strategy;
use commitrift_model::provider::{ActorProvider, ExtractRequest, Extracted};
use commitrift_model::report::{CheckOptions, SourceOptions, check, render_text};
use commitrift_model::scenario::load_file;

use crate::{EXIT_FINDINGS, EXIT_INCOMPLETE, EXIT_OK};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum McFormat {
    Text,
    Json,
    Html,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum FaultMode {
    Any,
    InFlight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum StrategyArg {
    Bfs,
    Dfs,
}

#[derive(Debug, Args)]
pub struct ModelCheckArgs {
    /// A scenario file (JSON; see docs/phase6/INVARIANTS.md).
    pub scenario: PathBuf,
    /// Output format.
    #[arg(long, value_enum, default_value_t = McFormat::Text)]
    pub format: McFormat,
    /// Write to this file instead of standard output.
    #[arg(long, short, value_name = "FILE")]
    pub output: Option<PathBuf>,
    /// Replay the counterexample on a real PostgreSQL (a private, disposable
    /// cluster; needs the PostgreSQL server binaries). Without a
    /// counterexample nothing is run.
    #[arg(long)]
    pub validate_postgres: bool,
    /// Disable partial-order reduction (explore every interleaving).
    #[arg(long)]
    pub no_por: bool,
    /// Search strategy when reduction is off (reduction implies depth-first).
    #[arg(long, value_enum)]
    pub strategy: Option<StrategyArg>,
    /// Report the first counterexample found without minimizing it.
    #[arg(long)]
    pub no_minimize: bool,
    /// Omit wall-clock timings so that the output is byte-for-byte
    /// reproducible.
    #[arg(long)]
    pub no_timing: bool,
    /// Override bounds.maxSteps.
    #[arg(long)]
    pub max_steps: Option<u32>,
    /// Override bounds.maxStates.
    #[arg(long)]
    pub max_states: Option<u64>,
    /// Override bounds.maxWallMs.
    #[arg(long)]
    pub max_wall_ms: Option<u64>,
    /// Override bounds.maxInFlight (operations whose acknowledgment is lost
    /// in flight, Phase 9).
    #[arg(long)]
    pub max_in_flight: Option<u32>,
    /// Which crashes are explored: `any` (every crash point, plus in-flight
    /// ones when bounds.maxInFlight is set) or `in-flight` (a process only
    /// crashes with a COMMIT or an autocommit write submitted and not
    /// acknowledged; needs bounds.maxInFlight >= 1 and bounds.maxCrashes >=
    /// 1). Restricts the explored crash points.
    #[arg(long, value_enum)]
    pub fault_mode: Option<FaultMode>,
    /// Revision of the analyzed project to record in the report.
    #[arg(long, value_name = "REV")]
    pub source_revision: Option<String>,
    /// Text output: also list every modeling assumption.
    #[arg(long, short)]
    pub verbose: bool,
    /// Run the counterexample against the actors' ORIGINAL application code
    /// (an authorized source harness, the scenario's `sourceVerification`
    /// section). Executes code: requires --allow-execution.
    #[arg(long)]
    pub verify_source: bool,
    /// The source-harness manifest (default: `sourceVerification.harness`
    /// in the scenario, relative to it).
    #[arg(long, value_name = "MANIFEST", requires = "verify_source")]
    pub harness: Option<PathBuf>,
    /// Authorize executing the application code named by the harness.
    /// --validate-postgres never runs application code; this flag is the
    /// only way to.
    #[arg(long)]
    pub allow_execution: bool,
    /// Run the original code this many times, each on a fresh database.
    #[arg(
        long,
        value_name = "N",
        default_value_t = 1,
        requires = "verify_source"
    )]
    pub source_repeat: u32,
}

/// Extract functions from the project each request names (the scenario's
/// `extraction.root`), then link every extracted operation to the Effect IR
/// effect and the existing findings at the same source location.
struct ScenarioProvider;

impl ActorProvider for ScenarioProvider {
    fn extract_schema(
        &self,
        root: &std::path::Path,
        file: &str,
    ) -> Result<commitrift_model::schema_facts::ExtractedSchema, String> {
        commitrift_analyzer::schema::extract_schema(root, file)
    }

    fn extract(&self, request: &ExtractRequest) -> Result<Extracted, String> {
        let mut extracted = TsActorProvider::new(request.root.clone()).extract(request)?;
        if let Err(why) = link_effects(&request.root, &mut extracted) {
            extracted.assumptions.push(format!(
                "operations are not linked to Effect IR effects or existing findings: {why}"
            ));
        }
        Ok(extracted)
    }
}

/// Fill `effect_id` and `diagnostics` of extracted operations from a normal
/// static analysis of the project (`commitrift check` semantics, default
/// options). Linking is by exact (file, line, column); nothing is inferred
/// from names. A failed analysis leaves the links empty and says why.
fn link_effects(root: &std::path::Path, extracted: &mut Extracted) -> Result<(), String> {
    use std::collections::BTreeMap;
    let config_path = root.join("commitrift.json");
    let config = if config_path.is_file() {
        commitrift_core::config::Config::load(&config_path).map_err(|e| e.to_string())?
    } else {
        commitrift_core::config::Config::default()
    };
    let project = commitrift_analyzer::Analyzer::new(config)
        .analyze_project(&[root.to_path_buf()])
        .map_err(|e| e.to_string())?;
    let rel = |p: &str| -> String {
        let path = std::path::Path::new(p);
        path.strip_prefix(root)
            .map(|r| r.to_string_lossy().into_owned())
            .unwrap_or_else(|_| p.to_owned())
    };
    // (file, line, column) -> (function, effect id)
    let mut effects: BTreeMap<(String, u32, u32), (String, u32)> = BTreeMap::new();
    let mut findings: BTreeMap<(String, u32, u32), Vec<String>> = BTreeMap::new();
    for file in &project.files {
        let path = rel(&file.ir.path);
        for f in &file.ir.functions {
            for e in f.effects() {
                if e.summary.is_none() && e.location.file.is_none() {
                    effects
                        .entry((path.clone(), e.location.line, e.location.column))
                        .or_insert((f.name.clone(), e.id.0));
                }
            }
        }
        for d in &file.diagnostics {
            for re in &d.effects {
                let key = (rel(&re.location.file), re.location.line, re.location.column);
                let id = format!("{}@{}", d.code.as_str(), d.fingerprint);
                let list = findings.entry(key).or_default();
                if !list.contains(&id) {
                    list.push(id);
                }
            }
        }
    }
    for step in extracted.codes.iter_mut().flatten() {
        let key = (step.src.file.clone(), step.src.line, step.src.column);
        if let Some((function, id)) = effects.get(&key)
            && *function == step.src.function
        {
            step.src.effect_id = Some(*id);
        }
        if let Some(list) = findings.get(&key) {
            step.src.diagnostics = list.clone();
        }
    }
    Ok(())
}

pub fn model_check(args: &ModelCheckArgs) -> Result<u8, String> {
    let (mut model, load) = load_file(&args.scenario, &ScenarioProvider)
        .map_err(|e| format!("invalid scenario: {e}"))?;
    if args.no_por {
        model.options.por = false;
    }
    if let Some(s) = args.strategy {
        model.options.strategy = match s {
            StrategyArg::Bfs => Strategy::Bfs,
            StrategyArg::Dfs => Strategy::Dfs,
        };
    }
    if args.no_minimize {
        model.options.minimize = false;
    }
    if let Some(v) = args.max_steps {
        model.bounds.max_steps = v;
    }
    if let Some(v) = args.max_states {
        model.bounds.max_states = v;
    }
    if let Some(v) = args.max_wall_ms {
        model.bounds.max_wall_ms = Some(v);
    }
    if let Some(v) = args.max_in_flight {
        model.bounds.max_in_flight = v;
    }
    if let Some(mode) = args.fault_mode {
        model.bounds.crash_policy = match mode {
            FaultMode::Any => commitrift_model::model::CrashPolicy::Any,
            FaultMode::InFlight => commitrift_model::model::CrashPolicy::InFlightOnly,
        };
    }
    if model.bounds.crash_policy == commitrift_model::model::CrashPolicy::InFlightOnly
        && (model.bounds.max_in_flight == 0 || model.bounds.max_crashes == 0)
    {
        return Err(
            "the in-flight fault mode needs bounds.maxInFlight >= 1 and bounds.maxCrashes >= 1 (nothing could crash otherwise)"
                .to_owned(),
        );
    }
    let source = if args.verify_source {
        let Some(spec) = load.source.clone() else {
            return Err(
                "--verify-source needs the scenario's `sourceVerification` section (how each actor's original code is called)"
                    .to_owned(),
            );
        };
        let harness = match (&args.harness, &spec.harness) {
            (Some(path), _) => path.clone(),
            (None, Some(rel)) => args
                .scenario
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join(rel),
            (None, None) => {
                return Err(
                    "--verify-source needs --harness or `sourceVerification.harness`".to_owned(),
                );
            }
        };
        if args.source_repeat == 0 || args.source_repeat > 20 {
            return Err("--source-repeat must be between 1 and 20".to_owned());
        }
        Some(SourceOptions {
            harness,
            spec,
            allow_execution: args.allow_execution,
            repeat: args.source_repeat,
        })
    } else {
        None
    };
    let options = CheckOptions {
        validate_postgres: args.validate_postgres,
        include_timing: !args.no_timing,
        scenario_file: Some(args.scenario.display().to_string()),
        source_revision: args.source_revision.clone(),
        tool_version: env!("CARGO_PKG_VERSION").to_owned(),
        load,
        source,
    };
    let report = check(&model, &options);
    let output = match args.format {
        McFormat::Text => render_text(&model, &report, args.verbose),
        McFormat::Json => report.to_json(),
        McFormat::Html => commitrift_model::html::model_check_html(&report),
    };
    match &args.output {
        Some(path) => std::fs::write(path, output)
            .map_err(|e| format!("cannot write `{}`: {e}", path.display()))?,
        None => {
            let mut stdout = std::io::stdout().lock();
            if let Err(e) = stdout
                .write_all(output.as_bytes())
                .and_then(|()| stdout.flush())
                && e.kind() != std::io::ErrorKind::BrokenPipe
            {
                return Err(format!("cannot write to standard output: {e}"));
            }
        }
    }
    Ok(match report.exploration.verdict.as_str() {
        "COUNTEREXAMPLE_FOUND" => EXIT_FINDINGS,
        "NO_COUNTEREXAMPLE_WITHIN_BOUNDS" => EXIT_OK,
        _ => EXIT_INCOMPLETE,
    })
}

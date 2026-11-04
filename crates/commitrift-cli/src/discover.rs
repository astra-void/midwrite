//! `commitrift discover`: automatic scenario synthesis (Phase 11).

use std::io::Write as _;
use std::path::PathBuf;

use clap::{Args, ValueEnum};

use commitrift_discover::pipeline::{Options, discover as run_discovery};
use commitrift_discover::report::{TextOptions, render_html, render_text_with};

use crate::model_check::SourceDporArg;
use crate::{EXIT_FINDINGS, EXIT_OK};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum DiscoverFormat {
    Text,
    Json,
    Html,
}

#[derive(Debug, Args)]
pub struct DiscoverArgs {
    /// Project root.
    pub root: PathBuf,
    /// Verification contracts (default: `commitrift-contracts.json` in the
    /// root, if present).
    #[arg(long, value_name = "FILE")]
    pub contracts: Option<PathBuf>,
    /// Model-check the synthesized scenarios (bounded; budgets below).
    /// Without it nothing is explored.
    #[arg(long)]
    pub model_check: bool,
    /// Write every synthesized scenario into this directory (scenario
    /// format of `commitrift model-check`).
    #[arg(long, value_name = "DIR")]
    pub emit_scenarios: Option<PathBuf>,
    /// Also build HYPOTHETICAL variants (a schema the application does not
    /// have). Their results are never findings about the application.
    #[arg(long)]
    pub hypothetical: bool,
    /// Use source-set DPOR for model checking (sleep sets are the default).
    #[arg(
        long,
        value_enum,
        value_name = "MODE",
        num_args = 0..=1,
        default_missing_value = "stateful-sleep"
    )]
    pub source_dpor: Option<SourceDporArg>,
    /// Maximum number of candidates for which scenarios are built; the rest
    /// are reported as static candidates.
    #[arg(long, default_value_t = 64)]
    pub max_candidates: usize,
    /// Maximum number of other entries paired with one candidate through a
    /// shared resource; the rest are counted as truncated.
    #[arg(long, default_value_t = 6)]
    pub max_partners: usize,
    /// Maximum distinct states per scenario.
    #[arg(long, default_value_t = 200_000)]
    pub max_states: u64,
    /// Maximum wall-clock time per scenario, in milliseconds.
    #[arg(long, default_value_t = 60_000)]
    pub max_wall_ms: u64,
    /// Maximum approximate memory of one scenario's visited states, in bytes.
    #[arg(long)]
    pub max_memory_bytes: Option<u64>,
    /// Maximum distinct states over all scenarios; later scenarios are
    /// skipped and counted.
    #[arg(long, default_value_t = 5_000_000)]
    pub max_total_states: u64,
    /// Initial worlds (database rows and actor arguments) tried per actual
    /// variant, the synthesized one included. Further worlds change one
    /// input relationship each (a nullable argument set to null, a stored
    /// value distinct from the written one, a branch boundary...). 1 turns
    /// the search off.
    #[arg(long, default_value_t = commitrift_discover::pipeline::DEFAULT_MAX_WORLDS, value_name = "N")]
    pub max_worlds: usize,
    /// Keep exploring initial worlds after a validated counterexample.
    #[arg(long)]
    pub all_worlds: bool,
    /// Text output: list every explored and unexplored initial world.
    #[arg(long)]
    pub explain_coverage: bool,
    /// Only entries whose selector contains this text (repeatable).
    #[arg(long = "entry", value_name = "TEXT")]
    pub entries: Vec<String>,
    /// Score the report against labeled entries (a labels file such as
    /// fixtures/phase11/labels.json); the scores are added to the report.
    #[arg(long, value_name = "FILE")]
    pub labels: Option<PathBuf>,
    /// Cache extractions in this file (keyed by everything they read).
    #[arg(long, value_name = "FILE")]
    pub cache: Option<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = DiscoverFormat::Text)]
    pub format: DiscoverFormat,
    /// Write to this file instead of standard output.
    #[arg(long, short, value_name = "FILE")]
    pub output: Option<PathBuf>,
    /// Omit timings so that the output is reproducible byte for byte.
    #[arg(long)]
    pub no_timing: bool,
    /// Text output: also list assumptions and rejected combinations.
    #[arg(long, short)]
    pub verbose: bool,
}

pub fn discover(args: &DiscoverArgs) -> Result<u8, String> {
    use commitrift_model::model::SourceDpor as M;
    let options = Options {
        contracts: args.contracts.clone(),
        model_check: args.model_check,
        emit_dir: args.emit_scenarios.clone(),
        hypothetical: args.hypothetical,
        source_dpor: args.source_dpor.map(|m| match m {
            SourceDporArg::StatefulSleep => M::StatefulSleep,
            SourceDporArg::Stateful => M::Stateful,
            SourceDporArg::StatelessSleep => M::StatelessSleep,
        }),
        max_candidates: args.max_candidates,
        max_partners: args.max_partners,
        max_states: args.max_states,
        max_wall_ms: Some(args.max_wall_ms),
        max_memory_bytes: args.max_memory_bytes,
        max_total_states: args.max_total_states,
        entries: args.entries.clone(),
        include_timing: !args.no_timing,
        cache: args.cache.clone(),
        tool_version: env!("CARGO_PKG_VERSION").to_owned(),
        max_worlds: args.max_worlds.max(1),
        all_worlds: args.all_worlds,
    };
    let mut report = run_discovery(&args.root, &options)?;
    if let Some(path) = &args.labels {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;
        let labels: commitrift_discover::eval::Labels =
            serde_json::from_str(&text).map_err(|e| format!("`{}`: {e}", path.display()))?;
        report.evaluation = Some(commitrift_discover::eval::evaluate(&report, &labels));
    }
    let output = match args.format {
        DiscoverFormat::Text => render_text_with(
            &report,
            &TextOptions {
                verbose: args.verbose,
                explain_coverage: args.explain_coverage,
                emit_dir: args
                    .emit_scenarios
                    .as_ref()
                    .map(|d| d.display().to_string()),
            },
        ),
        DiscoverFormat::Json => report.to_json(),
        DiscoverFormat::Html => render_html(&report),
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
    Ok(if report.summary.model_counterexamples > 0 {
        EXIT_FINDINGS
    } else {
        EXIT_OK
    })
}

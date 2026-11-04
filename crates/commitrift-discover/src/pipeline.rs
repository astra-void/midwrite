//! The discovery pipeline: index → extraction → effect summaries →
//! candidates → (scenarios → model checking) → ranking → report.
//!
//! Discovery is static. Building scenarios runs the model's engine on the
//! extracted code (no application code). Model checking is a separate,
//! budgeted step that runs only when asked.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use commitrift_analyzer::entries::EntryKind;
use commitrift_model::ir::Step;
use commitrift_model::model::SourceDpor;
use commitrift_model::provider::{EntryInfo, Extracted, InvocationContract};
use commitrift_model::report::{CheckOptions, check};

use crate::candidate::{Candidate, Origin};
use crate::contracts::{CONTRACTS_FILE, Contracts};
use crate::detect::{Analyzed, detect};
use crate::effects::{Access, summarize};
use crate::extract::{MemoProvider, extract_entry};
use crate::project::{ProjectIndex, index};
use crate::report::{
    BranchCoverage, Budgets, CandidateReport, Coverage, DimensionCoverage, DiscoveryReport,
    EntryReport, IndexStats, InvariantReport, ModelResult, ProjectInfo, REPORT_KIND,
    REPORT_SCHEMA_VERSION, ScenarioResult, ScheduleCoverage, SourceCoverage, Summary, Timings,
    Tool, Validation, WorldInfo, WorldOutcome,
};
use crate::synth::{self, ScenarioBudget, VariantKind};
use crate::worlds::{Observations, WorldOrigin, WorldSpec};

#[derive(Clone, Debug)]
pub struct Options {
    /// `commitrift-contracts.json` (default: the project root's, if any).
    pub contracts: Option<PathBuf>,
    pub model_check: bool,
    /// Write every built scenario into this directory.
    pub emit_dir: Option<PathBuf>,
    /// Also build hypothetical variants (a schema the application does not
    /// have). Never findings.
    pub hypothetical: bool,
    pub source_dpor: Option<SourceDpor>,
    pub max_candidates: usize,
    pub max_partners: usize,
    pub max_states: u64,
    pub max_wall_ms: Option<u64>,
    pub max_memory_bytes: Option<u64>,
    pub max_total_states: u64,
    /// Only entries whose selector contains one of these.
    pub entries: Vec<String>,
    pub include_timing: bool,
    /// A cache file for extractions (keyed by everything they read).
    pub cache: Option<PathBuf>,
    pub tool_version: String,
    /// Phase 15: initial worlds tried per actual variant (the base world
    /// included); 1 turns world search off (the Phase 14 behavior).
    pub max_worlds: usize,
    /// Phase 15: keep exploring worlds after a validated counterexample.
    pub all_worlds: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            contracts: None,
            model_check: false,
            emit_dir: None,
            hypothetical: false,
            source_dpor: None,
            max_candidates: 64,
            max_partners: 6,
            max_states: 200_000,
            max_wall_ms: Some(60_000),
            max_memory_bytes: None,
            max_total_states: 5_000_000,
            entries: Vec::new(),
            include_timing: true,
            cache: None,
            tool_version: String::new(),
            max_worlds: DEFAULT_MAX_WORLDS,
            all_worlds: false,
        }
    }
}

/// Phase 15: the default world budget per actual variant.
pub const DEFAULT_MAX_WORLDS: usize = 8;

fn ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

// ---------------------------------------------------------------------------
// Extraction cache
// ---------------------------------------------------------------------------

/// Bumped whenever extraction or this cache's format changes.
const CACHE_VERSION: &str = "commitrift-discover-cache-4";

#[derive(Clone, Serialize, Deserialize)]
struct CachedExtraction {
    params: Vec<String>,
    codes: Vec<Vec<Step>>,
    assumptions: Vec<String>,
    entry: Option<EntryInfo>,
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    version: String,
    entries: BTreeMap<String, Result<CachedExtraction, String>>,
}

fn file_digest(root: &Path, rel: &str) -> String {
    let text = std::fs::read(root.join(rel)).unwrap_or_default();
    format!(
        "{:016x}",
        commitrift_core::diagnostic::stable_hash(&[&String::from_utf8_lossy(&text)])
    )
}

/// Everything an extraction of `selector` can read: the files of its import
/// closure, project configuration that changes resolution or receivers,
/// the table and external bindings, transaction helpers, the contract.
/// Per-run memo of what cache keys read (Phase 12): each file is digested
/// once, each module's import closure computed once — before, every entry
/// re-read and re-hashed its whole closure, which on large projects cost
/// more than the extraction the cache saves.
#[derive(Default)]
struct KeyMemo {
    digests: BTreeMap<String, String>,
    closures: BTreeMap<String, BTreeSet<String>>,
}

fn cache_key(
    index: &ProjectIndex,
    selector: &str,
    contract: &Option<InvocationContract>,
    config_digest: &str,
    tool: &str,
    memo: &mut KeyMemo,
) -> String {
    let file = selector.split('#').next().unwrap_or_default();
    let reachable = memo
        .closures
        .entry(file.to_owned())
        .or_insert_with(|| index.import_closure(file))
        .clone();
    let closure: Vec<String> = reachable
        .iter()
        .map(|f| {
            let digest = memo
                .digests
                .entry(f.clone())
                .or_insert_with(|| file_digest(&index.root, f));
            format!("{f}={digest}")
        })
        .collect();
    // Phase 12: tables bound by declaration can only be reached through the
    // entry's imports, so the key covers those (a change to another schema
    // module does not invalidate this entry). Name-bound tables (one schema
    // module) keep the Phase 11 key: every binding.
    let tables: Vec<_> = index
        .bindings
        .tables
        .iter()
        .filter(|t| t.source.as_ref().is_none_or(|(f, _)| reachable.contains(f)))
        .collect();
    // Phase 13: whether the project builds a replicated database depends on
    // every module (not only the closure), so the flag itself is keyed.
    let bindings = format!("{tables:?}")
        + &format!("{:?}", index.bindings.externals)
        + &format!("{:?}", index.bindings.transaction_retries)
        + &format!(
            "replicas={} {:?} js-values={}",
            index.bindings.replicas, index.bindings.replica_factories, index.bindings.js_values
        );
    format!(
        "{:016x}",
        commitrift_core::diagnostic::stable_hash(&[
            CACHE_VERSION,
            tool,
            selector,
            &format!("{contract:?}"),
            &closure.join(","),
            &bindings,
            config_digest,
            &format!("{:?}", index.receivers),
        ])
    )
}

fn config_digest(root: &Path) -> String {
    // tsconfig.json / package.json change resolution; commitrift.json the
    // database receivers.
    let mut parts = Vec::new();
    for entry in walk_configs(root) {
        parts.push(format!("{entry}={}", file_digest(root, &entry)));
    }
    parts.join(",")
}

fn walk_configs(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut todo = vec![root.to_path_buf()];
    while let Some(dir) = todo.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut items: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        items.sort();
        for p in items {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if p.is_dir() {
                if !commitrift_analyzer::DEFAULT_EXCLUDED_DIRS.contains(&name)
                    && !name.starts_with('.')
                {
                    todo.push(p);
                }
            } else if matches!(name, "tsconfig.json" | "package.json" | "commitrift.json") {
                out.push(
                    p.strip_prefix(root)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// The pipeline
// ---------------------------------------------------------------------------

pub fn discover(root: &Path, opts: &Options) -> Result<DiscoveryReport, String> {
    let total = Instant::now();
    let root = root
        .canonicalize()
        .map_err(|e| format!("cannot open `{}`: {e}", root.display()))?;
    let contracts_path = opts
        .contracts
        .clone()
        .or_else(|| Some(root.join(CONTRACTS_FILE)).filter(|p| p.is_file()));
    let contracts = match &contracts_path {
        Some(p) => Contracts::load(p)?,
        None => Contracts::default(),
    };
    let provider = MemoProvider::new(&root);
    let t = Instant::now();
    let index = index(&root, contracts, &provider)?;
    let index_ms = ms(t);

    // -- extraction ---------------------------------------------------------
    let t = Instant::now();
    let mut cache: Cache = opts
        .cache
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(|c: &Cache| c.version == CACHE_VERSION)
        .unwrap_or_default();
    cache.version = CACHE_VERSION.to_owned();
    let cfg = if opts.cache.is_some() {
        config_digest(&root)
    } else {
        String::new()
    };
    let generated_columns = index
        .tables
        .iter()
        .flat_map(|t| {
            let source = index.source_table(&t.name);
            t.columns.iter().filter_map(move |c| {
                let g = source?.column(c)?.generation.as_ref()?;
                let generator = g.fresh_identifier()?;
                let side = match g.class {
                    commitrift_model::schema_facts::GeneratorClass::ApplicationGenerated => {
                        "application"
                    }
                    _ => "database",
                };
                Some((
                    format!("{}.{c}", t.name),
                    side.to_owned(),
                    generator.to_owned(),
                ))
            })
        })
        .collect();
    let mut stats = IndexStats {
        tables_modeled: index.tables.len(),
        tables_refused: index.refused_tables.clone(),
        generated_columns,
        transaction_helpers: index.helpers.clone(),
        ..IndexStats::default()
    };
    // Files whose text mentions a receiver or a declared external callee.
    let mut words: Vec<String> = index.receivers.clone();
    words.extend(
        index
            .contracts
            .externals
            .iter()
            .map(|e| e.callee.rsplit('.').next().unwrap_or(&e.callee).to_owned()),
    );
    let mentions: BTreeSet<String> = index
        .modules
        .iter()
        .filter(|m| {
            let text = std::fs::read_to_string(root.join(&m.file)).unwrap_or_default();
            words.iter().any(|w| contains_word(&text, w))
        })
        .map(|m| m.file.clone())
        .collect();
    let mut analyzed: Vec<Analyzed> = Vec::new();
    let mut entry_reports = Vec::new();
    let mut used_keys = BTreeSet::new();
    let mut key_memo = KeyMemo::default();
    for (module, fact) in index.entries() {
        stats.entry_candidates += 1;
        *stats
            .entries_by_kind
            .entry(fact.kind.as_str().to_owned())
            .or_default() += 1;
        if !opts.entries.is_empty()
            && !opts
                .entries
                .iter()
                .any(|e| fact.selector.contains(e.as_str()))
        {
            continue;
        }
        let contract_decl = index.contracts.entry(&fact.selector).cloned();
        let closure = index.import_closure(&module.file);
        if !closure.iter().any(|f| mentions.contains(f)) {
            stats.entries_without_database_reach += 1;
            entry_reports.push(EntryReport {
                selector: fact.selector.clone(),
                kind: fact.kind.as_str().to_owned(),
                invocation: String::new(),
                status: "not-extracted".to_owned(),
                reads: 0,
                writes: 0,
                externals: 0,
                unknown: Vec::new(),
                detail: "its import closure mentions no database receiver and no declared external"
                    .to_owned(),
            });
            continue;
        }
        let invocation = contract_decl.as_ref().and_then(|c| {
            (c.invocation.is_some() || !c.preconditions.is_empty()).then(|| InvocationContract {
                invocation: c.invocation.clone(),
                preconditions: c.preconditions.clone(),
            })
        });
        let key = opts.cache.as_ref().map(|_| {
            cache_key(
                &index,
                &fact.selector,
                &invocation,
                &cfg,
                &opts.tool_version,
                &mut key_memo,
            )
        });
        let cached = key.as_ref().and_then(|k| cache.entries.get(k).cloned());
        let extracted: Result<Extracted, String> = match cached {
            Some(hit) => {
                stats.cache_hits += 1;
                hit.map(|c| Extracted {
                    params: c.params,
                    codes: c.codes,
                    assumptions: c.assumptions,
                    entry: c.entry,
                })
            }
            None => {
                if key.is_some() {
                    stats.cache_misses += 1;
                }
                let r = extract_entry(&provider, &index.bindings, &fact.selector, invocation);
                if let Some(k) = &key {
                    cache.entries.insert(
                        k.clone(),
                        r.clone().map(|x| CachedExtraction {
                            params: x.params,
                            codes: x.codes,
                            assumptions: x.assumptions,
                            entry: x.entry,
                        }),
                    );
                }
                r
            }
        };
        if let Some(k) = key {
            used_keys.insert(k);
        }
        stats.entries_extracted += 1;
        let extracted = match extracted {
            Ok(x) => x,
            Err(e) => {
                stats.extraction_failures += 1;
                entry_reports.push(EntryReport {
                    selector: fact.selector.clone(),
                    kind: fact.kind.as_str().to_owned(),
                    invocation: String::new(),
                    status: "extraction-failed".to_owned(),
                    reads: 0,
                    writes: 0,
                    externals: 0,
                    unknown: Vec::new(),
                    detail: e,
                });
                continue;
            }
        };
        let summary = summarize(&extracted);
        let a = Analyzed {
            fact: fact.clone(),
            use_server: module.use_server,
            summary,
            info: extracted.entry.clone(),
            contract: contract_decl,
        };
        let reads = a.summary.db.iter().filter(|e| !e.access.writes()).count();
        let writes = a.summary.db.iter().filter(|e| e.access.writes()).count();
        let externals = a.summary.externals.len();
        let status = if writes + externals > 0 {
            stats.effectful_entries += 1;
            "effectful"
        } else if reads > 0 {
            stats.read_only_entries += 1;
            "read-only"
        } else {
            "no-database-effects"
        };
        if !a.summary.unknown.is_empty() {
            stats.entries_with_unknown_operations += 1;
            stats.unknown_operations += a.summary.unknown.len();
        }
        entry_reports.push(EntryReport {
            selector: fact.selector.clone(),
            kind: fact.kind.as_str().to_owned(),
            invocation: a.invocation(),
            status: status.to_owned(),
            reads,
            writes,
            externals,
            unknown: a
                .summary
                .unknown
                .iter()
                .map(|u| format!("{} ({})", u.detail, u.src.label()))
                .collect(),
            detail: String::new(),
        });
        analyzed.push(a);
    }
    if let Some(path) = &opts.cache {
        cache.entries.retain(|k, _| used_keys.contains(k));
        let text = serde_json::to_string(&cache).map_err(|e| e.to_string())?;
        std::fs::write(path, text)
            .map_err(|e| format!("cannot write `{}`: {e}", path.display()))?;
    }
    let mut resources = BTreeSet::new();
    for a in &analyzed {
        for e in a.summary.db.iter().filter(|e| e.access.writes()) {
            for (c, _) in &e.writes {
                resources.insert(format!("{}.{c}", e.table));
            }
            if e.access == Access::Delete {
                resources.insert(e.table.clone());
            }
        }
        for x in &a.summary.externals {
            resources.insert(format!("channel:{}", x.channel));
        }
    }
    stats.resources = resources.len();
    let extraction_ms = ms(t);

    // -- candidates -----------------------------------------------------------
    let t = Instant::now();
    let (mut candidates, rejected, truncated) = detect(&index, &analyzed, opts.max_partners);
    // Phase 13: foreign keys and their row locks are modeled (scenarios
    // with foreign keys get the `foreign-keys` feature), so the Phase 12
    // `foreign-key-locks` model-limitation assumption is no longer stated.
    for c in &mut candidates {
        // Idempotency keys the conflict's effects take from the arguments.
        for op in c.conflict.iter().filter(|o| o.is_external) {
            let selector = &c.entries[op.entry].selector;
            if let Some(a) = analyzed.iter().find(|a| &a.fact.selector == selector)
                && let Some(x) = a.summary.externals.get(op.effect)
                && let Some(crate::effects::Val::Param { path }) =
                    x.key.as_ref().map(|k| crate::detect::canon(k, &a.summary))
                && !c.request_keys.contains(&path)
            {
                c.request_keys.push(path);
            }
        }
        c.fingerprint = fingerprint(c);
        c.id = format!("CR-SYNTH-{}", c.fingerprint[..6].to_ascii_uppercase());
    }
    candidates.sort_by_key(|c| c.fingerprint.clone());
    candidates.dedup_by(|a, b| a.fingerprint == b.fingerprint);
    let detection_ms = ms(t);

    let found = candidates.len();
    // Candidates beyond the budget are reported, not dropped, without
    // scenarios.
    let mut budgets = Budgets {
        max_candidates: opts.max_candidates,
        max_partners: opts.max_partners,
        max_states_per_scenario: opts.max_states,
        max_wall_ms_per_scenario: opts.max_wall_ms,
        max_total_states: opts.max_total_states,
        max_worlds_per_variant: opts.max_worlds.max(1),
        candidates_found: found,
        candidates_reported: found,
        partners_truncated: truncated,
        ..Budgets::default()
    };
    // Scenario work in a preliminary order (static strength first) so that a
    // budget cuts the weakest candidates.
    let mut order: Vec<usize> = (0..candidates.len()).collect();
    order.sort_by_key(|&i| static_rank(&candidates[i], &analyzed));

    let mut synthesis_ms = 0;
    let mut checking_ms = 0;
    let mut reports: Vec<CandidateReport> = Vec::new();
    let emit_dir = match &opts.emit_dir {
        Some(d) => {
            std::fs::create_dir_all(d)
                .map_err(|e| format!("cannot create `{}`: {e}", d.display()))?;
            Some(d.canonicalize().map_err(|e| e.to_string())?)
        }
        None => None,
    };
    for (n, &ci) in order.iter().enumerate() {
        let c = &candidates[ci];
        let mut scenarios = Vec::new();
        // Phase 15: the world search of this candidate.
        let world_search = opts.max_worlds > 1;
        let mut seen: BTreeMap<String, String> = BTreeMap::new();
        let mut outcomes: Vec<WorldOutcome> = Vec::new();
        let mut observed = Observations::default();
        let mut first_counterexample: Option<String> = None;
        let mut unknown_nullability: Vec<String> = Vec::new();
        if n >= opts.max_candidates {
            budgets.candidates_over_budget += 1;
        } else if c.blocked.is_none() {
            let entries: Vec<&Analyzed> = c
                .entries
                .iter()
                .map(|e| {
                    analyzed
                        .iter()
                        .find(|a| a.fact.selector == e.selector)
                        .expect("candidate entries are analyzed")
                })
                .collect();
            let cx = WorldCtx {
                c,
                entries: &entries,
                index: &index,
                provider: &provider,
                opts,
                emit_dir: emit_dir.as_deref(),
                root: &root,
            };
            let mut timing = (0u64, 0u64);
            let mut extra = Vec::new();
            for variant in synth::variants(c, opts.hypothetical) {
                let base = run_world(
                    &cx,
                    &variant,
                    &WorldSpec::base(),
                    &mut budgets,
                    &mut timing,
                    &mut seen,
                )?;
                outcomes.push(outcome_of(&variant, &WorldSpec::base(), &base));
                let base_found = base.counterexample();
                if let Some(b) = &base.built {
                    observed.merge(&b.observations);
                }
                if base_found {
                    first_counterexample.get_or_insert_with(|| "base".to_owned());
                }
                let search = world_search
                    && variant.kind == VariantKind::Actual
                    && opts.model_check
                    && (!base_found || opts.all_worlds)
                    && (first_counterexample.is_none() || opts.all_worlds);
                if search {
                    let mut pending = Vec::new();
                    if let Ok(args) = synth::base_arguments(c, &variant, &entries, &index) {
                        let domains: Vec<_> = c
                            .actor_entries
                            .iter()
                            .map(|&ai| crate::domains::entry_domains(entries[ai], &index))
                            .collect();
                        for (k, ds) in domains.iter().enumerate() {
                            for d in ds {
                                let name = format!("actor {} `{}`", k + 1, d.path);
                                if d.null == crate::domains::Permission::Unknown
                                    && !unknown_nullability.contains(&name)
                                {
                                    unknown_nullability.push(name);
                                }
                            }
                        }
                        pending.extend(crate::worlds::domain_worlds(c, &entries, &domains, &args));
                        if variant.relation == crate::candidate::Relation::Same {
                            pending.extend(crate::worlds::actor_worlds(c, &entries, &index, &args));
                        }
                    }
                    if let Some(b) = &base.built {
                        pending.extend(crate::worlds::relationship_worlds(
                            c, &entries, &index, &b.plan,
                        ));
                        pending
                            .extend(crate::worlds::boundary_worlds(c, &entries, &index, &b.plan));
                    }
                    crate::worlds::prioritize(&mut pending, &unflipped(&observed, &entries));
                    let mut attempts = 1;
                    while !pending.is_empty() {
                        if attempts >= opts.max_worlds {
                            for w in pending.drain(..) {
                                budgets.worlds_skipped_for_budget += 1;
                                outcomes.push(WorldOutcome {
                                    id: w.id,
                                    variant: variant.name.clone(),
                                    origin: w.origin,
                                    status: "skipped-budget".to_owned(),
                                    verdict: None,
                                    detail: format!(
                                        "the world budget ({} per variant, --max-worlds) is spent",
                                        opts.max_worlds
                                    ),
                                });
                            }
                            break;
                        }
                        let spec = pending.remove(0);
                        attempts += 1;
                        let run =
                            run_world(&cx, &variant, &spec, &mut budgets, &mut timing, &mut seen)?;
                        outcomes.push(outcome_of(&variant, &spec, &run));
                        if let Some(b) = &run.built {
                            observed.merge(&b.observations);
                            crate::worlds::prioritize(
                                &mut pending,
                                &unflipped(&observed, &entries),
                            );
                        }
                        let found = run.counterexample();
                        if run.checked {
                            extra.push(run.result);
                        }
                        if found {
                            first_counterexample.get_or_insert_with(|| spec.id.clone());
                            if !opts.all_worlds {
                                for w in pending.drain(..) {
                                    outcomes.push(WorldOutcome {
                                        id: w.id,
                                        variant: variant.name.clone(),
                                        origin: w.origin,
                                        status: "skipped-after-counterexample".to_owned(),
                                        verdict: None,
                                        detail: "a validated counterexample was found first (--all-worlds explores the rest)".to_owned(),
                                    });
                                }
                                break;
                            }
                        }
                    }
                }
                scenarios.push(base.result);
            }
            scenarios.extend(extra);
            synthesis_ms += timing.0;
            checking_ms += timing.1;
        }
        let (status, mut finding) = status_of(
            c,
            &scenarios,
            opts.model_check,
            n >= opts.max_candidates,
            unenforced_unique(c, &index).as_deref(),
        );
        let coverage = (opts.model_check && !scenarios.is_empty()).then(|| {
            let entries: Vec<&Analyzed> = c
                .entries
                .iter()
                .filter_map(|e| analyzed.iter().find(|a| a.fact.selector == e.selector))
                .collect();
            coverage_of(
                c,
                &entries,
                &scenarios,
                &outcomes,
                &observed,
                first_counterexample.clone(),
                &unknown_nullability,
                world_search,
                &status,
                opts,
            )
        });
        if let Some(cov) = &coverage {
            match status.as_str() {
                "NO_VIOLATION_WITHIN_BOUNDS" => {
                    finding.push(' ');
                    finding.push_str(&cov.conclusion);
                }
                "MODEL_COUNTEREXAMPLE" => {
                    if let Some(w) = deciding_scenario(&scenarios).and_then(|s| s.world.as_ref())
                        && w.origin != WorldOrigin::Base
                    {
                        finding = format!(
                            "In the initial world `{}` ({}): {finding}",
                            w.id,
                            w.relationships.join("; ")
                        );
                    }
                }
                _ => {}
            }
        }
        let next_step = crate::report::next_step(c, &status, opts.model_check);
        // Phase 15: the deciding counterexample's reproduction artifact.
        if status == "MODEL_COUNTEREXAMPLE"
            && let Some(dir) = &emit_dir
            && let Some(s) = deciding_scenario(&scenarios)
            && let (Some(doc), Some(file)) = (&s.reproduction, &s.file)
        {
            let mut doc = doc.clone();
            doc["status"] = serde_json::json!(status);
            doc["finding"] = serde_json::json!(finding);
            let name = format!(
                "{}.counterexample.json",
                file.strip_suffix(".json").unwrap_or(file)
            );
            let mut text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
            text.push('\n');
            std::fs::write(dir.join(&name), text)
                .map_err(|e| format!("cannot write `{name}`: {e}"))?;
        }
        reports.push(CandidateReport {
            rank: 0,
            candidate: c.clone(),
            status,
            finding,
            group: group_of(c),
            duplicate_of: None,
            ranking: ranking_factors(c, &analyzed),
            scenarios,
            next_step,
            coverage,
        });
    }

    // -- ranking and deduplication ---------------------------------------
    reports.sort_by_key(|r| final_rank(r, &analyzed));
    let mut first_of_group: BTreeMap<String, String> = BTreeMap::new();
    for (i, r) in reports.iter_mut().enumerate() {
        r.rank = i + 1;
        match first_of_group.get(&r.group) {
            Some(first) => r.duplicate_of = Some(first.clone()),
            None => {
                first_of_group.insert(r.group.clone(), r.candidate.id.clone());
            }
        }
    }
    let mut summary = summarize_report(&reports);
    for e in &entry_reports {
        for u in &e.unknown {
            *summary
                .unknown_operation_breakdown
                .entry(unknown_category(u).to_owned())
                .or_default() += 1;
        }
    }
    let algorithm = match opts.source_dpor {
        Some(m) => format!("source-set DPOR ({}), opt-in", m.as_str()),
        None => "sleep-set partial-order reduction (default)".to_owned(),
    };
    let mut notes = index.notes.clone();
    if contracts_path.is_none() {
        notes.push(format!(
            "no {CONTRACTS_FILE}: externals, delivery and retry contracts, business invariants and harness inputs are not declared"
        ));
    }
    let timings = opts.include_timing.then(|| Timings {
        index_ms,
        extraction_ms,
        detection_ms,
        synthesis_ms,
        model_checking_ms: checking_ms,
        total_ms: ms(total),
    });
    Ok(DiscoveryReport {
        kind: REPORT_KIND.to_owned(),
        schema_version: REPORT_SCHEMA_VERSION,
        tool: Tool {
            name: "commitrift".to_owned(),
            version: opts.tool_version.clone(),
        },
        project: ProjectInfo {
            name: root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            modules: index.modules.len(),
            parse_errors: index
                .modules
                .iter()
                .filter(|m| m.parse_error.is_some())
                .count(),
            schema_files: index.schema_files.clone(),
            contracts: contracts_path.map(|p| {
                p.strip_prefix(&root)
                    .map(|r| r.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| {
                        p.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    })
            }),
        },
        mode: if opts.model_check {
            "discover+model-check".to_owned()
        } else {
            "discover".to_owned()
        },
        algorithm,
        executes_application_code: false,
        index: stats,
        summary,
        budgets,
        candidates: reports,
        rejected,
        entries: entry_reports,
        notes,
        evaluation: None,
        timings,
    })
}

fn contains_word(text: &str, word: &str) -> bool {
    let bytes = text.as_bytes();
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
    let mut from = 0;
    while let Some(pos) = text[from..].find(word) {
        let start = from + pos;
        let end = start + word.len();
        let before = start == 0 || !ident(bytes[start - 1]);
        let after = end >= bytes.len() || !ident(bytes[end]);
        if before && after {
            return true;
        }
        from = start + 1;
    }
    false
}

fn fingerprint(c: &Candidate) -> String {
    let mut selectors: Vec<&str> = c
        .actor_entries
        .iter()
        .map(|&i| c.entries[i].selector.as_str())
        .collect();
    selectors.sort_unstable();
    let props: Vec<String> = c
        .properties
        .iter()
        .map(|p| {
            format!(
                "{}:{}",
                p.origin.as_str(),
                serde_json::to_string(&p.spec).unwrap_or_default()
            )
        })
        .collect();
    let env = format!(
        "{:?}|{:?}|{}|{}|{}",
        c.environment.relation,
        c.environment.retry,
        c.environment.max_crashes,
        c.environment.max_retries,
        c.environment.max_in_flight
    );
    let roles: Vec<String> = c
        .conflict
        .iter()
        .map(|o| format!("{}:{}:{}", o.entry, o.role, o.operation))
        .collect();
    format!(
        "{:016x}",
        commitrift_core::diagnostic::stable_hash(&[
            c.class.as_str(),
            &selectors.join(","),
            &c.resource.text,
            &props.join(";"),
            &env,
            &roles.join(";"),
        ])
    )
}

/// Candidates that describe the same issue: the same class and resource and
/// the same operations of the first entry.
fn group_of(c: &Candidate) -> String {
    let core: Vec<String> = c
        .conflict
        .iter()
        .filter(|o| o.entry == 0)
        .map(|o| format!("{}@{}", o.role, o.location))
        .collect();
    format!(
        "{:016x}",
        commitrift_core::diagnostic::stable_hash(&[
            c.class.as_str(),
            &c.resource.table,
            &c.resource.columns.join(","),
            &core.join(";"),
        ])
    )
}

fn exposure(c: &Candidate, analyzed: &[Analyzed]) -> u8 {
    c.entries
        .iter()
        .map(|e| {
            let a = analyzed.iter().find(|a| a.fact.selector == e.selector);
            match a {
                Some(a) if a.contract.is_some() => 0,
                Some(a)
                    if matches!(
                        a.fact.kind,
                        EntryKind::ServerAction | EntryKind::RouteHandler
                    ) =>
                {
                    0
                }
                _ => 1,
            }
        })
        .max()
        .unwrap_or(1)
}

fn static_rank(c: &Candidate, analyzed: &[Analyzed]) -> StaticRank {
    (
        u8::from(c.blocked.is_some()),
        c.strongest_origin().unwrap_or(Origin::HeuristicHypothesis),
        c.unsupported_on_path,
        exposure(c, analyzed),
        c.entries.len(),
        c.assumptions.len(),
        c.fingerprint.clone(),
    )
}

fn status_order(s: &str) -> u8 {
    match s {
        "MODEL_COUNTEREXAMPLE" => 0,
        "STATIC_CANDIDATE" => 1,
        "INCONCLUSIVE" => 2,
        "NO_VIOLATION_WITHIN_BOUNDS" => 3,
        "UNSUPPORTED" => 4,
        _ => 5,
    }
}

type StaticRank = (u8, Origin, bool, u8, usize, usize, String);

fn final_rank(r: &CandidateReport, analyzed: &[Analyzed]) -> (u8, StaticRank) {
    (status_order(&r.status), static_rank(&r.candidate, analyzed))
}

fn ranking_factors(c: &Candidate, analyzed: &[Analyzed]) -> Vec<String> {
    let mut out = vec![format!(
        "property origin: {}",
        c.strongest_origin().map_or("none", Origin::as_str)
    )];
    out.push(if c.unsupported_on_path {
        "an unsupported operation lies on the path".to_owned()
    } else {
        "no unsupported operation on the path".to_owned()
    });
    out.push(if exposure(c, analyzed) == 0 {
        "entries are request handlers or have declared contracts".to_owned()
    } else {
        "an entry's invocation context is not established".to_owned()
    });
    out.push(format!("{} assumption(s)", c.assumptions.len()));
    if !c.protections.is_empty() {
        out.push(format!("{} recognized protection(s)", c.protections.len()));
    }
    out
}

fn scenario_description(c: &Candidate, v: &synth::Variant) -> String {
    let origins: Vec<String> = c
        .properties
        .iter()
        .map(|p| format!("{} ({})", p.name, p.origin.as_str()))
        .collect();
    format!(
        "GENERATED by `commitrift discover` from candidate {} ({}): {}. Variant `{}` [{}]: {}. Properties: {}. Every value in this file was synthesized; the rows satisfy the modeled constraints. A counterexample to a HEURISTIC_HYPOTHESIS is not a confirmed defect.",
        c.id,
        c.class.as_str(),
        c.title,
        v.name,
        match v.kind {
            VariantKind::Actual => "actual configuration",
            VariantKind::Control => "control",
            VariantKind::Hypothetical => "HYPOTHETICAL, not the application",
        },
        v.description,
        origins.join(", ")
    )
}

// ---------------------------------------------------------------------------
// Phase 15: worlds
// ---------------------------------------------------------------------------

/// What one world's run needs.
struct WorldCtx<'a> {
    c: &'a Candidate,
    entries: &'a [&'a Analyzed],
    index: &'a ProjectIndex,
    provider: &'a MemoProvider,
    opts: &'a Options,
    emit_dir: Option<&'a Path>,
    root: &'a Path,
}

struct WorldRun {
    result: ScenarioResult,
    built: Option<synth::Built>,
    /// Model-checked (built, not a duplicate, within the state budget).
    checked: bool,
    duplicate_of: Option<String>,
}

impl WorldRun {
    /// A validated counterexample.
    fn counterexample(&self) -> bool {
        self.result.model.as_ref().is_some_and(|m| {
            m.verdict == "COUNTEREXAMPLE_FOUND"
                && m.validation
                    .as_ref()
                    .is_some_and(|v| v.replayed && v.violates)
        })
    }
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

/// Build, instantiate, emit and (optionally) model-check one world of a
/// variant. `timing` accumulates (synthesis, model checking) milliseconds.
fn run_world(
    cx: &WorldCtx<'_>,
    variant: &synth::Variant,
    spec: &WorldSpec,
    budgets: &mut Budgets,
    timing: &mut (u64, u64),
    seen: &mut BTreeMap<String, String>,
) -> Result<WorldRun, String> {
    let (c, index, opts) = (cx.c, cx.index, cx.opts);
    let t = Instant::now();
    let built = synth::build_world(
        c,
        variant,
        cx.entries,
        index,
        cx.provider,
        ScenarioBudget {
            max_states: opts.max_states,
            max_wall_ms: opts.max_wall_ms,
            max_memory_bytes: opts.max_memory_bytes,
        },
        spec,
    );
    let world_search = opts.max_worlds > 1;
    let mut result = ScenarioResult {
        variant: variant.clone(),
        file: None,
        synthesis: match &built {
            Ok(b) => b.synthesis.clone(),
            Err(r) => r.clone(),
        },
        actors: Vec::new(),
        initial_state: BTreeMap::new(),
        invariants: Vec::new(),
        invariant_notes: Vec::new(),
        source_verification: false,
        model: None,
        reproduction: None,
        world: world_search.then(|| WorldInfo {
            id: spec.id.clone(),
            origin: spec.origin,
            relationships: spec.relationships.clone(),
            feedback: spec.feedback.clone(),
            fingerprint: String::new(),
        }),
    };
    let mut built = match built {
        Ok(b) => b,
        Err(_) => {
            timing.0 += ms(t);
            if !spec.is_base() {
                budgets.worlds_invalid += 1;
            }
            return Ok(WorldRun {
                result,
                built: None,
                checked: false,
                duplicate_of: None,
            });
        }
    };
    result.actors = built.plan.actors.clone();
    result.initial_state = built.plan.rows.clone();
    let (invariants, notes) = synth::invariants(
        c,
        &built.plan,
        variant.relation,
        cx.entries,
        index,
        cx.provider,
    );
    result.invariant_notes = notes;
    built.plan.name = if spec.is_base() {
        format!("{}-{}", c.id.to_ascii_lowercase(), variant.name)
    } else {
        format!(
            "{}-{}--{}",
            c.id.to_ascii_lowercase(),
            variant.name,
            slug(&spec.id)
        )
    };
    built.plan.description = scenario_description(c, variant);
    if !spec.is_base() {
        built.plan.description.push_str(&format!(
            " Initial world `{}`: {}.",
            spec.id,
            spec.relationships.join("; ")
        ));
    }
    built.plan.invariants = invariants.iter().map(|(_, i)| i.clone()).collect();
    result.invariants = invariants
        .iter()
        .map(|(p, i)| InvariantReport {
            name: i.name.clone(),
            origin: p.origin.as_str().to_owned(),
            formula: format!("{:?}", i.formula),
        })
        .collect();
    timing.0 += ms(t);
    if built.plan.invariants.is_empty() {
        result
            .synthesis
            .notes
            .push("no property could be instantiated: nothing to check".to_owned());
        return Ok(WorldRun {
            result,
            built: Some(built),
            checked: false,
            duplicate_of: None,
        });
    }
    // Phase 15: equivalent worlds (everything the model checker is given
    // is identical) are explored once.
    let fp = crate::worlds::fingerprint(&built.plan, index);
    if let Some(w) = result.world.as_mut() {
        w.fingerprint = fp.clone();
    }
    if world_search
        && !spec.is_base()
        && let Some(prev) = seen.get(&fp).cloned()
    {
        budgets.worlds_duplicate += 1;
        return Ok(WorldRun {
            result,
            built: Some(built),
            checked: false,
            duplicate_of: Some(prev),
        });
    }
    seen.entry(fp).or_insert_with(|| spec.id.clone());
    budgets.scenarios_built += 1;
    budgets.worlds_built += 1;
    if let Some(dir) = cx.emit_dir {
        let root_rel = synth::relative_path(dir, cx.root);
        if let Some(h) = &index.contracts.harness {
            let manifest = format!("{root_rel}/{}", h.manifest);
            built.plan.source_verification =
                synth::source_verification(&built.plan, cx.entries, c, index, &manifest);
        }
        result.source_verification = built.plan.source_verification.is_some();
        let doc = built.plan.to_json(index, &root_rel);
        let name = format!("{}.json", built.plan.name);
        let mut text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
        text.push('\n');
        std::fs::write(dir.join(&name), text).map_err(|e| format!("cannot write `{name}`: {e}"))?;
        result.file = Some(name);
        // Model checking loads the in-memory plan (root-relative).
        built.plan.source_verification = None;
    }
    let mut checked = false;
    if opts.model_check {
        if budgets.total_states_explored >= opts.max_total_states {
            budgets.scenarios_skipped_for_budget += 1;
            result
                .synthesis
                .notes
                .push("not model-checked: the total state budget is spent".to_owned());
        } else {
            let t = Instant::now();
            result.model = Some(model_check(c, &built.plan, index, cx.provider, opts));
            timing.1 += ms(t);
            budgets.scenarios_model_checked += 1;
            checked = true;
            if let Some(m) = &result.model {
                budgets.total_states_explored += m.states;
            }
        }
    }
    let mut run = WorldRun {
        result,
        built: Some(built),
        checked,
        duplicate_of: None,
    };
    if run.counterexample()
        && cx.emit_dir.is_some()
        && let Some(b) = &run.built
    {
        run.result.reproduction = reproduction(cx, variant, spec, &run.result, &b.plan);
    }
    Ok(run)
}

fn outcome_of(variant: &synth::Variant, spec: &WorldSpec, run: &WorldRun) -> WorldOutcome {
    let (status, detail) = if let Some(prev) = &run.duplicate_of {
        (
            "duplicate".to_owned(),
            format!("equivalent to world `{prev}` (same fingerprint)"),
        )
    } else if run.built.is_none() {
        let d = &run.result.synthesis.detail;
        (
            if run.result.synthesis.status == "unsupported" {
                "unsupported"
            } else if d.contains("no longer reaches") || d.contains("does not reach") {
                // A valid state in which an actor stops before the
                // candidate's operations (a guard, a missing row).
                "unreached"
            } else {
                "invalid"
            }
            .to_owned(),
            run.result.synthesis.detail.clone(),
        )
    } else if run.checked {
        ("explored".to_owned(), String::new())
    } else {
        (
            "not-checked".to_owned(),
            run.result
                .synthesis
                .notes
                .last()
                .cloned()
                .unwrap_or_default(),
        )
    };
    WorldOutcome {
        id: spec.id.clone(),
        variant: variant.name.clone(),
        origin: spec.origin,
        status,
        verdict: run.result.model.as_ref().map(|m| m.verdict.clone()),
        detail,
    }
}

/// Parameter paths of branches that the explored worlds' solo runs took in
/// one direction only.
fn unflipped(observed: &Observations, entries: &[&Analyzed]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for ((selector, at), (fell, jumped)) in &observed.branches {
        if *fell && *jumped {
            continue;
        }
        if let Some(a) = entries.iter().find(|a| &a.fact.selector == selector)
            && let Some(paths) = crate::domains::branch_paths(a).get(at)
        {
            out.extend(paths.iter().cloned());
        }
    }
    out
}

/// Phase 15: the coverage section of a candidate.
#[allow(clippy::too_many_arguments)]
fn coverage_of(
    c: &Candidate,
    entries: &[&Analyzed],
    scenarios: &[ScenarioResult],
    outcomes: &[WorldOutcome],
    observed: &Observations,
    first_counterexample: Option<String>,
    unknown_nullability: &[String],
    world_search: bool,
    status: &str,
    opts: &Options,
) -> Coverage {
    let explored: Vec<&ScenarioResult> = scenarios
        .iter()
        .filter(|s| s.variant.kind == VariantKind::Actual && s.model.is_some())
        .collect();
    let rows: BTreeSet<String> = explored
        .iter()
        .map(|s| serde_json::to_string(&s.initial_state).unwrap_or_default())
        .collect();
    let args: BTreeSet<String> = explored
        .iter()
        .map(|s| {
            serde_json::to_string(
                &s.actors
                    .iter()
                    .map(|a| a.arguments.clone())
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_default()
        })
        .collect();
    let mut state_rel = Vec::new();
    let mut input_rel = Vec::new();
    for s in &explored {
        let rel = s.world.as_ref().map_or_else(
            || vec!["the synthesized initial state and arguments".to_owned()],
            |w| w.relationships.clone(),
        );
        let origin = s.world.as_ref().map_or(WorldOrigin::Base, |w| w.origin);
        let tagged: Vec<String> = rel
            .into_iter()
            .map(|r| format!("{}: {r}", s.variant.name))
            .collect();
        match origin {
            WorldOrigin::Relationship => state_rel.extend(tagged),
            WorldOrigin::Domain => input_rel.extend(tagged),
            WorldOrigin::Base => {
                state_rel.extend(tagged.iter().cloned());
                input_rel.push(format!(
                    "{}: {}",
                    s.variant.name,
                    match s.variant.relation {
                        crate::candidate::Relation::Same => "the actors use the same key",
                        crate::candidate::Relation::DistinctKey => "the actors use different keys",
                        crate::candidate::Relation::DistinctRequestKey =>
                            "the actors use different request keys",
                        crate::candidate::Relation::DistinctRow =>
                            "the actors address different rows",
                        crate::candidate::Relation::Swapped => "the actors' keys are swapped",
                    }
                ));
            }
        }
    }
    let mut schedules = ScheduleCoverage {
        algorithm: match opts.source_dpor {
            Some(m) => format!("source-dpor:{}", m.as_str()),
            None => "sleep-sets".to_owned(),
        },
        ..ScheduleCoverage::default()
    };
    for s in &explored {
        let m = s.model.as_ref().expect("explored");
        if m.verdict == "UNSUPPORTED" {
            schedules.worlds_unsupported += 1;
        } else if m.complete || m.verdict == "COUNTEREXAMPLE_FOUND" {
            schedules.worlds_complete += 1;
        } else {
            schedules.worlds_truncated += 1;
        }
    }
    // Branch outcomes: every `JumpIf` of the actors' extracted code.
    let mut relevant: BTreeSet<(String, String)> = BTreeSet::new();
    for a in entries {
        if !c.actor_entries.iter().any(|&ai| {
            c.entries
                .get(ai)
                .is_some_and(|e| e.selector == a.fact.selector)
        }) {
            continue;
        }
        for step in a.summary.codes.iter().flatten() {
            if matches!(step.op, commitrift_model::ir::Op::JumpIf { .. }) {
                relevant.insert((a.fact.selector.clone(), step.src.label()));
            }
        }
    }
    let reached = observed
        .branches
        .iter()
        .filter(|(k, _)| relevant.contains(*k))
        .map(|(_, (f, j))| usize::from(*f) + usize::from(*j))
        .sum();
    let mut unexplored = Vec::new();
    if !world_search {
        unexplored.push(
            "world search is off (--max-worlds 1): one synthesized initial world per variant"
                .to_owned(),
        );
    }
    for w in outcomes.iter().filter(|o| o.status != "explored") {
        unexplored.push(format!(
            "world `{}` ({}): {}{}",
            w.id,
            w.variant,
            w.status,
            if w.detail.is_empty() {
                String::new()
            } else {
                format!(" — {}", w.detail)
            }
        ));
    }
    for n in unknown_nullability {
        unexplored.push(format!(
            "{n}: whether null is a valid value is unknown (no TypeScript annotation): null not explored"
        ));
    }
    unexplored.push(
        "values outside the listed relationship classes (each parameter's value domain is not enumerated)"
            .to_owned(),
    );
    if c.actor_entries.len() > 1 {
        unexplored.push("more actors than the candidate's pair".to_owned());
    }
    let classes_exhaustive = world_search
        && outcomes
            .iter()
            .all(|o| matches!(o.status.as_str(), "explored" | "duplicate"));
    let source = SourceCoverage {
        unknown_operations: entries.iter().map(|a| a.summary.unknown.len()).sum(),
        unsupported_on_path: c.unsupported_on_path,
        unsupported: c.unsupported.clone(),
    };
    let n = explored.len();
    let conclusion = match status {
        "MODEL_COUNTEREXAMPLE" => format!(
            "A counterexample was found in initial world `{}` after exploring {n} world(s).",
            first_counterexample
                .clone()
                .unwrap_or_else(|| "base".to_owned())
        ),
        "NO_VIOLATION_WITHIN_BOUNDS" => format!(
            "Input exploration: no counterexample among {n} explored initial world(s) of the actual configuration ({} distinct row set(s), {} distinct argument vector(s)); schedules {} for the explored worlds. The input domain was not exhaustively explored{}; general application safety is not established.",
            rows.len(),
            args.len(),
            if schedules.worlds_truncated == 0 && schedules.worlds_unsupported == 0 {
                "completed within the bounds"
            } else {
                "did not complete for every world"
            },
            if source.unknown_operations > 0 || source.unsupported_on_path {
                " and the actors contain operations the extractor did not understand"
            } else {
                ""
            }
        ),
        _ => format!("{n} initial world(s) were model-checked."),
    };
    Coverage {
        mode: if world_search {
            "relationship-search".to_owned()
        } else {
            "single-world".to_owned()
        },
        source_semantics: source,
        initial_states: DimensionCoverage {
            distinct_explored: rows.len(),
            relationships: state_rel,
        },
        actor_inputs: DimensionCoverage {
            distinct_explored: args.len(),
            relationships: input_rel,
        },
        schedules,
        branch_outcomes: BranchCoverage {
            relevant_outcomes: relevant.len() * 2,
            reached_outcomes: reached,
            basis: "branch directions taken by each actor running alone on the explored worlds' final rows (concurrent schedules may take others)".to_owned(),
        },
        worlds: outcomes.to_vec(),
        domain_exhaustive: false,
        classes_exhaustive,
        unexplored,
        first_counterexample_world: first_counterexample,
        conclusion,
    }
}

/// Phase 15: a stable reproduction artifact for a validated counterexample
/// (written next to its emitted scenario once the candidate's status is
/// known). Every value in it was synthesized.
fn reproduction(
    cx: &WorldCtx<'_>,
    variant: &synth::Variant,
    spec: &WorldSpec,
    result: &ScenarioResult,
    plan: &synth::Plan,
) -> Option<serde_json::Value> {
    let m = result.model.as_ref()?;
    let c = cx.c;
    let mut modules: Vec<String> = c
        .entries
        .iter()
        .filter_map(|e| e.selector.split('#').next().map(str::to_owned))
        .collect();
    modules.sort();
    modules.dedup();
    let source: Vec<serde_json::Value> = modules
        .iter()
        .map(|m| serde_json::json!({ "file": m, "fnv64": file_digest(cx.root, m) }))
        .chain(
            cx.index
                .schema_files
                .iter()
                .map(|f| serde_json::json!({ "file": f, "fnv64": file_digest(cx.root, f) })),
        )
        .collect();
    let property = m.violated.clone().unwrap_or_default();
    let origin = c
        .properties
        .iter()
        .find(|p| p.name == property)
        .map_or(Origin::HeuristicHypothesis, |p| p.origin);
    let file = result.file.clone().unwrap_or_default();
    let doc = serde_json::json!({
        "kind": "commitrift.counterexample",
        "schemaVersion": 1,
        "candidate": c.id,
        "class": c.class.as_str(),
        "variant": variant.name,
        "world": { "id": spec.id, "origin": spec.origin, "relationships": spec.relationships },
        "property": { "name": property, "origin": origin.as_str() },
        "scenario": file,
        "initialState": plan.rows,
        "actors": plan.actors.iter().map(|a| serde_json::json!({
            "name": a.name, "selector": a.selector, "arguments": a.arguments,
        })).collect::<Vec<_>>(),
        "assumptions": c.assumptions.iter().map(|a| a.text.clone()).collect::<Vec<_>>(),
        "bounds": {
            "maxSteps": plan.max_steps, "maxStates": plan.max_states,
            "maxCrashes": plan.max_crashes, "maxRetries": plan.max_retries,
            "maxInFlight": plan.max_in_flight,
        },
        "algorithm": m.algorithm,
        "schedule": m.schedule,
        "finalTerms": m.final_terms,
        "sourceFingerprint": source,
        "evidence": "model counterexample (not an original-code reproduction)",
        "reproduce": format!("commitrift model-check {file}"),
        "reproduceOnOriginalCode": result.source_verification.then(|| format!(
            "commitrift model-check {file} --verify-source --allow-execution"
        )),
    });
    Some(doc)
}

fn model_check(
    c: &Candidate,
    plan: &synth::Plan,
    index: &ProjectIndex,
    provider: &MemoProvider,
    opts: &Options,
) -> ModelResult {
    let blank = |verdict: &str, detail: String| ModelResult {
        verdict: verdict.to_owned(),
        evidence: "none".to_owned(),
        algorithm: String::new(),
        complete: false,
        states: 0,
        transitions: 0,
        unsupported: vec![detail],
        violated: None,
        schedule: Vec::new(),
        final_terms: Vec::new(),
        validation: None,
        wall_ms: None,
    };
    let (mut model, load) = match plan.load(index, provider) {
        Ok(x) => x,
        Err(e) => return blank("NOT_LOADED", e),
    };
    if let Some(m) = opts.source_dpor {
        model.options.source_dpor = Some(m);
    }
    let algorithm = match opts.source_dpor {
        Some(m) => format!("source-dpor:{}", m.as_str()),
        None => "sleep-sets".to_owned(),
    };
    let start = Instant::now();
    let report = check(
        &model,
        &CheckOptions {
            validate_postgres: false,
            include_timing: false,
            scenario_file: None,
            source_revision: None,
            tool_version: opts.tool_version.clone(),
            load,
            source: None,
            profile_storage: false,
        },
    );
    let wall = opts.include_timing.then(|| ms(start));
    let mut result = ModelResult {
        verdict: report.exploration.verdict.clone(),
        evidence: serde_json::to_value(report.evidence.level)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default(),
        algorithm,
        complete: report.exploration.complete,
        states: report.exploration.metrics.states,
        transitions: report.exploration.metrics.transitions,
        unsupported: report
            .exploration
            .unsupported
            .iter()
            .map(|u| format!("{} ({}, {})", u.detail, u.actor, u.location))
            .collect(),
        violated: None,
        schedule: Vec::new(),
        final_terms: Vec::new(),
        validation: None,
        wall_ms: wall,
    };
    if let Some(w) = &report.counterexample {
        result.violated = Some(w.invariant.clone());
        result.schedule = w
            .steps
            .iter()
            .flat_map(|s| {
                s.events.iter().map(move |e| {
                    format!(
                        "{} {} {}{}",
                        s.actor,
                        serde_json::to_value(e.kind)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_default(),
                        e.text,
                        e.src
                            .as_ref()
                            .map(|s| format!("  ({})", s.label()))
                            .unwrap_or_default()
                    )
                })
            })
            .collect();
        result.final_terms = w
            .final_state
            .terms
            .iter()
            .map(|t| format!("{} = {}", t.term, t.value.render()))
            // Phase 16: why a formula over generated identities failed
            // (identity mismatch vs absent or stale value); empty for every
            // earlier invariant.
            .chain(
                w.final_state
                    .diagnosis
                    .iter()
                    .map(|d| format!("diagnosis: {d}")),
            )
            .collect();
        // Validation: strict replay, violation, operations exercised.
        let replayed = commitrift_model::witness::replay(&model, &w.schedule, true);
        let violates =
            commitrift_model::witness::check_schedule(&model, &w.schedule, &w.invariant).is_some();
        let executed: BTreeSet<String> = w
            .steps
            .iter()
            .flat_map(|s| {
                s.events
                    .iter()
                    .filter_map(|e| e.src.as_ref().map(|s| s.label()))
            })
            .collect();
        let mut exercised = Vec::new();
        let mut not = Vec::new();
        for op in &c.conflict {
            if executed.contains(&op.location) {
                exercised.push(format!("{} {}", op.role, op.location));
            } else {
                not.push(format!("{} {}", op.role, op.location));
            }
        }
        result.validation = Some(Validation {
            replayed: replayed.is_ok(),
            violates,
            initial_state_valid: true,
            operations_exercised: exercised,
            operations_not_exercised: not,
            minimized: w.minimized,
        });
    }
    result
}

/// The scenario a candidate's status comes from: an actual-configuration
/// variant with a counterexample, else the first actual variant (controls and
/// hypothetical variants never decide).
fn deciding_scenario(scenarios: &[ScenarioResult]) -> Option<&ScenarioResult> {
    scenarios
        .iter()
        .find(|s| {
            s.variant.kind == VariantKind::Actual
                && s.model
                    .as_ref()
                    .is_some_and(|m| m.verdict == "COUNTEREXAMPLE_FOUND")
        })
        .or_else(|| {
            // Phase 15: the base world, unless only another world could be
            // built and checked.
            scenarios
                .iter()
                .find(|s| s.variant.kind == VariantKind::Actual)
                .filter(|s| s.model.is_some())
                .or_else(|| {
                    scenarios
                        .iter()
                        .find(|s| s.variant.kind == VariantKind::Actual && s.model.is_some())
                })
        })
        .or_else(|| {
            scenarios
                .iter()
                .find(|s| s.variant.kind == VariantKind::Actual)
        })
}

/// Phase 15: a unique rule of the candidate's table that the model does not
/// enforce although PostgreSQL does: `NULLS NOT DISTINCT` over a nullable
/// column. Phase 16: the model enforces such a rule when it models every
/// column of it (the scenario loader then lists it with NULL equal to NULL);
/// only a rule with an unmodeled column (or on an unmodeled table) is still
/// unenforced. Partial unique indexes are handled apart (`protections`).
fn unenforced_unique(c: &Candidate, index: &ProjectIndex) -> Option<String> {
    let t = index.source_table(&c.resource.table)?;
    let modeled = index.table_by_name(&c.resource.table);
    t.unique
        .iter()
        .find(|u| {
            u.predicate.is_none()
                && u.nulls_not_distinct
                && u.columns
                    .iter()
                    .any(|col| t.column_by_sql(col).is_some_and(|x| !x.not_null))
                && !u.columns.iter().all(|col| {
                    t.column_by_sql(col)
                        .is_some_and(|x| modeled.is_some_and(|m| m.columns.contains(&x.property)))
                })
        })
        .map(|u| {
            format!(
                "unique {} `{}` NULLS NOT DISTINCT on ({})",
                u.kind,
                u.name.clone().unwrap_or_default(),
                u.columns.join(", ")
            )
        })
}

fn status_of(
    c: &Candidate,
    scenarios: &[ScenarioResult],
    model_checked: bool,
    over_budget: bool,
    unenforced: Option<&str>,
) -> (String, String) {
    if let Some(b) = &c.blocked {
        return ("BLOCKED".to_owned(), format!("STATIC_CANDIDATE only: {b}."));
    }
    if over_budget {
        return (
            "STATIC_CANDIDATE".to_owned(),
            "STATIC_CANDIDATE: over the candidate budget, no scenario was built.".to_owned(),
        );
    }
    let actual = deciding_scenario(scenarios);
    let Some(actual) = actual else {
        return (
            "STATIC_CANDIDATE".to_owned(),
            "STATIC_CANDIDATE: no scenario.".to_owned(),
        );
    };
    if actual.synthesis.status == "unsupported" {
        return (
            "UNSUPPORTED".to_owned(),
            format!("UNSUPPORTED: {}", actual.synthesis.detail),
        );
    }
    if actual.synthesis.status != "satisfied" {
        return (
            "INCONCLUSIVE".to_owned(),
            format!(
                "INCONCLUSIVE: no valid initial state: {}",
                actual.synthesis.detail
            ),
        );
    }
    if actual.invariants.is_empty() {
        return (
            "INCONCLUSIVE".to_owned(),
            "INCONCLUSIVE: no property could be instantiated for the synthesized scenario."
                .to_owned(),
        );
    }
    if !model_checked {
        return (
            "STATIC_CANDIDATE".to_owned(),
            "STATIC_CANDIDATE: a valid scenario was built; it was not model-checked (use --model-check).".to_owned(),
        );
    }
    let Some(m) = &actual.model else {
        return (
            "STATIC_CANDIDATE".to_owned(),
            "STATIC_CANDIDATE: not model-checked (budget).".to_owned(),
        );
    };
    let control = scenarios
        .iter()
        .filter(|s| s.variant.kind == VariantKind::Control)
        .filter_map(|s| {
            s.model
                .as_ref()
                .map(|m| format!("control `{}`: {}", s.variant.name, m.verdict))
        })
        .collect::<Vec<_>>();
    let control = if control.is_empty() {
        String::new()
    } else {
        format!(" ({})", control.join("; "))
    };
    match m.verdict.as_str() {
        "COUNTEREXAMPLE_FOUND" => {
            // Phase 14: a partial unique index the model does not enforce
            // may make the violating schedule impossible: the counterexample
            // is not a finding until such indexes are modeled.
            if let Some(index) = c
                .protections
                .iter()
                .find(|p| p.starts_with("partial unique"))
            {
                return (
                    "INCONCLUSIVE".to_owned(),
                    format!(
                        "INCONCLUSIVE: the model found a violating schedule, but the table has a {index}; PostgreSQL may reject the duplicate the schedule creates, so it is not reported as a counterexample."
                    ),
                );
            }
            // Phase 15: the same for a NULLS NOT DISTINCT unique rule over
            // a nullable column (Phase 16: only one the model does not
            // enforce, see `unenforced_unique`).
            if let Some(rule) = unenforced {
                return (
                    "INCONCLUSIVE".to_owned(),
                    format!(
                        "INCONCLUSIVE: the model found a violating schedule, but the table has a {rule}, which the model does not enforce (a column of it is not modeled); PostgreSQL may reject the duplicate the schedule creates, so it is not reported as a counterexample."
                    ),
                );
            }
            let v = m.validation.as_ref();
            let valid = v.is_some_and(|v| v.replayed && v.violates);
            if !valid {
                return (
                    "INCONCLUSIVE".to_owned(),
                    "INCONCLUSIVE: the counterexample did not validate (replay or violation check failed).".to_owned(),
                );
            }
            let prop = m.violated.clone().unwrap_or_default();
            let origin = c
                .properties
                .iter()
                .find(|p| p.name == prop)
                .map_or(Origin::HeuristicHypothesis, |p| p.origin);
            let finding = if origin.established() {
                format!(
                    "MODEL_COUNTEREXAMPLE: a schedule within the bounds violates `{prop}` ({}){control}. The property is {}; the counterexample replays in the model.",
                    origin.as_str(),
                    match origin {
                        Origin::SchemaEnforced => "enforced by the schema",
                        Origin::UserDeclared => "declared by the user",
                        Origin::FrameworkContract => "implied by a declared integration contract",
                        _ => "asserted in source",
                    }
                )
            } else {
                format!(
                    "MODEL_COUNTEREXAMPLE against a HEURISTIC_HYPOTHESIS: a schedule within the bounds violates `{prop}`{control}. That the application requires this property has NOT been established; this is not a confirmed defect."
                )
            };
            ("MODEL_COUNTEREXAMPLE".to_owned(), finding)
        }
        "NO_COUNTEREXAMPLE_WITHIN_BOUNDS" => (
            "NO_VIOLATION_WITHIN_BOUNDS".to_owned(),
            format!(
                "No violation found within the modeled bounds{control}. This is not a proof that the application is safe."
            ),
        ),
        "UNSUPPORTED" => (
            "UNSUPPORTED".to_owned(),
            format!(
                "UNSUPPORTED: the model reached unsupported behavior: {}",
                m.unsupported.join("; ")
            ),
        ),
        other => (
            "INCONCLUSIVE".to_owned(),
            format!("INCONCLUSIVE: the search ended with `{other}` (bounds) before a verdict."),
        ),
    }
}

/// The category of an extractor's `unknown` detail.
fn unknown_category(detail: &str) -> &'static str {
    const RULES: &[(&str, &str)] = &[
        // Phase 13: replica routing.
        (
            "read through a read replica (replica modeling not enabled)",
            "read replica (modeling not enabled)",
        ),
        (
            "read through a read replica or the primary",
            "read replica or primary (ambiguous handle)",
        ),
        (
            "read through a read replica with a row lock",
            "row lock through a read replica",
        ),
        (
            "database handle of a replicated database",
            "replicated database: routing unknown",
        ),
        (
            "package call with a verified built-in contract",
            "package call with a contract (result not modeled)",
        ),
        ("method calls are not resolved", "unresolved method call"),
        (
            "imported from the package",
            "call into an unanalyzed package",
        ),
        ("is not bound to a table", "table not modeled (schema)"),
        (
            "database handle of an unresolved type",
            "database handle of an unresolved type",
        ),
        ("is not a column of table", "column not modeled (schema)"),
        ("raw SQL", "unsupported query (raw SQL)"),
        ("select predicate", "unsupported query shape"),
        ("findFirst option", "unsupported query shape"),
        ("mutation", "unsupported query shape"),
        (
            "loop over a collection",
            "loop over a collection of unknown length",
        ),
        ("exceeds the inline depth", "inline depth limit"),
        ("re-export", "re-export not followed"),
        ("no exported function", "unresolved import"),
        ("not a function declared", "unresolved call"),
        ("unresolved identifier", "unresolved identifier"),
        ("transaction", "unsupported transaction shape"),
        ("expression of kind", "unsupported expression"),
        ("optional chain", "unsupported expression"),
        ("operator", "unsupported expression"),
        ("destructuring", "unsupported binding pattern"),
        ("spread", "unsupported expression"),
        ("computed member", "unsupported expression"),
    ];
    RULES
        .iter()
        .find(|(needle, _)| detail.contains(needle))
        .map_or("other", |(_, category)| category)
}

fn unsupported_category(r: &CandidateReport) -> Option<&'static str> {
    let c = &r.candidate;
    if r.status == "BLOCKED" {
        return Some(
            if c.blocked
                .as_deref()
                .is_some_and(|b| b.contains("invariant"))
            {
                "missing invariant"
            } else {
                "missing contract"
            },
        );
    }
    let detail = r
        .scenarios
        .first()
        .map(|s| s.synthesis.detail.as_str())
        .unwrap_or("");
    match r.status.as_str() {
        "UNSUPPORTED" => Some(
            if detail.contains("external") || detail.contains("method calls") {
                "unknown external effect"
            } else if detail.contains("SQL")
                || detail.contains("query")
                || detail.contains("execute")
            {
                "unsupported query"
            } else if detail.contains("invokes the callback") {
                "uncertain actor invocation"
            } else if detail.contains("schema") || detail.contains("not modeled") {
                "missing schema information"
            } else {
                "unsupported operation"
            },
        ),
        "INCONCLUSIVE" => Some(if detail.contains("does not load") {
            // The extracted code reads a local only an unsupported operation
            // assigns: an extraction gap, not a state or budget problem.
            "scenario does not load (extraction gap)"
        } else if detail.contains("initial state")
            || detail.contains("row")
            || detail.contains("contradictory")
        {
            "unsatisfiable initial state"
        } else if r.finding.contains("before a verdict") {
            "exploration budget exceeded"
        } else {
            "inconclusive (other)"
        }),
        _ => None,
    }
}

fn summarize_report(reports: &[CandidateReport]) -> Summary {
    let mut s = Summary {
        candidates: reports.len(),
        ..Summary::default()
    };
    let mut groups = BTreeSet::new();
    for r in reports {
        let c = &r.candidate;
        *s.by_class.entry(c.class.as_str().to_owned()).or_default() += 1;
        *s.by_status.entry(r.status.clone()).or_default() += 1;
        if let Some(o) = c.strongest_origin() {
            *s.by_property_origin
                .entry(o.as_str().to_owned())
                .or_default() += 1;
        }
        if c.properties
            .iter()
            .any(|p| p.origin == Origin::UserDeclared)
        {
            s.with_declared_property += 1;
        }
        if !c.assumptions.is_empty() {
            s.requiring_assumptions += 1;
        }
        if r.scenarios.iter().any(|x| {
            x.variant.kind == VariantKind::Actual
                && x.synthesis.status == "satisfied"
                && !x.invariants.is_empty()
        }) {
            s.model_checkable += 1;
        }
        if r.status == "MODEL_COUNTEREXAMPLE" {
            s.model_counterexamples += 1;
            groups.insert(r.group.clone());
            let violated = deciding_scenario(&r.scenarios)
                .and_then(|x| x.model.as_ref())
                .and_then(|m| m.violated.clone())
                .unwrap_or_default();
            if c.properties
                .iter()
                .find(|p| p.name == violated)
                .is_none_or(|p| p.origin == Origin::HeuristicHypothesis)
            {
                s.counterexamples_against_hypotheses += 1;
            }
        }
        if let Some(cat) = unsupported_category(r) {
            *s.unsupported_breakdown.entry(cat.to_owned()).or_default() += 1;
        }
    }
    s.deduplicated_findings = groups.len();
    s
}

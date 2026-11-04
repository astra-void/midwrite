//! Phase 15: typed input domains, initial worlds, feedback-guided search and
//! coverage accounting (`docs/phase15/`).
//!
//! The two historical false negatives are permanent regressions here:
//! Phase 14 E21 (the stored value equals the written one, so a stale read is
//! indistinguishable) and Phase 13 E26 (the defect needs `parentId = null`).
//! Both must be found by the generic search, not by a rule naming them.

use std::path::PathBuf;

use commitrift_discover::pipeline::{Options, discover};
use commitrift_discover::report::{CandidateReport, DiscoveryReport};
use commitrift_discover::worlds::WorldOrigin;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn options() -> Options {
    Options {
        model_check: true,
        include_timing: false,
        tool_version: "test".to_owned(),
        ..Options::default()
    }
}

fn run(app: &str, entry: &str, opts: Options) -> DiscoveryReport {
    discover(
        &repo().join(app),
        &Options {
            entries: vec![entry.to_owned()],
            ..opts
        },
    )
    .expect("discovery runs")
}

fn only(r: &DiscoveryReport) -> &CandidateReport {
    assert_eq!(r.candidates.len(), 1, "one candidate expected");
    &r.candidates[0]
}

fn by_entry<'a>(r: &'a DiscoveryReport, entry: &str) -> &'a CandidateReport {
    r.candidates
        .iter()
        .find(|c| {
            c.candidate
                .entries
                .iter()
                .all(|e| e.selector.ends_with(&format!("#{entry}")))
        })
        .unwrap_or_else(|| panic!("a candidate of `{entry}`"))
}

fn deciding_world(c: &CandidateReport) -> String {
    c.scenarios
        .iter()
        .find(|s| {
            s.model
                .as_ref()
                .is_some_and(|m| m.verdict == "COUNTEREXAMPLE_FOUND")
        })
        .and_then(|s| s.world.as_ref())
        .map(|w| w.id.clone())
        .unwrap_or_default()
}

#[test]
fn e21_a_stored_value_distinct_from_the_written_one_exposes_the_stale_read() {
    let r = run("fixtures/phase14/eval-app", "#saveDisplayName", options());
    let c = only(&r);
    assert_eq!(c.status, "MODEL_COUNTEREXAMPLE", "{}", c.finding);
    // The base world (stored = written) holds; the relationship world does
    // not.
    let base = &c.scenarios[0];
    assert_eq!(base.world.as_ref().unwrap().origin, WorldOrigin::Base);
    assert_eq!(
        base.model.as_ref().unwrap().verdict,
        "NO_COUNTEREXAMPLE_WITHIN_BOUNDS"
    );
    assert_eq!(
        deciding_world(c),
        "stored-distinct:accounts.displayName",
        "found by the value-distinction relationship, not by a fixture rule"
    );
    let cov = c.coverage.as_ref().unwrap();
    assert_eq!(
        cov.first_counterexample_world.as_deref(),
        Some("stored-distinct:accounts.displayName")
    );
    assert!(!cov.domain_exhaustive);
    assert!(
        c.finding
            .starts_with("In the initial world `stored-distinct")
    );
}

#[test]
fn e21_with_world_search_off_a_single_world_is_not_described_as_all_worlds() {
    let r = run(
        "fixtures/phase14/eval-app",
        "#saveDisplayName",
        Options {
            max_worlds: 1,
            ..options()
        },
    );
    let c = only(&r);
    assert_eq!(c.status, "NO_VIOLATION_WITHIN_BOUNDS");
    let cov = c.coverage.as_ref().unwrap();
    assert_eq!(cov.mode, "single-world");
    assert!(!cov.domain_exhaustive && !cov.classes_exhaustive);
    assert!(
        cov.unexplored
            .iter()
            .any(|u| u.contains("world search is off")),
        "{:?}",
        cov.unexplored
    );
    assert!(
        c.finding.contains("not exhaustively explored")
            && c.finding
                .contains("general application safety is not established"),
        "{}",
        c.finding
    );
}

#[test]
fn e26_a_null_argument_permitted_by_the_signature_reaches_the_duplicate() {
    let r = run("fixtures/phase13/eval-app", "#createFolder", options());
    let c = only(&r);
    assert_eq!(c.status, "MODEL_COUNTEREXAMPLE", "{}", c.finding);
    assert_eq!(deciding_world(c), "null:parentId");
    // Every actor got `null` for `parentId` (their relationship is kept).
    let s = c
        .scenarios
        .iter()
        .find(|s| s.world.as_ref().is_some_and(|w| w.id == "null:parentId"))
        .unwrap();
    for a in &s.actors {
        assert_eq!(a.arguments[1], commitrift_model::value::Value::Null);
    }
    // The null world flips the `parentId === null` branch.
    let cov = c.coverage.as_ref().unwrap();
    assert!(cov.branch_outcomes.reached_outcomes > 0);
    assert!(cov.branch_outcomes.reached_outcomes <= cov.branch_outcomes.relevant_outcomes);
}

#[test]
fn worlds_app_finds_both_input_dependent_defects_and_keeps_the_safe_counterparts() {
    let r = discover(&repo().join("fixtures/phase15/worlds-app"), &options()).unwrap();
    let folder = by_entry(&r, "createFolder");
    assert_eq!(folder.status, "MODEL_COUNTEREXAMPLE");
    assert_eq!(deciding_world(folder), "null:parentId");
    let export = by_entry(&r, "startExport");
    assert_eq!(export.status, "MODEL_COUNTEREXAMPLE");
    assert_eq!(deciding_world(export), "stored-boundary:quotas.remaining=1");
    // Phase 16: the model enforces the NULLS NOT DISTINCT unique key (NULL
    // equal to NULL), so the safe counterpart has no violating schedule in
    // any world, the null one included. (Phase 15 reported INCONCLUSIVE:
    // the model treated NULLs as distinct and found a schedule that the
    // original code on PostgreSQL did not reproduce, 0/3; the lockstep
    // differential of `crates/commitrift-model/tests/phase16_nulls.rs`
    // shows model and PostgreSQL 17 agree on the rule.)
    let label = by_entry(&r, "createLabel");
    assert_eq!(
        label.status, "NO_VIOLATION_WITHIN_BOUNDS",
        "{}",
        label.finding
    );
    assert!(!label.finding.contains("INCONCLUSIVE"), "{}", label.finding);
    assert!(label.scenarios.iter().any(|s| {
        s.world.as_ref().is_some_and(|w| w.id == "null:parentId")
            && s.model
                .as_ref()
                .is_some_and(|m| m.verdict == "NO_COUNTEREXAMPLE_WITHIN_BOUNDS" && m.complete)
    }));
    // The atomic variant has no guarded-update candidate at all.
    assert!(r.candidates.iter().all(|c| {
        !c.candidate
            .entries
            .iter()
            .any(|e| e.selector.ends_with("#startExportAtomic"))
    }));
}

#[test]
fn a_world_budget_is_reported_as_unexplored_input() {
    let r = run(
        "fixtures/phase15/worlds-app",
        "#startExport",
        Options {
            max_worlds: 2,
            ..options()
        },
    );
    let c = by_entry(&r, "startExport");
    let cov = c.coverage.as_ref().unwrap();
    assert!(
        cov.worlds.iter().any(|w| w.status == "skipped-budget"),
        "{:?}",
        cov.worlds
    );
    assert!(!cov.classes_exhaustive);
    assert!(r.budgets.worlds_skipped_for_budget > 0);
    assert!(
        cov.unexplored
            .iter()
            .any(|u| u.contains("skipped-budget") && u.contains("--max-worlds"))
    );
}

#[test]
fn redelivered_actors_keep_identical_arguments() {
    // Phase 13 E25: two deliveries of one job; giving the actors different
    // job ids would check another environment (a false positive found and
    // removed during Phase 15).
    let r = run(
        "fixtures/phase13/eval-app",
        "#handleWelcomeJobOnce",
        options(),
    );
    for c in &r.candidates {
        assert_ne!(c.status, "MODEL_COUNTEREXAMPLE", "{}", c.finding);
        if let Some(cov) = &c.coverage {
            assert!(
                cov.worlds
                    .iter()
                    .all(|w| !w.id.starts_with("actors-differ:"))
            );
        }
    }
}

#[test]
fn world_fingerprints_differ_with_the_inputs_and_reports_are_deterministic() {
    let a = run("fixtures/phase13/eval-app", "#createFolder", options());
    let b = run("fixtures/phase13/eval-app", "#createFolder", options());
    assert_eq!(
        a.to_json(),
        b.to_json(),
        "discovery output is deterministic"
    );
    let fps: Vec<&str> = only(&a)
        .scenarios
        .iter()
        .filter_map(|s| s.world.as_ref())
        .filter(|w| !w.fingerprint.is_empty())
        .map(|w| w.fingerprint.as_str())
        .collect();
    let mut unique = fps.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(fps.len(), unique.len(), "every explored world is distinct");
}

#[test]
fn the_extraction_cache_never_carries_verification_results_across_input_domains() {
    // Only extractions are cached; verdicts are recomputed. Changing the
    // input domain (the world budget) with a warm cache changes the result.
    let dir = std::env::temp_dir().join(format!("commitrift-p15-cache-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cache = dir.join("cache.json");
    let with_cache = |max_worlds: usize| {
        run(
            "fixtures/phase14/eval-app",
            "#saveDisplayName",
            Options {
                cache: Some(cache.clone()),
                max_worlds,
                ..options()
            },
        )
    };
    let wide = with_cache(8);
    let narrow = with_cache(1);
    let wide_again = with_cache(8);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(only(&wide).status, "MODEL_COUNTEREXAMPLE");
    assert_eq!(only(&narrow).status, "NO_VIOLATION_WITHIN_BOUNDS");
    assert_eq!(only(&wide_again).status, "MODEL_COUNTEREXAMPLE");
}

#[test]
fn a_synthesized_authorization_precondition_is_stated_never_assumed_silently() {
    // An admin guard over a stored membership row: the row's role is set so
    // that the guard passes, and the scenario says so.
    let r = run("fixtures/phase15/eval-app", "#createEnvironment", options());
    let c = only(&r);
    let notes = &c.scenarios[0].synthesis.notes;
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("SYNTHESIZED precondition") && n.contains("\"admin\"")),
        "{notes:?}"
    );
}

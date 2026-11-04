//! Phase 16: `UNIQUE ... NULLS NOT DISTINCT` (PostgreSQL 15+) in discovery
//! (`docs/phase16/NULLS-NOT-DISTINCT.md`).
//!
//! The first-party slice `fixtures/phase16/nulls` has the same
//! check-then-insert twice per Drizzle spelling: under a NULLS DISTINCT key
//! (two NULL keys coexist: a defect the null world exposes) and under a
//! NULLS NOT DISTINCT key (the model enforces it: the second insert fails
//! with 23505, no duplicate). The safe twin is a verified model result, not
//! a downgraded one.

use std::path::{Path, PathBuf};

use commitrift_analyzer::TsActorProvider;
use commitrift_discover::pipeline::{Options, discover};
use commitrift_discover::report::{CandidateReport, DiscoveryReport};
use commitrift_model::witness::{enumerate_schedules, replay};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn app() -> PathBuf {
    repo().join("fixtures/phase16/nulls")
}

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "commitrift-phase16-nulls-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(emit: Option<&Path>) -> DiscoveryReport {
    discover(
        &app(),
        &Options {
            model_check: true,
            include_timing: false,
            tool_version: "test".to_owned(),
            emit_dir: emit.map(Path::to_path_buf),
            ..Options::default()
        },
    )
    .expect("discovery runs")
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

/// The schema fact the loader states for the rule, by origin.
fn nnd_fact(report: &commitrift_model::scenario::LoadReport, table: &str) -> String {
    report
        .schema
        .iter()
        .find(|t| t.table == table)
        .and_then(|t| {
            t.facts
                .iter()
                .find(|f| f.fact.contains("NULLS NOT DISTINCT"))
                .map(|f| format!("{:?}: {}", f.origin, f.fact))
        })
        .unwrap_or_default()
}

#[test]
fn the_nulls_distinct_twins_have_validated_counterexamples_in_the_null_world() {
    let r = run(None);
    for (entry, world) in [
        ("createTag", "null:groupId"),
        ("registerDevice", "null:serialNo"),
    ] {
        let c = by_entry(&r, entry);
        assert_eq!(c.status, "MODEL_COUNTEREXAMPLE", "{entry}: {}", c.finding);
        assert_eq!(deciding_world(c), world, "{entry}");
        let s = c
            .scenarios
            .iter()
            .find(|s| s.world.as_ref().is_some_and(|w| w.id == world))
            .unwrap();
        let v = s.model.as_ref().unwrap().validation.as_ref().unwrap();
        assert!(v.replayed && v.violates, "{entry}");
    }
}

#[test]
fn the_nulls_not_distinct_twins_are_verified_within_bounds_not_downgraded() {
    let out = Temp::new("emit");
    let r = run(Some(&out.0));
    for (entry, table, world) in [
        ("createBadge", "badges", "null:groupId"),
        ("registerScanner", "scanners", "null:serialNo"),
    ] {
        let c = by_entry(&r, entry);
        assert_eq!(
            c.status, "NO_VIOLATION_WITHIN_BOUNDS",
            "{entry}: {}",
            c.finding
        );
        assert!(!c.finding.contains("INCONCLUSIVE"), "{}", c.finding);
        // Every world was checked to completion, the null world included.
        let null = c
            .scenarios
            .iter()
            .find(|s| s.world.as_ref().is_some_and(|w| w.id == world))
            .unwrap_or_else(|| panic!("{entry}: no `{world}` world"));
        let m = null.model.as_ref().unwrap();
        assert_eq!(m.verdict, "NO_COUNTEREXAMPLE_WITHIN_BOUNDS", "{entry}");
        assert!(m.complete, "{entry}");
        for s in &null.actors {
            assert!(
                s.arguments.contains(&commitrift_model::value::Value::Null),
                "{entry}: {:?}",
                s.arguments
            );
        }
        // The explored schedules of that world include the one where the
        // second insert of the NULL key fails with 23505.
        let file = out.0.join(null.file.as_ref().unwrap());
        let root = app();
        let (model, report) =
            commitrift_model::scenario::load_file(&file, &TsActorProvider::new(root)).unwrap();
        let fact = nnd_fact(&report, table);
        assert!(fact.starts_with("ExtractedFromSource: "), "{fact}");
        let t = model.schema.by_name(table).unwrap();
        assert_eq!(t.nulls_not_distinct.len(), 1, "{entry}");
        let (schedules, truncated) = enumerate_schedules(&model, 50_000);
        assert!(!truncated);
        let mut unique_violations = 0;
        for schedule in &schedules {
            let end = replay(&model, schedule, false).unwrap().state;
            let summary = commitrift_model::witness::summarize(&model, &end, &model.invariants[0]);
            if summary.actors.iter().any(|(_, o)| o.contains("23505")) {
                unique_violations += 1;
            }
            // No schedule leaves two rows with the same key.
            assert!(
                model.invariants.iter().all(|i| i.holds(&model, &end)),
                "{entry}: {:?}",
                summary.tables
            );
        }
        assert!(unique_violations > 0, "{entry}: no schedule ends in 23505");
        eprintln!(
            "{entry}: world {world}: {} schedule(s), {unique_violations} end with an actor failing with 23505",
            schedules.len()
        );
    }
}

//! Discovery on the first-party locks application
//! (`fixtures/apps/locks`): two single-row locks taken in argument order
//! deadlock (a failure, reported against the synthesized "no request
//! fails" hypothesis); one multi-row lock ordered by the key does not; a
//! Drizzle predicate held in a local reaches the NULLS DISTINCT duplicate
//! at the root, and the counterpart serialized by a row lock does not.

use std::path::PathBuf;

use midwrite_discover::pipeline::{Options, discover};
use midwrite_discover::report::{CandidateReport, DiscoveryReport};

fn report() -> DiscoveryReport {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/apps/locks");
    discover(
        &root,
        &Options {
            model_check: true,
            include_timing: false,
            tool_version: "test".to_owned(),
            ..Options::default()
        },
    )
    .expect("discovery runs")
}

/// The candidate whose entries are exactly `entries` (by export name).
fn candidate<'a>(r: &'a DiscoveryReport, entries: &[&str]) -> &'a CandidateReport {
    r.candidates
        .iter()
        .find(|c| {
            let mut names: Vec<&str> = c
                .candidate
                .entries
                .iter()
                .map(|e| e.selector.rsplit('#').next().unwrap_or_default())
                .collect();
            names.sort_unstable();
            names.dedup();
            let mut want = entries.to_vec();
            want.sort_unstable();
            names == want
        })
        .unwrap_or_else(|| {
            panic!(
                "a candidate of {entries:?}: {:?}",
                r.candidates
                    .iter()
                    .map(|c| c
                        .candidate
                        .entries
                        .iter()
                        .map(|e| e.selector.clone())
                        .collect::<Vec<_>>())
                    .collect::<Vec<_>>()
            )
        })
}

fn counterexample_schedule(c: &CandidateReport) -> Vec<String> {
    c.scenarios
        .iter()
        .filter_map(|s| s.model.as_ref())
        .find(|m| m.verdict == "COUNTEREXAMPLE_FOUND")
        .map(|m| m.schedule.clone())
        .unwrap_or_default()
}

#[test]
fn locks_application_findings() {
    let r = report();
    // Nothing in the application is unsupported any more.
    for e in &r.entries {
        if e.selector.contains("/services/") {
            assert_eq!(e.status, "effectful", "{}: {:?}", e.selector, e.detail);
        }
    }

    // Two single-row locks in argument order: crossed transfers deadlock and
    // one fails with 40P01 (a failure, not a broken balance).
    let transfer = candidate(&r, &["transfer"]);
    assert_eq!(transfer.candidate.class.as_str(), "lock-order");
    assert_eq!(
        transfer.status, "MODEL_COUNTEREXAMPLE",
        "{}",
        transfer.finding
    );
    let schedule = counterexample_schedule(transfer);
    assert!(
        schedule.iter().any(|l| l.contains("40P01")),
        "{schedule:#?}"
    );
    assert!(
        schedule
            .iter()
            .any(|l| l.contains("SELECT ALL accounts") && l.contains("FOR UPDATE")),
        "{schedule:#?}"
    );
    assert!(
        !schedule.iter().any(|l| l.contains("no account")),
        "the deadlock, not a missing row: {schedule:#?}"
    );

    // One multi-row lock ordered by the key, crossed arguments: no wait
    // cycle within the bounds. The same without `.orderBy` is modeled in
    // key order too (an approximation the extraction states).
    for entry in ["transferLocked", "transferLockedUnordered"] {
        let c = candidate(&r, &[entry]);
        assert_eq!(
            c.status, "NO_VIOLATION_WITHIN_BOUNDS",
            "{entry}: {}",
            c.finding
        );
        assert!(
            c.scenarios.iter().all(|s| s
                .model
                .as_ref()
                .is_some_and(|m| m.verdict == "NO_COUNTEREXAMPLE_WITHIN_BOUNDS")),
            "{entry}"
        );
    }

    // The predicate held in a local reaches the duplicate at the root.
    let unlocked = candidate(&r, &["createDocument"]);
    assert_eq!(
        unlocked.status, "MODEL_COUNTEREXAMPLE",
        "{}",
        unlocked.finding
    );
    assert!(
        unlocked.scenarios.iter().any(|s| s
            .world
            .as_ref()
            .is_some_and(|w| w.id == "null:folderId")
            && s.model
                .as_ref()
                .is_some_and(|m| m.verdict == "COUNTEREXAMPLE_FOUND")),
        "{}",
        unlocked.finding
    );
    // With a non-null folder the unique key catches the duplicate.
    assert!(unlocked.scenarios.iter().any(|s| {
        s.world.as_ref().is_some_and(|w| w.id == "base")
            && s.model
                .as_ref()
                .is_some_and(|m| m.verdict == "NO_COUNTEREXAMPLE_WITHIN_BOUNDS")
    }));

    // Serialized by the lock on the space row (a predicate helper builds
    // the folder condition): safe in every explored world.
    let locked = candidate(&r, &["createDocumentLocked"]);
    assert_eq!(
        locked.status, "NO_VIOLATION_WITHIN_BOUNDS",
        "{}",
        locked.finding
    );
    assert!(
        locked
            .scenarios
            .iter()
            .any(|s| s.world.as_ref().is_some_and(|w| w.id == "null:folderId"))
    );
}

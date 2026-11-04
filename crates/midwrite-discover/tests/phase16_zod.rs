//! Phase 16 Track A: zod-validated inputs in discovery
//! (`docs/phase16/ZOD-VALIDATION.md`).
//!
//! `fixtures/phase16/zod/app` validates server-action input with zod 4
//! strict object schemas (UUID and email formats, an enum with a default, a
//! nullish field) before a check-then-insert. The synthesized arguments must
//! satisfy the real validator (the extracted `ZodCheck`), unknown keys are
//! never added to a strict object, and the inputs the schema distinguishes
//! (a missing key, `undefined`, `null`) are explored as worlds.

use std::path::PathBuf;

use midwrite_discover::pipeline::{Options, discover};
use midwrite_discover::report::{CandidateReport, DiscoveryReport};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run(entry: &str, all_worlds: bool) -> DiscoveryReport {
    discover(
        &repo().join("fixtures/phase16/zod/app"),
        &Options {
            model_check: true,
            include_timing: false,
            tool_version: "test".to_owned(),
            entries: vec![entry.to_owned()],
            all_worlds,
            ..Options::default()
        },
    )
    .expect("discovery runs")
}

fn only(r: &DiscoveryReport) -> &CandidateReport {
    assert_eq!(r.candidates.len(), 1, "one candidate expected");
    &r.candidates[0]
}

#[test]
fn zod_validated_input_is_synthesized_to_pass_and_the_defect_is_found() {
    let r = run("#inviteMember", false);
    let c = only(&r);
    assert_eq!(c.status, "MODEL_COUNTEREXAMPLE", "{}", c.finding);
    let notes = &c.scenarios[0].synthesis.notes;
    let input = notes
        .iter()
        .find(|n| n.starts_with("SYNTHESIZED input"))
        .unwrap_or_else(|| panic!("the synthesized input is stated: {notes:?}"));
    // Every required key, valid values, nothing a strict object rejects.
    assert!(input.contains("zod 4 schema"), "{input}");
    assert!(
        input.contains("teamId=\"") && input.contains("-4000-a000-"),
        "{input}"
    );
    assert!(
        input.contains("email=\"") && input.contains("@example.test"),
        "{input}"
    );
    assert!(input.contains("role=\"member\""), "{input}");
    // No privilege is granted implicitly: nothing in this entry needs a
    // synthesized authorization precondition.
    assert!(
        !notes
            .iter()
            .any(|n| n.starts_with("SYNTHESIZED precondition")),
        "{notes:?}"
    );
}

#[test]
fn missing_undefined_and_null_keys_are_explored_as_worlds() {
    let r = run("#inviteMember", true);
    let c = only(&r);
    let cov = c.coverage.as_ref().expect("coverage");
    let worlds: Vec<(&str, &str)> = cov
        .worlds
        .iter()
        .map(|w| (w.id.as_str(), w.status.as_str()))
        .collect();
    for id in [
        "absent:input.note",
        "undefined:input.note",
        "null:input.note",
        "absent:input.role",
        "undefined:input.role",
    ] {
        assert!(
            worlds.contains(&(id, "explored")),
            "world `{id}` explored: {worlds:?}"
        );
    }
    // `role` does not admit null; the strict object admits no other key.
    assert!(
        !worlds.iter().any(|(id, _)| *id == "null:input.role"),
        "{worlds:?}"
    );
    // Every explored world passes validation (none stops at the ZodError).
    assert!(
        !cov.worlds.iter().any(|w| w.detail.contains("ZodError")),
        "{worlds:?}"
    );
}

#[test]
fn the_unique_key_counterpart_holds() {
    let r = run("#addMember", false);
    let c = only(&r);
    assert_eq!(c.status, "NO_VIOLATION_WITHIN_BOUNDS", "{}", c.finding);
}

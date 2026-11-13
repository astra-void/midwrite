//! tRPC procedures in discovery (DEV-167).
//!
//! `fixtures/apps/trpc-app` declares in `midwrite-contracts.json` what its
//! `protectedProcedure` middleware guarantees and that `ctx.db` is its
//! database. Resolvers of that builder are extracted with `ctx.db` bound to
//! the database, other `ctx` fields as request values, and `input` as the
//! procedure's `.input(schema)` parse; their check-then-insert race is
//! found. A resolver of an undeclared builder, and one whose parameter is
//! not destructured, are reported as not modeled, never assumed.

use std::path::PathBuf;

use midwrite_discover::pipeline::{Options, discover};
use midwrite_discover::report::DiscoveryReport;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run(entries: &[&str], all_worlds: bool) -> DiscoveryReport {
    discover(
        &repo().join("fixtures/apps/trpc-app"),
        &Options {
            model_check: true,
            include_timing: false,
            tool_version: "test".to_owned(),
            entries: entries.iter().map(|e| (*e).to_owned()).collect(),
            all_worlds,
            ..Options::default()
        },
    )
    .expect("discovery runs")
}

fn entry<'r>(r: &'r DiscoveryReport, property: &str) -> &'r midwrite_discover::report::EntryReport {
    r.entries
        .iter()
        .find(|e| e.selector.contains(&format!("/property[{property}]/")))
        .unwrap_or_else(|| panic!("entry {property} listed"))
}

#[test]
fn declared_procedures_are_extracted_and_their_race_is_found() {
    let r = run(&[], false);
    for name in ["invite", "inviteNested"] {
        let e = entry(&r, name);
        assert_eq!(e.kind, "trpc-procedure");
        assert_eq!(e.status, "effectful", "{name}: {e:?}");
        assert_eq!(e.invocation, "declared", "{name}");
        assert_eq!((e.reads, e.writes), (1, 1), "{name}");
        assert!(e.unknown.is_empty(), "{name}: {:?}", e.unknown);
    }
    for name in ["invite", "inviteNested"] {
        let c = r
            .candidates
            .iter()
            .find(|c| {
                c.candidate.entries.len() == 1
                    && c.candidate.entries[0]
                        .selector
                        .contains(&format!("/property[{name}]/"))
            })
            .unwrap_or_else(|| panic!("a candidate for {name}"));
        assert_eq!(c.status, "MODEL_COUNTEREXAMPLE", "{name}: {}", c.finding);
        let notes = &c.scenarios[0].synthesis.notes;
        assert!(
            notes.iter().any(|n| n.starts_with("SYNTHESIZED input")
                && n.contains("`_param0.input`")
                && n.contains("zod 4 schema")),
            "{name}: the input satisfies the procedure's schema: {notes:?}"
        );
    }
}

#[test]
fn the_declared_middleware_guarantee_is_carried_as_a_precondition() {
    let r = run(&["/property[invite]/"], false);
    let text = serde_json::to_string(&r).unwrap();
    assert!(
        text.contains("the request has a session; the middleware sets ctx.teamId to its team"),
        "the declared precondition is reported"
    );
}

#[test]
fn optional_input_fields_are_explored_as_worlds() {
    let r = run(&["/property[invite]/"], true);
    let c = &r.candidates[0];
    let worlds: Vec<&str> = c
        .coverage
        .as_ref()
        .expect("coverage")
        .worlds
        .iter()
        .map(|w| w.id.as_str())
        .collect();
    for w in ["absent:_param0.input.role", "undefined:_param0.input.role"] {
        assert!(worlds.contains(&w), "{w} in {worlds:?}");
    }
}

#[test]
fn undeclared_builders_and_undestructured_parameters_are_not_modeled() {
    let r = run(&[], false);
    let admin = entry(&r, "adminInvite");
    assert_eq!(admin.status, "extraction-failed");
    assert!(
        admin.detail.contains("a tRPC procedure resolver"),
        "{}",
        admin.detail
    );
    let opts = entry(&r, "inviteByOpts");
    assert_eq!(opts.writes, 0, "{opts:?}");
    assert!(
        opts.unknown
            .iter()
            .any(|u| u.contains("parameter is not destructured")),
        "{:?}",
        opts.unknown
    );
    // Neither takes part in a candidate.
    assert!(
        r.candidates
            .iter()
            .all(|c| c.candidate.entries.iter().all(|e| {
                !e.selector.contains("/property[adminInvite]/")
                    && !e.selector.contains("/property[inviteByOpts]/")
            }))
    );
}

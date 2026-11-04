//! Phase 16: invariant terms over generated identity
//! (`docs/phase16/GENERATED-ID-INVARIANTS.md`): `generatedBy`,
//! `sameGenerated`, `distinctGenerated`, `same`, `countMatching` /
//! `rowMatching` with `insertedBy`, `forEachActor` with `returned` and
//! `argument`, `forEachRow` with `field`; their semantics on aliasing,
//! retried attempts and replica reads; how a violation is diagnosed; and
//! that every earlier invariant keeps its JSON.

use midwrite_model::engine::{Recorder, apply, enabled, initial_state};
use midwrite_model::explore::{Verdict, explore};
use midwrite_model::invariant::{Formula, Invariant};
use midwrite_model::model::Model;
use midwrite_model::scenario::load_str;
use midwrite_model::state::{State, TKind};
use midwrite_model::value::Value;
use serde_json::json;

fn load(doc: serde_json::Value) -> Result<Model, String> {
    load_str(
        &doc.to_string(),
        std::path::Path::new("."),
        &midwrite_model::provider::NoProvider,
    )
    .map(|(m, _)| m)
    .map_err(|e| e.to_string())
}

/// Run actor `who` alone until it cannot move.
fn run_alone(model: &Model, mut s: State, who: u16) -> State {
    for _ in 0..200 {
        let Some(t) = enabled(model, &s)
            .into_iter()
            .find(|t| t.actor == who && matches!(t.kind, TKind::Step | TKind::Resume))
        else {
            break;
        };
        s = apply(model, &s, t, &mut Recorder::off())
            .expect("supported")
            .state;
    }
    s
}

fn inv(assert: serde_json::Value) -> Invariant {
    serde_json::from_value(json!({ "name": "p", "assert": assert })).expect("an invariant")
}

fn holds(model: &Model, s: &State, assert: serde_json::Value) -> bool {
    let i = inv(assert);
    i.validate(model).expect("valid against the model");
    i.holds(model, s)
}

fn cell(table: &str, id: i64, column: &str) -> serde_json::Value {
    json!({ "row": { "table": table, "where": { "id": id }, "column": column } })
}

// ---------------------------------------------------------------------
// Equality, aliasing, actor and generator
// ---------------------------------------------------------------------

/// Actor A generates `t` (a package call), stores it in two tables (one
/// value, two places), generates `u` and stores it once more; it returns
/// `{ token: t }`. Actor B stores a string.
fn tokens_model() -> Model {
    load(json!({
        "schemaVersion": 1,
        "name": "tokens",
        "database": {
            "tables": [
                { "name": "tokens", "columns": [
                    {"name": "id", "notNull": true, "type": "int"},
                    {"name": "token", "type": "text"}
                  ], "primaryKey": ["id"] },
                { "name": "copies", "columns": [
                    {"name": "id", "notNull": true, "type": "int"},
                    {"name": "token", "type": "text"}
                  ], "primaryKey": ["id"] }
            ]
        },
        "actors": [
            { "name": "A", "function": "issue", "arguments": {}, "program":
              "let t = generate \"nanoid(21)\"\ninsert tokens set @id = 1, @token = t\ninsert copies set @id = 1, @token = t\nlet u = generate \"nanoid(21)\"\ninsert tokens set @id = 2, @token = u\nlet r = select tokens where @id == 1\nreturn r\n" },
            { "name": "B", "function": "plain", "arguments": {}, "program":
              "insert tokens set @id = 3, @token = \"fixed\"\nreturn \"fixed\"\n" }
        ],
        "invariants": [{ "name": "reads-returns", "assert": { "forEachActor": { "assert": { "same": [
            { "returned": {} }, { "returned": {} } ] } } } }]
    }))
    .expect("the scenario loads")
}

#[test]
fn generated_values_compare_by_identity_and_comparisons_keep_their_meaning() {
    let m = tokens_model();
    assert!(m.tracks_results(), "`returned` makes return values tracked");
    let s = run_alone(&m, run_alone(&m, initial_state(&m), 0), 1);
    let (t1, c1, t2, t3) = (
        cell("tokens", 1, "token"),
        cell("copies", 1, "token"),
        cell("tokens", 2, "token"),
        cell("tokens", 3, "token"),
    );
    // Aliasing: one generation event stored in two places is one identity.
    assert!(holds(&m, &s, json!({ "sameGenerated": [t1, c1] })));
    assert!(holds(&m, &s, json!({ "same": [t1, c1] })));
    // Two events of the same actor and generator are distinct identities.
    assert!(holds(&m, &s, json!({ "distinctGenerated": [t1, t2] })));
    assert!(!holds(&m, &s, json!({ "sameGenerated": [t1, t2] })));
    // `distinctGenerated` is not the negation of `sameGenerated`: a string
    // is neither.
    assert!(!holds(&m, &s, json!({ "sameGenerated": [t1, t3] })));
    assert!(!holds(&m, &s, json!({ "distinctGenerated": [t1, t3] })));
    // `same` on concrete values; absent values are never the same.
    assert!(holds(&m, &s, json!({ "same": [t3, "fixed"] })));
    assert!(!holds(
        &m,
        &s,
        json!({ "same": [cell("tokens", 9, "token"), null] })
    ));
    // Phase 6-15 comparisons are unchanged: a comparison of two generated
    // values is false (only `eq`/`ne` against null are defined), even for
    // one identity.
    assert!(!holds(&m, &s, json!({ "eq": [t1, c1] })));
    assert!(!holds(&m, &s, json!({ "ne": [t1, t2] })));
    // Who generated it, and with which generator.
    assert!(holds(
        &m,
        &s,
        json!({ "generatedBy": { "value": t1, "by": { "actor": "A" }, "generator": "call" } })
    ));
    assert!(holds(
        &m,
        &s,
        json!({ "generatedBy": { "value": t2, "by": { "function": "issue" } } })
    ));
    assert!(!holds(
        &m,
        &s,
        json!({ "generatedBy": { "value": t1, "by": { "actor": "B" } } })
    ));
    assert!(!holds(&m, &s, json!({ "generatedBy": { "value": t3 } })));
    // The returned object's field is the stored identity.
    assert!(holds(
        &m,
        &s,
        json!({ "sameGenerated": [{ "returned": { "actor": "A", "path": "token" } }, t1] })
    ));
    assert!(holds(
        &m,
        &s,
        json!({ "forEachActor": { "function": "issue", "assert":
            { "generatedBy": { "value": { "returned": { "path": "token" } }, "by": "self" } } } })
    ));
    // `forEachActor` ranges over completed actors by default.
    let only_a = run_alone(&m, initial_state(&m), 0);
    assert!(holds(
        &m,
        &only_a,
        json!({ "forEachActor": { "assert":
            { "generatedBy": { "value": { "returned": { "path": "token" } }, "by": "self" } } } })
    ));
    assert!(!holds(
        &m,
        &only_a,
        json!({ "forEachActor": { "completed": false, "assert":
            { "generatedBy": { "value": { "returned": { "path": "token" } }, "by": "self" } } } })
    ));
}

#[test]
fn rows_are_matched_by_term_values_and_by_their_inserting_actor() {
    let m = tokens_model();
    let s = run_alone(&m, run_alone(&m, initial_state(&m), 0), 1);
    // The copy references the token row: matched by identity.
    let refers = json!({ "eq": [
        { "countMatching": { "table": "tokens", "match": { "token": cell("copies", 1, "token") } } }, 1
    ] });
    assert!(holds(&m, &s, refers));
    // Inserted by A: rows 1 and 2; by B: row 3.
    assert!(holds(
        &m,
        &s,
        json!({ "eq": [{ "countMatching": { "table": "tokens", "insertedBy": { "actor": "A" } } }, 2] })
    ));
    assert!(holds(
        &m,
        &s,
        json!({ "same": [
            { "rowMatching": { "table": "tokens", "insertedBy": { "actor": "B" }, "column": "token" } },
            "fixed"
        ] })
    ));
    // A null match value matches no row (as SQL `=`).
    assert!(holds(
        &m,
        &s,
        json!({ "eq": [{ "countMatching": { "table": "tokens", "match": { "token": null } } }, 0] })
    ));
    // Every copy refers to an existing token row (referenced-row identity).
    assert!(holds(
        &m,
        &s,
        json!({ "forEachRow": { "table": "copies", "assert": { "and": [
            { "generatedBy": { "value": { "field": "token" }, "generator": "call" } },
            { "eq": [{ "countMatching": { "table": "tokens", "match": { "token": { "field": "token" } } } }, 1] }
        ] } } })
    ));
    // Not every token row's value was generated (row 3 holds a string).
    assert!(!holds(
        &m,
        &s,
        json!({ "forEachRow": { "table": "tokens", "assert": { "generatedBy": { "value": { "field": "token" } } } } })
    ));
}

#[test]
fn arguments_name_the_row_a_call_works_on() {
    let m = load(json!({
        "schemaVersion": 1, "name": "links",
        "database": { "tables": [{ "name": "links", "columns": [
            {"name": "id", "notNull": true, "type": "int"}, {"name": "token", "type": "text"}
          ], "primaryKey": ["id"] }], "initialState": { "links": [{"id": 7, "token": null}] } },
        "actors": [{ "name": "A", "function": "ensure", "arguments": { "params": { "linkId": 7 } },
          "program": "let t = generate \"nanoid(21)\"\nupdate links where @id == params.linkId set @token = t\nreturn t\n" }],
        "invariants": [{ "name": "told-is-stored", "assert": { "forEachActor": { "assert": { "same": [
            { "returned": {} },
            { "rowMatching": { "table": "links", "match": { "id": { "argument": { "path": "params.linkId" } } }, "column": "token" } }
        ] } } } }]
    }))
    .expect("loads");
    assert_eq!(explore(&m).verdict, Verdict::NoCounterexampleWithinBounds);
}

// ---------------------------------------------------------------------
// Retried attempts
// ---------------------------------------------------------------------

fn signup_model(invariant: serde_json::Value) -> Model {
    load(json!({
        "schemaVersion": 1,
        "name": "signup",
        "database": {
            "tables": [{ "name": "events", "columns": [
                {"name": "id", "notNull": true, "type": "text",
                 "default": { "generated": { "side": "database", "generator": "gen_random_uuid()" } }},
                {"name": "account", "notNull": true, "type": "int"}
              ], "primaryKey": ["id"] }]
        },
        "actors": [{
            "name": "client", "function": "recordSignup", "arguments": {"account": 1},
            "program": "insert events set @account = account returning e\nreturn e\n",
            "retry": { "trigger": "onCrash" },
            "response": true
        }],
        "bounds": { "maxCrashes": 1, "maxRetries": 1 },
        "invariants": [invariant]
    }))
    .expect("the scenario loads")
}

fn returns_own_row() -> serde_json::Value {
    json!({ "name": "returned-id-is-own-row", "assert": { "forEachActor": { "assert": { "and": [
        { "generatedBy": { "value": { "returned": { "path": "id" } }, "by": "self", "generator": { "column": "events.id" } } },
        { "eq": [{ "countMatching": { "table": "events", "match": { "id": { "returned": { "path": "id" } } }, "insertedBy": "self" } }, 1] }
    ] } } } })
}

#[test]
fn a_retried_attempt_generates_a_new_identity_of_the_same_actor_and_generator() {
    // Insert, crash before the response, retry, insert, respond.
    let m = signup_model(returns_own_row());
    let mut s = initial_state(&m);
    let step = |s: &State, kind: TKind| {
        let t = enabled(&m, s)
            .into_iter()
            .find(|t| t.kind == kind)
            .unwrap_or_else(|| panic!("{kind:?} is enabled"));
        apply(&m, s, t, &mut Recorder::off())
            .expect("supported")
            .state
    };
    s = step(&s, TKind::Step);
    s = step(&s, TKind::Crash);
    s = step(&s, TKind::Retry);
    s = step(&s, TKind::Step);
    s = step(&s, TKind::Step);
    let rows = s.committed_rows(&m, 0);
    let ids: Vec<_> = rows
        .iter()
        .map(|r| match r.get("id") {
            Some(Value::Gen(g)) => (g.generator, g.actor, g.n),
            other => panic!("a generated id, got {other:?}"),
        })
        .collect();
    // One actor, one generator, two generation events.
    assert_eq!(ids.len(), 2);
    assert_eq!((ids[0].0, ids[0].1), (ids[1].0, ids[1].1));
    assert_ne!(ids[0].2, ids[1].2);
    // "Generated by the actor from the generator" matches both events: it
    // names the actor and the generator, not an occurrence.
    assert!(holds(
        &m,
        &s,
        json!({ "forEachRow": { "table": "events", "assert":
            { "generatedBy": { "value": { "field": "id" }, "by": { "actor": "client" }, "generator": { "column": "events.id" } } } } })
    ));
    // The response is the retried attempt's: its id is the second row's,
    // which the same actor inserted; the first attempt's id is a distinct
    // identity.
    assert!(m.invariants[0].holds(&m, &s));
    assert!(holds(
        &m,
        &s,
        json!({ "eq": [{ "countMatching": { "table": "events", "match": { "id": { "returned": { "actor": "client", "path": "id" } } } } }, 1] })
    ));
    // In every schedule within the bounds the returned id is the caller's
    // own row, while the duplicate row (the Phase 13 counterexample) exists.
    assert_eq!(explore(&m).verdict, Verdict::NoCounterexampleWithinBounds);
    let dup = signup_model(json!({ "name": "one-row", "assert": { "le": [
        { "countMatching": { "table": "events", "insertedBy": { "actor": "client" } } }, 1
    ] } }));
    assert_eq!(explore(&dup).verdict, Verdict::CounterexampleFound);
}

#[test]
fn a_response_carrying_an_id_generated_before_the_retry_is_reported() {
    // The defect of `fixtures/phase16/genid` `createProject`: the first
    // insert fails on the slug, the retry inserts with a new id, the
    // response carries the first id.
    let program = |returned: &str| {
        format!(
            "let id = generate \"nanoid()\"\nlet taken = count projects where @slug == slug\nif taken > 0 {{\n  let retry = generate \"nanoid()\"\n  insert projects set @id = retry, @slug = fallback\n  return {returned}\n}}\ninsert projects set @id = id, @slug = slug\nreturn id\n"
        )
    };
    let model = |returned: &str| {
        load(json!({
            "schemaVersion": 1, "name": "projects",
            "database": { "tables": [{ "name": "projects", "columns": [
                {"name": "id", "notNull": true, "type": "text"}, {"name": "slug", "notNull": true, "type": "text"}
              ], "primaryKey": ["id"], "unique": [["slug"]] }],
              "initialState": { "projects": [{"id": "p-0", "slug": "home"}] } },
            "actors": [{ "name": "A", "function": "createProject", "arguments": { "slug": "home", "fallback": "home-2" },
              "program": program(returned) }],
            "invariants": [{ "name": "returned-id-is-own-row", "assert": { "forEachActor": { "assert": { "and": [
                { "generatedBy": { "value": { "returned": {} }, "by": "self", "generator": "call" } },
                { "eq": [{ "countMatching": { "table": "projects", "match": { "id": { "returned": {} } }, "insertedBy": "self" } }, 1] }
            ] } } } }]
        }))
        .expect("loads")
    };
    let buggy = model("id");
    let r = explore(&buggy);
    assert_eq!(r.verdict, Verdict::CounterexampleFound);
    let w = midwrite_model::witness::build_witness(&buggy, &r.counterexample.unwrap(), true)
        .expect("a witness");
    // The id was generated by the caller; no row has it.
    assert!(
        w.final_state.diagnosis.iter().any(|d| d.starts_with(
            "actor `A`: `count(projects[id=returned(self)] inserted by self) = 1` is false"
        )),
        "{:?}",
        w.final_state.diagnosis
    );
    assert_eq!(
        explore(&model("retry")).verdict,
        Verdict::NoCounterexampleWithinBounds
    );
}

// ---------------------------------------------------------------------
// Replica-observed values, and how violations are told apart
// ---------------------------------------------------------------------

fn invites_model(programs: &[(&str, &str)]) -> Model {
    load(json!({
        "schemaVersion": 1,
        "name": "invites",
        "features": ["replicas"],
        "database": {
            "tables": [{ "name": "invites", "columns": [
                {"name": "id", "notNull": true, "type": "text",
                 "default": { "generated": { "side": "database", "generator": "gen_random_uuid()" } }},
                {"name": "team", "notNull": true, "type": "int"}
              ], "primaryKey": ["id"] }]
        },
        "actors": programs.iter().map(|(name, program)| json!({
            "name": name, "function": "createInvite", "program": program, "arguments": {}
        })).collect::<Vec<_>>(),
        "invariants": [{ "name": "invite-read-your-writes", "assert": { "forEachActor": { "assert": { "and": [
            { "generatedBy": { "value": { "returned": { "path": "id" } }, "by": "self", "generator": { "column": "invites.id" } } },
            { "eq": [{ "countMatching": { "table": "invites", "match": { "id": { "returned": { "path": "id" } } }, "insertedBy": "self" } }, 1] }
        ] } } } }]
    }))
    .expect("the scenario loads")
}

const INVITE_REPLICA: &str = "\
insert invites set @team = 1 returning c
let row = replica select invites where @id == c.id
return row
";

const INVITE_PRIMARY: &str = "\
insert invites set @team = 1 returning c
let row = select invites where @id == c.id
return row
";

/// Reads the team's first invitation instead of its own (by key order).
const INVITE_FIRST_OF_TEAM: &str = "\
insert invites set @team = 1 returning c
let row = select invites where @team == 1
return row
";

#[test]
fn a_stale_replica_read_of_a_generated_id_is_an_absent_value() {
    let m = invites_model(&[("A", INVITE_REPLICA)]);
    let r = explore(&m);
    assert_eq!(r.verdict, Verdict::CounterexampleFound);
    let ce = r.counterexample.unwrap();
    // The read precedes the replica's replay of the INSERT.
    assert!(!ce.schedule.iter().any(|t| t.kind == TKind::Replicate));
    let w = midwrite_model::witness::build_witness(&m, &ce, true).expect("a witness");
    let d = w.final_state.diagnosis.join("\n");
    assert!(d.contains("actor `A`: absent value"), "{d}");
    assert!(d.contains("stale, which eventual consistency permits unless a declared property requires the write to be visible"), "{d}");
    assert!(!d.contains("generated-identity mismatch"), "{d}");
    assert!(
        midwrite_model::witness::render_text(&m, &w).contains("absent value"),
        "the text report carries the diagnosis"
    );
    // Read back through the primary: no counterexample.
    let primary = invites_model(&[("A", INVITE_PRIMARY)]);
    assert_eq!(
        explore(&primary).verdict,
        Verdict::NoCounterexampleWithinBounds
    );
}

#[test]
fn an_id_generated_by_another_caller_is_an_identity_mismatch() {
    let m = invites_model(&[("A", INVITE_FIRST_OF_TEAM), ("B", INVITE_FIRST_OF_TEAM)]);
    let r = explore(&m);
    assert_eq!(r.verdict, Verdict::CounterexampleFound);
    let w = midwrite_model::witness::build_witness(&m, &r.counterexample.unwrap(), true)
        .expect("a witness");
    let d = w.final_state.diagnosis.join("\n");
    assert!(d.contains("generated-identity mismatch"), "{d}");
    assert!(
        d.contains("generated by actor `A`") || d.contains("generated by actor `B`"),
        "{d}"
    );
    assert!(!d.contains("absent value"), "{d}");
}

#[test]
fn diagnoses_are_empty_for_earlier_invariants() {
    let m = signup_model(json!({ "name": "one-row", "assert": { "le": [
        { "count": { "table": "events", "where": { "account": 1 } } }, 1
    ] } }));
    let r = explore(&m);
    assert_eq!(r.verdict, Verdict::CounterexampleFound);
    let w = midwrite_model::witness::build_witness(&m, &r.counterexample.unwrap(), true)
        .expect("a witness");
    assert!(w.final_state.diagnosis.is_empty());
    let text = serde_json::to_string(&w).unwrap();
    assert!(!text.contains("diagnosis"), "not serialized when empty");
}

// ---------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------

#[test]
fn bindings_actors_and_generators_are_validated() {
    let m = tokens_model();
    let err = |assert: serde_json::Value| inv(assert).validate(&m).unwrap_err().message;
    assert!(err(json!({ "same": [{ "returned": {} }, 1] })).contains("bound by `forEachActor`"));
    assert!(
        err(json!({ "generatedBy": { "value": 1, "by": "self" } }))
            .contains("bound by `forEachActor`")
    );
    assert!(err(json!({ "eq": [{ "field": "token" }, 1] })).contains("outside one"));
    assert!(
        err(json!({ "forEachRow": { "table": "tokens", "assert": { "eq": [{ "field": "nope" }, 1] } } }))
            .contains("unknown column `tokens.nope`")
    );
    assert!(
        err(json!({ "generatedBy": { "value": 1, "generator": { "column": "tokens.token" } } }))
            .contains("not a column with a generated default")
    );
    assert!(
        err(json!({ "generatedBy": { "value": 1, "by": { "actor": "Z" } } }))
            .contains("no actor named `Z`")
    );
    assert!(
        err(json!({ "forEachActor": { "function": "nope", "assert": { "eq": [1, 1] } } }))
            .contains("no actor runs function `nope`")
    );
    assert!(
        err(json!({ "forEachActor": { "assert": { "same": [{ "argument": { "path": "missing" } }, 1] } } }))
            .contains("has no argument `missing`")
    );
    assert!(
        err(json!({ "eq": [{ "rowMatching": { "table": "tokens", "match": { "nope": 1 } } }, 1] }))
            .contains("unknown column `tokens.nope`")
    );
    // Unknown fields are refused, as in every invariant object.
    assert!(
        serde_json::from_value::<Formula>(json!({ "generatedBy": { "value": 1, "actor": "A" } }))
            .is_err()
    );
}

// ---------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------

#[test]
fn phase_16_formulas_round_trip_without_default_fields() {
    let doc = json!({ "forEachActor": { "function": "f", "assert": { "and": [
        { "generatedBy": { "value": { "returned": { "path": "id" } }, "by": "self", "generator": "call" } },
        { "generatedBy": { "value": { "field": "x" }, "by": { "actor": "A" }, "generator": { "column": "t.c" } } },
        { "generatedBy": { "value": 1, "by": { "function": "g" } } },
        { "sameGenerated": [{ "returned": { "actor": "A" } }, null] },
        { "distinctGenerated": ["a", 1] },
        { "same": [{ "argument": { "path": "params.id" } }, { "argument": { "actor": "B", "path": "id" } }] },
        { "forEachRow": { "table": "t", "where": { "k": 1 }, "assert": { "eq": [
            { "countMatching": { "table": "u", "match": { "id": { "field": "uid" } }, "insertedBy": "self" } }, 1 ] } } },
        { "forEachActor": { "completed": false, "assert": { "eq": [
            { "rowMatching": { "table": "u", "where": { "a": 2 }, "column": "c" } }, 2 ] } } }
    ] } } });
    let f: Formula = serde_json::from_value(doc.clone()).expect("parses");
    assert_eq!(serde_json::to_value(&f).unwrap(), doc);
}

#[test]
fn earlier_invariants_serialize_byte_identically() {
    // Representative Phase 6-15 invariants: their serialized form is fixed
    // (new fields and variants are additive and omitted when unused).
    let cases = [
        (
            r#"{"name":"no-oversell","when":"final","description":"","assert":{"le":[{"actors":{"function":"purchase","outcome":"ok"}},{"initial":{"table":"products","where":{"id":"product-1"},"column":"stock"}}]}}"#,
            r#"{"name":"no-oversell","when":"final","description":"","assert":{"le":[{"actors":{"function":"purchase","outcome":"ok"}},{"initial":{"table":"products","where":{"id":"product-1"},"column":"stock"}}]}}"#,
        ),
        (
            r#"{"name":"claimed","assert":{"le":[{"actors":{"function":"claimTask","outcome":"ok","returned":true}},1]}}"#,
            r#"{"name":"claimed","when":"final","description":"","assert":{"le":[{"actors":{"function":"claimTask","outcome":"ok","returned":true}},1]}}"#,
        ),
        (
            r#"{"name":"x","when":"always","assert":{"and":[{"eq":[{"stuck":null},0]},{"implies":[{"gt":[{"count":{"table":"t","where":{"a":1}}},0]},{"ge":[{"sum":{"table":"t","column":"n"}},{"sub":[{"app":"v"},{"externals":{"channel":"pay","key":"k","distinctKeys":true}}]}]}]},{"not":{"ne":[{"row":{"table":"t","column":"n"}},null]}}]}}"#,
            r#"{"name":"x","when":"always","description":"","assert":{"and":[{"eq":[{"stuck":null},0]},{"implies":[{"gt":[{"count":{"table":"t","where":{"a":1},"column":null}},0]},{"ge":[{"sum":{"table":"t","where":{},"column":"n"}},{"sub":[{"app":"v"},{"externals":{"channel":"pay","key":"k","distinctKeys":true}}]}]}]},{"not":{"ne":[{"row":{"table":"t","where":{},"column":"n"}},null]}}]}}"#,
        ),
    ];
    for (input, expected) in cases {
        let i: Invariant = serde_json::from_str(input).expect("parses");
        assert_eq!(serde_json::to_string(&i).unwrap(), expected);
        assert!(!i.formula.uses_identity());
    }
    // Every invariant of every committed scenario still parses, and its
    // serialization parses back to the same value.
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = 0;
    let mut stack = vec![
        root.join("fixtures"),
        root.join("crates/midwrite-model/tests/data"),
    ];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !p.ends_with("node_modules") {
                    stack.push(p);
                }
                continue;
            }
            if p.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let Ok(doc) =
                serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&p).unwrap())
            else {
                continue;
            };
            let Some(invs) = doc.get("invariants").and_then(|v| v.as_array()) else {
                continue;
            };
            if doc.get("kind").is_some()
                || doc.get("actors").is_none()
                || invs.iter().any(|v| v.get("assert").is_none())
            {
                continue; // contracts and replay manifests are not scenarios
            }
            for v in invs {
                let i: Invariant = serde_json::from_value(v.clone())
                    .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                let back: Invariant =
                    serde_json::from_value(serde_json::to_value(&i).unwrap()).unwrap();
                assert_eq!(i, back, "{}", p.display());
                assert!(
                    !serde_json::to_string(&i).unwrap().contains("insertedBy"),
                    "{}",
                    p.display()
                );
            }
            files += 1;
        }
    }
    assert!(files > 50, "scenario files found: {files}");
}

#[test]
fn the_declared_program_language_generates_values() {
    let m = tokens_model();
    let main = &m.actors[0].codes[0];
    assert!(main.iter().any(|s| matches!(
        &s.op,
        midwrite_model::ir::Op::Generate { source, .. } if source == "nanoid(21)"
    )));
    // `generate` followed by anything but one string is an expression.
    let doc = json!({
        "schemaVersion": 1, "name": "g",
        "database": { "tables": [{ "name": "t", "columns": [{"name": "id", "notNull": true, "type": "int"}], "primaryKey": ["id"] }] },
        "actors": [{ "name": "A", "function": "f", "arguments": { "generate": 1 }, "program": "let x = generate + 1\nreturn x\n" }],
        "invariants": [{ "name": "t", "assert": { "eq": [0, 0] } }]
    });
    let m = load(doc).expect("loads");
    assert!(matches!(
        m.actors[0].codes[0][0].op,
        midwrite_model::ir::Op::Let { .. }
    ));
}

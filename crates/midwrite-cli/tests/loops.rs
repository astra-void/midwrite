//! Bounded loops, retry semantics, caught errors and
//! request bodies.
//!
//! * `fixtures/apps/loops`: loop shapes and their classification
//!   (BOUNDED_UNROLLING / TRUNCATED / UNKNOWN / UNSUPPORTED), caught-error
//!   inspection (drizzle-orm 0.45's `DrizzleQueryError`, a thrown `Error`,
//!   an unmodeled thrown class), a route handler's `request.json()` and
//!   `Response.json(...)`.
//! * `fixtures/apps/retry`: a `for` retry loop that repeats an insert
//!   after an ambiguous COMMIT (model counterexample, reproduced on the
//!   original code against PostgreSQL), its idempotent counterpart (no
//!   counterexample; the original code keeps the invariant along the model's
//!   lost-acknowledgment-then-retry schedule), and a SERIALIZABLE
//!   transaction re-run on SQLSTATE 40001.
//!
//! Runs that need PostgreSQL or Node skip without them unless
//! `MIDWRITE_REQUIRE_REPLAY=1`.

use std::path::{Path, PathBuf};
use std::process::Command;

use midwrite_analyzer::TsActorProvider;
use midwrite_model::model::Model;
use midwrite_model::report::{CheckOptions, check};
use midwrite_model::value::Value as V;
use midwrite_model::witness::{enumerate_schedules, replay};
use serde_json::{Value as Json, json};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_midwrite"))
}

macro_rules! require_runtime {
    () => {
        let missing = midwrite_replay::postgres::Binaries::discover()
            .err()
            .or_else(|| {
                Command::new("node")
                    .arg("--version")
                    .output()
                    .is_err()
                    .then(|| "node is not installed".to_owned())
            });
        if let Some(why) = missing {
            assert!(
                std::env::var_os("MIDWRITE_REQUIRE_REPLAY").is_none(),
                "PostgreSQL and Node are required: {why}"
            );
            eprintln!("skipping: {why}");
            return;
        }
    };
}

// -- loop shapes --------------------------------------------------------------

fn shapes_root() -> PathBuf {
    repo().join("fixtures/apps/loops")
}

fn counters(rows: Json) -> Json {
    json!({
        "tables": [{
            "name": "counters",
            "columns": [
                {"name": "id", "type": "int", "notNull": true},
                {"name": "value", "type": "int", "notNull": true},
                {"name": "label", "type": "text", "notNull": true}
            ],
            "primaryKey": ["id"]
        }],
        "initialState": {"counters": rows}
    })
}

fn shapes_model(
    function: &str,
    arguments: Json,
    bounds: Json,
    invariant: Json,
) -> (Model, Vec<String>) {
    let root = shapes_root();
    let doc = json!({
        "schemaVersion": 1,
        "name": "phase16-loops",
        "features": ["js-values"],
        "extraction": {"root": root.display().to_string()},
        "database": counters(json!([{"id": 1, "value": 0, "label": "old"}])),
        "actors": [{"name": "A", "function": function, "file": "src/shapes.ts", "arguments": arguments}],
        "bounds": bounds,
        "invariants": [{"name": "property", "when": "final", "assert": invariant}]
    });
    let provider = TsActorProvider::new(&root);
    let (model, report) = midwrite_model::scenario::load_str(&doc.to_string(), &root, &provider)
        .unwrap_or_else(|e| panic!("{function}: {e:?}"));
    let assumptions = report
        .assumptions
        .into_iter()
        .flat_map(|(_, a)| a)
        .collect();
    (model, assumptions)
}

fn trivial() -> Json {
    json!({"le": [0, 1]})
}

/// (outcome, value, label) of the counter row in every final state.
fn finals(model: &Model) -> Vec<(String, i64, String)> {
    let (schedules, truncated) = enumerate_schedules(model, 10_000);
    assert!(!truncated);
    let mut out: Vec<_> = schedules
        .iter()
        .map(|s| {
            let r = replay(model, s, false).unwrap();
            let row = &r.state.committed_rows(model, 0)[0];
            let value = match row.get("value") {
                Some(V::Int(n)) => *n,
                other => panic!("{other:?}"),
            };
            let label = match row.get("label") {
                Some(V::Str(s)) => s.clone(),
                other => panic!("{other:?}"),
            };
            (r.state.actors[0].outcome.label(), value, label)
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn class_of(assumptions: &[String]) -> Vec<String> {
    assumptions
        .iter()
        .filter(|a| a.starts_with("loop "))
        .map(|a| a.split_whitespace().nth(1).unwrap_or_default().to_owned())
        .collect()
}

#[test]
fn bounded_loops_run_every_iteration_in_order() {
    for (function, value) in [
        ("countUp", 3),
        ("countToLimit", 5),
        ("skipAndStop", 2),
        ("whileCounter", 2),
        ("doOnce", 1),
        ("countDown", 2),
    ] {
        let (model, assumptions) = shapes_model(function, json!([1]), json!({}), trivial());
        assert!(
            model.actors[0].has_unknown().is_empty(),
            "{function}: {:?}",
            model.actors[0].has_unknown()
        );
        assert_eq!(
            class_of(&assumptions),
            vec!["BOUNDED_UNROLLING"],
            "{function}: {assumptions:?}"
        );
        assert_eq!(
            finals(&model),
            vec![("ok".to_owned(), value, "old".to_owned())],
            "{function}"
        );
    }
}

#[test]
fn a_loop_without_an_established_bound_is_exact_within_it_and_unsupported_beyond() {
    // The counter is assigned in the body: no bound is established, yet the
    // unrolled iterations are exact (two writes).
    let (model, assumptions) = shapes_model("jumpAhead", json!([1]), json!({}), trivial());
    assert_eq!(class_of(&assumptions), vec!["TRUNCATED"], "{assumptions:?}");
    assert!(
        assumptions
            .iter()
            .any(|a| a.contains("loop may execute beyond the explored bound")),
        "{assumptions:?}"
    );
    assert_eq!(finals(&model), vec![("ok".to_owned(), 2, "old".to_owned())]);
    // A bound from an argument: two iterations stay within the unrolling
    // limit; nine reach the unsupported residual.
    let (model, assumptions) = shapes_model("countTo", json!([1, 2]), json!({}), trivial());
    assert_eq!(class_of(&assumptions), vec!["TRUNCATED"], "{assumptions:?}");
    let r = check(&model, &CheckOptions::default());
    assert_eq!(r.exploration.verdict, "NO_COUNTEREXAMPLE_WITHIN_BOUNDS");
    assert_eq!(finals(&model), vec![("ok".to_owned(), 2, "old".to_owned())]);
    let (model, _) = shapes_model("countTo", json!([1, 9]), json!({}), trivial());
    let r = check(&model, &CheckOptions::default());
    assert_eq!(r.exploration.verdict, "UNSUPPORTED");
    assert!(
        r.exploration.unsupported.iter().any(|u| u
            .detail
            .contains("loop may execute beyond the explored bound")),
        "{:?}",
        r.exploration.unsupported
    );
}

#[test]
fn loops_outside_the_subset_are_refused_with_their_classification() {
    for (function, class, detail) in [
        ("varHead", "UNSUPPORTED", "`var`"),
        (
            "unknownCondition",
            "UNKNOWN",
            "loop condition is not modeled",
        ),
    ] {
        let (model, assumptions) = shapes_model(function, json!([1]), json!({}), trivial());
        assert_eq!(
            class_of(&assumptions),
            vec![class],
            "{function}: {assumptions:?}"
        );
        let unknown = model.actors[0].has_unknown();
        assert!(
            unknown.iter().any(|(d, _)| d.contains(detail)),
            "{function}: {unknown:?}"
        );
        let r = check(&model, &CheckOptions::default());
        assert_eq!(r.exploration.verdict, "UNSUPPORTED", "{function}");
    }
}

#[test]
fn a_retry_loop_re_executes_its_body_after_a_lost_acknowledgment() {
    // `while (true)` with an explicit attempt count: TRUNCATED (no bound
    // from the source), exact at run time. An UPDATE whose acknowledgment is
    // lost is either applied (then re-run: +2) or not (+1); the loop runs it
    // again for real, never merging the attempts.
    let (model, assumptions) = shapes_model(
        "retryForever",
        json!([1]),
        json!({"maxInFlight": 1}),
        trivial(),
    );
    assert_eq!(class_of(&assumptions), vec!["TRUNCATED"], "{assumptions:?}");
    let outcomes = finals(&model);
    assert!(
        outcomes.contains(&("ok".to_owned(), 1, "old".to_owned())),
        "{outcomes:?}"
    );
    assert!(
        outcomes.contains(&("ok".to_owned(), 2, "old".to_owned())),
        "{outcomes:?}"
    );
    let r = check(&model, &CheckOptions::default());
    assert!(
        r.exploration.unsupported.is_empty(),
        "{:?}",
        r.exploration.unsupported
    );
}

#[test]
fn caught_errors_expose_only_what_their_modeled_origin_says() {
    // drizzle-orm 0.45: the duplicate key's SQLSTATE is `cause.code`.
    let (model, assumptions) = shapes_model("insertOrNote", json!([1]), json!({}), trivial());
    assert!(
        assumptions
            .iter()
            .any(|a| a.starts_with("caught-errors: ") && a.contains("DrizzleQueryError")),
        "{assumptions:?}"
    );
    assert_eq!(
        finals(&model),
        vec![("ok".to_owned(), 0, "duplicate".to_owned())]
    );
    // ... and the wrapper's own `code` is undefined: the error reaches the
    // caller with its SQLSTATE.
    let (model, _) = shapes_model("insertOrNoteOnCode", json!([1]), json!({}), trivial());
    let outcomes = finals(&model);
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert!(outcomes[0].0.contains("23505"), "{outcomes:?}");
    assert_eq!(outcomes[0].2, "old");
    // A thrown `Error`: `instanceof Error` and its message.
    let (model, _) = shapes_model("thrownMessage", json!([1]), json!({}), trivial());
    assert_eq!(
        finals(&model),
        vec![("ok".to_owned(), 0, "not today".to_owned())]
    );
    // A thrown instance of a class the model does not know: its `code` is
    // unknown, never `undefined`.
    let (model, _) = shapes_model("thrownCustom", json!([1]), json!({}), trivial());
    let r = check(&model, &CheckOptions::default());
    assert_eq!(r.exploration.verdict, "UNSUPPORTED");
    assert!(
        r.exploration
            .unsupported
            .iter()
            .any(|u| u.detail.contains("error object is not modeled")),
        "{:?}",
        r.exploration.unsupported
    );
}

#[test]
fn a_route_handler_reads_its_json_body_and_answers_a_response_value() {
    let returned = json!({"status": 201, "body": {"ok": true}});
    let (model, assumptions) = shapes_model(
        "POST",
        json!([{"id": 1, "label": "from-body"}]),
        json!({}),
        json!({"eq": [{"actors": {"outcome": "ok", "returned": returned}}, 1]}),
    );
    assert!(
        assumptions
            .iter()
            .any(|a| a.starts_with("request: ") && a.contains("valid JSON")),
        "{assumptions:?}"
    );
    assert_eq!(
        finals(&model),
        vec![("ok".to_owned(), 0, "from-body".to_owned())]
    );
    let r = check(&model, &CheckOptions::default());
    assert_eq!(r.exploration.verdict, "NO_COUNTEREXAMPLE_WITHIN_BOUNDS");
    // A second read of the body is refused.
    let (model, _) = shapes_model("PUT", json!([{"id": 1}]), json!({}), trivial());
    assert!(
        model.actors[0]
            .has_unknown()
            .iter()
            .any(|(d, _)| d.contains("read twice")),
        "{:?}",
        model.actors[0].has_unknown()
    );
}

// -- the retry application ------------------------------------------------------

fn app() -> PathBuf {
    repo().join("fixtures/apps/retry")
}

fn build_fixture(dir: &Path) {
    if dir.join("dist/verify").is_dir() {
        return;
    }
    let status = Command::new("npm")
        .args(["run", "build"])
        .current_dir(dir)
        .status()
        .expect("npm");
    assert!(status.success(), "building {} failed", dir.display());
}

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "midwrite-phase16-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn discover(extra: &[&str]) -> Json {
    let output = bin()
        .arg("discover")
        .arg(app())
        .args(["--model-check", "--format", "json", "--no-timing"])
        .args(extra)
        .output()
        .unwrap();
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!("{e}\n{}", String::from_utf8_lossy(&output.stderr));
    })
}

fn candidate<'a>(report: &'a Json, class: &str, entry: &str) -> &'a Json {
    report["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| {
            c["class"] == class
                && c["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|e| e["selector"].as_str().unwrap().ends_with(entry))
        })
        .unwrap_or_else(|| panic!("no {class} candidate of {entry}: {report:#}"))
}

fn has_assumption(c: &Json, prefix: &str) -> bool {
    c["assumptions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["text"].as_str().is_some_and(|t| t.starts_with(prefix)))
}

fn model_check(file: &Path, extra: &[&str]) -> Json {
    let output = bin()
        .arg("model-check")
        .arg(file)
        .args(["--format", "json", "--no-timing"])
        .args(extra)
        .output()
        .unwrap();
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!("{e}\n{}", String::from_utf8_lossy(&output.stderr));
    })
}

#[test]
fn a_retry_loop_that_repeats_a_committed_insert_is_found_and_its_idempotent_counterpart_is_not() {
    let r = discover(&[]);
    for entry in ["#placeOrder", "#POST"] {
        let c = candidate(&r, "ambiguous-commit", entry);
        assert_eq!(c["status"], "MODEL_COUNTEREXAMPLE", "{c:#}");
        assert!(
            has_assumption(c, "loop BOUNDED_UNROLLING"),
            "{entry}: {c:#}"
        );
    }
    // The route handler's body is its argument.
    assert!(has_assumption(
        candidate(&r, "ambiguous-commit", "#POST"),
        "request: "
    ));
    let safe = candidate(&r, "ambiguous-commit", "#recordPayment");
    assert_eq!(safe["status"], "NO_VIOLATION_WITHIN_BOUNDS", "{safe:#}");
    // The SERIALIZABLE withdrawal re-run on 40001 keeps the balance.
    let w = candidate(&r, "read-modify-write", "#withdraw");
    assert_eq!(w["status"], "NO_VIOLATION_WITHIN_BOUNDS", "{w:#}");
    assert!(has_assumption(w, "loop BOUNDED_UNROLLING"), "{w:#}");
    assert!(has_assumption(w, "caught-errors: "), "{w:#}");
}

#[test]
fn the_serializable_retry_loop_really_retries_on_40001() {
    // Two concurrent withdrawals of 3 from 10: whatever the schedule, the
    // loop re-runs the transaction that PostgreSQL aborts with 40001, so
    // both succeed and the balance ends at 4 (without the retry, one
    // withdrawal fails in some schedule).
    let out = Temp::new("withdraw");
    let r = discover(&[
        "--entry",
        "orders.ts#withdraw",
        "--emit-scenarios",
        out.0.to_str().unwrap(),
    ]);
    let c = candidate(&r, "read-modify-write", "#withdraw");
    let file = out.0.join(c["scenarios"][0]["file"].as_str().unwrap());
    let mut doc: Json = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(doc["actors"][0]["arguments"], json!([1, 3]), "{doc:#}");
    assert_eq!(
        doc["database"]["initialState"]["accounts"],
        json!([{"balance": 10, "id": 1}])
    );
    doc["invariants"] = json!([
        {"name": "both-succeed", "when": "final", "assert": {"eq": [{"actors": {"outcome": "ok"}}, 2]}},
    ]);
    let both = out.0.join("both.json");
    std::fs::write(&both, doc.to_string()).unwrap();
    let mc = model_check(&both, &[]);
    assert_eq!(
        mc["exploration"]["verdict"], "NO_COUNTEREXAMPLE_WITHIN_BOUNDS",
        "{mc:#}"
    );
    // The property is not vacuous: the same loop testing the wrapper's own
    // `code` (undefined with drizzle-orm 0.45) never retries, and a
    // withdrawal fails with the serialization failure.
    for actor in doc["actors"].as_array_mut().unwrap() {
        actor["entry"]["selector"] = json!("src/services/orders.ts#withdrawOnCode");
    }
    doc["sourceVerification"] = Json::Null;
    doc.as_object_mut().unwrap().remove("sourceVerification");
    let on_code = out.0.join("on-code.json");
    std::fs::write(&on_code, doc.to_string()).unwrap();
    let mc = model_check(&on_code, &[]);
    assert_eq!(
        mc["exploration"]["verdict"], "COUNTEREXAMPLE_FOUND",
        "{mc:#}"
    );
    assert!(mc.to_string().contains("40001"), "{mc:#}");
}

#[test]
fn the_lost_commit_retry_reproduces_on_the_original_code_and_the_idempotent_loop_holds() {
    require_runtime!();
    build_fixture(&app());
    let out = Temp::new("source");
    let r = discover(&[
        "--entry",
        "orders.ts",
        "--emit-scenarios",
        out.0.to_str().unwrap(),
    ]);
    // (a) the plain insert: the original code inserts a second row.
    let c = candidate(&r, "ambiguous-commit", "#placeOrder");
    let file = c["scenarios"][0]["file"].as_str().unwrap();
    let mc = model_check(
        &out.0.join(file),
        &[
            "--verify-source",
            "--allow-execution",
            "--source-repeat",
            "2",
        ],
    );
    let sv = &mc["sourceVerification"];
    assert_eq!(sv["status"], "reproduced", "{sv:#}");
    assert_eq!(
        mc["evidence"]["level"], "original-code-connection-loss-reproduced",
        "{mc:#}"
    );
    // (a') the idempotent insert: no model counterexample; along the
    // model's shortest schedule where the acknowledgment of the COMMIT is
    // lost, PostgreSQL commits it, and the loop runs the transaction again,
    // the original code keeps one row per key.
    let safe = candidate(&r, "ambiguous-commit", "#recordPayment");
    let file = out.0.join(safe["scenarios"][0]["file"].as_str().unwrap());
    let mut doc: Json = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let actor = doc["actors"][0]["name"].as_str().unwrap().to_owned();
    doc["sourceVerification"]["recoveryProbe"] = json!({
        "require": [
            format!("lost-committed:{actor}"),
            format!("server-complete:{actor}"),
            format!("statement:{actor}")
        ]
    });
    let probe = out.0.join("probe.json");
    std::fs::write(&probe, doc.to_string()).unwrap();
    let mc = model_check(
        &probe,
        &[
            "--verify-source",
            "--allow-execution",
            "--source-repeat",
            "2",
        ],
    );
    assert_eq!(
        mc["exploration"]["verdict"], "NO_COUNTEREXAMPLE_WITHIN_BOUNDS",
        "{mc:#}"
    );
    // The held COMMIT reached PostgreSQL after the client saw the broken
    // connection; the loop's next transaction inserted nothing new.
    let sv = &mc["sourceVerification"];
    assert_eq!(sv["status"], "recovery-reproduced", "{sv:#}");
    assert_eq!(
        sv["evidence"], "ORIGINAL_CODE_RECOVERY_REPRODUCED",
        "{sv:#}"
    );
    let runs = sv["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2, "{sv:#}");
    for run in runs {
        assert_eq!(run["invariant"], "holds", "{run:#}");
    }
}

#[test]
fn the_serializable_retry_reruns_the_original_transaction_after_40001() {
    require_runtime!();
    build_fixture(&app());
    let out = Temp::new("probe40001");
    let r = discover(&[
        "--entry",
        "orders.ts#withdraw",
        "--emit-scenarios",
        out.0.to_str().unwrap(),
    ]);
    let c = candidate(&r, "read-modify-write", "#withdraw");
    let file = out.0.join(c["scenarios"][0]["file"].as_str().unwrap());
    let mut doc: Json = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let a = doc["actors"][0]["name"].as_str().unwrap().to_owned();
    let b = doc["actors"][1]["name"].as_str().unwrap().to_owned();
    // The model's shortest schedule in which one withdrawal commits, the
    // other fails with a serialization failure and then commits through the
    // loop's re-run: the original code follows it.
    doc["sourceVerification"]["recoveryProbe"] = json!({
        "require": [format!("commit:{b}"), "error:40001", format!("commit:{a}")]
    });
    let probe = out.0.join("probe.json");
    std::fs::write(&probe, doc.to_string()).unwrap();
    let mc = model_check(
        &probe,
        &[
            "--verify-source",
            "--allow-execution",
            "--source-repeat",
            "2",
        ],
    );
    assert_eq!(
        mc["sourceVerification"]["status"], "invariant-held",
        "{:#}",
        mc["sourceVerification"]
    );
}

#[test]
fn explain_coverage_states_how_each_loop_was_bounded() {
    let out = bin()
        .args(["discover"])
        .arg(app())
        .args(["--model-check", "--explain-coverage"])
        .output()
        .expect("midwrite runs");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("Assumption (loop, source-derived): loop BOUNDED_UNROLLING at src/services/orders.ts:14:3"),
        "{text}"
    );
    // Without the flag (and without `-v`) assumptions stay out of the text.
    let plain = bin()
        .args(["discover"])
        .arg(app())
        .arg("--model-check")
        .output()
        .expect("midwrite runs");
    assert!(!String::from_utf8_lossy(&plain.stdout).contains("Assumption ("));
}

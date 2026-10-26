//! Phase 7 end to end: source-level extraction of a wrapped Server Action,
//! model counterexamples, and their verification by running the ORIGINAL
//! application code on a disposable PostgreSQL (fixtures/phase7/team-app),
//! plus the legacy-vs-compact path differential over every fixture.
//!
//! Tests that run application code need Node.js >= 22, the PostgreSQL
//! server binaries, and `npm ci` in fixtures/phase7/team-app; without them
//! they print SKIPPED and return, unless `COMMITRIFT_REQUIRE_REPLAY=1`.

use std::path::{Path, PathBuf};
use std::process::Command;

use commitrift_analyzer::{Analyzer, TsActorProvider};
use commitrift_core::analysis::{
    AnalysisOptions, enable_path_differential, take_path_differential,
};
use commitrift_core::config::Config;
use commitrift_model::engine::EventKind;
use commitrift_model::scenario::load_file;
use commitrift_model::source_types::SourceVerification;
use commitrift_model::source_verify::{VerifyRequest, verify};
use commitrift_model::witness::{enumerate_schedules, replay};
use serde_json::Value;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scenario(name: &str) -> PathBuf {
    repo().join("fixtures/phase7/scenarios").join(name)
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_commitrift"))
}

fn missing_dependency() -> Option<String> {
    if commitrift_replay::postgres::Binaries::discover().is_err() {
        return Some("the PostgreSQL server binaries are not installed".to_owned());
    }
    let output = Command::new("node").arg("--version").output();
    let Ok(output) = output else {
        return Some("Node.js is not installed".to_owned());
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let major: u32 = text
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|m| m.parse().ok())
        .unwrap_or(0);
    if major < 22 {
        return Some(format!("Node.js {} is older than 22", text.trim()));
    }
    team_app().err()
}

/// Compile the team application once per test process (offline; it needs
/// `npm ci` to have installed its pinned dependencies).
fn team_app() -> Result<PathBuf, String> {
    static BUILT: std::sync::OnceLock<Result<PathBuf, String>> = std::sync::OnceLock::new();
    BUILT
        .get_or_init(|| {
            let dir = repo().join("fixtures/phase7/team-app");
            if !dir.join("node_modules/drizzle-orm/package.json").is_file() {
                return Err(
                    "the team application's dependencies are not installed: run `npm ci` in fixtures/phase7/team-app"
                        .to_owned(),
                );
            }
            let output = Command::new("node")
                .args(["node_modules/typescript/bin/tsc", "-p", "tsconfig.json"])
                .current_dir(&dir)
                .output()
                .map_err(|e| format!("cannot run tsc: {e}"))?;
            if !output.status.success() {
                return Err(format!(
                    "the team application does not compile: {}",
                    String::from_utf8_lossy(&output.stdout)
                ));
            }
            Ok(dir)
        })
        .clone()
}

macro_rules! require_source_run {
    () => {
        if let Some(reason) = missing_dependency() {
            assert!(
                std::env::var_os("COMMITRIFT_REQUIRE_REPLAY").is_none(),
                "source-verification dependencies required: {reason}"
            );
            eprintln!("SKIPPED: {reason}");
            return;
        }
    };
}

fn model_check_json(path: &Path, extra: &[&str]) -> (i32, Value) {
    let output = bin()
        .arg("model-check")
        .arg(path)
        .args(["--format", "json", "--no-timing"])
        .args(extra)
        .output()
        .unwrap();
    let json: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "not JSON ({e}): {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.code().unwrap_or(-1), json)
}

#[test]
fn a_wrapped_action_counterexample_is_reproduced_by_the_original_code() {
    require_source_run!();
    let (code, r) = model_check_json(
        &scenario("team-app-invite-double-submit.json"),
        &[
            "--verify-source",
            "--allow-execution",
            "--source-repeat",
            "2",
        ],
    );
    assert_eq!(code, 1, "a counterexample exits 1");
    assert_eq!(r["schemaVersion"], 2);
    assert_eq!(r["evidence"]["level"], "original-code-reproduced");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["status"], "reproduced", "{sv:#}");
    assert_eq!(sv["runs"].as_array().unwrap().len(), 2);
    let statements = sv["statements"].as_array().unwrap();
    assert_eq!(statements.len(), 10);
    for s in statements {
        assert_eq!(s["status"], "agree", "{s:#}");
        // Every statement is attributed to the application's source, inside
        // the model operation's span.
        assert!(
            s["runtimeSource"].as_str().unwrap().starts_with("src/"),
            "{s:#}"
        );
        assert!(s.get("differences").is_none(), "{s:#}");
    }
    // The wrapper's own query is outside the modeled callback.
    let outside = sv["outsideModel"].as_array().unwrap();
    assert_eq!(outside.len(), 2);
    assert!(
        outside[0]
            .as_str()
            .unwrap()
            .contains("src/lib/db/queries.ts:17")
    );
    assert!(sv.get("unexpected").is_none());
    assert_eq!(sv["modelTables"], sv["runtimeTables"]);
    assert_eq!(
        sv["runtimeTables"]["invitations"].as_array().unwrap().len(),
        2
    );
    // Extraction report: source facts vs. declared assumptions.
    let x = &r["extraction"][0];
    assert_eq!(x["entry"]["contractStatus"], "consistent");
    assert_eq!(x["sourceCoverage"]["unsupportedOperations"], 0);
    assert_eq!(
        x["entry"]["wrapper"]["invocation"],
        "returned-function-returns-promise"
    );
}

#[test]
fn a_lock_wait_in_an_original_transaction_matches_the_model() {
    require_source_run!();
    let path = scenario("team-app-accept-invitation-twice.json");
    let provider = TsActorProvider::new(repo().join("fixtures/phase7/team-app"));
    let (model, load) = load_file(&path, &provider).unwrap();
    let invariant = &model.invariants[0];
    // A violating schedule in which Accept2's UPDATE waits for Accept1's
    // row lock and resumes after its commit.
    let (schedules, _) = enumerate_schedules(&model, 50_000);
    let (schedule, state) = schedules
        .iter()
        .find_map(|s| {
            let r = replay(&model, s, true).ok()?;
            let blocked = r
                .steps
                .iter()
                .any(|(_, events)| events.iter().any(|e| e.kind == EventKind::Blocked));
            (blocked && r.state.is_finished() && !invariant.holds(&model, &r.state))
                .then(|| (s.clone(), r.state))
        })
        .expect("a violating schedule with a lock wait exists");
    let harness = commitrift_replay::source::load_manifest(
        &repo().join("fixtures/phase7/team-app/commitrift-source.json"),
    )
    .unwrap();
    let sv: SourceVerification = verify(&VerifyRequest {
        model: &model,
        schedule: &schedule,
        final_state: &state,
        invariant,
        harness: &harness,
        spec: load.source.as_ref().unwrap(),
        entries: &load.entries,
        allow_execution: true,
        repeat: 1,
        probe: false,
        extracted_schema: None,
    });
    assert_eq!(sv.status, "reproduced", "{sv:#?}");
    let waited: Vec<_> = sv
        .statements
        .iter()
        .filter(|s| s.predicted.starts_with("waits for"))
        .collect();
    assert_eq!(waited.len(), 1, "{:#?}", sv.statements);
    assert_eq!(waited[0].status, "agree");
    assert!(waited[0].sql.starts_with("update \"invitations\""));
    assert!(sv.statements.iter().any(|s| s.sql == "begin"));
    assert!(sv.statements.iter().any(|s| s.sql == "commit"));
}

#[test]
fn a_harness_input_that_contradicts_the_scenario_is_not_reported_as_reproduced() {
    require_source_run!();
    let mut doc: Value = serde_json::from_str(
        &std::fs::read_to_string(scenario("team-app-invite-double-submit.json")).unwrap(),
    )
    .unwrap();
    for actor in ["Submit1", "Submit2"] {
        doc["sourceVerification"]["actors"][actor]["input"]["form"]["email"] =
            Value::from("someone-else@example.com");
    }
    let dir = std::env::temp_dir().join(format!("cr-p7-mismatch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scenario.json");
    doc["extraction"]["root"] = Value::from(
        repo()
            .join("fixtures/phase7/team-app")
            .display()
            .to_string(),
    );
    doc["sourceVerification"]["harness"] = Value::from(
        repo()
            .join("fixtures/phase7/team-app/commitrift-source.json")
            .display()
            .to_string(),
    );
    std::fs::write(&path, doc.to_string()).unwrap();
    let (_, r) = model_check_json(&path, &["--verify-source", "--allow-execution"]);
    let _ = std::fs::remove_dir_all(&dir);
    let sv = &r["sourceVerification"];
    assert_ne!(sv["status"], "reproduced", "{sv:#}");
    assert_eq!(sv["runs"][0]["tablesAgree"], false);
    assert_ne!(r["evidence"]["level"], "original-code-reproduced");
    assert!(
        r["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "CP7103")
    );
}

#[test]
fn application_code_never_runs_without_the_explicit_authorization() {
    // Static only: no dependency needed, nothing may start.
    let (_, r) = model_check_json(
        &scenario("team-app-invite-double-submit.json"),
        &["--verify-source"],
    );
    let sv = &r["sourceVerification"];
    assert_eq!(sv["status"], "blocked");
    assert_eq!(sv["performed"], false);
    assert!(
        sv["problems"][0]
            .as_str()
            .unwrap()
            .contains("--allow-execution")
    );
    assert!(sv.get("statements").is_none());
    assert_eq!(r["evidence"]["level"], "model-counterexample");
    // --validate-postgres replays model SQL only; it never runs the
    // application and never produces a source verification.
    let (_, r) = model_check_json(&scenario("team-app-invite-double-submit.json"), &[]);
    assert!(r.get("sourceVerification").is_none());
}

#[test]
fn declared_programs_are_not_eligible_for_source_verification() {
    let dir = std::env::temp_dir().join(format!("cr-p7-declared-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.json");
    let doc = serde_json::json!({
        "schemaVersion": 1,
        "name": "declared",
        "database": {"tables": [{"name": "t", "columns": [{"name": "id", "type": "int", "notNull": true}], "primaryKey": ["id"]}]},
        "actors": [
            {"name": "A", "function": "a", "program": "insert t set @id = 1\n"},
            {"name": "B", "function": "b", "program": "insert t set @id = 2\n"}
        ],
        "invariants": [{"name": "one", "assert": {"le": [{"count": {"table": "t"}}, 1]}}],
        "sourceVerification": {
            "harness": repo().join("fixtures/phase7/team-app/commitrift-source.json").display().to_string(),
            "actors": {"A": {"entry": "inviteTeamMember", "input": {}}, "B": {"entry": "inviteTeamMember", "input": {}}}
        }
    });
    std::fs::write(&path, doc.to_string()).unwrap();
    let (_, r) = model_check_json(&path, &["--verify-source", "--allow-execution"]);
    let _ = std::fs::remove_dir_all(&dir);
    let sv = &r["sourceVerification"];
    assert_eq!(sv["status"], "unsupported", "{sv:#}");
    assert_eq!(sv["performed"], false);
    assert!(
        sv["problems"][0]
            .as_str()
            .unwrap()
            .contains("declared in the scenario")
    );
}

/// Compact summaries reproduce the legacy walk exactly on every fixture.
#[test]
fn compact_summaries_equal_the_legacy_walk_on_every_fixture() {
    enable_path_differential(true);
    let _ = take_path_differential();
    let fixtures = repo().join("fixtures");
    for depth in [3, 6] {
        for dir in [
            "vulnerable",
            "safe",
            "unsupported",
            "projects",
            "replay",
            "concurrency",
            "phase7",
        ] {
            let analyzer = Analyzer::new(Config::default()).with_options(AnalysisOptions {
                max_call_depth: depth,
                ..AnalysisOptions::default()
            });
            let _ = analyzer.trace_project(&[fixtures.join(dir)]);
            let _ = analyzer.analyze_project(&[fixtures.join(dir)]);
        }
    }
    enable_path_differential(false);
    let (functions, stats, differences) = take_path_differential();
    assert!(functions > 300, "only {functions} functions compared");
    assert!(stats.built > 0, "no call scope was summarized");
    assert!(differences.is_empty(), "{differences:#?}");
}

// ---------------------------------------------------------------------------
// Control flow: try / catch / finally (fixtures/phase7/control)
// ---------------------------------------------------------------------------

fn control_model(function: &str, initial: Value, bounds: Value) -> commitrift_model::model::Model {
    control_model_with(function, serde_json::json!([1]), initial, bounds)
}

fn control_model_with(
    function: &str,
    arguments: Value,
    initial: Value,
    bounds: Value,
) -> commitrift_model::model::Model {
    let root = repo().join("fixtures/phase7/control");
    let doc = serde_json::json!({
        "schemaVersion": 1,
        "name": "control",
        "channels": {"payment": {}, "release": {}},
        "extraction": {
            "root": root.display().to_string(),
            "externals": [
                {"callee": "chargePayment", "channel": "payment", "keyArgument": 0},
                {"callee": "releaseResource", "channel": "release", "keyArgument": 0}
            ]
        },
        "database": {
            "tables": [
                {"name": "orders", "columns": [{"name": "id", "type": "int", "notNull": true}, {"name": "status", "type": "text", "notNull": true}], "primaryKey": ["id"]},
                {"name": "failures", "columns": [{"name": "orderId", "type": "int", "notNull": true}], "primaryKey": ["orderId"]}
            ],
            "initialState": initial
        },
        "actors": [{"name": "A", "function": function, "file": "src/orders.ts", "arguments": arguments}],
        "bounds": bounds,
        "invariants": [{"name": "trivial", "assert": {"le": [0, 1]}}]
    });
    let provider = TsActorProvider::new(&root);
    commitrift_model::scenario::load_str(&doc.to_string(), &root, &provider)
        .unwrap()
        .0
}

/// (outcome, external channels in order, orders rows, failures rows,
/// whether a failure was caught) of every final state.
fn control_outcomes(
    model: &commitrift_model::model::Model,
) -> Vec<(String, Vec<String>, usize, usize, bool)> {
    let (schedules, truncated) = enumerate_schedules(model, 10_000);
    assert!(!truncated);
    let mut out: Vec<_> = schedules
        .iter()
        .map(|s| {
            let r = replay(model, s, true).unwrap();
            let caught = r
                .steps
                .iter()
                .any(|(_, events)| events.iter().any(|e| e.text.starts_with("caught:")));
            (
                r.state.actors[0].outcome.label(),
                r.state
                    .externals
                    .iter()
                    .map(|x| x.channel.clone())
                    .collect(),
                r.state.committed_rows(model, 0).len(),
                r.state.committed_rows(model, 1).len(),
                caught,
            )
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

#[test]
fn exceptions_run_catch_and_finally_but_a_crash_runs_neither() {
    let empty = serde_json::json!({"orders": [], "failures": []});
    // Success: the charge, the insert, then `finally`.
    let ok = control_outcomes(&control_model(
        "processOrder",
        empty.clone(),
        serde_json::json!({}),
    ));
    assert_eq!(
        ok,
        vec![(
            "ok".to_owned(),
            vec!["payment".to_owned(), "release".to_owned()],
            1,
            0,
            false
        )]
    );
    // The insert fails (duplicate key): the handler records the failure,
    // `finally` still releases, and the actor completes normally.
    let taken = serde_json::json!({"orders": [{"id": 1, "status": "paid"}], "failures": []});
    let failed = control_outcomes(&control_model("processOrder", taken, serde_json::json!({})));
    assert_eq!(
        failed,
        vec![(
            "ok".to_owned(),
            vec!["payment".to_owned(), "release".to_owned()],
            1,
            1,
            true
        )]
    );
    // A process crash is not an exception: neither the handler nor
    // `finally` runs. A crash right after the charge leaves no release.
    let crashing = control_outcomes(&control_model(
        "processOrder",
        empty,
        serde_json::json!({"maxCrashes": 1}),
    ));
    assert!(
        crashing.contains(&(
            "crashed".to_owned(),
            vec!["payment".to_owned()],
            0,
            0,
            false
        )),
        "{crashing:?}"
    );
    assert!(
        crashing
            .iter()
            .all(|(outcome, effects, ..)| outcome == "crashed"
                || effects.last().map(String::as_str) == Some("release"))
    );
}

#[test]
fn a_helpers_handler_does_not_catch_its_callers_failure() {
    let empty = serde_json::json!({"orders": [], "failures": []});
    let outcomes = control_outcomes(&control_model("requireOrder", empty, serde_json::json!({})));
    assert_eq!(
        outcomes,
        vec![("failed: no such order".to_owned(), vec![], 0, 0, false)]
    );
}

#[test]
fn exception_handling_beyond_the_subset_is_unsupported_not_guessed() {
    let empty = serde_json::json!({"orders": [], "failures": []});
    // The insert of `inspectsError` must fail for its handler (which reads
    // `error.code`) to be reached; an unreachable unknown changes nothing.
    let taken = serde_json::json!({"orders": [{"id": 1, "status": "paid"}], "failures": []});
    let quiet = control_model("inspectsError", empty.clone(), serde_json::json!({}));
    let report =
        commitrift_model::report::check(&quiet, &commitrift_model::report::CheckOptions::default());
    assert_eq!(
        report.exploration.verdict,
        "NO_COUNTEREXAMPLE_WITHIN_BOUNDS"
    );
    for (function, initial, detail) in [
        (
            "inspectsError",
            taken,
            "caught error's properties are not modeled",
        ),
        (
            "tryInTransaction",
            empty.clone(),
            "inside a transaction callback",
        ),
        (
            "returnThroughFinally",
            empty.clone(),
            "inside a `try` with `finally`",
        ),
    ] {
        let model = control_model(function, initial, serde_json::json!({}));
        let unknown = model.actors[0].has_unknown();
        assert!(
            unknown.iter().any(|(d, _)| d.contains(detail)),
            "{function}: {unknown:?}"
        );
        let report = commitrift_model::report::check(
            &model,
            &commitrift_model::report::CheckOptions::default(),
        );
        assert_eq!(report.exploration.verdict, "UNSUPPORTED", "{function}");
    }
}

#[test]
fn switch_and_bounded_loops_follow_javascript_semantics() {
    let one = serde_json::json!({"orders": [{"id": 1, "status": "new"}], "failures": []});
    let status_after = |function: &str, arguments: Value| -> (String, String) {
        let model = control_model_with(function, arguments, one.clone(), serde_json::json!({}));
        assert!(
            model.actors[0].has_unknown().is_empty(),
            "{function}: {:?}",
            model.actors[0].has_unknown()
        );
        let (schedules, _) = enumerate_schedules(&model, 100);
        let r = replay(&model, &schedules[0], false).unwrap();
        let row = &r.state.committed_rows(&model, 0)[0];
        (
            r.state.actors[0].outcome.label(),
            row.get("status").unwrap().render(),
        )
    };
    assert_eq!(
        status_after("setStatus", serde_json::json!([1, "pay"])),
        ("ok".into(), "\"paid\"".into())
    );
    // `case "cancel": case "void":` falls through.
    assert_eq!(
        status_after("setStatus", serde_json::json!([1, "void"])),
        ("ok".into(), "\"cancelled\"".into())
    );
    assert_eq!(
        status_after("setStatus", serde_json::json!([1, "frobnicate"])),
        ("failed: unknown action".into(), "\"new\"".into())
    );
    // `continue` skips "b", `break` stops after "c": "d" is never written.
    assert_eq!(
        status_after("touchAll", serde_json::json!([1])),
        ("ok".into(), "\"c\"".into())
    );
    // An iteration count the model cannot know is not unrolled.
    let unknown = control_model_with(
        "deleteEach",
        serde_json::json!([null]),
        one.clone(),
        serde_json::json!({}),
    )
    .actors[0]
        .has_unknown();
    assert!(
        unknown.iter().any(|(d, _)| d.contains("unknown length")),
        "{unknown:?}"
    );
}

/// The original saas-starter callback (corpora/fetch.sh, rev 6e33e58) is
/// extracted as written: no adapted reproduction. Static only.
#[test]
fn the_original_saas_starter_invite_callback_is_extracted_as_written() {
    if !repo()
        .join(".corpora/saas-starter/app/(login)/actions.ts")
        .is_file()
    {
        eprintln!("SKIPPED: the saas-starter corpus is not fetched (corpora/fetch.sh)");
        return;
    }
    let (code, r) = model_check_json(&scenario("saas-starter-original-invite.json"), &[]);
    assert_eq!(code, 1);
    assert_eq!(r["exploration"]["verdict"], "COUNTEREXAMPLE_FOUND");
    let x = &r["extraction"][0];
    assert_eq!(x["entry"]["anchor"], "app/(login)/actions.ts@401:3");
    assert_eq!(x["entry"]["wrapper"]["wrapper"], "validatedActionWithUser");
    assert_eq!(
        x["entry"]["wrapper"]["definedAt"],
        "lib/auth/middleware.ts:37:8"
    );
    assert_eq!(x["sourceCoverage"]["unsupportedOperations"], 0);
    assert_eq!(x["sourceCoverage"]["representedOperations"], 5);
    // Nothing of the application is executed for this scenario.
    assert!(r.get("sourceVerification").is_none());
    assert_eq!(r["evidence"]["level"], "model-counterexample");
}

/// Application-controlled text (SQL, error messages, source excerpts) in the
/// source-verification section is escaped; the page stays script-free.
#[test]
fn the_html_report_escapes_runtime_observations() {
    let json = std::fs::read_to_string(
        repo().join("reports/phase7/model-check/team-app-invite-double-submit.json"),
    )
    .expect("the committed Phase 7 report");
    let mut report: commitrift_model::report::ModelCheckReport =
        serde_json::from_str(&json).unwrap();
    let evil = "<script>alert(1)</script><img src=x onerror=alert(2)>\"'";
    let sv = report.source_verification.as_mut().unwrap();
    sv.statements[0].sql = evil.to_owned();
    sv.statements[0].differences.push(evil.to_owned());
    sv.outside_model.push(evil.to_owned());
    sv.problems.push(evil.to_owned());
    report.extraction[0].source_facts.push(evil.to_owned());
    let html = commitrift_model::html::model_check_html(&report);
    assert!(!html.contains("<script"), "unescaped script");
    assert!(!html.contains("<img"), "unescaped tag");
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(html.contains("script-src 'none'"));
}

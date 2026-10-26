//! Phase 9 end to end: operations whose acknowledgment is lost in flight,
//! run against the ORIGINAL code of a first-party application
//! (fixtures/phase9/ledger-app) on a disposable PostgreSQL, through the
//! test-only protocol proxy. A worker is killed with its COMMIT or
//! autocommit write on the wire; the proxy establishes what the server
//! received, PostgreSQL's own state confirms the outcome, and the retry runs
//! as a new process.
//!
//! Tests that run application code need Node.js >= 22, the PostgreSQL server
//! binaries (14+) and `npm ci` in fixtures/phase9/ledger-app; without them
//! they print SKIPPED and return, unless `COMMITRIFT_REQUIRE_REPLAY=1`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scenario(name: &str) -> PathBuf {
    repo().join("fixtures/phase9/scenarios").join(name)
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_commitrift"))
}

fn compile(app: &str) -> Result<(), String> {
    let dir = repo().join(app);
    if !dir.join("node_modules/drizzle-orm/package.json").is_file() {
        return Err(format!(
            "the dependencies of {app} are not installed: run `npm ci` there"
        ));
    }
    let output = Command::new("node")
        .args(["node_modules/typescript/bin/tsc", "-p", "tsconfig.json"])
        .current_dir(&dir)
        .output()
        .map_err(|e| format!("cannot run tsc: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{app} does not compile: {}",
            String::from_utf8_lossy(&output.stdout)
        ))
    }
}

fn missing_dependency() -> Option<String> {
    static CHECKED: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CHECKED
        .get_or_init(|| {
            match commitrift_replay::postgres::Binaries::discover() {
                Err(_) => {
                    return Some("the PostgreSQL server binaries are not installed".to_owned());
                }
                Ok(b) if commitrift_replay::postgres::major_version(&b.version) < 14 => {
                    return Some(format!("{} is older than 14", b.version));
                }
                Ok(_) => {}
            }
            let Ok(output) = Command::new("node").arg("--version").output() else {
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
            compile("fixtures/phase9/ledger-app").err()
        })
        .clone()
}

/// Runs that start workers are serialized: `no_worker_or_proxy_outlives_a_run`
/// looks at the whole process table.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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

fn verify(name: &str) -> (i32, Value) {
    model_check_json(
        &scenario(name),
        &[
            "--verify-source",
            "--allow-execution",
            "--source-repeat",
            "2",
        ],
    )
}

/// A copy of a scenario in a private temporary directory, with the paths that
/// are relative to the scenario made absolute: the tests never write into the
/// repository and never depend on each other's files.
fn temp_scenario(name: &str, tweak: impl Fn(&mut Value)) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let mut doc: Value = serde_json::from_str(&std::fs::read_to_string(scenario(name)).unwrap())
        .expect("scenario JSON");
    let base = scenario(name).parent().unwrap().to_path_buf();
    for (section, key) in [("extraction", "root"), ("sourceVerification", "harness")] {
        if let Some(rel) = doc[section][key].as_str() {
            let abs = base.join(rel).canonicalize().expect("path exists");
            doc[section][key] = abs.display().to_string().into();
        }
    }
    tweak(&mut doc);
    let dir = std::env::temp_dir().join(format!(
        "cr-p9-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(name);
    std::fs::write(&file, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    file
}

fn remove_temp(file: &Path) {
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

fn crashes(sv: &Value) -> &Vec<Value> {
    sv["recovery"]["crashes"].as_array().expect("crash checks")
}

fn in_flight(sv: &Value) -> &Value {
    &crashes(sv)[0]["inFlight"]
}

fn rows<'a>(sv: &'a Value, table: &str) -> &'a Vec<Value> {
    sv["runtimeTables"][table].as_array().unwrap()
}

// ---- the vertical slice ---------------------------------------------------

#[test]
fn a_lost_commit_acknowledgment_duplicates_the_payment_of_the_blind_ledger() {
    require_source_run!();
    let (code, r) = verify("ledger-blind-commit-in-flight.json");
    let sv = &r["sourceVerification"];
    assert_eq!(r["schemaVersion"], 4);
    assert_eq!(
        sv["evidence"], "ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED",
        "{sv:#}"
    );
    assert_eq!(
        r["evidence"]["level"],
        "original-code-in-flight-failure-reproduced"
    );
    assert_eq!(code, 1, "a counterexample exits with the findings code");
    assert!(
        sv["runs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|run| run["reproduced"] == true)
    );

    // The COMMIT was in flight: the server completed it, the application
    // was killed before it heard.
    let f = in_flight(sv);
    assert_eq!(f["operation"], "COMMIT");
    assert_eq!(f["modelFate"], f["observedFate"]);
    assert_eq!(f["clientAcknowledged"], false);
    assert!(
        f["serverAnswer"].as_str().unwrap().contains("COMMIT"),
        "PostgreSQL's own answer was observed on the wire: {f:#}"
    );
    assert_eq!(crashes(sv)[0]["killedBySignal"], true);
    assert_eq!(crashes(sv)[0]["status"], "agree");
    // Physical state versus knowledge.
    assert!(
        f["physicalOutcome"]
            .as_str()
            .unwrap()
            .contains("completed by the server")
    );
    assert!(
        f["applicationKnowledge"]
            .as_str()
            .unwrap()
            .starts_with("ambiguous")
    );
    // The durable state right after the kill (read from PostgreSQL), and at
    // the end: the retry inserted the payment again.
    assert_eq!(
        crashes(sv)[0]["runtimeTables"]["payments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(rows(sv, "payments").len(), 2);
    // The retry ran as a new process.
    let restarts = sv["recovery"]["restarts"].as_array().unwrap();
    assert_eq!(restarts[0]["status"], "agree");
    assert_ne!(restarts[0]["pid"], restarts[0]["previousPid"]);
    // The model explains what the application could not know.
    let twins = r["epistemic"]["twins"].as_array().unwrap();
    assert_eq!(twins[0]["other"], "notCommitted");
    assert!(
        twins[0]["reading"]
            .as_str()
            .unwrap()
            .contains("never learn")
    );
}

#[test]
fn the_wire_log_names_events_but_carries_no_payloads() {
    require_source_run!();
    let (_, r) = verify("ledger-blind-commit-in-flight.json");
    let log = r["sourceVerification"]["wireLog"].as_array().unwrap();
    let text = log
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("FaultArmed"), "{text}");
    assert!(text.contains("OperationCompleted"), "{text}");
    assert!(text.contains("ResponseWithheld"), "{text}");
    for secret in ["req-1", "500", "insert"] {
        assert!(
            !text.to_lowercase().contains(secret),
            "`{secret}` leaked into the wire log: {text}"
        );
    }
}

#[test]
fn reading_the_ledger_first_does_not_make_the_retry_safe() {
    require_source_run!();
    let (_, r) = verify("ledger-reconciled-commit-in-flight.json");
    let sv = &r["sourceVerification"];
    assert_eq!(
        sv["evidence"], "ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED",
        "{sv:#}"
    );
    // The server had not received the COMMIT when the client was killed: its
    // backend sat in an open transaction (observed in pg_stat_activity), and
    // the session outlived the worker.
    let f = in_flight(sv);
    assert_eq!(f["observedFate"], "held-for-later-delivery");
    assert_eq!(f["backendStateBeforeKill"], "idle in transaction");
    assert_eq!(f["sessionOpenAfterKill"], true);
    assert_eq!(f["withheldFromServer"], true);
    // Nothing was durable at the kill ...
    assert!(
        crashes(sv)[0]["runtimeTables"]["payments"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // ... the restarted client looked, found nothing, and then the server
    // completed the COMMIT it had been holding.
    let completion = &sv["recovery"]["completions"][0];
    assert_eq!(completion["status"], "agree");
    assert!(
        completion["serverAnswer"]
            .as_str()
            .unwrap()
            .contains("COMMIT")
    );
    assert_eq!(rows(sv, "payments").len(), 2);
    let selects: Vec<&Value> = sv["statements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["model"].as_str().unwrap().starts_with("SELECT"))
        .collect();
    assert!(
        selects.iter().all(|s| s["observed"] == "no row"),
        "both reads found no payment: {selects:#?}"
    );
}

#[test]
fn the_idempotent_ledger_keeps_the_invariant_in_every_fate_of_the_commit() {
    require_source_run!();
    for (name, fate) in [
        ("ledger-idempotent-commit-not-reached.json", "not-reached"),
        (
            "ledger-idempotent-commit-answered.json",
            "answered-and-withheld",
        ),
        (
            "ledger-idempotent-commit-held.json",
            "held-for-later-delivery",
        ),
    ] {
        let (code, r) = verify(name);
        let sv = &r["sourceVerification"];
        assert_eq!(code, 0, "{name}");
        assert_eq!(
            sv["evidence"], "ORIGINAL_CODE_RECOVERY_REPRODUCED",
            "{name}: {sv:#}"
        );
        assert_eq!(in_flight(sv)["observedFate"], fate, "{name}");
        assert_eq!(rows(sv, "ledger").len(), 1, "{name}: exactly one payment");
    }
}

#[test]
fn an_unreached_commit_leaves_nothing_and_the_server_cleans_up() {
    require_source_run!();
    let (_, r) = verify("ledger-idempotent-commit-not-reached.json");
    let sv = &r["sourceVerification"];
    let f = in_flight(sv);
    assert_eq!(f["withheldFromServer"], true);
    assert_eq!(f["forwardedToServer"], false);
    assert_eq!(f["backendStateBeforeKill"], "idle in transaction");
    let c = &crashes(sv)[0];
    assert_eq!(c["sessionsEnded"], true, "PostgreSQL ended the session");
    assert!(c["runtimeTables"]["ledger"].as_array().unwrap().is_empty());
}

#[test]
fn an_autocommit_write_is_committed_although_its_acknowledgment_was_lost() {
    require_source_run!();
    let (_, r) = verify("ledger-autocommit-blind-in-flight.json");
    let sv = &r["sourceVerification"];
    assert_eq!(
        sv["evidence"], "ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED",
        "{sv:#}"
    );
    assert!(
        in_flight(sv)["operation"]
            .as_str()
            .unwrap()
            .starts_with("INSERT")
    );
    assert_eq!(rows(sv, "payments").len(), 2);
}

#[test]
fn the_durable_state_after_a_killed_update_is_what_the_server_did() {
    require_source_run!();
    let (_, answered) = verify("ledger-settle-update-answered.json");
    let (_, unreached) = verify("ledger-settle-update-not-reached.json");
    let status = |r: &Value| {
        r["sourceVerification"]["runtimeTables"]["orders"][0]["status"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(
        status(&answered),
        "paid",
        "the update committed; the client never heard"
    );
    assert_eq!(
        status(&unreached),
        "unpaid",
        "the update never reached the server"
    );
    for r in [&answered, &unreached] {
        assert_eq!(
            r["sourceVerification"]["evidence"],
            "ORIGINAL_CODE_RECOVERY_REPRODUCED"
        );
        assert_eq!(r["sourceVerification"]["actors"][0][2], "killed");
    }
}

// ---- authorization, modes and refusals ------------------------------------

#[test]
fn executing_the_application_still_needs_the_explicit_flag() {
    let (_, r) = model_check_json(
        &scenario("ledger-blind-commit-in-flight.json"),
        &["--verify-source"],
    );
    let sv = &r["sourceVerification"];
    assert_ne!(sv["performed"], true);
    assert!(
        sv["problems"][0]
            .as_str()
            .unwrap()
            .contains("--allow-execution"),
        "{sv:#}"
    );
}

#[test]
fn model_only_runs_explain_both_worlds_without_running_anything() {
    let (code, r) = model_check_json(
        &scenario("ledger-blind-commit-in-flight.json"),
        &["--fault-mode", "in-flight"],
    );
    assert_eq!(code, 1);
    assert_eq!(r["evidence"]["level"], "model-counterexample");
    assert!(r.get("sourceVerification").is_none() || r["sourceVerification"].is_null());
    let twin = &r["epistemic"]["twins"][0];
    assert_eq!(twin["operation"], "COMMIT");
    assert_eq!(twin["actual"], "committed");
    assert_eq!(twin["twin"], "replayed");
    // The invariant is violated in one world and holds in the other.
    let here = &twin["actualVerdicts"][0];
    let there = &twin["twinVerdicts"][0];
    assert_eq!(
        (here["holds"].as_bool(), there["holds"].as_bool()),
        (Some(false), Some(true))
    );
}

#[test]
fn the_in_flight_fault_mode_needs_a_budget_for_it() {
    let output = bin()
        .arg("model-check")
        .arg(scenario("ledger-blind-commit-in-flight.json"))
        .args(["--fault-mode", "in-flight", "--max-in-flight", "0"])
        .output()
        .unwrap();
    assert_ne!(output.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("maxInFlight"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_lost_acknowledgment_with_a_live_process_is_refused_not_faked() {
    // Connection loss while the process lives has no runtime realization yet:
    // the schedule is refused (UNSUPPORTED), never approximated by a crash.
    let file = temp_scenario("ledger-blind-commit-in-flight.json", |doc| {
        doc["bounds"]["maxCrashes"] = 0.into();
        doc["bounds"].as_object_mut().unwrap().remove("crashPolicy");
        doc["actors"][0]["retry"]["trigger"] = "onFailure".into();
    });
    let (_, r) = model_check_json(&file, &["--verify-source", "--allow-execution"]);
    remove_temp(&file);
    let sv = &r["sourceVerification"];
    assert_eq!(sv["status"], "unsupported", "{sv:#}");
    assert_eq!(sv["evidence"], "UNSUPPORTED");
    assert!(sv["problems"][0].as_str().unwrap().contains("live process"));
}

#[test]
fn no_worker_or_proxy_outlives_a_run() {
    require_source_run!();
    let (_, r) = verify("ledger-reconciled-commit-in-flight.json");
    assert_eq!(r["sourceVerification"]["status"], "reproduced");
    // Every worker the run started is gone: none carries the run's marker.
    let out = Command::new("ps")
        .args(["-axo", "command"])
        .output()
        .unwrap();
    let ps = String::from_utf8_lossy(&out.stdout);
    let survivors: Vec<&str> = ps
        .lines()
        .filter(|l| l.contains("dist/verify/harness.js") || l.contains("commitrift-actor"))
        .collect();
    assert!(
        survivors.is_empty(),
        "a worker survived the run: {survivors:?}"
    );
}

// ---- tasks that share a process and a pool ---------------------------------

#[test]
fn a_pool_of_one_connection_starves_the_original_code() {
    require_source_run!();
    let (code, r) = verify("ledger-pool-starvation.json");
    let sv = &r["sourceVerification"];
    assert_eq!(code, 1);
    assert_eq!(sv["status"], "reproduced", "{sv:#}");
    assert_eq!(r["counterexample"]["invariant"], "termination");
    // The run did not infer the stall: the pool itself reported it.
    let stuck = &sv["stuck"];
    assert_eq!(stuck["modelKind"], "pool-starvation");
    assert_eq!(stuck["status"], "agree");
    let observed = &stuck["observed"];
    assert_eq!(observed["confirmed"], true);
    assert_eq!(
        observed["pool"],
        serde_json::json!([1, 0, 1]),
        "waiting 1, idle 0, total 1"
    );
    assert!(observed["quietMs"].as_u64().unwrap() >= 800);
    assert_eq!(observed["unfinished"], serde_json::json!(["Client"]));
    // The model's own report distinguishes it from a PostgreSQL deadlock.
    assert!(
        r["exploration"]["stuck"][0]
            .as_str()
            .unwrap()
            .contains("pool starvation")
    );
}

#[test]
fn the_same_code_with_two_connections_terminates() {
    let (code, r) = model_check_json(&scenario("ledger-pool-of-two.json"), &[]);
    assert_eq!(code, 0);
    assert_eq!(
        r["exploration"]["verdict"],
        "NO_COUNTEREXAMPLE_WITHIN_BOUNDS"
    );
    assert!(r["exploration"].get("stuck").is_none());
}

#[test]
fn the_declared_pool_must_be_the_harnesss_pool() {
    require_source_run!();
    // The scenario says the process has two connections; the harness's
    // `max` is one: the run would not be about the model's system.
    let file = temp_scenario("ledger-shared-process-crash.json", |doc| {
        doc["pools"]["P"] = 2.into();
    });
    let (_, r) = model_check_json(&file, &["--verify-source", "--allow-execution"]);
    remove_temp(&file);
    let sv = &r["sourceVerification"];
    assert_eq!(r["exploration"]["verdict"], "COUNTEREXAMPLE_FOUND");
    assert_ne!(sv["status"], "reproduced", "{sv:#}");
    let problems = sv["problems"].to_string();
    assert!(
        problems.contains("connection pool of 2") && problems.contains("max Some(1)"),
        "{problems}"
    );
}

#[test]
fn a_process_crash_ends_every_task_of_the_process_and_the_pool_shows_who_waited() {
    require_source_run!();
    let (_, r) = verify("ledger-shared-process-crash.json");
    let sv = &r["sourceVerification"];
    assert_eq!(
        sv["evidence"], "ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED",
        "{sv:#}"
    );
    assert!(
        sv["sharedProcesses"][0]
            .as_str()
            .unwrap()
            .contains("A, B, C concurrent tasks in one process")
    );
    // One kill, one pid, three tasks.
    let crashes = crashes(sv);
    assert_eq!(crashes.len(), 3);
    let pid = &crashes[0]["pid"];
    assert!(
        crashes
            .iter()
            .all(|c| &c["pid"] == pid && c["killedBySignal"] == true)
    );
    assert!(crashes.iter().all(|c| c["status"] == "agree"));
    // What was in flight, what was held at its checkpoint.
    let in_flight: Vec<&Value> = crashes
        .iter()
        .filter(|c| c["inFlight"].is_object())
        .collect();
    assert_eq!(in_flight.len(), 1);
    assert!(
        in_flight[0]["inFlight"]["operation"]
            .as_str()
            .unwrap()
            .starts_with("INSERT")
    );
    assert!(crashes.iter().filter(|c| c["inFlight"].is_null()).all(|c| {
        c["boundary"]
            .as_str()
            .unwrap()
            .contains("EXTERNAL confirmation")
    }));
    // Every task of the process was killed.
    let actors = sv["actors"].as_array().unwrap();
    assert!(actors.iter().all(|a| a[1] == "crashed" && a[2] == "killed"));
    // The durable state is what PostgreSQL has: the other task's payment is
    // there (committed before), and so is the autocommit write that the
    // application never heard about; no confirmation was published.
    assert_eq!(rows(sv, "payments").len(), 2);
}

// ---- the HTML timeline -------------------------------------------------------

fn html_of(name: &str, tweak: impl Fn(&mut Value)) -> String {
    let file = temp_scenario(name, tweak);
    let output = bin()
        .arg("model-check")
        .arg(&file)
        .args([
            "--format",
            "html",
            "--verify-source",
            "--allow-execution",
            "--no-timing",
        ])
        .output()
        .unwrap();
    remove_temp(&file);
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn the_html_timeline_shows_what_was_observed_and_what_is_inferred() {
    require_source_run!();
    let html = html_of("ledger-shared-process-crash.json", |_| {});
    // Lanes: one per task of the shared process, PostgreSQL, the proxy.
    for lane in ["A#1", "B#1", "C#1", "PostgreSQL", "Protocol proxy (wire)"] {
        assert!(html.contains(lane), "missing lane {lane}");
    }
    // Every wire fact is tagged observed; the application's knowledge is
    // tagged inferred and never as observed.
    assert!(html.contains("tag-observed"));
    assert!(html.contains("tag-inferred"));
    assert!(html.contains("the application cannot tell whether PostgreSQL executed it"));
    assert!(html.contains("shared process"));
    assert!(html.contains("SIGKILL"));
    // Script-free with a content security policy.
    assert!(!html.contains("<script"));
    assert!(html.contains("Content-Security-Policy"));
    assert!(html.contains("script-src 'none'"));
}

#[test]
fn every_string_of_the_in_flight_report_is_escaped() {
    require_source_run!();
    let evil = "<script>alert(1)</script><img src=x onerror=alert(2)>";
    let html = html_of("ledger-blind-commit-in-flight.json", |doc| {
        doc["description"] = evil.into();
        doc["actors"][0]["description"] = evil.into();
        doc["invariants"][0]["name"] = evil.into();
        doc["invariants"][0]["description"] = evil.into();
    });
    assert!(!html.contains("<script>alert"), "raw script in the page");
    assert!(!html.contains("<img src=x"), "raw element in the page");
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    // The twin analysis (knowledge) names the invariant: escaped there too.
    assert!(html.contains("Application knowledge at each lost acknowledgment"));
}

#[test]
fn a_stuck_state_is_shown_as_observed() {
    require_source_run!();
    let html = html_of("ledger-pool-starvation.json", |_| {});
    assert!(html.contains("Stuck state"));
    assert!(html.contains("tag-observed"));
    assert!(html.contains("the pool reports 1 waiting, 0 idle of 1"));
}

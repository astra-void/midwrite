//! A database connection that breaks while the
//! application process lives, against the ORIGINAL code of first-party
//! applications on a disposable PostgreSQL, with node-postgres
//! (fixtures/apps/ledger-live) and postgres-js 3.4.5
//! (fixtures/apps/ledger-pgjs). The proxy closes the client's connection
//! and nothing else; the driver's error, the worker's survival, what the
//! server received and what is durable are each established separately.
//!
//! Tests that run application code need Node.js >= 22, the PostgreSQL server
//! binaries (14+) and the fixtures' dependencies; without them they print
//! SKIPPED and return, unless `MIDWRITE_REQUIRE_REPLAY=1`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scenario(name: &str) -> PathBuf {
    repo()
        .join("fixtures/scenarios/connection-loss")
        .join(format!("{name}.json"))
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_midwrite"))
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
            match midwrite_replay::postgres::Binaries::discover() {
                Err(_) => {
                    return Some("the PostgreSQL server binaries are not installed".to_owned());
                }
                Ok(b) if midwrite_replay::postgres::major_version(&b.version) < 14 => {
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
            compile("fixtures/apps/ledger-live")
                .and_then(|()| compile("fixtures/apps/ledger-pgjs"))
                .err()
        })
        .clone()
}

/// Runs that start workers are serialized (the process-table check).
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

macro_rules! require_source_run {
    () => {
        if let Some(reason) = missing_dependency() {
            assert!(
                std::env::var_os("MIDWRITE_REQUIRE_REPLAY").is_none(),
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

fn model_check_json(path: &Path, extra: &[&str]) -> Value {
    let output = bin()
        .arg("model-check")
        .arg(path)
        .args(["--format", "json", "--no-timing"])
        .args(extra)
        .output()
        .unwrap();
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "not JSON ({e}): {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn verify(name: &str, runs: &str) -> Value {
    model_check_json(
        &scenario(name),
        &[
            "--verify-source",
            "--allow-execution",
            "--source-repeat",
            runs,
        ],
    )
}

fn loss(sv: &Value) -> &Value {
    &sv["recovery"]["losses"][0]
}

fn rows<'a>(sv: &'a Value, table: &str) -> &'a Vec<Value> {
    sv["runtimeTables"][table].as_array().unwrap()
}

fn assert_runs_matched(sv: &Value, key: &str) {
    let runs = sv["runs"].as_array().unwrap();
    assert!(!runs.is_empty());
    for r in runs {
        assert_eq!(r["coordination"], "followed", "{sv:#}");
        assert_eq!(r[key], true, "{sv:#}");
    }
}

// ---- node-postgres -----------------------------------------------------------

#[test]
fn a_broken_connection_after_a_durable_commit_makes_the_in_process_retry_duplicate() {
    require_source_run!();
    let r = verify("live-blind-retry-lost-commit", "2");
    assert_eq!(r["schemaVersion"], 5);
    assert_eq!(r["exploration"]["verdict"], "COUNTEREXAMPLE_FOUND");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["status"], "reproduced", "{sv:#}");
    assert_eq!(sv["evidence"], "ORIGINAL_CODE_CONNECTION_LOSS_REPRODUCED");
    assert_eq!(
        r["evidence"]["level"],
        "original-code-connection-loss-reproduced"
    );
    assert_runs_matched(sv, "reproduced");
    let l = loss(sv);
    assert_eq!(l["status"], "agree", "{l:#}");
    assert_eq!(l["modelFate"], "answered-and-withheld");
    assert_eq!(l["observedFate"], "answered-and-withheld");
    // The process lived (the operating system's answer), the proxy closed only
    // the client's connection, the server's own answer was observed.
    assert_eq!(l["workerAliveAfter"], true);
    assert_eq!(l["clientSevered"], true);
    assert!(
        l["serverAnswer"]
            .as_str()
            .unwrap()
            .contains("CommandComplete COMMIT")
    );
    // The driver's error, quoted as node-postgres reported it.
    assert_eq!(l["driverError"], "Connection terminated unexpectedly");
    // Drizzle's ROLLBACK after the failure goes to the broken client, which
    // rejects it without sending it.
    let dead = l["deadConnectionStatements"].as_array().unwrap();
    assert!(
        dead.iter()
            .any(|d| d.as_str().unwrap().starts_with("rollback")
                && d.as_str().unwrap().contains("not queryable")),
        "{dead:?}"
    );
    // The retry ran on a new connection.
    let old = l["backendPid"].as_i64().unwrap();
    let later: Vec<i64> = l["laterBackendPids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    assert!(!later.is_empty() && !later.contains(&old), "{later:?}");
    // What is durable is PostgreSQL's: two payments for one request.
    assert_eq!(rows(sv, "payments").len(), 2);
    assert_eq!(
        sv["actors"][0][2], "returned",
        "the application went on and returned"
    );
}

#[test]
fn an_unreached_commit_is_rolled_back_and_the_same_retry_is_right() {
    require_source_run!();
    let r = verify("live-blind-retry-commit-not-durable", "1");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["status"], "recovery-reproduced", "{sv:#}");
    assert_eq!(sv["evidence"], "ORIGINAL_CODE_RECOVERY_REPRODUCED");
    let l = loss(sv);
    assert_eq!(l["status"], "agree", "{l:#}");
    assert_eq!(l["observedFate"], "not-reached");
    assert_eq!(l["withheldFromServer"], true);
    // The server had the open transaction and no COMMIT when the connection
    // broke; it rolled back when the session ended.
    assert_eq!(l["backendStateBefore"], "idle in transaction");
    assert_eq!(l["sessionEnded"], true);
    assert_eq!(l["runtimeTables"]["payments"].as_array().unwrap().len(), 0);
    assert_eq!(rows(sv, "payments").len(), 1);
    // The application could not tell this from the durable case.
    assert!(
        l["applicationKnowledge"]
            .as_str()
            .unwrap()
            .starts_with("ambiguous")
    );
}

#[test]
fn reconciling_after_the_error_races_the_commit_the_server_still_holds() {
    require_source_run!();
    let r = verify("live-reconciled-retry-lost-commit", "2");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["status"], "reproduced", "{sv:#}");
    let l = loss(sv);
    assert_eq!(l["observedFate"], "held-for-later-delivery");
    assert_eq!(l["sessionOpen"], true);
    let c = &sv["recovery"]["completions"][0];
    assert_eq!(c["status"], "agree", "{c:#}");
    assert!(c["serverAnswer"].as_str().unwrap().contains("COMMIT"));
    // The look found nothing (the COMMIT was still held), then the COMMIT
    // landed, then the retry inserted.
    assert_eq!(rows(sv, "payments").len(), 2);
}

#[test]
fn a_durable_unique_request_key_keeps_the_in_process_retry_harmless() {
    require_source_run!();
    for name in [
        "live-idempotent-retry-lost-commit",
        "live-idempotent-retry-commit-not-durable",
    ] {
        let r = verify(name, "1");
        let sv = &r["sourceVerification"];
        assert_eq!(
            r["exploration"]["verdict"],
            "NO_COUNTEREXAMPLE_WITHIN_BOUNDS"
        );
        assert_eq!(sv["status"], "recovery-reproduced", "{name}: {sv:#}");
        assert_eq!(loss(sv)["status"], "agree", "{name}");
        assert_eq!(rows(sv, "ledger").len(), 1, "{name}");
    }
}

#[test]
fn an_autocommit_write_whose_answer_is_lost_is_durable_and_retried() {
    require_source_run!();
    let r = verify("live-autocommit-retry-lost-write", "1");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["status"], "reproduced", "{sv:#}");
    let l = loss(sv);
    assert!(l["serverAnswer"].as_str().unwrap().contains("INSERT 0 1"));
    assert_eq!(rows(sv, "payments").len(), 2);
}

#[test]
fn a_pool_of_one_replaces_the_broken_connection() {
    require_source_run!();
    let r = verify("live-blind-retry-pool-of-one", "1");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["status"], "reproduced", "{sv:#}");
    let l = loss(sv);
    let old = l["backendPid"].as_i64().unwrap();
    let later = l["laterBackendPids"].as_array().unwrap();
    assert_eq!(later.len(), 1, "one replacement connection: {later:?}");
    assert_ne!(later[0].as_i64().unwrap(), old);
    assert!(sv["stuck"].is_null(), "no starvation");
}

#[test]
fn without_a_client_error_handler_the_process_dies_and_the_run_diverges() {
    require_source_run!();
    let r = verify("live-blind-retry-unhandled-error-event", "1");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["status"], "diverged", "{sv:#}");
    assert_eq!(sv["evidence"], "DIVERGED");
    let problem = sv["problems"][0].as_str().unwrap();
    assert!(problem.contains("Unhandled 'error' event"), "{problem}");
    // No absolute path of the machine leaks into the report.
    assert!(
        !problem.contains("/Users/") && !problem.contains("/home/"),
        "{problem}"
    );
    assert_ne!(
        r["evidence"]["level"],
        "original-code-connection-loss-reproduced"
    );
}

// ---- postgres-js -------------------------------------------------------------

#[test]
fn the_same_application_on_postgres_js_fails_the_same_way_with_its_own_error() {
    require_source_run!();
    let r = verify("pgjs-blind-retry-lost-commit", "1");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["status"], "reproduced", "{sv:#}");
    assert_eq!(sv["evidence"], "ORIGINAL_CODE_CONNECTION_LOSS_REPRODUCED");
    let l = loss(sv);
    assert_eq!(l["status"], "agree", "{l:#}");
    let err = l["driverError"].as_str().unwrap();
    // postgres-js's own error, with the private run directory redacted.
    assert!(err.contains("CONNECTION_CLOSED"), "{err}");
    assert!(
        !err.contains("/tmp/") && !err.contains("/private/"),
        "{err}"
    );
    assert_eq!(rows(sv, "payments").len(), 2);
}

#[test]
fn a_postgres_js_statement_fault_targets_the_write_not_the_drivers_catalog_read() {
    require_source_run!();
    let r = verify("pgjs-autocommit-retry-lost-write", "1");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["status"], "reproduced", "{sv:#}");
    let answer = loss(sv)["serverAnswer"].as_str().unwrap().to_owned();
    assert!(answer.contains("INSERT 0 1"), "{answer}");
    assert!(!answer.contains("SELECT"), "{answer}");
}

#[test]
fn postgres_js_process_crashes_with_an_operation_in_flight_are_reproduced() {
    require_source_run!();
    for (name, status) in [
        ("pgjs-crash-blind-commit-in-flight", "reproduced"),
        ("pgjs-crash-reconciled-commit-in-flight", "reproduced"),
        (
            "pgjs-crash-idempotent-commit-not-reached",
            "recovery-reproduced",
        ),
    ] {
        let r = verify(name, "1");
        let sv = &r["sourceVerification"];
        assert_eq!(sv["status"], status, "{name}: {sv:#}");
        let c = &sv["recovery"]["crashes"][0];
        assert_eq!(c["status"], "agree", "{name}: {c:#}");
        assert_eq!(c["inFlight"]["modelFate"], c["inFlight"]["observedFate"]);
    }
}

// ---- hygiene -----------------------------------------------------------------

#[test]
fn breaking_a_connection_still_needs_the_explicit_execution_flag() {
    let r = model_check_json(
        &scenario("live-blind-retry-lost-commit"),
        &["--verify-source"],
    );
    let sv = &r["sourceVerification"];
    assert_ne!(sv["status"], "reproduced");
    assert_eq!(sv["performed"], false, "{sv:#}");
}

#[test]
fn no_worker_or_proxy_outlives_a_broken_connection_run() {
    require_source_run!();
    let r = verify("live-blind-retry-lost-commit", "1");
    assert_eq!(r["sourceVerification"]["status"], "reproduced");
    let out = Command::new("ps")
        .args(["-axo", "command"])
        .output()
        .unwrap();
    let ps = String::from_utf8_lossy(&out.stdout);
    let survivors: Vec<&str> = ps
        .lines()
        .filter(|l| l.contains("ledger-live/dist/verify") || l.contains("midwrite-actor"))
        .collect();
    assert!(
        survivors.is_empty(),
        "a worker survived the run: {survivors:?}"
    );
}

#[test]
fn every_string_of_a_connection_loss_report_is_escaped_in_html() {
    require_source_run!();
    let r = verify("live-blind-retry-lost-commit", "1");
    // Replace every string with a markup payload and render it.
    fn poison(v: &mut Value, n: &mut usize) {
        match v {
            Value::String(s) if !s.is_empty() => {
                *n += 1;
                *s = format!("<script>x{n}</script><img src=x onerror=y{n}>");
            }
            Value::Array(a) => a.iter_mut().for_each(|x| poison(x, n)),
            Value::Object(o) => o.values_mut().for_each(|x| poison(x, n)),
            _ => {}
        }
    }
    let mut doc = r.clone();
    let mut n = 0;
    // Only fields that exist (indexing a missing key would insert `null`).
    let sv = doc["sourceVerification"].as_object_mut().unwrap();
    for key in ["recovery", "problems", "wireLog"] {
        if let Some(v) = sv.get_mut(key) {
            if key == "recovery" {
                if let Some(l) = v.get_mut("losses") {
                    poison(l, &mut n);
                }
            } else {
                poison(v, &mut n);
            }
        }
    }
    assert!(n > 10, "{n}");
    let dir = std::env::temp_dir().join(format!("cr-p10-html-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("report.json");
    std::fs::write(&file, serde_json::to_string(&doc).unwrap()).unwrap();
    let report: midwrite_model::report::ModelCheckReport =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let html = midwrite_model::html::model_check_html(&report);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!html.contains("<script>x"), "raw markup in the page");
    assert!(!html.contains("<img src=x"), "raw markup in the page");
    // The loss is on the page (escaped), so the check above is not vacuous.
    assert!(html.contains("CONNECTION BROKEN"));
    assert!(
        html.contains("&lt;script&gt;x"),
        "the payloads are not rendered at all"
    );
}

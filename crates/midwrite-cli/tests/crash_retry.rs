//! Crash/retry counterexamples and recovery probes run
//! against the ORIGINAL code of a first-party application
//! (fixtures/apps/shop-app, fixtures/apps/team-app) on a disposable
//! PostgreSQL: workers are killed with SIGKILL at the planned checkpoint,
//! PostgreSQL's termination of their sessions is observed, and retries run
//! as new processes.
//!
//! Tests that run application code need Node.js >= 22, the PostgreSQL
//! server binaries (14+), and `npm ci` in fixtures/apps/shop-app,
//! fixtures/apps/pgjs-app and fixtures/apps/team-app; without them they
//! print SKIPPED and return, unless `MIDWRITE_REQUIRE_REPLAY=1`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scenario(name: &str) -> PathBuf {
    repo().join("fixtures/scenarios/crash-retry").join(name)
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_midwrite"))
}

/// Compile a fixture application once per test process (offline; it needs
/// `npm ci` to have installed its pinned dependencies).
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
            if !repo()
                .join("fixtures/apps/pgjs-app/node_modules/postgres/package.json")
                .is_file()
            {
                return Some(
                    "postgres-js is not installed: run `npm ci` in fixtures/apps/pgjs-app"
                        .to_owned(),
                );
            }
            compile("fixtures/apps/shop-app")
                .and_then(|()| compile("fixtures/apps/team-app"))
                .and_then(|()| compile("fixtures/apps/pgjs-app"))
                .err()
        })
        .clone()
}

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

fn verify(name: &str, repeat: &str) -> (i32, Value) {
    model_check_json(
        &scenario(name),
        &[
            "--verify-source",
            "--allow-execution",
            "--source-repeat",
            repeat,
        ],
    )
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(|x| x.as_str().unwrap().to_owned()).collect())
        .unwrap_or_default()
}

/// Every statement and effect agrees, nothing unexpected happened.
fn assert_all_agree(sv: &Value) {
    for s in sv["statements"].as_array().unwrap() {
        assert_eq!(s["status"], "agree", "{s:#}");
    }
    assert!(sv.get("unexpected").is_none(), "{sv:#}");
    assert!(sv.get("problems").is_none(), "{sv:#}");
}

#[test]
fn a_payment_charged_twice_after_a_crash_is_reproduced_by_the_original_code() {
    require_source_run!();
    let (code, r) = verify("checkout-crash-after-charge.json", "2");
    assert_eq!(code, 1, "a counterexample exits 1");
    assert_eq!(r["schemaVersion"], 3);
    assert_eq!(r["evidence"]["level"], "original-code-crash-reproduced");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["status"], "reproduced", "{sv:#}");
    assert_eq!(sv["evidence"], "ORIGINAL_CODE_CRASH_REPRODUCED");
    assert_all_agree(sv);
    for run in sv["runs"].as_array().unwrap() {
        assert_eq!(run["reproduced"], true, "{run:#}");
        assert_eq!(run["recoveryAgrees"], true);
        assert_eq!(run["invariant"], "violated");
    }
    let rec = &sv["recovery"];
    // The crash: killed by SIGKILL with the UPDATE announced and held (never
    // sent); PostgreSQL ended the session; committed rows as the model says.
    let crash = &rec["crashes"][0];
    assert_eq!(crash["status"], "agree", "{crash:#}");
    assert_eq!(crash["killedBySignal"], true);
    assert_eq!(crash["sessionsEnded"], true);
    assert!(
        crash["neverSent"]
            .as_str()
            .unwrap()
            .starts_with("update \"orders\"")
    );
    assert_eq!(crash["modelTables"], crash["runtimeTables"]);
    assert_eq!(crash["runtimeTables"]["orders"][0]["status"], "unpaid");
    // The retry is a new process.
    let restart = &rec["restarts"][0];
    assert_eq!(restart["status"], "agree");
    assert_eq!(restart["previousOutcome"], "killed");
    assert_ne!(restart["pid"], restart["previousPid"]);
    assert_eq!(restart["attempt"], 2);
    // Two effective charges: one per execution.
    assert_eq!(rec["effects"]["agree"], true);
    assert_eq!(
        strings(&rec["effects"]["runtime"]),
        [
            "EXTERNAL card-charge by Client#1",
            "EXTERNAL card-charge by Client#2"
        ]
    );
    assert_eq!(rec["attempts"][0], serde_json::json!(["Client", 2, 2]));
    assert!(
        strings(&rec["contracts"])
            .iter()
            .any(|c| c.contains("TEST DOUBLE"))
    );
    assert_eq!(
        strings(&rec["serverSettings"]),
        ["client_connection_check_interval=50ms"]
    );
    // Schema facts read from source, and the run database's catalog agrees
    // with them.
    assert_eq!(r["schemaFacts"][0]["table"], "orders");
    assert_eq!(
        sv["schemaFidelity"]["status"], "agree",
        "{:#}",
        sv["schemaFidelity"]
    );
    // Statements of the retry are attributed to the original source.
    let statements = sv["statements"].as_array().unwrap();
    assert_eq!(statements.len(), 7);
    assert!(statements.iter().all(|s| {
        s["runtimeSource"]
            .as_str()
            .unwrap()
            .starts_with("src/app/checkout.ts")
    }));
}

#[test]
fn the_idempotent_checkout_keeps_one_charge_under_the_same_crash_and_retry() {
    require_source_run!();
    let (code, r) = verify("checkout-idempotent-crash-after-charge.json", "2");
    assert_eq!(code, 0, "no counterexample exits 0");
    assert_eq!(r["schemaVersion"], 3);
    assert_eq!(r["evidence"]["level"], "original-code-recovery-reproduced");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["status"], "recovery-reproduced", "{sv:#}");
    assert_eq!(sv["evidence"], "ORIGINAL_CODE_RECOVERY_REPRODUCED");
    assert_all_agree(sv);
    for run in sv["runs"].as_array().unwrap() {
        assert_eq!(run["recoveryReproduced"], true, "{run:#}");
        assert_eq!(run["invariant"], "holds");
        assert_eq!(run["reproduced"], false);
    }
    let schedule = strings(&sv["probeSchedule"]);
    assert!(schedule.contains(&"crash process Client".to_owned()));
    assert!(schedule.contains(&"retry Client".to_owned()));
    let rec = &sv["recovery"];
    assert_eq!(
        strings(&rec["effects"]["runtime"]),
        [
            "EXTERNAL card-charge key=\"req-1\" by Client#1",
            "EXTERNAL card-charge key=\"req-1\" by Client#2 (deduplicated by the channel)"
        ]
    );
    // The retry's ON CONFLICT DO NOTHING inserts nothing.
    let conflict = sv["statements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| {
            s["model"]
                .as_str()
                .unwrap()
                .contains("ON CONFLICT DO NOTHING")
        })
        .map(|s| s["observed"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(conflict, ["1 row(s)", "0 row(s)"]);
}

#[test]
fn a_dual_write_loses_the_event_when_the_process_dies_after_commit() {
    require_source_run!();
    let (_, r) = verify("shipping-dual-write-crash.json", "1");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["evidence"], "ORIGINAL_CODE_CRASH_REPRODUCED", "{sv:#}");
    assert_all_agree(sv);
    let crash = &sv["recovery"]["crashes"][0];
    assert!(
        crash["boundary"]
            .as_str()
            .unwrap()
            .contains("EXTERNAL order-events")
    );
    assert_eq!(crash["neverSent"], "EFFECT order-events");
    // The UPDATE committed before the crash and stays committed.
    assert_eq!(crash["runtimeTables"]["orders"][0]["status"], "shipped");
    assert_eq!(sv["recovery"]["effects"]["runtime"], serde_json::json!([]));
    // No recovery mechanism is declared and none is invented.
    assert!(sv["recovery"].get("restarts").is_none());
    assert!(
        sv["recovery"]["quiescence"]
            .as_str()
            .unwrap()
            .contains("no recovery mechanism is declared")
    );
}

#[test]
fn an_outbox_relay_crash_duplicates_delivery_but_loses_nothing() {
    require_source_run!();
    let (_, r) = verify("shipping-outbox-relay-duplicate.json", "1");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["evidence"], "ORIGINAL_CODE_CRASH_REPRODUCED", "{sv:#}");
    assert_all_agree(sv);
    assert_eq!(
        strings(&sv["recovery"]["effects"]["runtime"]),
        [
            "EXTERNAL order-events key=1 by Relay#1",
            "EXTERNAL order-events key=1 by Relay#2"
        ]
    );
    // The same relay crash and restart keep the no-loss invariant.
    let (_, r) = verify("shipping-outbox-no-loss.json", "1");
    let sv = &r["sourceVerification"];
    assert_eq!(
        sv["evidence"], "ORIGINAL_CODE_RECOVERY_REPRODUCED",
        "{sv:#}"
    );
    assert_all_agree(sv);
    assert_eq!(sv["runtimeTables"]["outbox"][0]["published"], true);
}

#[test]
fn job_redelivery_without_idempotency_sends_twice_and_a_claimed_effect_once() {
    require_source_run!();
    let (_, r) = verify("job-redelivery-duplicate.json", "1");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["evidence"], "ORIGINAL_CODE_CRASH_REPRODUCED", "{sv:#}");
    assert_all_agree(sv);
    assert!(
        sv["recovery"]["restarts"][0]["mechanism"]
            .as_str()
            .unwrap()
            .contains("AtLeastOnce")
    );
    // Concurrent delivery to two consumers: no crash, Phase 7 evidence.
    let (_, r) = verify("job-concurrent-claims-duplicate.json", "1");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["status"], "reproduced", "{sv:#}");
    assert!(sv.get("evidence").is_none(), "{sv:#}");
    assert_eq!(r["evidence"]["level"], "original-code-reproduced");
    // The unique claim: at most one receipt under redelivery...
    let (_, r) = verify("job-claimed-effect-redelivery.json", "1");
    let sv = &r["sourceVerification"];
    assert_eq!(
        sv["evidence"], "ORIGINAL_CODE_RECOVERY_REPRODUCED",
        "{sv:#}"
    );
    assert_all_agree(sv);
    // ...and under concurrent delivery (a probe without crash).
    let (_, r) = verify("job-claimed-effect-concurrent.json", "1");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["evidence"], "ORIGINAL_CODE_INVARIANT_HELD", "{sv:#}");
    assert_eq!(r["evidence"]["level"], "original-code-invariant-held");
    assert_all_agree(sv);
}

#[test]
fn a_worker_killed_while_waiting_for_a_lock_is_rolled_back_by_postgresql() {
    require_source_run!();
    let (_, r) = verify("accept-crash-while-waiting.json", "2");
    let sv = &r["sourceVerification"];
    assert_eq!(
        sv["evidence"], "ORIGINAL_CODE_RECOVERY_REPRODUCED",
        "{sv:#}"
    );
    assert_all_agree(sv);
    let crash = &sv["recovery"]["crashes"][0];
    assert!(
        crash["checkpoint"]
            .as_str()
            .unwrap()
            .contains("wait_event_type = Lock"),
        "{crash:#}"
    );
    assert_eq!(crash["sessionsEnded"], true);
    assert_eq!(
        sv["actors"][1],
        serde_json::json!(["Accept2", "crashed", "killed"])
    );
    // The lock holder dies instead: its waiter proceeds and commits.
    let (_, r) = verify("accept-crash-lock-holder.json", "2");
    let sv = &r["sourceVerification"];
    assert_eq!(
        sv["evidence"], "ORIGINAL_CODE_RECOVERY_REPRODUCED",
        "{sv:#}"
    );
    assert_all_agree(sv);
    assert_eq!(
        sv["actors"][0],
        serde_json::json!(["Accept1", "crashed", "killed"])
    );
    assert_eq!(
        sv["actors"][1],
        serde_json::json!(["Accept2", "ok", "returned"])
    );
}

/// A copy of a crash/retry scenario with absolute paths, edited by `edit`.
fn edited(name: &str, tag: &str, edit: impl FnOnce(&mut Value)) -> (PathBuf, PathBuf) {
    let mut doc: Value =
        serde_json::from_str(&std::fs::read_to_string(scenario(name)).unwrap()).unwrap();
    let app = repo().join("fixtures/apps/shop-app");
    doc["extraction"]["root"] = Value::from(app.display().to_string());
    doc["sourceVerification"]["harness"] =
        Value::from(app.join("midwrite-source.json").display().to_string());
    edit(&mut doc);
    let dir = std::env::temp_dir().join(format!("cr-p8-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scenario.json");
    std::fs::write(&path, doc.to_string()).unwrap();
    (dir, path)
}

#[test]
fn a_crash_point_the_original_code_does_not_reach_is_a_divergence() {
    require_source_run!();
    // The harness asks for an order the model does not have: the original
    // code throws before the charge, so the planned crash point (after the
    // charge) never comes.
    let (dir, path) = edited("checkout-crash-after-charge.json", "diverge", |doc| {
        doc["sourceVerification"]["actors"]["Client"]["input"]["orderId"] = Value::from(2);
    });
    let (_, r) = model_check_json(&path, &["--verify-source", "--allow-execution"]);
    let _ = std::fs::remove_dir_all(&dir);
    let sv = &r["sourceVerification"];
    assert_eq!(sv["status"], "diverged", "{sv:#}");
    assert_eq!(sv["evidence"], "DIVERGED");
    assert_ne!(r["evidence"]["level"], "original-code-crash-reproduced");
    assert_eq!(sv["runs"][0]["reproduced"], false);
}

#[test]
fn crash_verification_never_runs_without_the_explicit_authorization() {
    // Static only: no dependency needed, nothing may start.
    let (_, r) = model_check_json(
        &scenario("checkout-crash-after-charge.json"),
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
    assert_eq!(r["evidence"]["level"], "model-counterexample");
    // A probe is not run either.
    let (_, r) = model_check_json(
        &scenario("checkout-idempotent-crash-after-charge.json"),
        &["--verify-source"],
    );
    let sv = &r["sourceVerification"];
    assert_eq!(sv["performed"], false);
    assert_eq!(r["evidence"]["level"], "none");
}

#[test]
fn a_retry_mechanism_is_never_invented() {
    // Without a declared retry the model has no retry transition, so the
    // double charge is unreachable: no counterexample, nothing to run.
    let (dir, path) = edited("checkout-crash-after-charge.json", "noretry", |doc| {
        doc["actors"][0].as_object_mut().unwrap().remove("retry");
        doc["bounds"]["maxRetries"] = Value::from(0);
    });
    let (code, r) = model_check_json(&path, &[]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(code, 0, "{r:#}");
    assert_eq!(
        r["exploration"]["verdict"],
        "NO_COUNTEREXAMPLE_WITHIN_BOUNDS"
    );
}

#[test]
fn an_application_retry_after_40001_reruns_the_transaction_but_not_the_effect() {
    require_source_run!();
    // The notification inside the transaction is performed again by the
    // retry: PostgreSQL's rollback does not undo an external effect.
    let (_, r) = verify("inventory-retry-duplicate-notice.json", "2");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["status"], "reproduced", "{sv:#}");
    assert_eq!(r["evidence"]["level"], "original-code-reproduced");
    assert_all_agree(sv);
    let statements = sv["statements"].as_array().unwrap();
    let r2: Vec<(&str, &str)> = statements
        .iter()
        .filter(|s| s["actor"] == "R2")
        .map(|s| {
            (
                s["model"].as_str().unwrap(),
                s["observed"].as_str().unwrap(),
            )
        })
        .collect();
    // 40001, the helper's rollback, then a fresh transaction.
    let failed = r2
        .iter()
        .position(|(_, o)| *o == "SQLSTATE 40001")
        .expect("a real serialization failure");
    assert_eq!(r2[failed + 1], ("ROLLBACK (after a failure)", "rollback"));
    assert_eq!(r2[failed + 2], ("BEGIN", "begin"));
    assert_eq!(r2.last().unwrap(), &("COMMIT", "commit"));
    // The retried BEGIN is attributed through the un-awaited
    // `return withTransactionRetry(...)`.
    assert!(statements.iter().any(|s| {
        s["runtimeSource"]
            .as_str()
            .unwrap()
            .contains("call chain restored")
    }));
    assert_eq!(
        strings(&sv["recovery"]["effects"]["runtime"]),
        [
            "EXTERNAL warehouse by R1#1",
            "EXTERNAL warehouse by R2#1",
            "EXTERNAL warehouse by R2#1"
        ]
    );
    assert_eq!(
        sv["runtimeTables"]["reservations"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(sv["runtimeTables"]["products"][0]["stock"], 6);
    // Notifying after commit: one notice per reservation through the same
    // real 40001 and retry.
    let (_, r) = verify("inventory-notify-after-commit.json", "1");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["evidence"], "ORIGINAL_CODE_INVARIANT_HELD", "{sv:#}");
    assert_all_agree(sv);
    assert!(
        sv["statements"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["observed"] == "SQLSTATE 40001")
    );
    // Without retries the 40001 is final: the request fails, its changes
    // never commit.
    let (_, r) = verify("inventory-retry-exhausted.json", "1");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["evidence"], "ORIGINAL_CODE_INVARIANT_HELD", "{sv:#}");
    assert_all_agree(sv);
    let r2 = &sv["actors"][1];
    assert!(r2[1].as_str().unwrap().starts_with("failed: 40001"), "{r2}");
    assert!(r2[2].as_str().unwrap().starts_with("threw"), "{r2}");
    assert_eq!(
        sv["runtimeTables"]["reservations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(sv["runtimeTables"]["products"][0]["stock"], 8);
}

fn schema_cli(root: &str, file: &str, extra: &[&str]) -> (i32, Value) {
    let output = bin()
        .arg("schema")
        .arg(repo().join(root))
        .args(["--file", file, "--format", "json"])
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
fn extracted_schemas_agree_with_the_databases_their_migrations_create() {
    require_source_run!();
    for (root, file, migration) in [
        (
            "fixtures/apps/shop-app",
            "src/lib/db/schema.ts",
            "fixtures/apps/shop-app/drizzle/0000_init.sql",
        ),
        (
            "fixtures/apps/team-app",
            "src/lib/db/schema.ts",
            "fixtures/apps/team-app/drizzle/0000_init.sql",
        ),
        (
            "fixtures/apps/drizzle-schemas",
            "src/schema.ts",
            "fixtures/apps/drizzle-schemas/drizzle/0000_init.sql",
        ),
    ] {
        let migration = repo().join(migration);
        let (code, r) = schema_cli(
            root,
            file,
            &[
                "--migrations",
                migration.to_str().unwrap(),
                "--allow-execution",
            ],
        );
        assert_eq!(code, 0, "{root}: {r:#}");
        assert_eq!(r["fidelity"]["status"], "agree", "{root}: {r:#}");
    }
}

#[test]
fn schema_drift_between_source_and_database_is_reported_never_repaired() {
    require_source_run!();
    let migration = repo().join("fixtures/apps/drizzle-schemas/drizzle/drift.sql");
    let (code, r) = schema_cli(
        "fixtures/apps/drizzle-schemas",
        "src/schema.ts",
        &[
            "--migrations",
            migration.to_str().unwrap(),
            "--allow-execution",
        ],
    );
    assert_eq!(code, 1, "drift exits 1");
    assert_eq!(r["fidelity"]["status"], "drift");
    let drift: Vec<String> = r["fidelity"]["tables"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| {
            let table = t["table"].as_str().unwrap().to_owned();
            t["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|i| i["status"] == "drift")
                .map(move |i| {
                    format!(
                        "{table} {} — {}",
                        i["fact"].as_str().unwrap(),
                        i["detail"].as_str().unwrap()
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let has = |needle: &str| drift.iter().any(|d| d.contains(needle));
    assert!(has("accounts column email nullability"), "{drift:#?}");
    assert!(has("accounts column role default"), "{drift:#?}");
    assert!(
        has("accounts unique (email) — absent from the database"),
        "{drift:#?}"
    );
    assert!(has("is GLOBAL; the source's is partial"), "{drift:#?}");
    assert!(
        has("NULLS NOT DISTINCT — absent from the database"),
        "{drift:#?}"
    );
    assert!(has("memberships foreign key (account_id)"), "{drift:#?}");
    assert!(has("memberships CHECK constraints"), "{drift:#?}");
    assert_eq!(drift.len(), 8, "{drift:#?}");
}

#[test]
fn reading_a_schema_is_static_and_comparing_it_needs_authorization() {
    // Static extraction: no dependency needed.
    let (code, r) = schema_cli("fixtures/apps/shop-app", "src/lib/db/schema.ts", &[]);
    assert_eq!(code, 0);
    assert_eq!(r["schema"]["tables"].as_array().unwrap().len(), 8);
    assert!(r["fidelity"].is_null());
    // Applying migrations without --allow-execution is refused.
    let migration = repo().join("fixtures/apps/shop-app/drizzle/0000_init.sql");
    let output = bin()
        .arg("schema")
        .arg(repo().join("fixtures/apps/shop-app"))
        .args(["--file", "src/lib/db/schema.ts", "--migrations"])
        .arg(&migration)
        .output()
        .unwrap();
    assert_ne!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--allow-execution"));
}

#[test]
fn the_postgres_js_adapter_reproduces_what_the_pg_adapter_does() {
    require_source_run!();
    // The same actions and schedule on both drivers: same statements, rows
    // and verdict. postgres-js reads the type catalog when it connects
    // (reported, not held).
    let (_, pg) = model_check_json(
        &repo().join("fixtures/scenarios/server-actions/team-app-invite-double-submit.json"),
        &["--verify-source", "--allow-execution"],
    );
    let (_, js) = verify("pgjs-invite-double-submit.json", "2");
    let (pg, js) = (&pg["sourceVerification"], &js["sourceVerification"]);
    assert_eq!(js["status"], "reproduced", "{js:#}");
    assert_all_agree(js);
    let observed = |sv: &Value| -> Vec<(String, String)> {
        sv["statements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                // Unmodeled `now()` columns differ between any two runs.
                let observed: String = s["observed"]
                    .as_str()
                    .unwrap()
                    .split(',')
                    .filter(|cell| !cell.contains("created_at"))
                    .collect::<Vec<_>>()
                    .join(",");
                (s["model"].as_str().unwrap().to_owned(), observed)
            })
            .collect()
    };
    assert_eq!(observed(pg), observed(js));
    assert_eq!(pg["runtimeTables"], js["runtimeTables"]);
    assert!(pg.get("driverInternal").is_none());
    assert!(
        js["driverInternal"][0]
            .as_str()
            .unwrap()
            .contains("pg_catalog.pg_type")
    );
}

#[test]
fn postgres_js_crashes_lock_waits_and_serialization_retries_are_coordinated() {
    require_source_run!();
    let (_, r) = verify("pgjs-checkout-crash-after-charge.json", "1");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["evidence"], "ORIGINAL_CODE_CRASH_REPRODUCED", "{sv:#}");
    assert_all_agree(sv);
    // A lock wait on a connection whose pid the driver does not expose is
    // found by the execution's application_name.
    let (_, r) = verify("pgjs-accept-crash-while-waiting.json", "1");
    let sv = &r["sourceVerification"];
    assert_eq!(
        sv["evidence"], "ORIGINAL_CODE_RECOVERY_REPRODUCED",
        "{sv:#}"
    );
    assert!(
        sv["recovery"]["crashes"][0]["checkpoint"]
            .as_str()
            .unwrap()
            .contains("wait_event_type = Lock")
    );
    // postgres-js sets the isolation level in a statement of its own after
    // `begin`: released with the BEGIN and reported.
    let (_, r) = verify("pgjs-inventory-retry-duplicate-notice.json", "1");
    let sv = &r["sourceVerification"];
    assert_eq!(sv["status"], "reproduced", "{sv:#}");
    assert_all_agree(sv);
    assert_eq!(
        strings(&sv["transactionConfig"]),
        [
            "R1: set transaction isolation level repeatable read",
            "R2: set transaction isolation level repeatable read",
            "R2: set transaction isolation level repeatable read"
        ]
    );
    assert!(
        sv["statements"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["observed"] == "SQLSTATE 40001")
    );
}

#[test]
fn the_postgres_js_adapter_refuses_unverified_versions() {
    require_source_run!();
    let output = Command::new("node")
        .args(["--test", "runtime/node/test/postgres-js.test.js"])
        .current_dir(repo())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The extracted crash/retry models (static only): crashes, retries,
/// at-least-once redelivery, external effects under a deduplication
/// contract, lock waits, generated ids.
fn crash_retry_models() -> Vec<(String, midwrite_model::model::Model)> {
    let mut out = Vec::new();
    let dir = repo().join("fixtures/scenarios/crash-retry");
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    names.sort();
    for path in names {
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let root = path
            .parent()
            .unwrap()
            .join(doc["extraction"]["root"].as_str().unwrap());
        let provider = midwrite_analyzer::TsActorProvider::new(root);
        let (model, _) = midwrite_model::scenario::load_file(&path, &provider)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        out.push((
            path.file_name().unwrap().to_string_lossy().into_owned(),
            model,
        ));
    }
    out
}

#[test]
fn reduction_reaches_every_state_of_the_full_search_with_crashes_retries_and_effects() {
    use midwrite_model::explore::{ExploreOptions, Verdict, audit_independence, explore_with};
    use midwrite_model::model::Strategy;
    let run = |m: &midwrite_model::model::Model, por: bool| {
        explore_with(
            m,
            &ExploreOptions {
                por,
                strategy: Strategy::Dfs,
                collect_states: true,
                stop_at_first: false,
            },
        )
    };
    let mut compared = 0;
    for (name, model) in crash_retry_models() {
        let full = run(&model, false);
        let reduced = run(&model, true);
        if full.truncation.resource_limit() || reduced.truncation.resource_limit() {
            continue;
        }
        compared += 1;
        assert_eq!(
            full.reached, reduced.reached,
            "{name}: reduction missed states"
        );
        assert_eq!(full.violating_states, reduced.violating_states, "{name}");
        assert_eq!(
            full.verdict == Verdict::CounterexampleFound,
            reduced.verdict == Verdict::CounterexampleFound,
            "{name}"
        );
        assert!(
            reduced.metrics.transitions <= full.metrics.transitions,
            "{name}"
        );
        let violations = audit_independence(&model, 100_000);
        assert!(
            violations.is_empty(),
            "{name}: {:?}",
            violations.first().map(|v| (v.reason, v.a, v.b))
        );
    }
    assert!(compared >= 18, "only {compared} scenarios compared");
}

#[test]
fn the_html_report_escapes_crash_recovery_and_schema_fields() {
    let json = std::fs::read_to_string(
        repo().join("fixtures/reports/model-check/checkout-crash-after-charge.json"),
    )
    .expect("the committed crash/retry report");
    let mut report: midwrite_model::report::ModelCheckReport = serde_json::from_str(&json).unwrap();
    let evil = "<script>alert(1)</script><img src=x onerror=alert(2)>\"'";
    let sv = report.source_verification.as_mut().unwrap();
    let rec = sv.recovery.as_mut().unwrap();
    rec.crashes[0].checkpoint = evil.to_owned();
    rec.restarts[0].mechanism = evil.to_owned();
    rec.contracts.push(evil.to_owned());
    rec.effects.runtime.push(evil.to_owned());
    rec.quiescence = evil.to_owned();
    sv.statements[0].runtime_source = evil.to_owned();
    sv.statements[2].sql = format!("EFFECT {evil}");
    sv.driver_internal.push(evil.to_owned());
    sv.transaction_config.push(evil.to_owned());
    let fidelity = sv.schema_fidelity.as_mut().unwrap();
    fidelity.status = "drift".to_owned();
    fidelity.tables[0].items[0].status = "drift".to_owned();
    fidelity.tables[0].items[0].detail = evil.to_owned();
    report.schema_facts[0].facts[0].fact = evil.to_owned();
    report.schema_facts[0].binding = Some(evil.to_owned());
    let html = midwrite_model::html::model_check_html(&report);
    assert!(!html.contains("<script"), "unescaped script");
    assert!(!html.contains("<img"), "unescaped tag");
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(html.contains("script-src 'none'"));
    // The timeline is there: a lane per execution, the kill and the retry.
    assert!(html.contains("<th>Client#1</th><th>Client#2</th><th>PostgreSQL</th>"));
    assert!(html.contains("✕ CRASH — SIGKILL pid"));
    assert!(html.contains("↻ RETRY — new process pid"));
}

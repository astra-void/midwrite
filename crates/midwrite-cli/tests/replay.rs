//! Runtime replay integration tests (Phase 3, Milestones 2–4 and 8).
//!
//! They execute the first-party harnesses under `fixtures/replay/` and
//! generated probe harnesses against a disposable PostgreSQL cluster. When
//! Bun or the PostgreSQL server binaries are missing they print `SKIPPED`
//! and return; set `MIDWRITE_REQUIRE_REPLAY=1` to make that a failure.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_midwrite")
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// Why replay tests cannot run here, if they cannot.
fn missing_dependency() -> Option<String> {
    if !on_path("bun") {
        return Some("bun is not installed".to_owned());
    }
    if std::env::var_os("MIDWRITE_PG_BIN").is_none()
        && !on_path("postgres")
        && !on_path("pg_config")
    {
        return Some("PostgreSQL server binaries are not installed".to_owned());
    }
    missing_isolation()
}

/// Why the platform isolation backend cannot be used here, if it cannot.
fn missing_isolation() -> Option<String> {
    if cfg!(target_os = "macos") {
        return (!Path::new("/usr/bin/sandbox-exec").is_file())
            .then(|| "macOS sandbox-exec is missing".to_owned());
    }
    if cfg!(target_os = "linux") {
        if !on_path("bwrap") && !Path::new("/usr/bin/bwrap").is_file() {
            return Some("bubblewrap (bwrap) is not installed".to_owned());
        }
        let probe = Command::new("bwrap")
            .args([
                "--unshare-user",
                "--unshare-net",
                "--unshare-pid",
                "--ro-bind",
                "/",
                "/",
                "--dev",
                "/dev",
                "--proc",
                "/proc",
                "true",
            ])
            .output();
        return match probe {
            Ok(output) if output.status.success() => None,
            Ok(output) => Some(format!(
                "bubblewrap cannot create namespaces here: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(error) => Some(format!("bubblewrap could not be started: {error}")),
        };
    }
    Some(format!(
        "replay isolation is not implemented for {}",
        std::env::consts::OS
    ))
}

/// The isolation backend name the report must show on this platform.
fn expected_isolation() -> &'static str {
    if cfg!(target_os = "macos") {
        "sandbox-exec"
    } else {
        "bubblewrap"
    }
}

macro_rules! require_replay {
    () => {
        if let Some(reason) = missing_dependency() {
            assert!(
                std::env::var_os("MIDWRITE_REQUIRE_REPLAY").is_none(),
                "replay dependencies required: {reason}"
            );
            eprintln!("SKIPPED: {reason}");
            return;
        }
    };
}

fn replay(dir: &Path, extra: &[&str], envs: &[(&str, &str)]) -> (Output, Option<Value>) {
    let mut command = Command::new(bin());
    command
        .arg("replay")
        .arg(dir)
        .args(["--allow-execution", "--format", "json"])
        .args(extra)
        .current_dir(repo());
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command.output().expect("run midwrite");
    let json = serde_json::from_slice(&output.stdout).ok();
    (output, json)
}

fn case<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("no case {id}"))
}

/// A probe harness written to a scratch directory.
struct Probe {
    dir: PathBuf,
}

impl Probe {
    fn new(name: &str, manifest: &str, setup_sql: &str) -> Self {
        // Inside the repository, so that the runtime client it imports is
        // part of the harness project (Linux isolation mounts only that).
        let dir = repo()
            .join("target/midwrite-probes")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let client = repo().join("runtime/typescript/midwrite-replay.ts");
        std::fs::write(
            dir.join("harness.ts"),
            PROBE_HARNESS.replace("@CLIENT@", &client.display().to_string()),
        )
        .unwrap();
        std::fs::write(dir.join("midwrite-replay.json"), manifest).unwrap();
        std::fs::write(dir.join("schema.sql"), setup_sql).unwrap();
        Self { dir }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const PROBE_HARNESS: &str = r#"
import { SQL } from "bun";
import { connect } from "node:net";
import { checkpoint, replayDatabase, runScenarios } from "@CLIENT@";

const sql = new SQL({ ...replayDatabase(), max: 1 });

await runScenarios(
  {
    noop: async () => {},
    insert: async () => {
      await sql.unsafe("INSERT INTO probe (id) VALUES (1)");
      await checkpoint("after-db-commit", "probe");
    },
    detach: async () => {
      const { spawn } = await import("node:child_process");
      // A grandchild in its own session (setsid), outside the process group.
      spawn("sleep", [process.env.SLEEP_MARK ?? "3017"], { detached: true, stdio: "ignore" }).unref();
      await new Promise((resolve) => setTimeout(resolve, 200));
    },
    hang: async () => {
      await checkpoint("before-hang");
      await new Promise((resolve) => setTimeout(resolve, 3_600_000));
    },
    env: async () => {
      console.log("env:" + JSON.stringify(Object.keys(process.env).sort()));
      console.log("secret:" + (process.env.MIDWRITE_TEST_SECRET ?? "absent"));
      console.log("database-url:" + (process.env.DATABASE_URL ?? "absent"));
    },
    network: async () => {
      try {
        await fetch("http://example.com", { signal: AbortSignal.timeout(3000) });
        console.log("network: reachable");
      } catch {
        console.log("network: blocked");
      }
      const tryConnect = (label: string, path: string) =>
        new Promise((resolve) => {
          const socket = connect(path);
          socket.on("connect", () => { console.log(label + ": reachable"); socket.destroy(); resolve(0); });
          socket.on("error", () => { console.log(label + ": blocked"); resolve(0); });
        });
      await tryConnect("outside-socket", "/tmp/.s.PGSQL.5432");
      await tryConnect("decoy-socket", process.env.DECOY_SOCKET ?? "/nonexistent");
      try {
        await Bun.write(process.cwd() + "/escape-" + process.pid, "x");
        console.log("outside-write: allowed");
      } catch {
        console.log("outside-write: blocked");
      }
      try {
        await Bun.write("/tmp/midwrite-probe-escape-" + process.env.ESCAPE_TAG, "x");
        console.log("tmp-write: allowed");
      } catch {
        console.log("tmp-write: blocked");
      }
    },
  },
  () => sql.close(),
);
"#;

fn probe_manifest(cases: &str, extra: &str) -> String {
    format!(
        r#"{{
  "schemaVersion": 1,
  "harness": "probe",
  "command": ["bun", "harness.ts"],
  "timeoutSeconds": 15,
  {extra}
  "database": {{ "provider": "ephemeral-postgres", "setup": ["schema.sql"] }},
  "observations": [{{ "name": "rows", "sql": "SELECT count(*) FROM probe" }}],
  "invariants": [{{ "id": "at-most-one-row", "description": "d", "require": [{{ "observation": "rows", "atMost": 1 }}] }}],
  "control": {{ "scenario": "noop" }},
  "cases": [{cases}]
}}"#
    )
}

const PROBE_SQL: &str = "CREATE TABLE probe (id int);\n";

#[test]
fn vulnerable_order_flow_reproduces_consistently_across_runs() {
    require_replay!();
    let (output, report) = replay(
        &repo().join("fixtures/replay/order-vulnerable"),
        &["--repeat", "3"],
        &[],
    );
    let report = report.unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(&output.stderr)));
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(report["kind"], "commitrift.verification");
    assert_eq!(report["control"]["verification"], "RUNTIME_NOT_REPRODUCED");
    assert_eq!(
        report["control"]["observations"]["after"]["publishedEvents"],
        1
    );

    let kill = case(&report, "kill-after-db-commit");
    assert_eq!(kill["verification"], "RUNTIME_REPRODUCED");
    assert_eq!(kill["consistent"], true);
    assert_eq!(kill["staticLink"]["status"], "matched");
    assert_eq!(kill["boundary"], "onTraceBoundary");
    assert_eq!(kill["trace"]["code"], "MW1001");
    assert_eq!(kill["trace"]["verification"], "STATIC_CANDIDATE");
    let runs = kill["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 3);
    for run in runs {
        // Identical initial state, the commit verified from another session
        // before the kill, the kill delivered at the checkpoint.
        assert_eq!(run["observations"]["before"]["orderStatus"], "pending");
        assert_eq!(run["observations"]["before"]["publishedEvents"], 0);
        assert_eq!(run["preInjectionChecks"][0]["holds"], true);
        assert_eq!(run["injected"], true);
        assert_eq!(run["outcome"], "killed");
        assert_eq!(run["exit"]["signal"], 9);
        assert_eq!(run["observations"]["after"]["orderStatus"], "paid");
        assert_eq!(run["observations"]["after"]["publishedEvents"], 0);
        assert!(
            run["prediction"]
                .as_array()
                .unwrap()
                .iter()
                .all(|p| p["matches"] == true)
        );
        let last = run["checkpoints"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(last["name"], "after-db-commit");
        assert_eq!(last["action"], "kill");
    }

    // An exception at the same point: same state, but the process exits.
    let exception = case(&report, "exception-after-db-commit");
    assert_eq!(exception["verification"], "RUNTIME_REPRODUCED");
    assert_eq!(exception["runs"][0]["outcome"], "threw");
    assert_eq!(exception["runs"][0]["exit"]["code"], 1);

    // Failure before the commit and after the publication: no violation.
    assert_eq!(
        case(&report, "kill-before-db-write")["verification"],
        "RUNTIME_NOT_REPRODUCED"
    );
    assert_eq!(
        case(&report, "kill-before-db-write")["boundary"],
        "outsideTraceBoundary"
    );
    assert_eq!(
        case(&report, "kill-after-event-publish")["verification"],
        "RUNTIME_NOT_REPRODUCED"
    );

    // Exception and crash differ when a catch block compensates.
    assert_eq!(
        case(&report, "compensation-exception-before-publish")["verification"],
        "RUNTIME_NOT_REPRODUCED"
    );
    assert_eq!(
        case(&report, "compensation-kill-before-publish")["verification"],
        "RUNTIME_REPRODUCED"
    );
    assert_eq!(report["summary"]["mismatchedExpectations"], 0);
}

#[test]
fn transactional_outbox_keeps_the_intent_and_exposes_duplicates() {
    require_replay!();
    let (output, report) = replay(&repo().join("fixtures/replay/order-outbox"), &[], &[]);
    let report = report.unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(&output.stderr)));
    assert_eq!(output.status.code(), Some(0));
    for c in report["cases"].as_array().unwrap() {
        assert_eq!(c["verification"], "RUNTIME_NOT_REPRODUCED", "{}", c["id"]);
    }
    let rolled_back = &case(&report, "kill-before-tx-commit")["runs"][0];
    assert_eq!(
        rolled_back["observations"]["after"]["orderStatus"],
        "pending"
    );
    assert_eq!(rolled_back["observations"]["after"]["outboxRows"], 0);

    let committed = &case(&report, "kill-after-tx-commit")["runs"][0];
    assert_eq!(committed["observations"]["after"]["pendingOutbox"], 1);
    assert_eq!(committed["observations"]["after"]["publishedEvents"], 0);
    assert_eq!(
        committed["observations"]["afterRecovery"]["publishedEvents"],
        1
    );

    let relay = case(&report, "kill-relay-after-publish-before-ack");
    assert_eq!(relay["staticLink"]["status"], "matched");
    assert_eq!(relay["code"], "MW1002");
    let run = &relay["runs"][0];
    assert_eq!(run["observations"]["after"]["publishedEvents"], 1);
    assert_eq!(run["observations"]["after"]["pendingOutbox"], 1);
    // At-least-once: the retry delivers the same event again.
    assert_eq!(run["observations"]["afterRecovery"]["publishedEvents"], 2);
    assert_eq!(
        run["observations"]["afterRecovery"]["duplicateDeliveries"],
        1
    );
    assert_eq!(run["observations"]["afterRecovery"]["distinctEvents"], 1);
}

#[test]
fn replay_requires_explicit_authorization() {
    let output = Command::new(bin())
        .arg("replay")
        .arg(repo().join("fixtures/replay/order-vulnerable"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Nothing was executed"), "{stderr}");
    assert!(stderr.contains("--allow-execution"), "{stderr}");
    assert!(output.stdout.is_empty());
}

#[test]
fn a_directory_without_a_manifest_is_not_a_harness() {
    let dir = repo().join("fixtures/vulnerable");
    let output = Command::new(bin())
        .args(["replay", "--allow-execution"])
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no replay harness"));
}

#[test]
fn production_looking_database_targets_are_refused_without_echoing_secrets() {
    let manifest = probe_manifest(
        r#"{ "id": "c", "scenario": "noop" }"#,
        "",
    )
    .replace(
        r#""provider": "ephemeral-postgres","#,
        r#""provider": "ephemeral-postgres", "url": "postgres://admin:hunter2@prod-db.example.com:5432/app","#,
    );
    let probe = Probe::new("prod-url", &manifest, PROBE_SQL);
    let output = Command::new(bin())
        .args(["replay", "--allow-execution"])
        .arg(&probe.dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("external database targets are refused"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("hunter2") && !stderr.contains("prod-db"),
        "{stderr}"
    );

    for (name, value) in [
        ("DATABASE_URL", "x"),
        ("STRIPE_SECRET_KEY", "x"),
        ("PGHOST", "x"),
        ("API_BASE", "https://x"),
    ] {
        let manifest = probe_manifest(
            r#"{ "id": "c", "scenario": "noop" }"#,
            &format!(r#""env": {{ "{name}": "{value}" }},"#),
        );
        let probe = Probe::new("env", &manifest, PROBE_SQL);
        let output = Command::new(bin())
            .args(["replay", "--allow-execution"])
            .arg(&probe.dir)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{name}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("env:"),
            "{name}"
        );
    }
}

#[test]
fn the_harness_sees_no_inherited_secrets_and_no_network() {
    require_replay!();
    // A decoy service socket outside the replay directory, like a local
    // PostgreSQL or Docker socket. The harness must not reach it.
    let decoy_path = std::env::temp_dir().join(format!("cr-decoy-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&decoy_path);
    let decoy = std::os::unix::net::UnixListener::bind(&decoy_path).unwrap();
    decoy.set_nonblocking(true).unwrap();
    let tag = format!("{}-{}", std::process::id(), line!());
    let escape = PathBuf::from(format!("/tmp/midwrite-probe-escape-{tag}"));
    let probe = Probe::new(
        "isolation",
        &probe_manifest(
            r#"{ "id": "env", "scenario": "env" }, { "id": "network", "scenario": "network" }"#,
            &format!(
                r#""env": {{ "DECOY_SOCKET": "{}", "ESCAPE_TAG": "{tag}" }},"#,
                decoy_path.display()
            ),
        ),
        PROBE_SQL,
    );
    let secret = "hunter2-do-not-leak";
    let (output, report) = replay(
        &probe.dir,
        &[],
        &[
            ("MIDWRITE_TEST_SECRET", secret),
            (
                "DATABASE_URL",
                "postgres://admin:hunter2-do-not-leak@prod.example.com/app",
            ),
            ("AWS_SECRET_ACCESS_KEY", secret),
        ],
    );
    let accepted = decoy.accept().is_ok();
    let _ = std::fs::remove_file(&decoy_path);
    let report = report.unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(&output.stderr)));
    let text = serde_json::to_string(&report).unwrap();
    assert!(!text.contains(secret), "a secret reached the report");
    let env = &case(&report, "env")["runs"][0]["logs"]["stdout"];
    let env = env.as_str().unwrap();
    assert!(env.contains("secret:absent"), "{env}");
    assert!(env.contains("database-url:absent"), "{env}");
    assert!(!env.contains("AWS_"), "{env}");
    let network = case(&report, "network")["runs"][0]["logs"]["stdout"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(network.contains("network: blocked"), "{network}");
    assert!(network.contains("outside-socket: blocked"), "{network}");
    assert!(network.contains("decoy-socket: blocked"), "{network}");
    assert!(!accepted, "the decoy socket received a connection");
    assert!(network.contains("outside-write: blocked"), "{network}");
    assert!(
        !probe.dir.read_dir().unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("escape-")),
        "the harness wrote into its (read-only) directory"
    );
    // macOS denies the write; Linux gives the harness a private /tmp. Either
    // way nothing reaches the host's /tmp.
    assert!(!escape.exists(), "a write to /tmp escaped the isolation");
    assert!(
        report["environment"]["networkIsolation"]
            .as_str()
            .unwrap()
            .starts_with(expected_isolation()),
        "{}",
        report["environment"]["networkIsolation"]
    );
    assert_eq!(report["environment"]["isolation"], expected_isolation());
    // Variable names are recorded, never values.
    let names = report["environment"]["environmentVariables"]
        .as_array()
        .unwrap();
    assert!(names.iter().all(|n| !n.as_str().unwrap().contains('=')));
}

#[test]
fn timeouts_kill_the_harness_and_clean_up() {
    require_replay!();
    let probe = Probe::new(
        "timeout",
        &probe_manifest(r#"{ "id": "hang", "scenario": "hang" }"#, "")
            .replace(r#""timeoutSeconds": 15"#, r#""timeoutSeconds": 2"#),
        PROBE_SQL,
    );
    let child = Command::new(bin())
        .args(["replay", "--allow-execution", "--format", "json"])
        .arg(&probe.dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    let start = std::time::Instant::now();
    let output = child.wait_with_output().unwrap();
    assert!(start.elapsed().as_secs() < 30);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let hang = case(&report, "hang");
    assert_eq!(hang["verification"], "INCONCLUSIVE");
    assert_eq!(hang["runs"][0]["outcome"], "timedOut");
    assert!(
        hang["runs"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("did not finish")
    );
    assert_eq!(output.status.code(), Some(3));
    // The private replay directory (cluster, sockets, logs) is gone.
    for base in [std::env::temp_dir(), PathBuf::from("/tmp")] {
        let leftovers: Vec<_> = std::fs::read_dir(&base)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(&format!("cr-{pid}-"))
            })
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
    // No harness process survived.
    let ps = Command::new("ps")
        .args(["-A", "-o", "args"])
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&ps.stdout).contains(&probe.dir.display().to_string()));
}

#[test]
fn failed_database_setup_is_inconclusive() {
    require_replay!();
    let probe = Probe::new(
        "bad-setup",
        &probe_manifest(r#"{ "id": "insert", "scenario": "insert" }"#, ""),
        "CREATE TABLE probe (id int);\nTHIS IS NOT SQL;\n",
    );
    let (output, report) = replay(&probe.dir, &[], &[]);
    let report = report.unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(report["control"]["verification"], "INCONCLUSIVE");
    assert!(
        report["control"]["reason"]
            .as_str()
            .unwrap()
            .contains("database setup failed")
    );
    assert_eq!(case(&report, "insert")["verification"], "INCONCLUSIVE");
    assert!(
        case(&report, "insert")["runs"]
            .as_array()
            .unwrap()
            .is_empty(),
        "no case runs after a failed control"
    );
}

#[test]
fn unreachable_checkpoints_failed_pre_checks_and_missing_runtimes_are_inconclusive() {
    require_replay!();
    let probe = Probe::new(
        "inconclusive",
        &probe_manifest(
            r#"
            { "id": "never", "scenario": "noop", "inject": { "checkpoint": "after-db-commit", "mode": "processKill" } },
            { "id": "precheck", "scenario": "insert",
              "inject": { "checkpoint": "after-db-commit", "mode": "processKill" },
              "verifyBeforeInjection": [{ "observation": "rows", "equals": 5 }] },
            { "id": "ok", "scenario": "insert",
              "inject": { "checkpoint": "after-db-commit", "mode": "processKill" },
              "verifyBeforeInjection": [{ "observation": "rows", "equals": 1 }],
              "expect": "RUNTIME_NOT_REPRODUCED" }"#,
            "",
        ),
        PROBE_SQL,
    );
    let (_, report) = replay(&probe.dir, &[], &[]);
    let report = report.unwrap();
    let never = case(&report, "never");
    assert_eq!(never["verification"], "INCONCLUSIVE");
    assert!(
        never["runs"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("never reached")
    );
    let precheck = case(&report, "precheck");
    assert_eq!(precheck["verification"], "INCONCLUSIVE");
    assert_eq!(precheck["runs"][0]["injected"], false);
    assert!(
        precheck["runs"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("pre-injection check failed")
    );
    assert_eq!(
        case(&report, "ok")["verification"],
        "RUNTIME_NOT_REPRODUCED"
    );

    let missing = Probe::new(
        "missing-runtime",
        &probe_manifest(r#"{ "id": "c", "scenario": "noop" }"#, "").replace(
            r#"["bun", "harness.ts"]"#,
            r#"["midwrite-no-such-runtime"]"#,
        ),
        PROBE_SQL,
    );
    let (output, report) = replay(&missing.dir, &[], &[]);
    let report = report.unwrap();
    assert_eq!(output.status.code(), Some(3));
    let reason = report["control"]["reason"].as_str().unwrap();
    assert!(reason.contains("could not be started"), "{reason}");
}

#[test]
fn reports_are_deterministic_apart_from_durations() {
    require_replay!();
    let strip = |mut value: Value| {
        fn walk(value: &mut Value) {
            match value {
                Value::Object(map) => {
                    map.remove("durationMs");
                    map.values_mut().for_each(walk);
                }
                Value::Array(items) => items.iter_mut().for_each(walk),
                _ => {}
            }
        }
        walk(&mut value);
        value
    };
    let dir = repo().join("fixtures/replay/order-vulnerable");
    let args = ["--case", "kill-after-db-commit"];
    let first = strip(replay(&dir, &args, &[]).1.unwrap());
    let second = strip(replay(&dir, &args, &[]).1.unwrap());
    assert_eq!(first, second);
}

#[test]
fn a_diagnostic_code_must_identify_a_unique_case() {
    let output = Command::new(bin())
        .args(["replay", "--case", "MW1001"])
        .arg(repo().join("fixtures/replay/order-vulnerable"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not identify a unique trace"),
        "{stderr}"
    );
    // In the outbox harness exactly... two cases target MW1002 as well.
    let output = Command::new(bin())
        .args(["replay", "--case", "kill-after-tx-commit"])
        .arg(repo().join("fixtures/replay/order-outbox"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("kill-after-tx-commit"), "{stderr}");
    assert!(
        !stderr.contains("kill-relay-before-publish"),
        "only the selected case: {stderr}"
    );
}

#[test]
fn static_analysis_never_executes_target_code() {
    let dir = std::env::temp_dir().join(format!("midwrite-noexec-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let marker = dir.join("EXECUTED");
    std::fs::write(
        dir.join("index.ts"),
        format!(
            "import {{ writeFileSync }} from \"node:fs\";\nwriteFileSync({:?}, \"x\");\nexport async function f(id: string) {{\n  await db.update(orders).set({{ status: \"paid\" }});\n  await queue.publish(\"x\", {{ id }});\n}}\n",
            marker.display().to_string()
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("midwrite-replay.json"),
        probe_manifest(r#"{ "id": "c", "scenario": "noop" }"#, ""),
    )
    .unwrap();
    for args in [
        vec!["check", "."],
        vec!["trace", "."],
        vec!["ir", "."],
        vec!["trace", ".", "--format", "json"],
    ] {
        let output = Command::new(bin())
            .args(&args)
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(output.status.code().is_some());
    }
    // A replay manifest in the directory does not make anything run either.
    let _ = Command::new(bin())
        .args(["replay", "."])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(!marker.exists(), "static analysis executed target code");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn verification_reports_render_static_and_runtime_evidence_separately() {
    require_replay!();
    let dir = repo().join("fixtures/replay/order-vulnerable");
    let out =
        std::env::temp_dir().join(format!("midwrite-verification-{}.json", std::process::id()));
    let (output, _) = replay(
        &dir,
        &[
            "--case",
            "kill-after-db-commit",
            "--output",
            out.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(output.status.code(), Some(0));
    let html = Command::new(bin())
        .args(["report", out.to_str().unwrap()])
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&out);
    assert_eq!(html.status.code(), Some(0));
    let html = String::from_utf8(html.stdout).unwrap();
    assert!(html.contains("RUNTIME REPRODUCED"));
    assert!(html.contains("STATIC CANDIDATE — not executed"));
    assert!(html.contains("Static prediction") && html.contains("Runtime observation"));
    assert!(html.contains("CONTROL: invariants held"));
    assert!(!html.contains("<script"));
}

#[test]
fn the_isolated_invoice_pattern_loses_the_job_unless_retried() {
    require_replay!();
    let (output, report) = replay(&repo().join("fixtures/replay/invoice-pattern"), &[], &[]);
    let report = report.unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(&output.stderr)));
    assert_eq!(output.status.code(), Some(0));
    let kill = case(&report, "kill-after-status-commit");
    assert_eq!(kill["verification"], "RUNTIME_REPRODUCED");
    assert_eq!(kill["staticLink"]["status"], "matched");
    assert_eq!(
        kill["runs"][0]["observations"]["after"]["invoiceStatus"],
        "unpaid"
    );
    assert_eq!(kill["runs"][0]["observations"]["after"]["generateJobs"], 0);
    let retry = &case(&report, "client-retry-repairs")["runs"][0];
    assert_eq!(retry["observations"]["afterRecovery"]["generateJobs"], 1);
    let after_recovery: Vec<&Value> = retry["invariants"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["stage"] == "afterRecovery")
        .collect();
    assert!(after_recovery.iter().all(|i| i["holds"] == true));
}

// ---------------------------------------------------------------------------
// Phase 4: Node.js runtime and the real Drizzle ORM.

/// Why Node.js harness tests cannot run here, if they cannot.
fn missing_node() -> Option<String> {
    if !on_path("node") {
        return Some("node is not installed".to_owned());
    }
    let version = Command::new("node").arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&version.stdout);
    let major: u64 = text
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|m| m.parse().ok())
        .unwrap_or(0);
    (major < 22).then(|| format!("Node.js {} is older than 22", text.trim()))
}

/// Compile the Drizzle fixture once per test process (locally, offline; it
/// needs `npm ci` to have installed its pinned dependencies).
fn drizzle_fixture() -> Result<PathBuf, String> {
    static BUILT: std::sync::OnceLock<Result<PathBuf, String>> = std::sync::OnceLock::new();
    BUILT
        .get_or_init(|| {
            let dir = repo().join("fixtures/replay/drizzle-node");
            if !dir.join("node_modules/drizzle-orm/package.json").is_file() {
                return Err(
                    "the Drizzle fixture's dependencies are not installed: run `npm ci` in fixtures/replay/drizzle-node"
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
                    "the Drizzle fixture does not compile: {}",
                    String::from_utf8_lossy(&output.stdout)
                ));
            }
            Ok(dir)
        })
        .clone()
}

macro_rules! require_drizzle {
    () => {{
        require_replay!();
        if let Some(reason) = missing_node() {
            assert!(
                std::env::var_os("MIDWRITE_REQUIRE_REPLAY").is_none(),
                "replay dependencies required: {reason}"
            );
            eprintln!("SKIPPED: {reason}");
            return;
        }
        match drizzle_fixture() {
            Ok(dir) => dir,
            Err(reason) => {
                assert!(
                    std::env::var_os("MIDWRITE_REQUIRE_REPLAY").is_none(),
                    "replay dependencies required: {reason}"
                );
                eprintln!("SKIPPED: {reason}");
                return;
            }
        }
    }};
}

fn checkpoint_names(run: &Value) -> Vec<String> {
    run["checkpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            format!(
                "{}({})",
                c["name"].as_str().unwrap(),
                c["label"].as_str().unwrap_or("")
            )
        })
        .collect()
}

#[test]
fn the_node_runtime_protocol_client_passes_its_own_tests() {
    if let Some(reason) = missing_node() {
        assert!(
            std::env::var_os("MIDWRITE_REQUIRE_REPLAY").is_none(),
            "replay dependencies required: {reason}"
        );
        eprintln!("SKIPPED: {reason}");
        return;
    }
    let output = Command::new("node")
        .args(["--test", "runtime/node/test/runtime.test.js"])
        .current_dir(repo())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("# fail 0"), "{stdout}");
}

#[test]
fn real_drizzle_on_node_reproduces_the_dual_write_and_respects_transactions() {
    let dir = require_drizzle!();
    let (output, report) = replay(&dir, &[], &[]);
    let report = report.unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(&output.stderr)));
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        report["environment"]["runtime"]
            .as_str()
            .unwrap()
            .starts_with("node v")
    );
    assert_eq!(report["environment"]["isolation"], expected_isolation());
    assert_eq!(report["control"]["verification"], "RUNTIME_NOT_REPRODUCED");
    assert_eq!(report["summary"]["mismatchedExpectations"], 0);
    assert_eq!(report["summary"]["inconclusive"], 0);

    // A: the auto-commit UPDATE survives the kill, the event does not exist.
    let a = case(&report, "a-kill-after-db-commit");
    assert_eq!(a["verification"], "RUNTIME_REPRODUCED");
    assert_eq!(a["staticLink"]["status"], "matched");
    assert_eq!(a["code"], "MW1001");
    assert_eq!(a["boundary"], "onTraceBoundary");
    let run = &a["runs"][0];
    assert_eq!(run["preInjectionChecks"][0]["holds"], true);
    assert_eq!(run["outcome"], "killed");
    assert_eq!(run["exit"]["signal"], 9);
    assert_eq!(run["observations"]["before"]["order1Status"], "pending");
    assert_eq!(run["observations"]["after"]["order1Status"], "paid");
    assert_eq!(run["observations"]["after"]["completedEvents"], 0);
    assert_eq!(
        checkpoint_names(run),
        ["before-db-write(orders)", "after-db-commit(orders)"]
    );
    // Exception and kill stay distinguishable.
    let a_exception = &case(&report, "a-exception-after-db-commit")["runs"][0];
    assert_eq!(a_exception["outcome"], "threw");
    assert_eq!(a_exception["exit"]["code"], 1);
    // Drizzle wraps the fault raised in the driver call; the runtime finds
    // it in the cause chain.
    assert!(
        a_exception["error"]
            .as_str()
            .unwrap()
            .contains("caused by InjectedFault"),
        "{}",
        a_exception["error"]
    );
    assert_eq!(a_exception["injectedFaultSurfaced"], true);
    assert!(run.get("injectedFaultSurfaced").is_none());
    assert_eq!(a_exception["failureMode"], "exception");
    assert_eq!(run["failureMode"], "processKill");

    // B: a Drizzle transaction that fails before COMMIT leaves nothing, and
    // the uncommitted writes were invisible from another session.
    for id in ["b-exception-before-tx-commit", "b-kill-after-tx-write"] {
        let b = case(&report, id);
        assert_eq!(b["verification"], "RUNTIME_NOT_REPRODUCED", "{id}");
        let after = &b["runs"][0]["observations"]["after"];
        assert_eq!(after["order2Status"], "pending", "{id}");
        assert_eq!(after["order2Payments"], 0, "{id}");
        assert!(
            !checkpoint_names(&b["runs"][0]).contains(&"after-db-commit(transaction)".to_owned()),
            "{id}: no commit may be claimed"
        );
    }
    let b_exception = &case(&report, "b-exception-before-tx-commit")["runs"][0];
    assert!(
        b_exception["preInjectionChecks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["holds"] == true)
    );
    assert!(checkpoint_names(b_exception).contains(&"after-tx-rollback(transaction)".to_owned()));
    let b_commit = &case(&report, "b-kill-after-tx-commit")["runs"][0];
    assert_eq!(b_commit["observations"]["after"]["order2Status"], "paid");
    assert_eq!(b_commit["observations"]["after"]["order2Payments"], 1);

    // C: the event published inside the transaction survives its rollback,
    // whether the rollback comes from an exception, a crash, or a COMMIT
    // that PostgreSQL rejects.
    for id in [
        "c-exception-after-event-publish",
        "c-kill-after-event-publish",
        "c-commit-fails-after-publish",
    ] {
        let c = case(&report, id);
        assert_eq!(c["verification"], "RUNTIME_REPRODUCED", "{id}");
        assert_eq!(c["code"], "MW1003", "{id}");
        let after = &c["runs"][0]["observations"]["after"];
        assert_eq!(after["shippedEvents"], 1, "{id}");
        assert_eq!(after["order3Status"], "pending", "{id}");
        assert_eq!(after["order3Shipments"], 0, "{id}");
    }
    let failed_commit = &case(&report, "c-commit-fails-after-publish")["runs"][0];
    assert_eq!(failed_commit["injected"], false);
    assert_eq!(failed_commit["failureMode"], "none");
    let names = checkpoint_names(failed_commit);
    assert!(names.contains(&"before-tx-commit(transaction)".to_owned()));
    assert!(!names.contains(&"after-db-commit(transaction)".to_owned()));
    assert!(
        failed_commit["logs"]["stderr"]
            .as_str()
            .unwrap()
            .contains("shipments_tracking_number_key"),
        "{}",
        failed_commit["logs"]["stderr"]
    );
    assert_eq!(
        case(&report, "c-kill-before-event-publish")["verification"],
        "RUNTIME_NOT_REPRODUCED"
    );

    // D: the outbox keeps the intent at every failure point; the relay's
    // own window yields a duplicate, never a loss.
    for c in report["cases"].as_array().unwrap() {
        if c["id"].as_str().unwrap().starts_with("d-") {
            assert_eq!(c["verification"], "RUNTIME_NOT_REPRODUCED", "{}", c["id"]);
        }
    }
    let committed = &case(&report, "d-kill-after-tx-commit")["runs"][0];
    assert_eq!(committed["observations"]["after"]["pendingOutbox"], 1);
    assert_eq!(committed["observations"]["after"]["confirmedEvents"], 0);
    assert_eq!(
        committed["observations"]["afterRecovery"]["confirmedEvents"],
        1
    );
    let relay = &case(&report, "d-kill-relay-after-publish-before-ack")["runs"][0];
    assert_eq!(
        relay["observations"]["afterRecovery"]["confirmedDuplicates"],
        1
    );
    assert_eq!(relay["observations"]["afterRecovery"]["pendingOutbox"], 0);
}

#[test]
fn the_drizzle_dual_write_reproduces_in_every_repeated_run() {
    let dir = require_drizzle!();
    let (output, report) = replay(
        &dir,
        &["--case", "a-kill-after-db-commit", "--repeat", "5"],
        &[],
    );
    let report = report.unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(&output.stderr)));
    assert_eq!(output.status.code(), Some(0));
    let a = case(&report, "a-kill-after-db-commit");
    assert_eq!(a["verification"], "RUNTIME_REPRODUCED");
    assert_eq!(a["consistent"], true);
    let runs = a["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 5);
    assert!(
        runs.iter()
            .all(|r| r["verification"] == "RUNTIME_REPRODUCED"
                && r["observations"]["after"]["order1Status"] == "paid"
                && r["observations"]["after"]["completedEvents"] == 0)
    );
}

#[test]
fn a_declared_node_runtime_without_a_compiled_entry_point_is_blocked() {
    let manifest = probe_manifest(
        r#"{ "id": "c", "scenario": "noop" }"#,
        r#""runtime": "node","#,
    )
    .replace(r#"["bun", "harness.ts"]"#, r#"["node", "dist/harness.js"]"#);
    let probe = Probe::new("node-unbuilt", &manifest, PROBE_SQL);
    let output = Command::new(bin())
        .args(["replay", "--allow-execution"])
        .arg(&probe.dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("compile the harness first"), "{stderr}");
    assert!(stderr.contains("never installs packages"), "{stderr}");

    let mismatched = probe_manifest(
        r#"{ "id": "c", "scenario": "noop" }"#,
        r#""runtime": "node","#,
    );
    let probe = Probe::new("node-mismatch", &mismatched, PROBE_SQL);
    let output = Command::new(bin())
        .args(["replay", "--allow-execution"])
        .arg(&probe.dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not start it"));
}

const NODE_PROBE: &str = r#"
import { connect } from "node:net";
import { writeFileSync } from "node:fs";
import { runScenarios } from "@RUNTIME@/index.js";

await runScenarios({
  noop: async () => {},
  network: async () => {
    try {
      await fetch("http://example.com", { signal: AbortSignal.timeout(3000) });
      console.log("network: reachable");
    } catch {
      console.log("network: blocked");
    }
    await new Promise((resolve) => {
      const socket = connect(process.env.DECOY_SOCKET ?? "/nonexistent");
      socket.on("connect", () => { console.log("decoy-socket: reachable"); socket.destroy(); resolve(0); });
      socket.on("error", () => { console.log("decoy-socket: blocked"); resolve(0); });
    });
    try {
      writeFileSync(process.cwd() + "/escape-" + process.pid, "x");
      console.log("outside-write: allowed");
    } catch {
      console.log("outside-write: blocked");
    }
  },
});
"#;

#[test]
fn node_harnesses_are_isolated_like_bun_harnesses() {
    require_replay!();
    if let Some(reason) = missing_node() {
        assert!(
            std::env::var_os("MIDWRITE_REQUIRE_REPLAY").is_none(),
            "replay dependencies required: {reason}"
        );
        eprintln!("SKIPPED: {reason}");
        return;
    }
    let decoy_path = std::env::temp_dir().join(format!("cr-ndecoy-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&decoy_path);
    let decoy = std::os::unix::net::UnixListener::bind(&decoy_path).unwrap();
    decoy.set_nonblocking(true).unwrap();
    let manifest = probe_manifest(
        r#"{ "id": "network", "scenario": "network" }"#,
        &format!(
            r#""runtime": "node", "env": {{ "DECOY_SOCKET": "{}" }},"#,
            decoy_path.display()
        ),
    )
    .replace(r#"["bun", "harness.ts"]"#, r#"["node", "probe.mjs"]"#);
    let probe = Probe::new("node-isolation", &manifest, PROBE_SQL);
    std::fs::write(
        probe.dir.join("probe.mjs"),
        NODE_PROBE.replace(
            "@RUNTIME@",
            &repo().join("runtime/node").display().to_string(),
        ),
    )
    .unwrap();
    let (output, report) = replay(&probe.dir, &[], &[]);
    let accepted = decoy.accept().is_ok();
    let _ = std::fs::remove_file(&decoy_path);
    let report = report.unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(&output.stderr)));
    let stdout = case(&report, "network")["runs"][0]["logs"]["stdout"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(stdout.contains("network: blocked"), "{stdout}");
    assert!(stdout.contains("decoy-socket: blocked"), "{stdout}");
    assert!(stdout.contains("outside-write: blocked"), "{stdout}");
    assert!(!accepted);
    assert_eq!(report["environment"]["isolation"], expected_isolation());
}

// ---------------------------------------------------------------------------
// Phase 4: trace-to-harness generation (`midwrite init-replay`).

const DRIZZLE_MW1001: &str = "4f53a6215ac16913";

fn drizzle_dir() -> PathBuf {
    repo().join("fixtures/replay/drizzle-node")
}

/// `midwrite trace src --format json`, run in the Drizzle fixture.
fn drizzle_trace_report(into: &Path) {
    let output = Command::new(bin())
        .args(["trace", "src", "--format", "json"])
        .current_dir(drizzle_dir())
        .output()
        .unwrap();
    assert!(output.status.success());
    std::fs::write(into, &output.stdout).unwrap();
}

/// A scratch output directory inside the Drizzle project, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = drizzle_dir().join(format!(".gen-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn init_replay(args: &[&str]) -> Output {
    Command::new(bin())
        .arg("init-replay")
        .args(args)
        .current_dir(drizzle_dir())
        .output()
        .unwrap()
}

#[test]
fn init_replay_writes_a_skeleton_linked_to_the_finding_that_refuses_to_run() {
    let scratch = Scratch::new("skeleton");
    let trace = scratch.0.with_extension("trace.json");
    drizzle_trace_report(&trace);
    let out = scratch.0.file_name().unwrap().to_str().unwrap().to_owned();
    let output = init_replay(&[
        "--trace",
        trace.to_str().unwrap(),
        "--finding",
        DRIZZLE_MW1001,
        "--output",
        &out,
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(
        !stderr.contains("warning"),
        "the finding must link: {stderr}"
    );
    for file in [
        "midwrite-replay.json",
        "harness.ts",
        "schema.sql",
        "package.json",
        "tsconfig.json",
        "README.md",
        "finding.json",
        "midwrite-runtime/index.js",
        "midwrite-runtime/pg.js",
    ] {
        assert!(scratch.0.join(file).is_file(), "{file}");
    }
    // Nothing else was created or installed, and the application is untouched.
    assert!(!scratch.0.join("node_modules").exists());
    assert!(!scratch.0.join("dist").exists());

    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(scratch.0.join("midwrite-replay.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["runtime"], "node");
    assert_eq!(manifest["analysis"]["root"], "..");
    assert_eq!(manifest["analysis"]["paths"][0], "src");
    let cases = manifest["cases"].as_array().unwrap();
    assert!(cases.iter().all(|c| c["finding"]["id"] == DRIZZLE_MW1001));
    let boundary = &cases[0];
    assert_eq!(boundary["inject"]["checkpoint"], "after-db-commit");
    assert_eq!(boundary["inject"]["label"], "orders");
    assert_eq!(boundary["inject"]["mode"], "processKill");
    assert_eq!(cases[1]["inject"]["mode"], "exception");
    let ids: Vec<&str> = cases.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"negative-kill-before-db-write"), "{ids:?}");
    assert!(
        ids.contains(&"negative-kill-after-event-publish"),
        "{ids:?}"
    );
    // No invented SQL, topics, inputs or invariants: placeholders only, and
    // no `expect` (the developer decides).
    assert!(
        manifest["observations"][0]["sql"]
            .as_str()
            .unwrap()
            .starts_with("MIDWRITE-TODO")
    );
    assert!(
        manifest["invariants"][0]["description"]
            .as_str()
            .unwrap()
            .starts_with("MIDWRITE-TODO")
    );
    assert!(cases.iter().all(|c| c.get("expect").is_none()));
    let harness = std::fs::read_to_string(scratch.0.join("harness.ts")).unwrap();
    assert!(harness.contains("throw new Error(\"MIDWRITE-TODO"));
    assert!(
        harness.contains("src/service.ts:13:9"),
        "source spans of the effects"
    );
    let readme = std::fs::read_to_string(scratch.0.join("README.md")).unwrap();
    assert!(readme.contains("failure injection boundary"));
    assert!(readme.contains("What you must do"));
    // The vendored client is byte-identical to runtime/node.
    for file in ["index.js", "pg.js", "event-log.js"] {
        assert_eq!(
            std::fs::read(scratch.0.join("midwrite-runtime").join(file)).unwrap(),
            std::fs::read(repo().join("runtime/node").join(file)).unwrap(),
            "{file}"
        );
    }

    // Replay refuses the unfinished skeleton before anything runs.
    let replayed = Command::new(bin())
        .args(["replay", "--allow-execution"])
        .arg(&scratch.0)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&replayed.stderr);
    assert_eq!(replayed.status.code(), Some(2), "{text}");
    assert!(text.contains("unfinished placeholder"), "{text}");
    assert!(text.contains("/observations/0/sql"), "{text}");

    // Existing files are never overwritten without --force.
    let again = init_replay(&[
        "--trace",
        trace.to_str().unwrap(),
        "--finding",
        DRIZZLE_MW1001,
        "--output",
        &out,
    ]);
    assert_eq!(again.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&again.stderr).contains("refusing to overwrite"));
    std::fs::write(scratch.0.join("notes.txt"), "mine").unwrap();
    let forced = init_replay(&[
        "--trace",
        trace.to_str().unwrap(),
        "--finding",
        DRIZZLE_MW1001,
        "--output",
        &out,
        "--force",
    ]);
    assert_eq!(forced.status.code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(scratch.0.join("notes.txt")).unwrap(),
        "mine"
    );
    let _ = std::fs::remove_file(&trace);
}

#[test]
fn init_replay_refuses_bad_requests() {
    let scratch = Scratch::new("refusals");
    let trace = scratch.0.with_extension("trace.json");
    drizzle_trace_report(&trace);
    let trace_arg = trace.to_str().unwrap().to_owned();
    // Unknown finding.
    let output = init_replay(&[
        "--trace",
        &trace_arg,
        "--finding",
        "0000000000000000",
        "--output",
        "x",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no finding or trace"));
    // Output outside the project.
    let outside = std::env::temp_dir().join(format!("cr-outside-{}", std::process::id()));
    let output = init_replay(&[
        "--trace",
        &trace_arg,
        "--finding",
        DRIZZLE_MW1001,
        "--output",
        outside.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("inside the project"));
    assert!(!outside.exists());
    // The project root itself is not an output directory.
    let output = init_replay(&[
        "--trace",
        &trace_arg,
        "--finding",
        DRIZZLE_MW1001,
        "--output",
        ".",
    ]);
    assert_eq!(output.status.code(), Some(2));
    // A wrong --project is detected from the finding's file.
    let output = Command::new(bin())
        .args([
            "init-replay",
            "--trace",
            &trace_arg,
            "--finding",
            DRIZZLE_MW1001,
            "--output",
            "target/x",
        ])
        .current_dir(repo())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not under --project"));
    // Not a trace report.
    let output = init_replay(&[
        "--trace",
        "midwrite.json",
        "--finding",
        DRIZZLE_MW1001,
        "--output",
        "x",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not a trace report"));
    let _ = std::fs::remove_file(&trace);
}

#[test]
fn the_completed_generated_harness_reproduces_the_drizzle_dual_write() {
    let dir = require_drizzle!();
    let harness = dir.join("generated-harness");
    // The committed, completed skeleton: compile it with the project's tsc.
    let output = Command::new("node")
        .args(["../node_modules/typescript/bin/tsc", "-p", "tsconfig.json"])
        .current_dir(&harness)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    // Its vendored client has not drifted from runtime/node.
    for file in ["index.js", "pg.js", "event-log.js"] {
        assert_eq!(
            std::fs::read(harness.join("midwrite-runtime").join(file)).unwrap(),
            std::fs::read(repo().join("runtime/node").join(file)).unwrap(),
            "{file}: regenerate generated-harness/midwrite-runtime"
        );
    }
    let (output, report) = replay(&harness, &[], &[]);
    let report = report.unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(&output.stderr)));
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(report["summary"]["mismatchedExpectations"], 0);
    let kill = case(&report, "kill-after-db-commit");
    assert_eq!(kill["verification"], "RUNTIME_REPRODUCED");
    // Linked by the exact fingerprint of `midwrite trace src` in the project.
    assert_eq!(kill["findingId"], DRIZZLE_MW1001);
    assert_eq!(kill["staticLink"]["status"], "matched");
    assert_eq!(kill["boundary"], "onTraceBoundary");
    assert_eq!(
        case(&report, "negative-kill-before-db-write")["verification"],
        "RUNTIME_NOT_REPRODUCED"
    );
}

// ---------------------------------------------------------------------------
// Phase 4: replay regression against a baseline verification report.

#[test]
fn replay_regressions_compare_with_a_baseline_and_never_pass_unverified_cases() {
    require_replay!();
    let dir = repo().join("fixtures/replay/order-vulnerable");
    let scratch = repo()
        .join("target/midwrite-probes")
        .join(format!("regression-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();

    // The Phase 3 report (schema version 1) is a valid baseline: same case
    // identities, same outcomes.
    let phase3 = repo().join("reports/phase3/replay/order-vulnerable.json");
    let regression = scratch.join("regression.json");
    let (output, report) = replay(
        &dir,
        &[
            "--baseline",
            phase3.to_str().unwrap(),
            "--regression-output",
            regression.to_str().unwrap(),
        ],
        &[],
    );
    let report = report.unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(&output.stderr)));
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(report["schemaVersion"], 2);
    assert_eq!(
        case(&report, "kill-after-db-commit")["key"],
        "order-vulnerable/kill-after-db-commit"
    );
    let comparison: Value = serde_json::from_slice(&std::fs::read(&regression).unwrap()).unwrap();
    assert_eq!(comparison["kind"], "commitrift.regression");
    assert_eq!(comparison["summary"]["stillReproduced"], 3);
    assert_eq!(comparison["summary"]["stillNotReproduced"], 3);
    let entry = comparison["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == "order-vulnerable/kill-after-db-commit")
        .unwrap();
    assert_eq!(entry["status"], "stillReproduced");
    assert_eq!(entry["violatedInvariants"][0], "paid-order-has-event");
    assert_eq!(entry["checkpoint"]["name"], "after-db-commit");

    // A baseline in which a reproduced case was safe: newly reproduced (1).
    let mut edited: Value = serde_json::from_slice(&std::fs::read(&phase3).unwrap()).unwrap();
    for c in edited["cases"].as_array_mut().unwrap() {
        if c["id"] == "kill-after-db-commit" {
            c["verification"] = "RUNTIME_NOT_REPRODUCED".into();
        }
    }
    let edited_path = scratch.join("edited.json");
    std::fs::write(&edited_path, serde_json::to_vec(&edited).unwrap()).unwrap();
    let (output, _) = replay(
        &dir,
        &[
            "--case",
            "kill-after-db-commit",
            "--baseline",
            edited_path.to_str().unwrap(),
            "--regression-output",
            regression.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let comparison: Value = serde_json::from_slice(&std::fs::read(&regression).unwrap()).unwrap();
    assert_eq!(comparison["summary"]["newlyReproduced"], 1);
    // The other baseline cases were not executed: reported, never passed.
    assert_eq!(comparison["summary"]["notExecuted"], 5);

    // Only unexecuted cases besides an unchanged one: exit 3, not 0.
    let (output, _) = replay(
        &dir,
        &[
            "--case",
            "kill-after-db-commit",
            "--baseline",
            phase3.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(output.status.code(), Some(3));

    // A report of another harness is not a baseline.
    let other = repo().join("reports/phase3/replay/order-outbox.json");
    let (output, _) = replay(&dir, &["--baseline", other.to_str().unwrap()], &[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not `order-vulnerable`"));
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn init_replay_never_writes_through_a_symlinked_subdirectory() {
    let scratch = Scratch::new("symlink");
    let trace = scratch.0.with_extension("trace.json");
    drizzle_trace_report(&trace);
    let outside = std::env::temp_dir().join(format!("cr-symlink-target-{}", std::process::id()));
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::create_dir_all(&scratch.0).unwrap();
    std::os::unix::fs::symlink(&outside, scratch.0.join("midwrite-runtime")).unwrap();
    let out = scratch.0.file_name().unwrap().to_str().unwrap().to_owned();
    let output = init_replay(&[
        "--trace",
        trace.to_str().unwrap(),
        "--finding",
        DRIZZLE_MW1001,
        "--output",
        &out,
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("outside the output directory"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_dir(&outside).unwrap().count(),
        0,
        "nothing written outside"
    );
    let _ = std::fs::remove_dir_all(&outside);
    let _ = std::fs::remove_file(&trace);
}

#[test]
fn detached_grandchildren_do_not_survive_on_linux() {
    require_replay!();
    let mark = format!("{}", 3000 + std::process::id() % 900);
    let probe = Probe::new(
        "detach",
        &probe_manifest(
            r#"{ "id": "detach", "scenario": "detach" }"#,
            &format!(r#""env": {{ "SLEEP_MARK": "{mark}" }},"#),
        ),
        PROBE_SQL,
    );
    let (output, report) = replay(&probe.dir, &[], &[]);
    let report = report.unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(&output.stderr)));
    assert_eq!(case(&report, "detach")["runs"][0]["outcome"], "returned");
    std::thread::sleep(std::time::Duration::from_millis(300));
    let ps = Command::new("ps")
        .args(["-A", "-o", "pid=,args="])
        .output()
        .unwrap();
    let survivors: Vec<String> = String::from_utf8_lossy(&ps.stdout)
        .lines()
        .filter(|l| l.trim_end().ends_with(&format!("sleep {mark}")))
        .map(str::to_owned)
        .collect();
    if cfg!(target_os = "linux") {
        // bubblewrap's PID namespace ends with the harness.
        assert!(survivors.is_empty(), "{survivors:?}");
    } else {
        // Documented macOS limitation: a process that leaves the process
        // group outlives the harness. Clean up and record it.
        for line in &survivors {
            if let Some(pid) = line.split_whitespace().next() {
                let _ = Command::new("kill").args(["-9", pid]).status();
            }
        }
        eprintln!(
            "macOS: {} detached process(es) outlived the harness (documented limitation)",
            survivors.len()
        );
    }
}

#[test]
fn html_renders_v1_reports_with_the_finding_to_case_table_and_no_scripts() {
    let report = repo().join("reports/phase3/replay/order-vulnerable.json");
    let output = Command::new(bin())
        .arg("report")
        .arg(&report)
        .output()
        .unwrap();
    assert!(output.status.success());
    let html = String::from_utf8_lossy(&output.stdout);
    assert!(html.contains("Findings and cases"));
    assert!(html.contains("href=\"#case-kill-after-db-commit\""));
    assert!(html.contains("id=\"case-kill-after-db-commit\""));
    assert!(html.contains("Evidence by provenance"));
    assert!(
        html.contains("reproduced in 5 of 5 run(s)"),
        "Phase 3 ran 5 repetitions"
    );
    assert!(!html.to_ascii_lowercase().contains("<script"));
    assert!(html.contains("Content-Security-Policy"));
}

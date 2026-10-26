//! Phase 8: process lifecycle of source verification — planned kills,
//! restarts that fail to start, hung and CPU-burning workers, and detached
//! grandchildren — against fixtures/phase8/lifecycle-harness. Needs Node.js
//! and PostgreSQL 14+ binaries; prints SKIPPED otherwise, unless
//! `COMMITRIFT_REQUIRE_REPLAY=1`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use commitrift_replay::source::{
    self, ActorLaunch, CrashStep, Expected, LoadedSource, PlanStep, RestartStep, RunRequest,
    SourceRun, SqlKind,
};
use serde_json::{Value, json};

fn harness() -> LoadedSource {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/phase8/lifecycle-harness/commitrift-source.json");
    source::load_manifest(&path).expect("lifecycle manifest")
}

fn missing() -> Option<String> {
    match commitrift_replay::postgres::Binaries::discover() {
        Err(e) => return Some(e),
        Ok(b) if commitrift_replay::postgres::major_version(&b.version) < 14 => {
            return Some(format!("{} is older than 14", b.version));
        }
        Ok(_) => {}
    }
    match std::process::Command::new("node").arg("--version").output() {
        Ok(o) if o.status.success() => None,
        _ => Some("Node.js is not installed".to_owned()),
    }
}

macro_rules! require {
    () => {
        if let Some(reason) = missing() {
            assert!(
                std::env::var_os("COMMITRIFT_REQUIRE_REPLAY").is_none(),
                "lifecycle test dependencies required: {reason}"
            );
            eprintln!("SKIPPED: {reason}");
            return;
        }
    };
}

fn ping(transition: usize) -> Expected {
    Expected {
        actor: 0,
        kind: SqlKind::Effect,
        table: Some("ping".to_owned()),
        region: None,
        expect_block: false,
        transition,
        label: "EXTERNAL ping".to_owned(),
        key: Some(json!("k")),
    }
}

fn run(entry: &str, plan: &[PlanStep]) -> SourceRun {
    let h = harness();
    let actors = [ActorLaunch {
        name: "W".to_owned(),
        entry: entry.to_owned(),
        input: Value::Null,
        scope: None,
    }];
    source::run(&RunRequest {
        harness: &h,
        allow_execution: true,
        keep_artifacts: false,
        actors: &actors,
        plan,
        seed_sql: "",
        final_queries: &[],
        settle: Duration::from_millis(50),
        channels: &BTreeMap::from([("ping".to_owned(), "byKey".to_owned())]),
        read_catalog: false,
    })
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks for existence.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[test]
fn a_planned_crash_kills_the_worker_with_the_next_operation_never_sent() {
    require!();
    let r = run(
        "twoEffects",
        &[
            PlanStep::Release(ping(1)),
            PlanStep::Crash(CrashStep {
                actor: 0,
                transition: 2,
                next: Some(ping(3)),
                waiting: false,
                may_have_run: false,
                label: "before EXTERNAL ping".to_owned(),
            }),
        ],
    );
    assert_eq!(r.status, "followed", "{:?}", r.problems);
    let c = &r.crashes[0];
    assert!(c.killed_by_signal, "{c:?}");
    assert!(c.sessions_ended);
    assert_eq!(c.never_sent.as_deref(), Some("EFFECT ping"));
    assert!(!alive(c.pid as i32));
    // Only the first effect reached the channel double.
    assert_eq!(r.effects.len(), 1);
    assert_eq!(r.actors[0].outcome, "killed");
    assert!(r.server_settings[0].starts_with("client_connection_check_interval"));
    // The second announced effect is recorded as never sent.
    assert!(r.statements.iter().any(|s| s.role == "never-sent"));
}

#[test]
fn a_restart_runs_a_new_process_and_the_channel_double_deduplicates() {
    require!();
    let r = run(
        "twoEffects",
        &[
            PlanStep::Release(ping(1)),
            PlanStep::Crash(CrashStep {
                actor: 0,
                transition: 2,
                next: Some(ping(3)),
                waiting: false,
                may_have_run: false,
                label: String::new(),
            }),
            PlanStep::Restart(RestartStep {
                actor: 0,
                transition: 3,
                attempt: 2,
                mechanism: "test".to_owned(),
            }),
            PlanStep::Release(ping(4)),
            PlanStep::Release(ping(5)),
        ],
    );
    assert_eq!(r.status, "followed", "{:?}", r.problems);
    assert_eq!(r.restarts.len(), 1);
    assert_ne!(r.restarts[0].pid, r.restarts[0].previous_pid);
    assert_eq!(r.attempts.len(), 2);
    assert_eq!(r.attempts[0].outcome, "killed");
    assert_eq!(r.attempts[1].outcome, "returned");
    assert_eq!(r.attempts[1].attempt, 2);
    // byKey: the double receives three effects, two deduplicated.
    let dedup: Vec<bool> = r.effects.iter().map(|e| e.deduplicated).collect();
    assert_eq!(dedup, [false, true, true]);
}

#[test]
fn a_retry_that_fails_to_start_is_inconclusive() {
    require!();
    let r = run(
        "failRetry",
        &[
            PlanStep::Release(ping(1)),
            PlanStep::Crash(CrashStep {
                actor: 0,
                transition: 2,
                next: Some(ping(3)),
                waiting: false,
                may_have_run: false,
                label: String::new(),
            }),
            PlanStep::Restart(RestartStep {
                actor: 0,
                transition: 3,
                attempt: 2,
                mechanism: "test".to_owned(),
            }),
        ],
    );
    assert_eq!(r.status, "inconclusive");
    assert!(
        r.problems
            .iter()
            .any(|p| p.contains("the retry could not start")),
        "{:?}",
        r.problems
    );
}

#[test]
fn a_hung_worker_is_killed_at_the_deadline() {
    require!();
    let started = std::time::Instant::now();
    let r = run("hang", &[PlanStep::Release(ping(1))]);
    assert_eq!(r.status, "inconclusive");
    assert!(r.problems.iter().any(|p| p.contains("deadline")));
    assert_eq!(r.actors[0].outcome, "killed");
    assert!(!alive(r.actors[0].pid as i32));
    assert!(started.elapsed() < Duration::from_secs(30));
}

#[test]
fn a_cpu_burning_worker_is_stopped_by_its_cpu_limit() {
    require!();
    let started = std::time::Instant::now();
    let r = run("spin", &[PlanStep::Release(ping(1))]);
    // RLIMIT_CPU (2 s) ends it well before the 8 s deadline.
    assert!(
        started.elapsed() < Duration::from_secs(7),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(r.actors[0].outcome, "killed");
    assert!(r.limits.contains("2 CPU s"));
}

/// Processes whose command line carries `marker` (`/proc` on Linux, `ps`
/// elsewhere).
fn processes_marked(marker: &str) -> Vec<String> {
    if cfg!(target_os = "linux") {
        let mut found = Vec::new();
        for entry in std::fs::read_dir("/proc").expect("/proc").flatten() {
            if let Ok(cmdline) = std::fs::read(entry.path().join("cmdline"))
                && String::from_utf8_lossy(&cmdline).contains(marker)
            {
                found.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        return found;
    }
    let out = std::process::Command::new("ps")
        .args(["-A", "-ww", "-o", "pid=,args="])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains(marker))
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_detached_grandchild_does_not_survive_the_run() {
    require!();
    let r = run("detach", &[PlanStep::Release(ping(1))]);
    assert!(
        r.actors[0].value["child"].as_i64().is_some(),
        "{:?}",
        r.actors[0]
    );
    let survivors = processes_marked("commitrift-lifecycle-grandchild");
    assert!(survivors.is_empty(), "{survivors:?}");
    assert_eq!(r.status, "followed", "{:?}", r.problems);
    // On Linux the worker's PID namespace ends with it, so the kernel kills
    // the grandchild (and the pid it reports is namespace-local). On macOS
    // the run-token sweep is what kills it, and the pid is a host pid.
    if cfg!(target_os = "macos") {
        let child = r.actors[0].value["child"].as_i64().unwrap() as i32;
        assert!(
            r.survivors_killed.contains(&child),
            "{:?}",
            r.survivors_killed
        );
        assert!(!alive(child));
    }
}

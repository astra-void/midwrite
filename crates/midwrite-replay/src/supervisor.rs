//! Runs one harness process and speaks the checkpoint protocol with it.
//!
//! Protocol (JSON lines over a Unix socket whose path is passed in
//! `MIDWRITE_REPLAY_SOCKET`; see `docs/architecture/replay-protocol.md`):
//!
//! ```text
//! harness -> supervisor  {"type":"hello","protocol":1,"scenario":"..."}
//! harness -> supervisor  {"type":"checkpoint","seq":1,"name":"after-db-commit","label":"orders"}
//! supervisor -> harness  {"type":"continue"} | {"type":"throw","message":"..."}
//!                        (process kill: no answer; SIGKILL to the process group)
//! harness -> supervisor  {"type":"done","outcome":"returned"|"threw","error":"..."}
//! ```
//!
//! The harness sends a checkpoint only after the operation it names has
//! completed, then blocks until the answer arrives. The supervisor therefore
//! kills a process that is provably waiting at the checkpoint: there is no
//! race between the notification and the termination.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use midwrite_core::verification::{CheckpointEvent, ConditionResult, ExitInfo, FailureMode};

use crate::manifest::InjectSpec;
use crate::source::{WorkerLimits, apply_limits, leader_exited, processes_with_token, run_token};

pub const PROTOCOL_VERSION: u64 = 1;
const SIGKILL: i32 = 9;

/// How to start the harness.
pub struct Process<'a> {
    pub harness_dir: &'a Path,
    /// The full command line, already wrapped by the network isolation.
    pub command: &'a [String],
    /// The complete environment of the harness (it inherits nothing else).
    pub env: &'a [(String, String)],
    pub run_dir: &'a Path,
    pub timeout: Duration,
}

/// What to do at the selected checkpoint.
pub struct Injection<'a> {
    pub spec: &'a InjectSpec,
    /// Checks run while the harness waits at the checkpoint. A check that
    /// fails, or an error, cancels the injection.
    pub verify: &'a mut dyn FnMut() -> Result<Vec<ConditionResult>, String>,
}

#[derive(Debug, Default)]
pub struct Execution {
    pub checkpoints: Vec<CheckpointEvent>,
    pub injected: bool,
    pub pre_checks: Vec<ConditionResult>,
    /// Why the run cannot be classified (setup, protocol, verification).
    pub problem: Option<String>,
    pub exit: Option<ExitInfo>,
    pub outcome: String,
    pub error: Option<String>,
    /// From the harness' `done` message: whether the error the scenario
    /// ended with is (or wraps) the injected fault.
    pub fault_surfaced: Option<bool>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

fn exit_info(status: ExitStatus) -> ExitInfo {
    match (status.code(), status.signal()) {
        (Some(code), _) => ExitInfo {
            kind: "exited".to_owned(),
            code: Some(code),
            signal: None,
        },
        (None, signal) => ExitInfo {
            kind: "signaled".to_owned(),
            code: None,
            signal,
        },
    }
}

fn kill_group(child: &mut Child) {
    let pid = child.id() as i32;
    // SAFETY: kill(2) with a negative pid signals the process group created
    // for the child (`process_group(0)`); it has no memory-safety
    // preconditions.
    unsafe {
        libc::kill(-pid, SIGKILL);
    }
    let _ = child.kill();
}

/// Wait for the child to exit until `deadline`, then kill its whole process
/// group and reap it. The group is killed on every path, also after a
/// normal exit, so processes the harness started in its group do not
/// outlive it. The leader is reaped only after the kill (`WNOWAIT` before),
/// so the group id cannot have been reused by then.
fn reap(child: &mut Child, deadline: Instant) -> (ExitStatus, bool) {
    let mut timed_out = false;
    while !leader_exited(child.id()) {
        if Instant::now() >= deadline || crate::cancel::cancelled() {
            timed_out = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    kill_group(child);
    let status = child
        .wait()
        .unwrap_or_else(|_| ExitStatus::from_raw(9 << 8));
    (status, timed_out)
}

/// Kill every process of this execution that left the harness' process
/// group (`setsid`, `detached: true`): they carry `token` in their
/// environment. Returns how many were found, and the ones that survived
/// SIGKILL.
fn sweep(token: &str) -> (usize, Vec<i32>) {
    // Processes of the group were just sent SIGKILL; give them (and a PID
    // namespace being torn down) a moment to disappear, so that only
    // processes that really left the group are counted.
    let grace = Instant::now() + Duration::from_millis(500);
    let mut found = processes_with_token(token);
    while !found.is_empty() && Instant::now() < grace {
        std::thread::sleep(Duration::from_millis(20));
        found = processes_with_token(token);
    }
    for pid in &found {
        // SAFETY: kill(2); the pid was just read with this execution's token.
        unsafe {
            libc::kill(*pid, SIGKILL);
        }
    }
    if found.is_empty() {
        return (0, Vec::new());
    }
    std::thread::sleep(Duration::from_millis(50));
    (found.len(), processes_with_token(token))
}

fn send(stream: &mut UnixStream, message: &Value) -> std::io::Result<()> {
    let mut line = message.to_string();
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()
}

/// Run `scenario` with `input` once. `label` names the log files.
pub fn execute(
    process: &Process<'_>,
    label: &str,
    scenario: &str,
    input: &Value,
    mut injection: Option<Injection<'_>>,
) -> Execution {
    let mut execution = Execution::default();
    let socket_path: PathBuf = process.run_dir.join(format!("{label}.sock"));
    let _ = std::fs::remove_file(&socket_path);
    let listener = match UnixListener::bind(&socket_path) {
        Ok(listener) => listener,
        Err(error) => {
            execution.problem = Some(format!("cannot create the protocol socket: {error}"));
            execution.outcome = "notStarted".to_owned();
            return execution;
        }
    };
    let _ = listener.set_nonblocking(true);
    let stdout_path = process.run_dir.join(format!("{label}.stdout"));
    let stderr_path = process.run_dir.join(format!("{label}.stderr"));
    let (Ok(stdout), Ok(stderr)) = (
        std::fs::File::create(&stdout_path),
        std::fs::File::create(&stderr_path),
    ) else {
        execution.problem = Some("cannot create log files".to_owned());
        execution.outcome = "notStarted".to_owned();
        return execution;
    };

    let token = run_token();
    let mut command = Command::new(&process.command[0]);
    command
        .args(&process.command[1..])
        .current_dir(process.harness_dir)
        .env_clear()
        .envs(process.env.iter().map(|(k, v)| (k, v)))
        .env("MIDWRITE_REPLAY_SOCKET", &socket_path)
        .env("MIDWRITE_REPLAY_SCENARIO", scenario)
        .env("MIDWRITE_REPLAY_INPUT", input.to_string())
        .env("MIDWRITE_RUN_TOKEN", &token)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .process_group(0);
    let limits = WorkerLimits::default();
    // SAFETY: the closure runs in the forked child before exec and only
    // calls getrlimit(2)/setrlimit(2), which are async-signal-safe.
    unsafe {
        command.pre_exec(move || apply_limits(limits));
    }
    let start = Instant::now();
    let deadline = start + process.timeout;
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            execution.problem = Some(format!(
                "the harness command `{}` could not be started: {error}",
                process.command.last().map_or("", String::as_str)
            ));
            execution.outcome = "notStarted".to_owned();
            execution.exit = Some(ExitInfo {
                kind: "notStarted".to_owned(),
                code: None,
                signal: None,
            });
            return execution;
        }
    };

    // Wait for the harness to connect.
    let stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break Some(stream),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if leader_exited(child.id()) {
                    break None;
                }
                if Instant::now() >= deadline || crate::cancel::cancelled() {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) => {
                execution.problem = Some(format!("protocol socket error: {error}"));
                break None;
            }
        }
    };

    let mut killed_at_checkpoint = false;
    let connected = stream.is_some();
    if let Some(mut stream) = stream {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_millis(20)));
        let mut reader = BufReader::new(stream.try_clone().expect("socket clone"));
        let mut line = String::new();
        let mut seen: u32 = 0;
        loop {
            if Instant::now() >= deadline || crate::cancel::cancelled() {
                execution.timed_out = true;
                break;
            }
            match reader.read_line(&mut line) {
                Ok(0) => break, // the harness closed the connection
                Ok(_) if !line.ends_with('\n') => continue,
                Ok(_) => {}
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    if leader_exited(child.id()) {
                        break;
                    }
                    continue;
                }
                Err(error) => {
                    execution.problem = Some(format!("protocol read error: {error}"));
                    break;
                }
            }
            let message: Value = match serde_json::from_str(line.trim()) {
                Ok(message) => message,
                Err(_) => {
                    execution.problem =
                        Some("the harness sent a malformed protocol message".to_owned());
                    kill_group(&mut child);
                    break;
                }
            };
            line.clear();
            match message["type"].as_str() {
                Some("hello") => {
                    if message["protocol"].as_u64() != Some(PROTOCOL_VERSION) {
                        execution.problem = Some(format!(
                            "the harness speaks protocol {}, expected {PROTOCOL_VERSION}",
                            message["protocol"]
                        ));
                        kill_group(&mut child);
                        break;
                    }
                }
                Some("checkpoint") => {
                    let name = message["name"].as_str().unwrap_or("").to_owned();
                    let label = message["label"].as_str().map(str::to_owned);
                    let seq = u32::try_from(message["seq"].as_u64().unwrap_or(0)).unwrap_or(0);
                    let selected = injection.as_ref().is_some_and(|inj| {
                        !execution.injected
                            && inj.spec.checkpoint == name
                            && inj
                                .spec
                                .label
                                .as_ref()
                                .is_none_or(|l| Some(l) == label.as_ref())
                    });
                    let fire = selected && {
                        seen += 1;
                        injection
                            .as_ref()
                            .is_some_and(|inj| seen == inj.spec.occurrence)
                    };
                    let mut event = CheckpointEvent {
                        seq,
                        name: name.clone(),
                        label,
                        action: "continue".to_owned(),
                    };
                    if fire {
                        let injection = injection.as_mut().expect("selected");
                        match (injection.verify)() {
                            Ok(checks) => {
                                let failed = checks.iter().any(|c| !c.holds);
                                execution.pre_checks = checks;
                                if failed {
                                    execution.problem = Some(format!(
                                        "a pre-injection check failed at `{name}`: the harness is not in the state the case expects, so nothing was injected"
                                    ));
                                    event.action = "abort".to_owned();
                                    execution.checkpoints.push(event);
                                    kill_group(&mut child);
                                    break;
                                }
                            }
                            Err(error) => {
                                execution.problem =
                                    Some(format!("pre-injection check error: {error}"));
                                event.action = "abort".to_owned();
                                execution.checkpoints.push(event);
                                kill_group(&mut child);
                                break;
                            }
                        }
                        match injection.spec.mode {
                            FailureMode::ProcessKill => {
                                event.action = "kill".to_owned();
                                execution.checkpoints.push(event);
                                execution.injected = true;
                                kill_group(&mut child);
                                killed_at_checkpoint = true;
                                break;
                            }
                            FailureMode::Exception => {
                                event.action = "throw".to_owned();
                                execution.injected = true;
                                let _ = send(
                                    &mut stream,
                                    &json!({
                                        "type": "throw",
                                        "message": format!("Midwrite injected fault at checkpoint `{name}`"),
                                    }),
                                );
                            }
                            FailureMode::None => {
                                let _ = send(&mut stream, &json!({"type": "continue"}));
                            }
                        }
                    } else {
                        let _ = send(&mut stream, &json!({"type": "continue"}));
                    }
                    execution.checkpoints.push(event);
                }
                Some("done") => {
                    execution.outcome = message["outcome"].as_str().unwrap_or("unknown").to_owned();
                    execution.error = message["error"].as_str().map(str::to_owned);
                    execution.fault_surfaced = message["injected"].as_bool();
                }
                _ => {
                    execution.problem =
                        Some("the harness sent an unknown protocol message".to_owned());
                    kill_group(&mut child);
                    break;
                }
            }
        }
    }

    let (status, reaped_by_deadline) = reap(&mut child, deadline);
    execution.timed_out |= reaped_by_deadline;
    let (escaped, survived) = sweep(&token);
    // A timed-out run is already inconclusive, and its reason says so.
    if escaped > 0 && execution.problem.is_none() && !execution.timed_out {
        execution.problem = Some(if survived.is_empty() {
            format!(
                "{escaped} process(es) started by the harness left its process group and were still running after it; they were killed, but may have changed the database after the scenario ended"
            )
        } else {
            format!(
                "{} process(es) started by the harness survived SIGKILL: {survived:?}",
                survived.len()
            )
        });
    }
    let exit = exit_info(status);
    if execution.timed_out {
        execution.outcome = "timedOut".to_owned();
        execution.exit = Some(ExitInfo {
            kind: "timedOut".to_owned(),
            ..exit
        });
    } else {
        if killed_at_checkpoint {
            execution.outcome = "killed".to_owned();
        } else if execution.outcome.is_empty() {
            // Exited without reporting completion.
            execution.outcome = "crashed".to_owned();
        }
        execution.exit = Some(exit);
    }
    execution.stdout = std::fs::read_to_string(&stdout_path).unwrap_or_default();
    execution.stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
    if !connected && !execution.timed_out && execution.problem.is_none() {
        let tail: String = execution
            .stderr
            .lines()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" | ");
        execution.problem = Some(format!(
            "the harness could not be started or exited without connecting to the replay supervisor (exit {:?}); check `command`{}",
            execution.exit.as_ref().and_then(|e| e.code.or(e.signal)),
            if tail.is_empty() {
                String::new()
            } else {
                format!(": {tail}")
            }
        ));
    }
    let _ = std::fs::remove_file(&socket_path);
    execution
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cr-sup-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn run_script(dir: &Path, script: &str, timeout: Duration) -> Execution {
        let command = vec!["/bin/sh".to_owned(), "-c".to_owned(), script.to_owned()];
        let env = vec![("PATH".to_owned(), "/usr/bin:/bin".to_owned())];
        let process = Process {
            harness_dir: dir,
            command: &command,
            env: &env,
            run_dir: dir,
            timeout,
        };
        execute(&process, "t", "scenario", &json!({}), None)
    }

    fn alive(pid: i32) -> bool {
        // SAFETY: kill(2) with signal 0 only checks for existence.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn read_pid(path: &Path) -> i32 {
        for _ in 0..200 {
            if let Some(pid) = std::fs::read_to_string(path)
                .ok()
                .and_then(|s| s.trim().parse().ok())
            {
                return pid;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("no pid in {}", path.display());
    }

    #[test]
    fn processes_left_in_the_group_die_with_a_normally_exiting_harness() {
        let dir = scratch("group");
        let execution = run_script(
            &dir,
            "sleep 30 & echo $! > child.pid; exit 0",
            Duration::from_secs(10),
        );
        let pid = read_pid(&dir.join("child.pid"));
        assert!(!execution.timed_out);
        std::thread::sleep(Duration::from_millis(50));
        assert!(!alive(pid), "a process of the harness' group outlived it");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // Linux only: the sweep reads `/proc/<pid>/environ`. On macOS 27 the
    // kernel no longer returns another process' environment (`ps -E`,
    // KERN_PROCARGS2), so a process that left the group is not found there
    // (docs/reference/security.md, "Not protected").
    #[cfg(target_os = "linux")]
    #[test]
    fn processes_that_leave_the_group_are_swept_and_reported() {
        let dir = scratch("setsid");
        // Fork a child that starts its own session (leaves the group) and
        // keeps running after the harness exits.
        let execution = run_script(
            &dir,
            "perl -e 'use POSIX; my $p = fork; if ($p == 0) { POSIX::setsid(); sleep 30; exit 0 } open(my $f, \">\", \"escaped.pid\"); print $f $p; close $f' ; exit 0",
            Duration::from_secs(10),
        );
        let pid = read_pid(&dir.join("escaped.pid"));
        std::thread::sleep(Duration::from_millis(50));
        assert!(!alive(pid), "the escaped process survived the sweep");
        let problem = execution.problem.unwrap_or_default();
        assert!(
            problem.contains("left its process group") || problem.contains("without connecting"),
            "{problem}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn harness_processes_run_under_resource_limits() {
        let dir = scratch("limits");
        run_script(
            &dir,
            "{ ulimit -t; ulimit -n; ulimit -c; } > limits.txt",
            Duration::from_secs(10),
        );
        let text = std::fs::read_to_string(dir.join("limits.txt")).unwrap();
        let values: Vec<&str> = text.lines().map(str::trim).collect();
        assert_eq!(values[0], "120", "CPU seconds: {text}");
        let files: u64 = values[1].parse().unwrap();
        assert!(files <= 1024, "open files: {text}");
        assert_eq!(values[2], "0", "core dumps: {text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_harness_that_never_finishes_is_killed_at_the_deadline() {
        let dir = scratch("deadline");
        let start = Instant::now();
        let execution = run_script(
            &dir,
            "sleep 30 & echo $! > child.pid; sleep 30",
            Duration::from_millis(500),
        );
        assert!(execution.timed_out);
        assert_eq!(execution.outcome, "timedOut");
        assert!(start.elapsed() < Duration::from_secs(5));
        let pid = read_pid(&dir.join("child.pid"));
        std::thread::sleep(Duration::from_millis(50));
        assert!(!alive(pid));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

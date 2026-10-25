//! Runs one harness process and speaks the checkpoint protocol with it.
//!
//! Protocol (JSON lines over a Unix socket whose path is passed in
//! `COMMITRIFT_REPLAY_SOCKET`; see `docs/phase3/REPLAY-PROTOCOL.md`):
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

use commitrift_core::verification::{CheckpointEvent, ConditionResult, ExitInfo, FailureMode};

use crate::manifest::InjectSpec;

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

/// Wait for the child to exit until `deadline`; kill it after.
fn reap(child: &mut Child, deadline: Instant) -> (ExitStatus, bool) {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return (status, false),
            Ok(None) if Instant::now() >= deadline => {
                kill_group(child);
                let status = child.wait().unwrap_or_else(|_| ExitStatus::from_raw(9));
                return (status, true);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => return (ExitStatus::from_raw(9 << 8), false),
        }
    }
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

    let mut command = Command::new(&process.command[0]);
    command
        .args(&process.command[1..])
        .current_dir(process.harness_dir)
        .env_clear()
        .envs(process.env.iter().map(|(k, v)| (k, v)))
        .env("COMMITRIFT_REPLAY_SOCKET", &socket_path)
        .env("COMMITRIFT_REPLAY_SCENARIO", scenario)
        .env("COMMITRIFT_REPLAY_INPUT", input.to_string())
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .process_group(0);
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
                if let Ok(Some(_)) = child.try_wait() {
                    break None;
                }
                if Instant::now() >= deadline {
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
            if Instant::now() >= deadline {
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
                    if let Ok(Some(_)) = child.try_wait() {
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
                                        "message": format!("CommitRift injected fault at checkpoint `{name}`"),
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
    if execution.timed_out {
        kill_group(&mut child);
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

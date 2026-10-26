//! Coordinated execution of original application code (Phases 7–8).
//!
//! `commitrift model-check --verify-source --allow-execution` runs the
//! *original* application function of every actor of a scenario in its own
//! worker process (an authorized harness, see `runtime/node/source.js`)
//! against a disposable PostgreSQL database. Every SQL statement a worker
//! sends is held until the coordinator releases it; statements are released
//! in the order of a *plan* (derived by the caller from a model
//! counterexample). The coordinator records what the application actually
//! sent and what PostgreSQL did: results, errors, lock waits.
//!
//! The coordinator never skips, reorders within an actor, rewrites or fakes
//! a statement. When the application sends something the plan does not
//! expect at that point, the run *diverges*: the coordinator records it and
//! stops following the plan. When an expected statement never comes, the
//! run is inconclusive.
//!
//! Phase 8 adds crash and recovery: a plan may kill an actor's worker
//! (SIGKILL to its process group) at an observed checkpoint — its next
//! operation announced and held, so never sent, or its statement waiting
//! for a lock on the server — then waits until PostgreSQL has ended the
//! worker's sessions, and may start a NEW worker process for the actor (a
//! declared retry or redelivery). External effects go to a channel test
//! double hosted by the coordinator, which outlives killed workers.
//!
//! Safety (same model as replay, `docs/phase4/SECURITY-REVIEW.md`): explicit
//! authorization, a private run directory, a private cluster reachable only
//! through a Unix socket inside it, OS isolation of every worker (no
//! network), an environment built from scratch, time limits, process-group
//! kill and directory removal on every exit path.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::isolation::{self, IsolationRequest};
use crate::postgres::{self, Binaries, Cluster};
use crate::safety::{self, ReplayRoot};

pub const SOURCE_MANIFEST_KIND: &str = "commitrift.source-harness";
const DATABASE: &str = "source_run";
const SIGKILL: i32 = 9;

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntrySpec {
    /// The source selector of the function the model checks for this entry.
    pub selector: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceManifest {
    pub schema_version: u32,
    pub kind: String,
    pub harness: String,
    #[serde(default)]
    pub description: String,
    /// The command that starts one worker (compiled harness).
    pub command: Vec<String>,
    #[serde(default)]
    pub runtime: Value,
    /// The application's schema migrations, applied in order to the
    /// disposable database (paths inside the harness directory).
    pub schema: Vec<String>,
    /// Only `deny` is accepted: source verification never runs workers
    /// with network access.
    #[serde(default = "deny")]
    pub network: String,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    pub entries: BTreeMap<String, EntrySpec>,
    /// Phase 8: per-worker resource limits (defaults when absent).
    #[serde(default)]
    pub limits: WorkerLimits,
}

/// Resource limits applied to every worker process with `setrlimit` before
/// it starts (inherited by its children). Memory is NOT limited: V8
/// reserves far more virtual memory than it uses, so `RLIMIT_AS` would
/// break Node.js, and no cgroup is created.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerLimits {
    /// CPU seconds (`RLIMIT_CPU`: SIGXCPU, then SIGKILL one second later).
    #[serde(default = "default_cpu")]
    pub cpu_seconds: u64,
    /// Largest file a worker may write, including its stdout/stderr logs
    /// (`RLIMIT_FSIZE`).
    #[serde(default = "default_file_mb")]
    pub file_size_mb: u64,
    /// Open file descriptors (`RLIMIT_NOFILE`).
    #[serde(default = "default_files")]
    pub open_files: u64,
}

impl Default for WorkerLimits {
    fn default() -> Self {
        Self {
            cpu_seconds: default_cpu(),
            file_size_mb: default_file_mb(),
            open_files: default_files(),
        }
    }
}

fn default_cpu() -> u64 {
    120
}
fn default_file_mb() -> u64 {
    64
}
fn default_files() -> u64 {
    1024
}

impl WorkerLimits {
    pub fn describe(&self) -> String {
        format!(
            "per worker: {} CPU s (RLIMIT_CPU), files up to {} MiB (RLIMIT_FSIZE), {} open files (RLIMIT_NOFILE), no core dumps; memory and process count are not limited",
            self.cpu_seconds, self.file_size_mb, self.open_files
        )
    }
}

fn deny() -> String {
    "deny".to_owned()
}
fn default_timeout() -> u64 {
    20_000
}

#[derive(Clone, Debug)]
pub struct LoadedSource {
    pub dir: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: SourceManifest,
}

/// Load and check a source-harness manifest. Nothing is executed.
pub fn load_manifest(path: &Path) -> Result<LoadedSource, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;
    let manifest: SourceManifest =
        serde_json::from_str(&text).map_err(|e| format!("`{}`: {e}", path.display()))?;
    if manifest.schema_version != 1 || manifest.kind != SOURCE_MANIFEST_KIND {
        return Err(format!(
            "`{}` is not a source-harness manifest (kind `{SOURCE_MANIFEST_KIND}`, schemaVersion 1)",
            path.display()
        ));
    }
    if manifest.network != "deny" {
        return Err(
            "source verification only runs harnesses with `\"network\": \"deny\"`".to_owned(),
        );
    }
    if manifest.command.is_empty() {
        return Err("the manifest's command is empty".to_owned());
    }
    if manifest.timeout_ms == 0 || manifest.timeout_ms > 300_000 {
        return Err("timeoutMs must be between 1 and 300000".to_owned());
    }
    let l = manifest.limits;
    if !(1..=3600).contains(&l.cpu_seconds)
        || !(1..=4096).contains(&l.file_size_mb)
        || !(64..=65_536).contains(&l.open_files)
    {
        return Err(
            "limits: cpuSeconds 1..=3600, fileSizeMb 1..=4096, openFiles 64..=65536".to_owned(),
        );
    }
    let dir = path
        .parent()
        .unwrap_or(Path::new("."))
        .canonicalize()
        .map_err(|e| format!("cannot resolve the harness directory: {e}"))?;
    let inside = |rel: &str| -> Result<PathBuf, String> {
        let p = dir.join(rel);
        let c = p.canonicalize().map_err(|_| {
            format!("`{rel}` does not exist in the harness directory (compile the harness first?)")
        })?;
        if !c.starts_with(&dir) {
            return Err(format!("`{rel}` is outside the harness directory"));
        }
        Ok(c)
    };
    for s in &manifest.schema {
        let file = inside(s)?;
        let sql = std::fs::read_to_string(&file).map_err(|e| format!("`{s}`: {e}"))?;
        if sql.contains("COMMITRIFT-TODO") {
            return Err(format!("`{s}` still contains a COMMITRIFT-TODO marker"));
        }
    }
    // The compiled entry point: the last argument that names a file.
    if let Some(script) = manifest
        .command
        .iter()
        .skip(1)
        .rev()
        .find(|a| a.ends_with(".js") || a.ends_with(".mjs") || a.ends_with(".ts"))
    {
        let file = inside(script)?;
        let code = std::fs::read_to_string(&file).unwrap_or_default();
        if code.contains("COMMITRIFT-TODO") {
            return Err(format!(
                "`{script}` still contains a COMMITRIFT-TODO marker"
            ));
        }
    }
    Ok(LoadedSource {
        dir,
        manifest_path: path.to_path_buf(),
        manifest,
    })
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SqlKind {
    Select,
    Insert,
    Update,
    Delete,
    Begin,
    Commit,
    Rollback,
    Other,
    /// Phase 8: an external effect through the coordinator's channel test
    /// double (`sourceEffect` in `source.js`); `table` names the channel.
    Effect,
}

fn strip_comments(text: &str) -> &str {
    let mut rest = text.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("--") {
            rest = after.split_once('\n').map_or("", |(_, r)| r).trim_start();
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, r)| r).trim_start();
        } else {
            return rest;
        }
    }
}

fn unquote(identifier: &str) -> String {
    let last = identifier.rsplit('.').next().unwrap_or(identifier).trim();
    if let Some(inner) = last.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        inner.replace("\"\"", "\"")
    } else {
        last.to_ascii_lowercase()
    }
}

/// The identifier (possibly quoted, possibly schema-qualified) at the start
/// of `text`.
fn identifier_at(text: &str) -> Option<String> {
    let text = text.trim_start();
    let mut end = 0;
    let bytes = text.as_bytes();
    loop {
        if bytes.get(end) == Some(&b'"') {
            end += 1;
            loop {
                match bytes.get(end) {
                    Some(b'"') if bytes.get(end + 1) == Some(&b'"') => end += 2,
                    Some(b'"') => {
                        end += 1;
                        break;
                    }
                    Some(_) => end += 1,
                    None => return None,
                }
            }
        } else {
            let start = end;
            while bytes
                .get(end)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$')
            {
                end += 1;
            }
            if end == start {
                return None;
            }
        }
        if bytes.get(end) == Some(&b'.') {
            end += 1;
            continue;
        }
        break;
    }
    Some(unquote(&text[..end]))
}

/// Classify a statement by its leading keyword, and name its table: the
/// target of a write, the first `FROM` table of a `SELECT`. Statements the
/// classifier does not understand are `Other` (and never match a plan
/// step).
pub fn classify(sql: &str) -> (SqlKind, Option<String>) {
    let text = strip_comments(sql);
    let lower = text.to_ascii_lowercase();
    let after = |kw: &str| -> Option<String> {
        lower
            .strip_prefix(kw)
            .filter(|r| r.starts_with(char::is_whitespace))
            .and_then(|_| identifier_at(&text[kw.len()..]))
    };
    if lower.starts_with("insert") {
        return (SqlKind::Insert, after("insert into"));
    }
    if lower.starts_with("update") {
        return (SqlKind::Update, after("update"));
    }
    if lower.starts_with("delete") {
        return (SqlKind::Delete, after("delete from"));
    }
    if lower.starts_with("select") {
        // The first top-level FROM (Drizzle does not emit subqueries in the
        // supported subset; a subquery makes the table name wrong, which
        // only fails the match).
        let table = lower
            .find(" from ")
            .and_then(|i| identifier_at(&text[i + 6..]));
        return (SqlKind::Select, table);
    }
    if lower == "begin" || lower.starts_with("begin ") || lower.starts_with("start transaction") {
        return (SqlKind::Begin, None);
    }
    if lower == "commit" || lower.starts_with("commit ") || lower == "end" {
        return (SqlKind::Commit, None);
    }
    if (lower == "rollback" || lower.starts_with("rollback ")) && !lower.contains(" to ") {
        return (SqlKind::Rollback, None);
    }
    (SqlKind::Other, None)
}

/// One application stack frame (relative to the harness directory).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    #[serde(default)]
    pub function: String,
    pub file: String,
    pub line: u32,
    pub column: u32,
}

/// A source region (`file`, inclusive line range).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    pub file: String,
    pub start: u32,
    pub end: u32,
}

impl Region {
    pub fn contains(&self, f: &Frame) -> bool {
        f.file == self.file && self.start <= f.line && f.line <= self.end
    }
}

/// What the plan expects one actor to send next.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Expected {
    pub actor: usize,
    pub kind: SqlKind,
    /// The table (or, for an [`SqlKind::Effect`], the channel).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    /// The source region of the model operation; the statement's innermost
    /// application frame must lie in it (for an effect or transaction
    /// control: any application frame, since a seam or a helper issues it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
    /// The model predicts the statement waits for a lock.
    pub expect_block: bool,
    /// Index of the model transition (for the report).
    pub transition: usize,
    pub label: String,
    /// Phase 8: the idempotency key an effect must carry (`null`: none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<Value>,
}

/// Phase 8: where a planned crash happens. The model crashes a process
/// *between* transitions; at runtime that boundary is a checkpoint the
/// coordinator can observe exactly, never a guess from source-line order.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashStep {
    pub actor: usize,
    pub transition: usize,
    /// The actor's next operation in the model: the worker must have
    /// announced it (held by the coordinator, so never sent) when it is
    /// killed. `None` with `waiting` set: the actor's released statement is
    /// waiting for a lock on the server (in flight).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<Expected>,
    #[serde(default)]
    pub waiting: bool,
    /// With `waiting`: the lock the statement waited for has been released
    /// in the model, so PostgreSQL may already have executed the statement
    /// (inside a transaction that the crash rolls back).
    #[serde(default)]
    pub may_have_run: bool,
    pub label: String,
}

/// Phase 8: a new execution of an actor, started by a declared recovery
/// mechanism (client retry, queue redelivery, worker restart).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestartStep {
    pub actor: usize,
    pub transition: usize,
    /// 2 for the first retry.
    pub attempt: u32,
    /// The declared mechanism, for the report.
    pub mechanism: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "step")]
pub enum PlanStep {
    /// Release the next statement of `actor`, which must match.
    Release(Expected),
    /// A statement of `actor` released earlier and still waiting must
    /// complete now (the model resumes it).
    Complete { actor: usize, transition: usize },
    /// Kill the actor's worker process (SIGKILL to its process group).
    Crash(CrashStep),
    /// Start a fresh worker process for the actor.
    Restart(RestartStep),
}

impl PlanStep {
    pub fn is_crash_or_restart(&self) -> bool {
        matches!(self, Self::Crash(_) | Self::Restart(_))
    }
}

/// One worker.
#[derive(Clone, Debug)]
pub struct ActorLaunch {
    pub name: String,
    pub entry: String,
    pub input: Value,
    /// The source region the model represents for this actor (the selected
    /// entry function). Statements none of whose application frames lie in
    /// it are outside the model.
    pub scope: Option<Region>,
}

/// What became of one statement (or, `kind: effect`, one external effect).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observed {
    pub actor: usize,
    /// Which execution of the actor sent it (1 for the first).
    #[serde(default = "first_attempt")]
    pub attempt: u32,
    pub seq: u64,
    pub client: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_pid: Option<i64>,
    pub sql: String,
    #[serde(default)]
    pub params: Value,
    pub kind: Option<SqlKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    #[serde(default)]
    pub frames: Vec<Frame>,
    /// The outer frames were restored from an earlier stack of the same
    /// execution (`source.js`: V8 loses callers across `return promise`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reconstructed: bool,
    /// `planned` (matched plan step `plan`), `outside-model`,
    /// `unexpected`, `beyond-plan`, `after-divergence`, or (Phase 8)
    /// `never-sent` (announced, then its process was killed).
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<usize>,
    /// Release order among all statements.
    pub released: Option<usize>,
    /// PostgreSQL reported a lock wait while it ran.
    pub blocked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_count: Option<i64>,
    /// Result rows as objects keyed `table.column` (columns of a table) or
    /// `column` (computed), with PostgreSQL's names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<Value>,
    #[serde(skip)]
    raw_fields: Vec<(String, i64)>,
    #[serde(skip)]
    raw_rows: Vec<Value>,
    #[serde(skip)]
    slot: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sqlstate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub completed: bool,
}

fn first_attempt() -> u32 {
    1
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorResult {
    pub name: String,
    pub entry: String,
    /// `returned`, `threw`, `killed` (by the coordinator at a planned
    /// crash, or at the deadline), `not-started`.
    pub outcome: String,
    #[serde(default)]
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr: String,
    /// Phase 8: which execution of the actor (1 for the first).
    #[serde(default = "first_attempt")]
    pub attempt: u32,
    /// Operating-system process id of the worker.
    #[serde(default)]
    pub pid: u32,
}

/// Phase 8: one effect the channel test double received.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectRecord {
    /// Receipt id (1, 2, … in the order the double received them).
    pub id: usize,
    pub actor: usize,
    pub attempt: u32,
    pub channel: String,
    #[serde(default)]
    pub key: Value,
    #[serde(default)]
    pub payload: Value,
    /// Suppressed by the channel's DECLARED key-based deduplication.
    pub deduplicated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<Frame>,
}

/// Phase 8: evidence that a planned crash happened as planned.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashEvidence {
    pub actor: usize,
    pub attempt: u32,
    /// Plan step index.
    pub plan: usize,
    pub transition: usize,
    pub pid: u32,
    /// What the worker was doing when it was killed.
    pub checkpoint: String,
    /// The announced operation that was never sent (killed while held).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub never_sent: Option<String>,
    /// The operating system reported the worker terminated by SIGKILL.
    pub killed_by_signal: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<String>,
    /// The worker's PostgreSQL sessions (backend pids) when it was killed.
    pub backends: Vec<i64>,
    /// PostgreSQL ended every one of those sessions (pg_stat_activity)
    /// before the next plan step.
    pub sessions_ended: bool,
    pub sessions_ended_ms: u64,
    /// Rows of the modeled tables right after the sessions ended (what is
    /// durable: committed work only).
    #[serde(default)]
    pub tables: BTreeMap<String, Vec<Value>>,
}

/// Phase 8: a new execution of an actor.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestartEvidence {
    pub actor: usize,
    pub attempt: u32,
    pub plan: usize,
    pub transition: usize,
    pub mechanism: String,
    pub pid: u32,
    pub previous_pid: u32,
    /// How the previous execution ended (`killed`, `returned`, `threw`).
    pub previous_outcome: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRun {
    /// `followed` (every plan step happened as planned), `diverged`,
    /// `inconclusive` or `blocked` (could not run).
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
    pub statements: Vec<Observed>,
    /// The last execution of each actor.
    pub actors: Vec<ActorResult>,
    /// Phase 8: every execution of every actor, in start order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<ActorResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crashes: Vec<CrashEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restarts: Vec<RestartEvidence>,
    /// Server settings the run changed from PostgreSQL's defaults.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub server_settings: Vec<String>,
    /// Phase 8: processes of the run found after the workers ended (they
    /// had left their process group) and killed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub survivors_killed: Vec<i32>,
    /// Phase 8: the resource limits the workers ran under.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub limits: String,
    /// Phase 8: statements a driver issued on its own connections (catalog
    /// reads after connecting), not held by the coordinator.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub driver_internal: Vec<String>,
    /// Phase 8: the run database's catalog after the migrations (JSON from
    /// [`crate::catalog::read`]), when requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<Value>,
    /// Final rows of each requested table (JSON objects).
    pub tables: BTreeMap<String, Vec<Value>>,
    pub postgres_version: String,
    pub isolation: String,
    pub environment_variables: Vec<String>,
    pub elapsed_ms: u64,
}

pub struct RunRequest<'a> {
    pub harness: &'a LoadedSource,
    /// The explicit authorization (`--allow-execution`).
    pub allow_execution: bool,
    pub keep_artifacts: bool,
    pub actors: &'a [ActorLaunch],
    pub plan: &'a [PlanStep],
    /// Seed SQL run after the schema (initial rows, sequence positions).
    pub seed_sql: &'a str,
    /// Final-state queries: table → SQL returning one JSON array.
    pub final_queries: &'a [(String, String)],
    /// How long a released statement may run before it is checked for a
    /// lock wait.
    pub settle: Duration,
    /// Phase 8: channel → declared deduplication (`none` or `byKey`) the
    /// coordinator's test double applies.
    pub channels: &'a BTreeMap<String, String>,
    /// Phase 8: read the catalog of the run database after the
    /// application's migrations (for the schema comparison).
    pub read_catalog: bool,
}

// ---------------------------------------------------------------------------
// Workers
// ---------------------------------------------------------------------------

/// One worker process: one execution (attempt) of one actor.
struct Worker {
    actor: usize,
    attempt: u32,
    child: Child,
    pid: u32,
    stream: Option<UnixStream>,
    hello: bool,
    done: Option<Value>,
    /// Statements announced and not yet released (in order).
    held: Vec<usize>,
    /// Released and not completed.
    running: Option<usize>,
    stderr_path: PathBuf,
    /// Killed by a planned crash.
    killed: bool,
    reaped: Option<std::process::ExitStatus>,
}

enum Message {
    Line(usize, Value),
    Closed(usize),
}

fn kill_group(child: &mut Child) {
    let pid = child.id() as i32;
    // SAFETY: kill(2) on the process group created for the child
    // (`process_group(0)`); no memory-safety preconditions.
    unsafe {
        libc::kill(-pid, SIGKILL);
    }
    let _ = child.kill();
}

/// Whether the child `pid` has exited, WITHOUT reaping it (`WNOWAIT`).
fn leader_exited(pid: u32) -> bool {
    // SAFETY: waitid(2) on our own child with WNOWAIT|WNOHANG writes only
    // into `info`, which is a zeroed, properly sized siginfo_t.
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        let r = libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
        );
        r == 0 && siginfo_pid(&info) != 0
    }
}

#[cfg(target_os = "linux")]
unsafe fn siginfo_pid(info: &libc::siginfo_t) -> libc::pid_t {
    // SAFETY: the caller passes a siginfo_t filled by waitid.
    unsafe { info.si_pid() }
}

#[cfg(not(target_os = "linux"))]
unsafe fn siginfo_pid(info: &libc::siginfo_t) -> libc::pid_t {
    info.si_pid
}

fn send(stream: &mut UnixStream, message: &Value) -> std::io::Result<()> {
    let mut line = message.to_string();
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()
}

fn reader(slot: usize, stream: UnixStream, tx: Sender<Message>) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(Message::Closed(slot));
                    return;
                }
                Ok(_) => match serde_json::from_str::<Value>(line.trim()) {
                    Ok(v) => {
                        if tx.send(Message::Line(slot, v)).is_err() {
                            return;
                        }
                    }
                    Err(_) => {
                        let _ = tx.send(Message::Closed(slot));
                        return;
                    }
                },
            }
        }
    });
}

/// Apply `limits` to the calling process (between fork and exec).
// `rlim_t` is `u64` on the supported platforms but not by definition.
#[allow(clippy::unnecessary_cast)]
fn apply_limits(limits: WorkerLimits) -> std::io::Result<()> {
    let set = |resource, soft: u64, hard: u64| {
        let limit = libc::rlimit {
            rlim_cur: soft as libc::rlim_t,
            rlim_max: hard as libc::rlim_t,
        };
        // SAFETY: setrlimit with a valid, initialized struct.
        if unsafe { libc::setrlimit(resource, &limit) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    };
    set(libc::RLIMIT_CPU, limits.cpu_seconds, limits.cpu_seconds + 1)?;
    let bytes = limits.file_size_mb * 1024 * 1024;
    set(libc::RLIMIT_FSIZE, bytes, bytes)?;
    // Lowering only: an inherited lower hard limit is kept.
    let mut current = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit writes into `current`.
    unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut current) };
    let files = (limits.open_files as libc::rlim_t).min(current.rlim_max);
    set(libc::RLIMIT_NOFILE, files as u64, files as u64)?;
    set(libc::RLIMIT_CORE, 0, 0)
}

/// A random token identifying this run's processes (environment variable
/// `COMMITRIFT_RUN_TOKEN`), for the survivor sweep.
fn run_token() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut bytes);
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let mut out: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    out.push_str(&format!("{:x}{:x}", std::process::id(), nanos));
    out
}

/// Processes of the current user whose environment carries `token`
/// (macOS: `ps -E`; Linux: `/proc/*/environ`). A worker's child that left
/// its process group (`setsid`, `detached: true`) is found this way.
pub fn processes_with_token(token: &str) -> Vec<i32> {
    let needle = format!("COMMITRIFT_RUN_TOKEN={token}");
    let mut out = Vec::new();
    if cfg!(target_os = "linux") {
        if let Ok(dir) = std::fs::read_dir("/proc") {
            for entry in dir.flatten() {
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|n| n.parse::<i32>().ok())
                else {
                    continue;
                };
                if let Ok(env) = std::fs::read(entry.path().join("environ"))
                    && env.split(|b| *b == 0).any(|kv| kv == needle.as_bytes())
                {
                    out.push(pid);
                }
            }
        }
    } else if let Ok(o) = Command::new("/bin/ps")
        .args(["-E", "-ww", "-x", "-o", "pid=,command="])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            let line = line.trim_start();
            if line.split_whitespace().any(|w| w == needle)
                && let Some(pid) = line.split_whitespace().next().and_then(|p| p.parse().ok())
            {
                out.push(pid);
            }
        }
    }
    out.retain(|p| *p != std::process::id() as i32);
    out
}

/// How workers are started (same for the first execution and restarts).
struct Launcher {
    command: Vec<String>,
    program: String,
    env: Vec<(String, String)>,
    runs: PathBuf,
    root: PathBuf,
    harness_dir: PathBuf,
    tx: Sender<Message>,
    token: String,
    limits: WorkerLimits,
}

/// The `application_name` a worker's sessions use (`source.js`).
fn application_name(actor: &str, attempt: u32) -> String {
    format!("commitrift-actor-{actor}#{attempt}")
}

fn sql_text(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

struct Coordinator<'r> {
    req: &'r RunRequest<'r>,
    cluster: &'r Cluster,
    launcher: Launcher,
    /// Every worker ever started (slots), in start order.
    workers: Vec<Worker>,
    /// Actor → slot of its current execution.
    current: Vec<usize>,
    rx: Receiver<Message>,
    statements: Vec<Observed>,
    effects: Vec<EffectRecord>,
    crashes: Vec<CrashEvidence>,
    restarts: Vec<RestartEvidence>,
    released: usize,
    problems: Vec<String>,
    /// Statements a driver issued for itself (not held; reported).
    driver_internal: Vec<String>,
    diverged: bool,
    inconclusive: bool,
    deadline: Instant,
}

impl Coordinator<'_> {
    /// Start one execution of `actor`; returns its slot.
    fn spawn(&mut self, actor: usize, attempt: u32) -> Result<usize, String> {
        let a = &self.req.actors[actor];
        let slot = self.workers.len();
        let l = &self.launcher;
        let socket = l.runs.join(format!("w{slot}.sock"));
        let listener = UnixListener::bind(&socket)
            .map_err(|e| format!("cannot create the coordination socket: {e}"))?;
        let _ = listener.set_nonblocking(true);
        let stdout = l.runs.join(format!("w{slot}.stdout"));
        let stderr = l.runs.join(format!("w{slot}.stderr"));
        let (Ok(out), Ok(err)) = (
            std::fs::File::create(&stdout),
            std::fs::File::create(&stderr),
        ) else {
            return Err("cannot create log files".to_owned());
        };
        let mut cmd = Command::new(&l.command[0]);
        cmd.args(&l.command[1..])
            .current_dir(&l.harness_dir)
            .env_clear()
            .envs(l.env.iter().map(|(k, v)| (k, v)))
            .env("COMMITRIFT_SOURCE_SOCKET", &socket)
            .env("COMMITRIFT_SOURCE_ACTOR", &a.name)
            .env("COMMITRIFT_SOURCE_ENTRY", &a.entry)
            .env("COMMITRIFT_SOURCE_INPUT", a.input.to_string())
            .env("COMMITRIFT_SOURCE_ATTEMPT", attempt.to_string())
            .env("COMMITRIFT_RUN_TOKEN", &l.token)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .process_group(0);
        let limits = l.limits;
        // SAFETY: the closure runs in the forked child before exec and only
        // calls setrlimit(2), which is async-signal-safe.
        unsafe {
            cmd.pre_exec(move || apply_limits(limits));
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("cannot start `{}`: {e}", l.program))?;
        let stream = loop {
            match listener.accept() {
                Ok((s, _)) => break Some(s),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if matches!(child.try_wait(), Ok(Some(_))) || Instant::now() >= self.deadline {
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(_) => break None,
            }
        };
        let _ = std::fs::remove_file(&socket);
        let Some(stream) = stream else {
            kill_group(&mut child);
            let _ = child.wait();
            let text = std::fs::read_to_string(&stderr).unwrap_or_default();
            return Err(format!(
                "worker `{}` (attempt {attempt}) did not connect: {}",
                a.name,
                safety::redact(text.trim(), &l.root)
            ));
        };
        let _ = stream.set_nonblocking(false);
        let read_half = stream.try_clone().map_err(|e| format!("socket: {e}"))?;
        reader(slot, read_half, l.tx.clone());
        let pid = child.id();
        self.workers.push(Worker {
            actor,
            attempt,
            child,
            pid,
            stream: Some(stream),
            hello: false,
            done: None,
            held: Vec::new(),
            running: None,
            stderr_path: stderr,
            killed: false,
            reaped: None,
        });
        if actor < self.current.len() {
            self.current[actor] = slot;
        } else {
            self.current.push(slot);
        }
        Ok(slot)
    }

    fn slot(&self, actor: usize) -> usize {
        self.current[actor]
    }

    fn in_scope(&self, actor: usize, frames: &[Frame]) -> Option<bool> {
        let scope = self.req.actors[actor].scope.as_ref()?;
        if frames.is_empty() {
            return None;
        }
        Some(frames.iter().any(|f| scope.contains(f)))
    }

    fn handle(&mut self, message: Message) {
        match message {
            Message::Closed(w) => {
                self.workers[w].stream = None;
            }
            Message::Line(w, v) => match v["type"].as_str().unwrap_or_default() {
                "hello" => self.workers[w].hello = true,
                kind @ ("query" | "effect") => {
                    let frames: Vec<Frame> = v["site"]["frames"]
                        .as_array()
                        .map(|f| {
                            f.iter()
                                .filter_map(|x| serde_json::from_value(x.clone()).ok())
                                .collect()
                        })
                        .unwrap_or_default();
                    let (sql, kind, table, params) = if kind == "effect" {
                        let channel = v["channel"].as_str().unwrap_or_default().to_owned();
                        (
                            format!("EFFECT {channel}"),
                            SqlKind::Effect,
                            Some(channel),
                            json!({"key": v["key"].clone(), "payload": v["payload"].clone()}),
                        )
                    } else {
                        let sql = v["sql"].as_str().unwrap_or_default().to_owned();
                        let (kind, table) = classify(&sql);
                        (sql, kind, table, v["params"].clone())
                    };
                    let index = self.statements.len();
                    let worker = &self.workers[w];
                    self.statements.push(Observed {
                        actor: worker.actor,
                        attempt: worker.attempt,
                        seq: v["seq"].as_u64().unwrap_or_default(),
                        client: v["client"].as_u64().unwrap_or_default(),
                        backend_pid: v["backendPid"].as_i64(),
                        sql,
                        params,
                        kind: Some(kind),
                        table,
                        frames,
                        reconstructed: v["site"]["reconstructed"] == true,
                        role: String::new(),
                        slot: w,
                        ..Observed::default()
                    });
                    self.workers[w].held.push(index);
                }
                "result" | "error" => {
                    let seq = v["seq"].as_u64().unwrap_or_default();
                    if let Some(s) = self
                        .statements
                        .iter_mut()
                        .find(|s| s.slot == w && s.seq == seq)
                    {
                        s.completed = true;
                        if v["type"] == "result" {
                            s.command = v["command"].as_str().map(str::to_owned);
                            s.row_count = v["rowCount"].as_i64();
                            s.raw_fields = v["fields"]
                                .as_array()
                                .map(|f| {
                                    f.iter()
                                        .map(|x| {
                                            (
                                                x["name"].as_str().unwrap_or_default().to_owned(),
                                                x["tableID"].as_i64().unwrap_or_default(),
                                            )
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            s.raw_rows = v["rows"].as_array().cloned().unwrap_or_default();
                        } else {
                            s.sqlstate = v["sqlstate"].as_str().map(str::to_owned);
                            s.error = v["message"].as_str().map(str::to_owned);
                        }
                    }
                    if let Some(r) = self.workers[w].running
                        && self.statements[r].seq == seq
                    {
                        self.workers[w].running = None;
                    }
                }
                "unsupported" => {
                    let name = &self.req.actors[self.workers[w].actor].name;
                    self.problems.push(format!(
                        "{name}: {}",
                        v["reason"].as_str().unwrap_or("unsupported query form")
                    ));
                    self.inconclusive = true;
                }
                "done" => self.workers[w].done = Some(v),
                "driver-internal" => {
                    let sql = short(v["sql"].as_str().unwrap_or_default());
                    let name = &self.req.actors[self.workers[w].actor].name;
                    self.driver_internal.push(format!("{name}: {sql}"));
                }
                _ => {}
            },
        }
    }

    fn pump(&mut self, wait: Duration) {
        match self.rx.recv_timeout(wait) {
            Ok(m) => {
                self.handle(m);
                while let Ok(m) = self.rx.try_recv() {
                    self.handle(m);
                }
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
        }
    }

    /// Whether the worker in `slot` has ended (reported, disconnected or
    /// killed).
    fn finished(&self, slot: usize) -> bool {
        self.workers[slot].done.is_some() || self.workers[slot].stream.is_none()
    }

    /// The channel test double receives an effect: record it under the
    /// channel's declared deduplication contract and return the receipt.
    fn perform_effect(&mut self, index: usize) -> Value {
        let s = &self.statements[index];
        let channel = s.table.clone().unwrap_or_default();
        let key = s.params["key"].clone();
        let by_key = self
            .req
            .channels
            .get(&channel)
            .is_some_and(|d| d == "byKey");
        let deduplicated = by_key
            && !key.is_null()
            && self
                .effects
                .iter()
                .any(|e| e.channel == channel && e.key == key && !e.deduplicated);
        let id = self.effects.len() + 1;
        self.effects.push(EffectRecord {
            id,
            actor: s.actor,
            attempt: s.attempt,
            channel,
            key,
            payload: s.params["payload"].clone(),
            deduplicated,
            frames: s.frames.clone(),
        });
        json!({"id": id, "deduplicated": deduplicated})
    }

    fn release(&mut self, index: usize, role: &str, plan: Option<usize>) -> bool {
        let w = self.statements[index].slot;
        self.workers[w].held.retain(|i| *i != index);
        self.released += 1;
        let effect = (self.statements[index].kind == Some(SqlKind::Effect))
            .then(|| self.perform_effect(index));
        if let Some(receipt) = &effect {
            self.statements[index].params["receipt"] = receipt.clone();
        }
        let s = &mut self.statements[index];
        s.role = role.to_owned();
        s.plan = plan;
        s.released = Some(self.released);
        self.workers[w].running = Some(index);
        let message = match effect {
            Some(receipt) => json!({"type": "go", "effect": receipt}),
            None => json!({"type": "go"}),
        };
        match self.workers[w].stream.as_mut() {
            Some(stream) => send(stream, &message).is_ok(),
            None => false,
        }
    }

    /// Release statements that lie outside the modeled region (a wrapper's
    /// own work before the callback). Writes outside the model make the
    /// run inconclusive: the model does not account for them.
    fn release_outside(&mut self) {
        for a in 0..self.current.len() {
            let w = self.slot(a);
            if self.workers[w].running.is_some() || self.workers[w].killed {
                continue;
            }
            let Some(&index) = self.workers[w].held.first() else {
                continue;
            };
            // A driver that sets the isolation level in a statement of its
            // own (postgres-js: `begin`, then `set transaction ...`): the
            // statement belongs to the BEGIN the plan released just before.
            if is_transaction_config(&self.statements[index].sql)
                && let Some(previous) = self
                    .statements
                    .iter()
                    .rposition(|s| s.slot == w && s.released.is_some())
                && self.statements[previous].kind == Some(SqlKind::Begin)
                && self.statements[previous].role == "planned"
            {
                let plan = self.statements[previous].plan;
                self.release(index, "begin-config", plan);
                continue;
            }
            if self.in_scope(a, &self.statements[index].frames) == Some(false) {
                if !matches!(self.statements[index].kind, Some(SqlKind::Select)) {
                    self.problems.push(format!(
                        "{} sent `{}` outside the modeled function: a write, an effect or transaction control the model does not represent",
                        self.req.actors[a].name,
                        short(&self.statements[index].sql)
                    ));
                    self.inconclusive = true;
                }
                self.release(index, "outside-model", None);
            }
        }
    }

    /// Wait until `index` completes, or PostgreSQL reports it waiting for a
    /// lock. Returns `Some(blocked)`, or `None` at the deadline.
    fn settle(&mut self, w: usize, index: usize) -> Option<bool> {
        let started = Instant::now();
        loop {
            if self.statements[index].completed {
                return Some(false);
            }
            if self.finished(w) {
                return Some(false);
            }
            if Instant::now() >= self.deadline {
                return None;
            }
            self.release_outside();
            self.pump(Duration::from_millis(5));
            if self.statements[index].completed {
                return Some(false);
            }
            if self.statements[index].kind == Some(SqlKind::Effect) {
                continue;
            }
            if started.elapsed() >= self.req.settle
                && let Some(filter) = self.backend_filter(index)
            {
                let sql = format!(
                    "SELECT coalesce(wait_event_type, '') || '/' || state FROM pg_stat_activity WHERE {filter}"
                );
                if let Ok(Value::String(w)) = self.cluster.query_value("postgres", &sql)
                    && w.starts_with("Lock/")
                {
                    // Re-check: a result may have arrived meanwhile.
                    self.pump(Duration::from_millis(1));
                    if !self.statements[index].completed {
                        self.statements[index].blocked = true;
                        return Some(true);
                    }
                }
            }
        }
    }

    /// How to find the backend running statement `index`: its pid when the
    /// driver reports it (node-postgres), else the one non-idle session of
    /// the execution's `application_name` (postgres-js does not expose the
    /// connection). With several non-idle sessions the query returns more
    /// than one row and the wait is not attributed.
    fn backend_filter(&self, index: usize) -> Option<String> {
        let s = &self.statements[index];
        if let Some(pid) = s.backend_pid {
            return Some(format!("pid = {pid}"));
        }
        let name = application_name(&self.req.actors[s.actor].name, s.attempt);
        Some(format!(
            "application_name = {} AND state <> 'idle' AND backend_type = 'client backend'",
            sql_text(&name)
        ))
    }

    /// Whether the held statement `index` is what `e` expects.
    fn matches(&self, index: usize, e: &Expected) -> bool {
        let s = &self.statements[index];
        let kind_ok = s.kind == Some(e.kind)
            && (e.table.is_none() || s.table.as_deref() == e.table.as_deref());
        // Effects and transaction control are issued on the application's
        // behalf by a seam or a helper (`db.transaction`, a retry helper):
        // the modeled call site must be on the call chain, not innermost.
        let on_chain = matches!(
            e.kind,
            SqlKind::Effect | SqlKind::Begin | SqlKind::Commit | SqlKind::Rollback
        );
        let site_ok = match (&e.region, s.frames.first()) {
            (Some(region), Some(_)) if on_chain => s.frames.iter().any(|f| region.contains(f)),
            (Some(region), Some(frame)) => region.contains(frame),
            (Some(_), None) | (None, _) => true,
        };
        let key_ok =
            e.kind != SqlKind::Effect || e.key.as_ref().is_none_or(|k| *k == s.params["key"]);
        kind_ok && site_ok && key_ok
    }

    fn mismatch(&mut self, k: usize, index: usize, e: &Expected, at: &str) {
        let s = &self.statements[index];
        let key = if s.kind == Some(SqlKind::Effect) {
            format!(" with key {}", s.params["key"])
        } else {
            String::new()
        };
        self.problems.push(format!(
            "plan step {k}: {} sent `{}`{key}{} {at} the model expects {}",
            self.req.actors[e.actor].name,
            short(&s.sql),
            s.frames
                .first()
                .map(|f| format!(" (at {}:{}:{})", f.file, f.line, f.column))
                .unwrap_or_default(),
            e.label
        ));
        self.statements[index].role = "unexpected".to_owned();
        self.diverged = true;
    }

    /// Wait for the actor's next announced statement inside the model.
    fn next_held(&mut self, k: usize, a: usize, what: &str) -> Option<usize> {
        loop {
            self.release_outside();
            let w = self.slot(a);
            if let Some(&i) = self.workers[w].held.first()
                && self.workers[w].running.is_none()
                && self.in_scope(a, &self.statements[i].frames) != Some(false)
            {
                return Some(i);
            }
            if self.finished(w) && self.workers[w].held.is_empty() {
                self.problems.push(format!(
                    "plan step {k}: {} finished without sending the expected {what}",
                    self.req.actors[a].name
                ));
                self.diverged = true;
                return None;
            }
            if Instant::now() >= self.deadline {
                self.problems.push(format!(
                    "plan step {k}: {} did not send the expected {what} before the deadline",
                    self.req.actors[a].name
                ));
                self.inconclusive = true;
                return None;
            }
            self.pump(Duration::from_millis(10));
        }
    }

    fn release_step(&mut self, k: usize, e: &Expected) {
        let a = e.actor;
        let Some(index) = self.next_held(k, a, &e.label) else {
            return;
        };
        if !self.matches(index, e) {
            self.mismatch(k, index, e, "where");
            return;
        }
        if !self.release(index, "planned", Some(k)) {
            self.problems
                .push(format!("{}: the worker went away", self.req.actors[a].name));
            self.inconclusive = true;
            return;
        }
        let w = self.slot(a);
        match self.settle(w, index) {
            None => {
                self.problems.push(format!(
                    "plan step {k}: `{}` did not complete before the deadline",
                    short(&self.statements[index].sql)
                ));
                self.inconclusive = true;
            }
            Some(blocked) if blocked != e.expect_block => {
                self.problems.push(format!(
                    "plan step {k}: the model predicts the statement {} but PostgreSQL {}",
                    if e.expect_block {
                        "waits for a lock"
                    } else {
                        "completes"
                    },
                    if blocked {
                        "reported a lock wait"
                    } else {
                        "completed it"
                    }
                ));
                self.diverged = true;
            }
            Some(_) => {}
        }
    }

    /// The sessions of one execution, by `application_name`.
    fn sessions(&self, actor: usize, attempt: u32) -> Result<Vec<i64>, String> {
        let name = application_name(&self.req.actors[actor].name, attempt);
        let sql = format!(
            "SELECT coalesce(json_agg(pid ORDER BY pid), '[]'::json) FROM pg_stat_activity WHERE application_name = {}",
            sql_text(&name)
        );
        match self.cluster.query_value("postgres", &sql)? {
            Value::String(text) => {
                serde_json::from_str::<Vec<i64>>(&text).map_err(|e| e.to_string())
            }
            other => Err(format!("unexpected {other}")),
        }
    }

    fn read_tables(&mut self) -> BTreeMap<String, Vec<Value>> {
        let mut tables = BTreeMap::new();
        for (name, sql) in self.req.final_queries {
            match self.cluster.query_value(DATABASE, sql) {
                Ok(Value::String(text)) => match serde_json::from_str::<Value>(&text) {
                    Ok(Value::Array(rows)) => {
                        tables.insert(name.clone(), rows);
                    }
                    _ => self
                        .problems
                        .push(format!("rows of `{name}` are not a JSON array")),
                },
                Ok(other) => self
                    .problems
                    .push(format!("rows of `{name}`: unexpected {other}")),
                Err(e) => self.problems.push(format!("reading `{name}` failed: {e}")),
            }
        }
        tables
    }

    /// A planned crash: wait for the checkpoint, SIGKILL the worker's
    /// process group, then wait until PostgreSQL has ended its sessions
    /// (which rolls back whatever they had not committed).
    fn crash_step(&mut self, k: usize, c: &CrashStep) {
        let a = c.actor;
        let name = self.req.actors[a].name.clone();
        // 1. The checkpoint.
        let (checkpoint, never_sent) = if c.waiting {
            // The statement released for the model's waiting operation.
            let w = self.slot(a);
            let Some(i) = self
                .statements
                .iter()
                .rposition(|s| s.slot == w && s.role == "planned" && s.blocked)
            else {
                self.problems.push(format!(
                    "plan step {k}: the model crashes {name} while its statement waits for a lock, but no such wait was observed"
                ));
                self.diverged = true;
                return;
            };
            let sql = short(&self.statements[i].sql);
            if c.may_have_run {
                // Whatever the statement did, its transaction is open (the
                // next operation, if any, is held): it never commits.
                (
                    format!(
                        "after the lock `{sql}` waited for was released: the statement {} in its open transaction (in flight)",
                        if self.statements[i].completed {
                            "had executed"
                        } else {
                            "may have executed"
                        }
                    ),
                    None,
                )
            } else {
                // PostgreSQL must still report the wait right before the
                // kill: otherwise the statement may have run.
                let waiting = !self.statements[i].completed
                    && self.backend_filter(i).is_some_and(|filter| {
                        matches!(
                            self.cluster.query_value(
                                "postgres",
                                &format!("SELECT wait_event_type FROM pg_stat_activity WHERE {filter}")
                            ),
                            Ok(Value::String(t)) if t == "Lock"
                        )
                    });
                if !waiting {
                    self.problems.push(format!(
                        "plan step {k}: the model crashes {name} while `{sql}` waits for a lock, but PostgreSQL no longer reports the wait: the crash point is not reproducible"
                    ));
                    self.inconclusive = true;
                    return;
                }
                (
                    format!(
                        "while `{sql}` waited for a lock (pg_stat_activity: wait_event_type = Lock; not executed)"
                    ),
                    None,
                )
            }
        } else {
            let Some(next) = &c.next else {
                self.problems.push(format!(
                    "plan step {k}: the crash boundary has no observable checkpoint"
                ));
                self.inconclusive = true;
                return;
            };
            let Some(index) = self.next_held(k, a, &next.label) else {
                return;
            };
            if !self.matches(index, next) {
                self.mismatch(k, index, next, "at the crash point, where");
                return;
            }
            let s = &self.statements[index];
            (
                format!(
                    "after the previous operation was acknowledged, with the next one (`{}`) announced and held: never sent",
                    short(&s.sql)
                ),
                Some(short(&s.sql)),
            )
        };
        // 2. Its PostgreSQL sessions, then SIGKILL.
        let w = self.slot(a);
        let attempt = self.workers[w].attempt;
        let backends = match self.sessions(a, attempt) {
            Ok(b) => b,
            Err(e) => {
                self.problems
                    .push(format!("plan step {k}: cannot list {name}'s sessions: {e}"));
                self.inconclusive = true;
                return;
            }
        };
        let pid = self.workers[w].pid;
        kill_group(&mut self.workers[w].child);
        let status = self.workers[w].child.wait().ok();
        self.workers[w].reaped = status;
        self.workers[w].killed = true;
        self.workers[w].stream = None;
        let held: Vec<usize> = std::mem::take(&mut self.workers[w].held);
        for i in held {
            self.statements[i].role = "never-sent".to_owned();
        }
        self.workers[w].running = None;
        use std::os::unix::process::ExitStatusExt;
        let signal = status.and_then(|s| s.signal());
        let killed_by_signal = signal == Some(SIGKILL);
        if !killed_by_signal {
            self.problems.push(format!(
                "plan step {k}: {name}'s worker was not terminated by SIGKILL ({status:?})"
            ));
            self.inconclusive = true;
        }
        // 3. PostgreSQL notices the lost connections and ends the sessions.
        let started = Instant::now();
        let until = (Instant::now() + Duration::from_secs(10)).min(self.deadline);
        let sessions_ended = loop {
            match self.sessions(a, attempt) {
                Ok(left) if left.is_empty() => break true,
                Ok(_) if Instant::now() < until => std::thread::sleep(Duration::from_millis(10)),
                _ => break false,
            }
        };
        let sessions_ended_ms = started.elapsed().as_millis() as u64;
        if !sessions_ended {
            self.problems.push(format!(
                "plan step {k}: PostgreSQL still had sessions of the killed {name} (attempt {attempt}) after {sessions_ended_ms} ms: the transaction's fate is not established"
            ));
            self.inconclusive = true;
        }
        let tables = self.read_tables();
        self.crashes.push(CrashEvidence {
            actor: a,
            attempt,
            plan: k,
            transition: c.transition,
            pid,
            checkpoint,
            never_sent,
            killed_by_signal,
            exit: status.map(|s| format!("{s}")),
            backends,
            sessions_ended,
            sessions_ended_ms,
            tables,
        });
    }

    /// A declared recovery mechanism starts a new execution: a new process
    /// with fresh connections (nothing of the previous one is reused).
    fn restart_step(&mut self, k: usize, r: &RestartStep) {
        let a = r.actor;
        let previous = self.slot(a);
        // The previous execution must have ended (the model retries only
        // after every task of the attempt ended).
        while !self.finished(previous) {
            if Instant::now() >= self.deadline {
                self.problems.push(format!(
                    "plan step {k}: {} did not end before its retry",
                    self.req.actors[a].name
                ));
                self.inconclusive = true;
                return;
            }
            self.release_outside();
            self.pump(Duration::from_millis(5));
        }
        let previous_outcome = if self.workers[previous].killed {
            "killed".to_owned()
        } else {
            self.workers[previous]
                .done
                .as_ref()
                .and_then(|d| d["outcome"].as_str())
                .unwrap_or("unknown")
                .to_owned()
        };
        let expected_attempt = self.workers[previous].attempt + 1;
        if r.attempt != expected_attempt {
            self.problems.push(format!(
                "plan step {k}: the plan starts attempt {} of {}, the coordinator is at attempt {expected_attempt}",
                r.attempt, self.req.actors[a].name
            ));
            self.inconclusive = true;
            return;
        }
        match self.spawn(a, r.attempt) {
            Ok(slot) => {
                let previous_pid = self.workers[previous].pid;
                self.restarts.push(RestartEvidence {
                    actor: a,
                    attempt: r.attempt,
                    plan: k,
                    transition: r.transition,
                    mechanism: r.mechanism.clone(),
                    pid: self.workers[slot].pid,
                    previous_pid,
                    previous_outcome,
                });
            }
            Err(e) => {
                self.problems
                    .push(format!("plan step {k}: the retry could not start: {e}"));
                self.inconclusive = true;
            }
        }
    }

    fn run_plan(&mut self) {
        let plan = self.req.plan;
        for (k, step) in plan.iter().enumerate() {
            if self.diverged || self.inconclusive {
                return;
            }
            match step {
                PlanStep::Release(e) => self.release_step(k, e),
                PlanStep::Complete { actor, transition } => {
                    let a = *actor;
                    let w = self.slot(a);
                    let Some(index) = self.workers[w].running else {
                        self.problems.push(format!(
                            "plan step {k} (transition {transition}): {} has no waiting statement to resume",
                            self.req.actors[a].name
                        ));
                        self.diverged = true;
                        return;
                    };
                    loop {
                        if self.statements[index].completed || self.finished(w) {
                            break;
                        }
                        if Instant::now() >= self.deadline {
                            self.problems.push(format!(
                                "plan step {k}: the waiting statement of {} did not complete",
                                self.req.actors[a].name
                            ));
                            self.inconclusive = true;
                            return;
                        }
                        self.release_outside();
                        self.pump(Duration::from_millis(5));
                    }
                }
                PlanStep::Crash(c) => self.crash_step(k, c),
                PlanStep::Restart(r) => self.restart_step(k, r),
            }
        }
    }

    /// After the plan: let every worker finish, releasing whatever it
    /// still sends (recorded as `beyond-plan`, which the comparison treats
    /// as unexpected if the model's actor had finished).
    fn drain(&mut self) {
        loop {
            self.release_outside();
            let mut progressed = false;
            for a in 0..self.current.len() {
                let w = self.slot(a);
                if self.workers[w].running.is_some() || self.workers[w].killed {
                    continue;
                }
                if let Some(&index) = self.workers[w].held.first() {
                    let role = if self.diverged || self.inconclusive {
                        "after-divergence"
                    } else {
                        "beyond-plan"
                    };
                    self.release(index, role, None);
                    progressed = true;
                }
            }
            if (0..self.workers.len()).all(|w| self.finished(w)) {
                return;
            }
            if Instant::now() >= self.deadline {
                self.problems
                    .push("workers did not finish before the deadline".to_owned());
                self.inconclusive = true;
                return;
            }
            if !progressed {
                self.pump(Duration::from_millis(10));
            }
        }
    }

    fn kill_all(&mut self) {
        for w in &mut self.workers {
            if w.reaped.is_none() {
                kill_group(&mut w.child);
                w.reaped = w.child.wait().ok();
            }
        }
    }
}

impl Drop for Coordinator<'_> {
    /// On every exit path, including a panic of the coordinator: no worker
    /// outlives it.
    fn drop(&mut self) {
        self.kill_all();
    }
}

/// `SET TRANSACTION ...` (isolation level, read/write mode).
fn is_transaction_config(sql: &str) -> bool {
    strip_comments(sql)
        .to_ascii_lowercase()
        .starts_with("set transaction")
}

fn short(sql: &str) -> String {
    let flat: String = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 90 {
        flat.chars().take(87).collect::<String>() + "..."
    } else {
        flat
    }
}

fn blocked(reason: impl Into<String>) -> SourceRun {
    SourceRun {
        status: "blocked".to_owned(),
        problems: vec![reason.into()],
        ..SourceRun::default()
    }
}

/// The server settings a plan needs beyond PostgreSQL's defaults. With a
/// planned crash, a backend must notice its client's death even while it
/// waits for a lock (by default it notices only when it next reads from the
/// socket, holding its locks until then): `client_connection_check_interval`
/// (PostgreSQL 14+).
pub fn server_settings(plan: &[PlanStep], server_major: u32) -> Result<Vec<String>, String> {
    if !plan.iter().any(|s| matches!(s, PlanStep::Crash(_))) {
        return Ok(Vec::new());
    }
    if server_major < 14 {
        return Err(format!(
            "planned crashes need client_connection_check_interval (PostgreSQL 14+); the server is {server_major}"
        ));
    }
    Ok(vec!["client_connection_check_interval=50ms".to_owned()])
}

/// Run the actors' original code under the plan. Returns a report; setup
/// failures are reported with status `blocked`, never as a verdict.
pub fn run(req: &RunRequest<'_>) -> SourceRun {
    if !req.allow_execution {
        return blocked(
            "source verification executes the application's code: it requires the explicit --allow-execution flag",
        );
    }
    let started = Instant::now();
    let manifest = &req.harness.manifest;
    for a in req.actors {
        if !manifest.entries.contains_key(&a.entry) {
            return blocked(format!(
                "actor `{}`: the harness has no entry `{}`",
                a.name, a.entry
            ));
        }
    }
    let bins = match Binaries::discover() {
        Ok(b) => b,
        Err(e) => return blocked(e),
    };
    let settings = match server_settings(req.plan, postgres::major_version(&bins.version)) {
        Ok(s) => s,
        Err(e) => return blocked(e),
    };
    let root = match ReplayRoot::create(req.keep_artifacts) {
        Ok(r) => r,
        Err(e) => return blocked(e),
    };
    let isolation = match isolation::prepare(&IsolationRequest {
        root: &root.path,
        harness_dir: &req.harness.dir,
        program: &manifest.command[0],
        allow_unisolated: false,
    }) {
        Ok(i) => i,
        Err(e) => return blocked(e),
    };
    let cluster = match Cluster::start_with(bins, &root.path.join("db"), &settings) {
        Ok(c) => c,
        Err(e) => return blocked(format!("the disposable PostgreSQL cluster failed: {e}")),
    };
    if let Err(e) = safety::check_database_target(&root.path, &cluster.socket_path()) {
        return blocked(e);
    }
    if let Err(e) = cluster.create_database(DATABASE) {
        return blocked(format!("cannot create the run database: {e}"));
    }
    for s in &manifest.schema {
        if let Err(e) = cluster.apply_file(DATABASE, &req.harness.dir.join(s)) {
            return blocked(format!("applying `{s}` failed: {e}"));
        }
    }
    let catalog = if req.read_catalog {
        match crate::catalog::read(&cluster, DATABASE) {
            Ok(c) => Some(c),
            Err(e) => return blocked(format!("reading the catalog failed: {e}")),
        }
    } else {
        None
    };
    for dir in ["home", "tmp", "runs"] {
        if let Err(e) = safety::create_private_dir(&root.path.join(dir)) {
            return blocked(e);
        }
    }
    let seed = root.path.join("runs").join("seed.sql");
    if std::fs::write(&seed, req.seed_sql).is_err() {
        return blocked("cannot write the seed SQL");
    }
    if let Err(e) = cluster.apply_file(DATABASE, &seed) {
        return blocked(format!("seeding the initial state failed: {e}"));
    }

    let env: Vec<(String, String)> = vec![
        (
            "PATH".to_owned(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_owned()),
        ),
        (
            "HOME".to_owned(),
            root.path.join("home").display().to_string(),
        ),
        (
            "TMPDIR".to_owned(),
            root.path.join("tmp").display().to_string(),
        ),
        ("LANG".to_owned(), "C".to_owned()),
        ("NO_COLOR".to_owned(), "1".to_owned()),
        ("COMMITRIFT_SOURCE".to_owned(), "1".to_owned()),
        (
            "COMMITRIFT_REPLAY_ROOT".to_owned(),
            root.path.display().to_string(),
        ),
        (
            "COMMITRIFT_REPLAY_PG_SOCKET".to_owned(),
            cluster.socket_path().display().to_string(),
        ),
        (
            "COMMITRIFT_REPLAY_PG_USER".to_owned(),
            postgres::USER.to_owned(),
        ),
        (
            "COMMITRIFT_REPLAY_PG_DATABASE".to_owned(),
            DATABASE.to_owned(),
        ),
        (
            "COMMITRIFT_SOURCE_PROJECT".to_owned(),
            req.harness.dir.display().to_string(),
        ),
    ];
    let (tx, rx) = channel();
    let mut co = Coordinator {
        req,
        cluster: &cluster,
        launcher: Launcher {
            command: isolation.wrap(&manifest.command),
            program: manifest.command[0].clone(),
            env: env.clone(),
            runs: root.path.join("runs"),
            root: root.path.clone(),
            harness_dir: req.harness.dir.clone(),
            tx,
            token: run_token(),
            limits: manifest.limits,
        },
        workers: Vec::new(),
        current: Vec::new(),
        rx,
        statements: Vec::new(),
        effects: Vec::new(),
        crashes: Vec::new(),
        restarts: Vec::new(),
        released: 0,
        problems: Vec::new(),
        driver_internal: Vec::new(),
        diverged: false,
        inconclusive: false,
        deadline: Instant::now() + Duration::from_millis(manifest.timeout_ms),
    };
    for i in 0..req.actors.len() {
        if let Err(e) = co.spawn(i, 1) {
            co.kill_all();
            return blocked(e);
        }
    }
    co.run_plan();
    // After the plan (or a divergence): let the workers finish.
    co.drain();
    // Reap.
    let mut attempts = Vec::new();
    for w in &mut co.workers {
        if w.reaped.is_none() {
            // Let the leader exit by itself (it reported `done`), without
            // reaping it: while it is unreaped its pid, which is also the
            // group id, cannot be reused. Then no process of the group may
            // outlive the run (a detached child of the harness): the group
            // is killed, and only then the leader is reaped.
            let until = Instant::now() + Duration::from_secs(5);
            while !leader_exited(w.pid) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
            kill_group(&mut w.child);
            w.reaped = w.child.wait().ok();
        }
        let stderr = std::fs::read_to_string(&w.stderr_path).unwrap_or_default();
        let stderr: String = safety::redact(stderr.trim(), &root.path)
            .chars()
            .take(4000)
            .collect();
        let (outcome, value, error) = match (&w.done, w.killed) {
            (_, true) => ("killed".to_owned(), Value::Null, None),
            (Some(d), false) => (
                d["outcome"].as_str().unwrap_or("unknown").to_owned(),
                d["value"].clone(),
                d["error"].as_str().map(str::to_owned),
            ),
            (None, false) => ("killed".to_owned(), Value::Null, None),
        };
        attempts.push(ActorResult {
            name: req.actors[w.actor].name.clone(),
            entry: req.actors[w.actor].entry.clone(),
            outcome,
            value,
            error,
            stderr,
            attempt: w.attempt,
            pid: w.pid,
        });
    }
    // Survivors: processes of this run that left their worker's process
    // group. Killed, then checked again.
    let mut survivors = processes_with_token(&co.launcher.token);
    for pid in &survivors {
        // SAFETY: kill(2); the pid was just read with this run's token.
        unsafe {
            libc::kill(*pid, SIGKILL);
        }
    }
    if !survivors.is_empty() {
        std::thread::sleep(Duration::from_millis(50));
        let left = processes_with_token(&co.launcher.token);
        if !left.is_empty() {
            co.problems.push(format!(
                "{} process(es) of the run survived SIGKILL: {left:?}",
                left.len()
            ));
            co.inconclusive = true;
        }
    }
    survivors.sort_unstable();
    let actors: Vec<ActorResult> = co
        .current
        .iter()
        .map(|&slot| attempts[slot].clone())
        .collect();
    let _ = cluster.wait_for_idle(DATABASE, Duration::from_secs(5));
    let tables = co.read_tables();
    // Name result columns by their table (PostgreSQL's catalog).
    let oids: BTreeMap<i64, String> = match cluster.query_value(
        DATABASE,
        "SELECT coalesce(json_object_agg(oid::bigint, relname), '{}'::json) FROM pg_class WHERE relkind = 'r'",
    ) {
        Ok(Value::String(text)) => serde_json::from_str::<BTreeMap<String, String>>(&text)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(k, v)| k.parse().ok().map(|k| (k, v)))
            .collect(),
        _ => BTreeMap::new(),
    };
    for s in &mut co.statements {
        let fields = std::mem::take(&mut s.raw_fields);
        for row in std::mem::take(&mut s.raw_rows) {
            let mut object = serde_json::Map::new();
            for (i, (name, table)) in fields.iter().enumerate() {
                let key = match oids.get(table) {
                    Some(t) => format!("{t}.{name}"),
                    None => name.clone(),
                };
                object.insert(key, row.get(i).cloned().unwrap_or(Value::Null));
            }
            s.rows.push(Value::Object(object));
        }
    }
    let status = if co.diverged {
        "diverged"
    } else if co.inconclusive {
        "inconclusive"
    } else {
        "followed"
    };
    let problems = co.problems.clone();
    let statements = std::mem::take(&mut co.statements);
    let mut names: Vec<String> = env.iter().map(|(k, _)| k.clone()).collect();
    names.extend(
        [
            "COMMITRIFT_SOURCE_SOCKET",
            "COMMITRIFT_SOURCE_ACTOR",
            "COMMITRIFT_SOURCE_ENTRY",
            "COMMITRIFT_SOURCE_INPUT",
            "COMMITRIFT_SOURCE_ATTEMPT",
            "COMMITRIFT_RUN_TOKEN",
        ]
        .map(str::to_owned),
    );
    names.sort();
    let multi_attempt = attempts.len() > req.actors.len();
    SourceRun {
        status: status.to_owned(),
        problems,
        statements,
        actors,
        attempts: if multi_attempt { attempts } else { Vec::new() },
        effects: std::mem::take(&mut co.effects),
        crashes: std::mem::take(&mut co.crashes),
        restarts: std::mem::take(&mut co.restarts),
        server_settings: settings,
        survivors_killed: survivors,
        limits: manifest.limits.describe(),
        driver_internal: std::mem::take(&mut co.driver_internal),
        catalog,
        tables,
        postgres_version: cluster.version().to_owned(),
        isolation: isolation.describe(),
        environment_variables: names,
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statements_are_classified_by_keyword_and_table() {
        assert_eq!(
            classify(
                "select \"id\", \"email\" from \"users\" left join \"team_members\" on x limit $1"
            ),
            (SqlKind::Select, Some("users".to_owned()))
        );
        assert_eq!(
            classify("insert into \"invitations\" (\"id\") values (default)"),
            (SqlKind::Insert, Some("invitations".to_owned()))
        );
        assert_eq!(
            classify("update public.\"Orders\" set x = 1"),
            (SqlKind::Update, Some("Orders".to_owned()))
        );
        assert_eq!(
            classify("delete from jobs where id = 1"),
            (SqlKind::Delete, Some("jobs".to_owned()))
        );
        assert_eq!(classify("begin"), (SqlKind::Begin, None));
        assert_eq!(
            classify("BEGIN ISOLATION LEVEL SERIALIZABLE"),
            (SqlKind::Begin, None)
        );
        assert_eq!(classify("commit"), (SqlKind::Commit, None));
        assert_eq!(classify("rollback"), (SqlKind::Rollback, None));
        assert_eq!(classify("rollback to savepoint s1"), (SqlKind::Other, None));
        assert_eq!(classify("/* c */ -- x\nselect 1"), (SqlKind::Select, None));
        assert_eq!(
            classify("with r as (delete from t) select 1").0,
            SqlKind::Other
        );
    }

    #[test]
    fn execution_needs_the_explicit_authorization() {
        let harness = LoadedSource {
            dir: PathBuf::from("/nonexistent"),
            manifest_path: PathBuf::from("/nonexistent/m.json"),
            manifest: SourceManifest {
                schema_version: 1,
                kind: SOURCE_MANIFEST_KIND.to_owned(),
                harness: "h".to_owned(),
                description: String::new(),
                command: vec!["node".to_owned()],
                runtime: Value::Null,
                schema: Vec::new(),
                network: "deny".to_owned(),
                timeout_ms: 1000,
                entries: BTreeMap::new(),
                limits: WorkerLimits::default(),
            },
        };
        let report = run(&RunRequest {
            harness: &harness,
            allow_execution: false,
            keep_artifacts: false,
            actors: &[],
            plan: &[],
            seed_sql: "",
            final_queries: &[],
            settle: Duration::from_millis(50),
            channels: &BTreeMap::new(),
            read_catalog: false,
        });
        assert_eq!(report.status, "blocked");
        assert!(report.problems[0].contains("--allow-execution"));
        assert!(report.statements.is_empty());
    }

    #[test]
    fn manifests_must_deny_the_network_and_name_existing_files() {
        let dir = std::env::temp_dir().join(format!("cr-src-manifest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("commitrift-source.json");
        let write = |doc: Value| std::fs::write(&path, doc.to_string()).unwrap();
        let base = json!({
            "schemaVersion": 1, "kind": SOURCE_MANIFEST_KIND, "harness": "h",
            "command": ["node", "dist/h.js"], "schema": [], "entries": {}
        });
        write(base.clone());
        assert!(
            load_manifest(&path)
                .unwrap_err()
                .contains("compile the harness first")
        );
        let mut allow = base.clone();
        allow["network"] = json!("allow");
        write(allow);
        assert!(load_manifest(&path).unwrap_err().contains("deny"));
        let mut outside = base.clone();
        outside["command"] = json!(["node"]);
        outside["schema"] = json!(["../../etc/passwd"]);
        write(outside);
        assert!(load_manifest(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

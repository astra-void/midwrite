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
use crate::proxy::{self, FaultProxy};
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
    /// Phase 9: the task waits for a connection of the process's bounded
    /// pool and has sent nothing.
    #[serde(default)]
    pub pool_wait: bool,
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

/// Phase 9: what the server sees of an operation whose acknowledgment is
/// lost. The proxy realizes each fate on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Fate {
    /// The server never receives the operation: the client sent it, the
    /// proxy withheld it, and the connection ends (the server rolls the
    /// transaction back).
    NotReached,
    /// The server completes the operation at once; its answer is observed
    /// and withheld from the client.
    AnsweredAndWithheld,
    /// The server has not received it yet when the client dies; the proxy
    /// delivers it later (a [`PlanStep::ServerComplete`] step), after other
    /// work (a retry) has happened.
    HeldForLaterDelivery,
}

/// Phase 9: kill a worker with an operation in flight. The operation is
/// released (so it really goes onto the wire), the proxy applies the fate,
/// the coordinator verifies that it happened, kills the worker's process
/// group and observes PostgreSQL.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InFlightStep {
    pub actor: usize,
    pub transition: usize,
    /// The operation (a COMMIT, or an autocommit write), as the model's next
    /// operation of the actor.
    pub operation: Expected,
    pub fate: Fate,
    pub label: String,
}

/// Phase 9: the server completes an operation it held for a dead client.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCompleteStep {
    pub actor: usize,
    pub transition: usize,
    /// The execution (attempt) whose held operation completes.
    pub attempt: u32,
}

/// Phase 9: after the plan nothing can proceed: every task is waiting for a
/// connection of the process's pool, or for a lock the application itself
/// holds. The coordinator observes quiescence (nothing announced or
/// completed for a while) and the pool's or PostgreSQL's own counters.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StuckStep {
    pub transition: usize,
    /// `pool-starvation` or `application-wait-cycle`.
    pub kind: String,
}

/// Phase 9: what was observed at a stuck state.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StuckEvidence {
    pub kind: String,
    /// Nothing was announced or completed for this long (ms).
    pub quiet_ms: u64,
    /// Tasks that had not ended.
    pub unfinished: Vec<String>,
    /// The pool's own counters (waiting, idle, total) of a process with an
    /// unfinished task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<(i64, i64, i64)>,
    /// Backends waiting for a lock held by a backend of the same process.
    #[serde(default)]
    pub lock_waits: usize,
    pub confirmed: bool,
    pub detail: String,
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
    /// Phase 9: kill the worker with its COMMIT or autocommit write in
    /// flight.
    InFlight(InFlightStep),
    /// Phase 9: the server completes a held operation.
    ServerComplete(ServerCompleteStep),
    /// Phase 9: nothing can proceed any more (observed, then the process is
    /// killed to end the run).
    Stuck(StuckStep),
}

impl PlanStep {
    pub fn is_crash_or_restart(&self) -> bool {
        matches!(
            self,
            Self::Crash(_)
                | Self::Restart(_)
                | Self::InFlight(_)
                | Self::ServerComplete(_)
                | Self::Stuck(_)
        )
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
    /// Phase 9: actors with the same process name run as concurrent tasks of
    /// ONE worker process (shared memory, shared connection pool, one
    /// failure domain). `None`: a process of its own.
    pub process: Option<String>,
    /// Phase 9: the capacity of the process's connection pool in the model
    /// (the harness's pool must have the same `max`).
    pub pool: Option<u16>,
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

/// Phase 9: what the proxy and PostgreSQL showed for an operation killed in
/// flight. `observed` facts come from the proxy's frame-level view or from
/// PostgreSQL itself; nothing here is inferred from timing.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InFlightEvidence {
    pub operation: String,
    /// `not-reached`, `answered-and-withheld`, `held-for-later-delivery`.
    pub fate: String,
    /// How the fate was produced on the wire.
    pub mechanism: String,
    /// The proxy forwarded the operation to the server (observed).
    pub forwarded_to_server: bool,
    /// The proxy withheld the operation from the server (observed).
    pub withheld_from_server: bool,
    /// The server's own answer to the operation, as the proxy saw it (before
    /// withholding it from the client); empty if none was seen.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub server_answer: String,
    /// The client never received an answer (the proxy withheld it, or the
    /// operation never reached the server).
    pub client_acknowledged: bool,
    /// The backend's state in `pg_stat_activity` right before the kill
    /// (`idle in transaction`: the server has the open transaction and no
    /// COMMIT).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub backend_state_before_kill: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_pid: Option<i64>,
    /// The backend session was still open after the worker was killed (the
    /// server holds the operation for a later delivery).
    pub session_open_after_kill: bool,
    /// The redacted wire events of the operation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proxy_events: Vec<String>,
}

/// Phase 9: a held operation delivered to the server.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionEvidence {
    pub actor: usize,
    pub attempt: u32,
    pub plan: usize,
    pub transition: usize,
    /// The server's answer to the delivered operation.
    pub server_answer: String,
    /// The backend session ended afterwards.
    pub session_ended: bool,
    /// Rows of the modeled tables right after the completion.
    #[serde(default)]
    pub tables: BTreeMap<String, Vec<Value>>,
}

/// Phase 8: evidence that a planned crash happened as planned.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashEvidence {
    /// Phase 9: the operation that was in flight when the worker was killed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_flight: Option<InFlightEvidence>,
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
    /// Phase 9: held operations the server completed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completions: Vec<CompletionEvidence>,
    /// Phase 9: the stuck state observed at the end of the plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stuck: Option<StuckEvidence>,
    /// Phase 9: the proxy's redacted wire log (only with in-flight steps).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proxy_log: Vec<proxy::ProxyEvent>,
    /// Phase 9: events the proxy could not keep.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub proxy_log_truncated: bool,
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

/// One worker process: one execution (attempt) of one actor, or of the
/// tasks of one process (Phase 9).
struct Worker {
    /// The actors whose tasks run in this process.
    actors: Vec<usize>,
    attempt: u32,
    child: Child,
    pid: u32,
    stream: Option<UnixStream>,
    hello: bool,
    /// The process reported the end of its tasks.
    done: Option<Value>,
    stderr_path: PathBuf,
    /// Killed by a planned crash.
    killed: bool,
    reaped: Option<std::process::ExitStatus>,
}

/// What the coordinator knows about one actor's task (indexed by actor).
#[derive(Default)]
struct TaskState {
    /// The worker (slot) running the actor's current execution.
    slot: usize,
    /// Statements announced and not yet released (in order).
    held: Vec<usize>,
    /// Released and not completed.
    running: Option<usize>,
    /// The task reported its end.
    done: Option<Value>,
}

/// A planned crash or in-flight kill whose checkpoint was observed and whose
/// fault is in place, waiting for the process to be killed (Phase 9).
enum Prepared {
    Crash {
        k: usize,
        c: CrashStep,
        checkpoint: String,
        never_sent: Option<String>,
    },
    InFlight {
        k: usize,
        s: InFlightStep,
        fault: proxy::Fault,
        conn: u64,
        backend_pid: Option<i64>,
        backend_state: String,
        server_answer: String,
        wire: Vec<String>,
    },
}

impl Prepared {
    fn actor(&self) -> usize {
        match self {
            Self::Crash { c, .. } => c.actor,
            Self::InFlight { s, .. } => s.actor,
        }
    }

    fn k(&self) -> usize {
        match self {
            Self::Crash { k, .. } | Self::InFlight { k, .. } => *k,
        }
    }
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
    /// Per actor: its current execution's worker, held statements, ...
    tasks: Vec<TaskState>,
    /// Phase 9: the pool the harness reported per worker slot.
    pool_max: BTreeMap<usize, Option<i64>>,
    /// Phase 9: the last pool counters a worker reported (answer to a probe).
    pool_probe: Option<(usize, Value)>,
    rx: Receiver<Message>,
    statements: Vec<Observed>,
    effects: Vec<EffectRecord>,
    crashes: Vec<CrashEvidence>,
    restarts: Vec<RestartEvidence>,
    completions: Vec<CompletionEvidence>,
    stuck: Option<StuckEvidence>,
    /// Phase 9: the fault proxy (only when the plan has in-flight steps).
    proxy: Option<FaultProxy>,
    /// Phase 9: (actor, attempt) → proxy connection holding its operation.
    held_ops: BTreeMap<(usize, u32), u64>,
    released: usize,
    problems: Vec<String>,
    /// Statements a driver issued for itself (not held; reported).
    driver_internal: Vec<String>,
    diverged: bool,
    inconclusive: bool,
    deadline: Instant,
}

impl Coordinator<'_> {
    /// The actors that share a process with `actor` (itself included).
    fn group_of(&self, actor: usize) -> Vec<usize> {
        match &self.req.actors[actor].process {
            Some(p) => (0..self.req.actors.len())
                .filter(|i| self.req.actors[*i].process.as_ref() == Some(p))
                .collect(),
            None => vec![actor],
        }
    }

    /// The `application_name` base of an actor's sessions: its process (when
    /// it shares one), else its own name.
    fn app_name(&self, actor: usize, attempt: u32) -> String {
        let a = &self.req.actors[actor];
        application_name(a.process.as_deref().unwrap_or(&a.name), attempt)
    }

    /// Start one execution of `actors` (one actor, or the tasks of one
    /// process); returns its slot.
    fn spawn(&mut self, actors: &[usize], attempt: u32) -> Result<usize, String> {
        let a = &self.req.actors[actors[0]];
        let shared = actors.len() > 1;
        let base = a
            .process
            .as_deref()
            .filter(|_| shared)
            .unwrap_or(&a.name)
            .to_owned();
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
            .env("COMMITRIFT_SOURCE_ACTOR", &base)
            .env("COMMITRIFT_SOURCE_ENTRY", &a.entry)
            .env("COMMITRIFT_SOURCE_INPUT", a.input.to_string())
            .env("COMMITRIFT_SOURCE_ATTEMPT", attempt.to_string())
            .env("COMMITRIFT_RUN_TOKEN", &l.token)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .process_group(0);
        if shared {
            let tasks: Vec<Value> = actors
                .iter()
                .map(|i| {
                    let t = &self.req.actors[*i];
                    json!({"actor": t.name, "entry": t.entry, "input": t.input})
                })
                .collect();
            cmd.env("COMMITRIFT_SOURCE_TASKS", Value::Array(tasks).to_string());
        }
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
                "worker `{base}` (attempt {attempt}) did not connect: {}",
                safety::redact(text.trim(), &l.root)
            ));
        };
        let _ = stream.set_nonblocking(false);
        let read_half = stream.try_clone().map_err(|e| format!("socket: {e}"))?;
        reader(slot, read_half, l.tx.clone());
        let pid = child.id();
        self.workers.push(Worker {
            actors: actors.to_vec(),
            attempt,
            child,
            pid,
            stream: Some(stream),
            hello: false,
            done: None,
            stderr_path: stderr,
            killed: false,
            reaped: None,
        });
        for &actor in actors {
            self.tasks[actor] = TaskState {
                slot,
                ..TaskState::default()
            };
        }
        Ok(slot)
    }

    fn slot(&self, actor: usize) -> usize {
        self.tasks[actor].slot
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
            Message::Line(w, v) => {
                // The task a message belongs to: named by the harness for a
                // process that runs several, else the process's only actor.
                let a = v["task"]
                    .as_str()
                    .and_then(|t| {
                        self.workers[w]
                            .actors
                            .iter()
                            .copied()
                            .find(|x| self.req.actors[*x].name == t)
                    })
                    .unwrap_or(self.workers[w].actors[0]);
                match v["type"].as_str().unwrap_or_default() {
                    "hello" => {
                        self.workers[w].hello = true;
                        self.pool_max.insert(w, v["pool"]["max"].as_i64());
                    }
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
                        let attempt = self.workers[w].attempt;
                        self.statements.push(Observed {
                            actor: a,
                            attempt,
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
                        self.tasks[a].held.push(index);
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
                                                    x["name"]
                                                        .as_str()
                                                        .unwrap_or_default()
                                                        .to_owned(),
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
                        if let Some(r) = self.tasks[a].running
                            && self.statements[r].seq == seq
                        {
                            self.tasks[a].running = None;
                        }
                    }
                    "unsupported" => {
                        let name = &self.req.actors[a].name;
                        self.problems.push(format!(
                            "{name}: {}",
                            v["reason"].as_str().unwrap_or("unsupported query form")
                        ));
                        self.inconclusive = true;
                    }
                    "done" => {
                        if v["task"].is_string() {
                            self.tasks[a].done = Some(v);
                        } else {
                            for x in self.workers[w].actors.clone() {
                                if self.tasks[x].done.is_none() {
                                    self.tasks[x].done = Some(v.clone());
                                }
                            }
                            self.workers[w].done = Some(v);
                        }
                    }
                    "pool" => {
                        self.pool_probe = Some((w, v));
                    }
                    "driver-internal" => {
                        let sql = short(v["sql"].as_str().unwrap_or_default());
                        let name = &self.req.actors[a].name;
                        self.driver_internal.push(format!("{name}: {sql}"));
                    }
                    _ => {}
                }
            }
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

    /// Whether the task of `actor` has ended: it reported its end, or its
    /// process is gone.
    fn task_finished(&self, actor: usize) -> bool {
        self.tasks[actor].done.is_some() || self.finished(self.slot(actor))
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
        let a = self.statements[index].actor;
        self.tasks[a].held.retain(|i| *i != index);
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
        self.tasks[a].running = Some(index);
        let seq = self.statements[index].seq;
        let message = match effect {
            Some(receipt) => json!({"type": "go", "seq": seq, "effect": receipt}),
            None => json!({"type": "go", "seq": seq}),
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
        for a in 0..self.tasks.len() {
            let w = self.slot(a);
            if self.tasks[a].running.is_some() || self.workers[w].killed {
                continue;
            }
            let Some(&index) = self.tasks[a].held.first() else {
                continue;
            };
            // A driver that sets the isolation level in a statement of its
            // own (postgres-js: `begin`, then `set transaction ...`): the
            // statement belongs to the BEGIN the plan released just before.
            if is_transaction_config(&self.statements[index].sql)
                && let Some(previous) = self
                    .statements
                    .iter()
                    .rposition(|s| s.actor == a && s.slot == w && s.released.is_some())
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
    fn settle(&mut self, _w: usize, index: usize) -> Option<bool> {
        let started = Instant::now();
        loop {
            if self.statements[index].completed {
                return Some(false);
            }
            if self.task_finished(self.statements[index].actor) {
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
        let name = self.app_name(s.actor, s.attempt);
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
            if let Some(&i) = self.tasks[a].held.first()
                && self.tasks[a].running.is_none()
                && self.in_scope(a, &self.statements[i].frames) != Some(false)
            {
                return Some(i);
            }
            if self.task_finished(a) && self.tasks[a].held.is_empty() {
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
        let name = self.app_name(actor, attempt);
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

    /// The pool's own counters, from the worker that owns it: (waiting,
    /// idle, total). `None` when the worker does not answer in time.
    fn probe_pool(&mut self, w: usize) -> Option<(i64, i64, i64)> {
        self.pool_probe = None;
        let stream = self.workers[w].stream.as_mut()?;
        send(stream, &json!({"type": "pool?"})).ok()?;
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if let Some((pw, v)) = &self.pool_probe
                && *pw == w
            {
                return Some((
                    v["waiting"].as_i64()?,
                    v["idle"].as_i64()?,
                    v["total"].as_i64()?,
                ));
            }
            self.pump(Duration::from_millis(10));
        }
        None
    }

    /// Phase 8/9: the checkpoint of a planned crash (an announced and held
    /// operation, a statement waiting for a lock, or a task waiting for a
    /// connection of the pool), observed before anything is killed.
    fn crash_prepare(&mut self, k: usize, c: &CrashStep) -> Option<Prepared> {
        let a = c.actor;
        let name = self.req.actors[a].name.clone();
        // 1. The checkpoint.
        let (checkpoint, never_sent) = if c.pool_wait {
            // The task has not sent anything: it waits for a connection. The
            // pool's own counters say so, and nothing of the task is held.
            let w = self.slot(a);
            self.release_outside();
            let silent = self.tasks[a].held.is_empty() && self.tasks[a].running.is_none();
            match self.probe_pool(w) {
                Some((waiting, idle, total)) if silent && waiting >= 1 && idle == 0 => (
                    format!(
                        "while waiting for a connection of the pool (the pool reports {waiting} waiting, {idle} idle of {total}); the task has sent nothing"
                    ),
                    None,
                ),
                other => {
                    self.problems.push(format!(
                        "plan step {k}: the model crashes {name} while it waits for a connection of the pool, but the pool reports {other:?} and the task is {}: the crash point is not reproducible",
                        if silent { "silent" } else { "not silent" }
                    ));
                    self.inconclusive = true;
                    return None;
                }
            }
        } else if c.waiting {
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
                return None;
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
                    return None;
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
                return None;
            };
            let index = self.next_held(k, a, &next.label)?;
            if !self.matches(index, next) {
                self.mismatch(k, index, next, "at the crash point, where");
                return None;
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
        Some(Prepared::Crash {
            k,
            c: c.clone(),
            checkpoint,
            never_sent,
        })
    }

    /// Kill a worker's process group and reap it. Returns the pid, the exit
    /// status and whether it was terminated by SIGKILL.
    fn kill_worker(&mut self, w: usize) -> (u32, Option<std::process::ExitStatus>, bool) {
        let pid = self.workers[w].pid;
        kill_group(&mut self.workers[w].child);
        let status = self.workers[w].child.wait().ok();
        self.workers[w].reaped = status;
        self.workers[w].killed = true;
        self.workers[w].stream = None;
        // Every task of the process ends with it: what they had announced
        // was never sent.
        for a in self.workers[w].actors.clone() {
            let held: Vec<usize> = std::mem::take(&mut self.tasks[a].held);
            for i in held {
                self.statements[i].role = "never-sent".to_owned();
            }
            self.tasks[a].running = None;
        }
        use std::os::unix::process::ExitStatusExt;
        let killed_by_signal = status.and_then(|s| s.signal()) == Some(SIGKILL);
        (pid, status, killed_by_signal)
    }

    /// The state of one backend in `pg_stat_activity` (`gone` if it is not
    /// there).
    fn backend_state(&self, pid: i64) -> String {
        match self.cluster.query_value(
            "postgres",
            &format!("SELECT coalesce(state, 'gone') FROM pg_stat_activity WHERE pid = {pid}"),
        ) {
            Ok(Value::String(s)) => s,
            Ok(Value::Null) => "gone".to_owned(),
            Ok(other) => other.to_string(),
            Err(e) => format!("unreadable ({e})"),
        }
    }

    fn wait_sessions_ended(&self, actor: usize, attempt: u32) -> (bool, u64) {
        let started = Instant::now();
        let until = (Instant::now() + Duration::from_secs(10)).min(self.deadline);
        loop {
            match self.sessions(actor, attempt) {
                Ok(left) if left.is_empty() => {
                    return (true, started.elapsed().as_millis() as u64);
                }
                Ok(_) if Instant::now() < until => std::thread::sleep(Duration::from_millis(10)),
                _ => return (false, started.elapsed().as_millis() as u64),
            }
        }
    }

    /// Phase 9: kill a worker with an operation in flight. The operation is
    /// released for real; the proxy applies the fate on the wire and the
    /// coordinator waits for the proxy to report that it did (never a delay
    /// standing in for proof), reads PostgreSQL's view, then kills the worker
    /// group and observes what is durable.
    fn in_flight_prepare(&mut self, k: usize, s: &InFlightStep) -> Option<Prepared> {
        let a = s.actor;
        let name = self.req.actors[a].name.clone();
        let index = self.next_held(k, a, &s.operation.label)?;
        if !self.matches(index, &s.operation) {
            self.mismatch(k, index, &s.operation, "at the in-flight point, where");
            return None;
        }
        let w = self.slot(a);
        let attempt = self.workers[w].attempt;
        let app = self.app_name(a, attempt);
        let target = match s.operation.kind {
            SqlKind::Commit => proxy::Target::Commit,
            SqlKind::Insert | SqlKind::Update | SqlKind::Delete => proxy::Target::Statement,
            other => {
                self.problems.push(format!(
                    "plan step {k}: an in-flight {other:?} cannot be injected (only a COMMIT or an autocommit write)"
                ));
                self.inconclusive = true;
                return None;
            }
        };
        let fault = match s.fate {
            Fate::AnsweredAndWithheld => proxy::Fault::DropResponse,
            Fate::NotReached | Fate::HeldForLaterDelivery => proxy::Fault::HoldRequest,
        };
        let Some(px) = self.proxy.as_ref() else {
            self.problems.push(format!(
                "plan step {k}: no fault proxy is running for this in-flight step"
            ));
            self.inconclusive = true;
            return None;
        };
        // The connection of the statement when the driver reports it (tasks of
        // one process share an `application_name`), else the execution's name.
        match self.statements[index].backend_pid {
            Some(pid) => px.arm_backend(pid, fault, target),
            None => px.arm(&app, fault, target),
        }
        let cursor = px.cursor();
        if !self.release(index, "planned", Some(k)) {
            self.problems.push(format!("{name}: the worker went away"));
            self.inconclusive = true;
            return None;
        }
        // The wire event that proves the fault took effect.
        let want = match fault {
            proxy::Fault::HoldRequest => proxy::EventKind::OperationHeld,
            proxy::Fault::DropResponse => proxy::EventKind::OperationCompleted,
        };
        let limit = self
            .deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(15));
        let px = self.proxy.as_ref().expect("checked");
        let Some(event) = px.wait(cursor, limit, |e| e.kind == want) else {
            self.problems.push(format!(
                "plan step {k}: the proxy did not observe `{}` on the wire: the in-flight point was not established (no delay is taken as proof)",
                s.label
            ));
            self.inconclusive = true;
            return None;
        };
        let conn = event.conn;
        if let Some(why) = px.unsupported(conn) {
            self.problems.push(format!(
                "plan step {k}: the proxy cannot inject faults on this connection: {why}"
            ));
            self.inconclusive = true;
            return None;
        }
        let backend_pid = px.backend_pid(conn);
        let server_answer = if fault == proxy::Fault::DropResponse {
            event.detail.clone()
        } else {
            String::new()
        };
        let backend_state = backend_pid
            .map(|p| self.backend_state(p))
            .unwrap_or_default();
        let wire: Vec<String> = self
            .proxy
            .as_ref()
            .expect("checked")
            .events()
            .into_iter()
            .filter(|e| e.conn == conn && e.seq >= cursor.saturating_sub(1))
            .filter(|e| {
                !matches!(
                    e.kind,
                    proxy::EventKind::Message | proxy::EventKind::BackendKey
                )
            })
            .map(|e| format!("{:?}: {}", e.kind, e.detail))
            .collect();
        Some(Prepared::InFlight {
            k,
            s: s.clone(),
            fault,
            conn,
            backend_pid,
            backend_state,
            server_answer,
            wire,
        })
    }

    /// Kill one process with everything that is in flight or waiting in it
    /// at once (Phase 9: a process crash ends every task of the process): the
    /// checkpoints and faults of all the plan steps `ks` (consecutive crash
    /// and in-flight steps of one process) are established first, then the
    /// process group is killed once, then PostgreSQL's state is observed.
    fn crash_group(&mut self, ks: &[usize]) {
        let plan = self.req.plan;
        let mut prepared = Vec::new();
        for &k in ks {
            let p = match &plan[k] {
                PlanStep::Crash(c) => self.crash_prepare(k, c),
                PlanStep::InFlight(s) => self.in_flight_prepare(k, s),
                _ => None,
            };
            match p {
                Some(p) => prepared.push(p),
                None => return,
            }
        }
        let first = prepared[0].actor();
        let w = self.slot(first);
        let attempt = self.workers[w].attempt;
        let name = self.req.actors[first].name.clone();
        let k0 = prepared[0].k();
        let backends = match self.sessions(first, attempt) {
            Ok(b) => b,
            Err(e) => {
                self.problems.push(format!(
                    "plan step {k0}: cannot list {name}'s sessions: {e}"
                ));
                self.inconclusive = true;
                return;
            }
        };
        let (pid, status, killed_by_signal) = self.kill_worker(w);
        if !killed_by_signal {
            self.problems.push(format!(
                "plan step {k0}: {name}'s worker was not terminated by SIGKILL ({status:?})"
            ));
            self.inconclusive = true;
        }
        // The server connections the proxy keeps for a later delivery: the
        // backends of those operations stay.
        let mut kept: Vec<i64> = Vec::new();
        for p in &prepared {
            if let Prepared::InFlight {
                s,
                conn,
                backend_pid,
                ..
            } = p
            {
                match s.fate {
                    Fate::NotReached => {
                        if let Some(px) = &self.proxy {
                            px.discard(*conn);
                        }
                    }
                    Fate::AnsweredAndWithheld => {}
                    Fate::HeldForLaterDelivery => kept.extend(backend_pid.iter().copied()),
                }
            }
        }
        // PostgreSQL notices the lost connections and ends the sessions.
        let started = Instant::now();
        let until = (Instant::now() + Duration::from_secs(10)).min(self.deadline);
        let sessions_ended = loop {
            match self.sessions(first, attempt) {
                Ok(left) if left.iter().all(|p| kept.contains(p)) => break true,
                Ok(_) if Instant::now() < until => std::thread::sleep(Duration::from_millis(10)),
                _ => break false,
            }
        };
        let sessions_ended_ms = started.elapsed().as_millis() as u64;
        if !sessions_ended {
            self.problems.push(format!(
                "plan step {k0}: PostgreSQL still had sessions of the killed {name} (attempt {attempt}) after {sessions_ended_ms} ms: the transaction's fate is not established"
            ));
            self.inconclusive = true;
        }
        let tables = self.read_tables();
        for p in prepared {
            match p {
                Prepared::Crash {
                    k,
                    c,
                    checkpoint,
                    never_sent,
                } => {
                    self.crashes.push(CrashEvidence {
                        in_flight: None,
                        actor: c.actor,
                        attempt,
                        plan: k,
                        transition: c.transition,
                        pid,
                        checkpoint,
                        never_sent,
                        killed_by_signal,
                        exit: status.map(|st| format!("{st}")),
                        backends: backends.clone(),
                        sessions_ended,
                        sessions_ended_ms,
                        tables: tables.clone(),
                    });
                }
                Prepared::InFlight {
                    k,
                    s,
                    fault,
                    conn,
                    backend_pid,
                    backend_state,
                    server_answer,
                    wire,
                } => {
                    let a = s.actor;
                    let (ended, ms, open) = match s.fate {
                        Fate::NotReached | Fate::AnsweredAndWithheld => {
                            (sessions_ended, sessions_ended_ms, !sessions_ended)
                        }
                        Fate::HeldForLaterDelivery => {
                            // The backend must still be there, holding its
                            // transaction.
                            std::thread::sleep(Duration::from_millis(100));
                            let open = backend_pid
                                .map(|p| self.backend_state(p) == "idle in transaction")
                                .unwrap_or(false);
                            if !open {
                                self.problems.push(format!(
                                    "plan step {k}: the held operation's backend is no longer waiting in its transaction"
                                ));
                                self.inconclusive = true;
                            }
                            self.held_ops.insert((a, attempt), conn);
                            (false, 0, open)
                        }
                    };
                    let evidence = InFlightEvidence {
                        operation: s.operation.label.clone(),
                        fate: match s.fate {
                            Fate::NotReached => "not-reached",
                            Fate::AnsweredAndWithheld => "answered-and-withheld",
                            Fate::HeldForLaterDelivery => "held-for-later-delivery",
                        }
                        .to_owned(),
                        mechanism: match s.fate {
                            Fate::NotReached => "the proxy withheld the operation from the server, then closed the server connection".to_owned(),
                            Fate::AnsweredAndWithheld => "the proxy forwarded the operation; the server answered; the proxy withheld the answer from the client".to_owned(),
                            Fate::HeldForLaterDelivery => "the proxy withheld the operation from the server and kept the server connection open for a later delivery".to_owned(),
                        },
                        forwarded_to_server: fault == proxy::Fault::DropResponse,
                        withheld_from_server: fault == proxy::Fault::HoldRequest,
                        server_answer,
                        client_acknowledged: false,
                        backend_state_before_kill: backend_state,
                        backend_pid,
                        session_open_after_kill: open,
                        proxy_events: wire,
                    };
                    self.crashes.push(CrashEvidence {
                        in_flight: Some(evidence),
                        actor: a,
                        attempt,
                        plan: k,
                        transition: s.transition,
                        pid,
                        checkpoint: format!(
                            "with {} in flight: sent by the application, never acknowledged to it",
                            s.operation.label
                        ),
                        never_sent: None,
                        killed_by_signal,
                        exit: status.map(|st| format!("{st}")),
                        backends: backend_pid.into_iter().collect(),
                        sessions_ended: ended,
                        sessions_ended_ms: ms,
                        tables: tables.clone(),
                    });
                }
            }
        }
    }

    /// Phase 9: deliver the operation the proxy held for a dead client; the
    /// server completes it now.
    fn server_complete_step(&mut self, k: usize, s: &ServerCompleteStep) {
        let a = s.actor;
        let name = self.req.actors[a].name.clone();
        let Some(conn) = self.held_ops.remove(&(a, s.attempt)) else {
            self.problems.push(format!(
                "plan step {k}: no operation of {name} (attempt {}) is held on the server",
                s.attempt
            ));
            self.diverged = true;
            return;
        };
        let Some(px) = self.proxy.as_ref() else {
            self.inconclusive = true;
            return;
        };
        let cursor = px.cursor();
        if let Err(e) = px.deliver(conn) {
            self.problems.push(format!("plan step {k}: {e}"));
            self.inconclusive = true;
            return;
        }
        let limit = self
            .deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(15));
        let Some(done) = px.wait(cursor, limit, |e| {
            e.kind == proxy::EventKind::OperationCompleted && e.conn == conn
        }) else {
            self.problems.push(format!(
                "plan step {k}: the server did not answer the delivered operation"
            ));
            self.inconclusive = true;
            return;
        };
        let (session_ended, _) = self.wait_sessions_ended(a, s.attempt);
        let tables = self.read_tables();
        self.completions.push(CompletionEvidence {
            actor: a,
            attempt: s.attempt,
            plan: k,
            transition: s.transition,
            server_answer: done.detail,
            session_ended,
            tables,
        });
    }

    /// The harness's pools have the capacity the model declares (Phase 9).
    fn check_pools(&mut self) {
        let until = (Instant::now() + Duration::from_secs(10)).min(self.deadline);
        while self.workers.iter().any(|w| !w.hello) && Instant::now() < until {
            self.pump(Duration::from_millis(10));
        }
        for w in 0..self.workers.len() {
            for a in self.workers[w].actors.clone() {
                let Some(cap) = self.req.actors[a].pool else {
                    continue;
                };
                let max = self.pool_max.get(&w).copied().flatten();
                if max != Some(i64::from(cap)) {
                    self.problems.push(format!(
                        "the scenario declares a connection pool of {cap} for {}, but the harness's pool reports max {max:?}: the model and the run would not be about the same system",
                        self.req.actors[a].name
                    ));
                    self.inconclusive = true;
                }
            }
        }
    }

    /// Phase 9: observe that nothing can proceed.
    fn stuck_step(&mut self, k: usize, s: &StuckStep) {
        let quiet = Duration::from_millis(800).max(self.req.settle * 4);
        let started = Instant::now();
        let announced = self.statements.len();
        let completed = self.statements.iter().filter(|x| x.completed).count();
        while started.elapsed() < quiet && Instant::now() < self.deadline {
            self.release_outside();
            self.pump(Duration::from_millis(10));
        }
        let quiet_ms = started.elapsed().as_millis() as u64;
        let unfinished: Vec<String> = (0..self.tasks.len())
            .filter(|a| !self.task_finished(*a))
            .map(|a| self.req.actors[a].name.clone())
            .collect();
        let moved = self.statements.len() != announced
            || self.statements.iter().filter(|x| x.completed).count() != completed;
        let mut evidence = StuckEvidence {
            kind: s.kind.clone(),
            quiet_ms,
            unfinished: unfinished.clone(),
            ..StuckEvidence::default()
        };
        if unfinished.is_empty() || moved {
            evidence.detail = if unfinished.is_empty() {
                "every task ended: the system was not stuck".to_owned()
            } else {
                "a task announced or completed a statement while the run waited: it was not stuck"
                    .to_owned()
            };
            self.problems
                .push(format!("plan step {k}: {}", evidence.detail));
            self.diverged = true;
            self.stuck = Some(evidence);
            return;
        }
        match s.kind.as_str() {
            "pool-starvation" => {
                let w = self.slot(
                    self.tasks
                        .iter()
                        .position(|t| t.done.is_none())
                        .unwrap_or(0),
                );
                let probe = self.probe_pool(w);
                evidence.pool = probe;
                evidence.confirmed =
                    probe.is_some_and(|(waiting, idle, _)| waiting >= 1 && idle == 0);
                evidence.detail = match probe {
                    Some((waiting, idle, total)) => format!(
                        "no statement was announced or completed for {quiet_ms} ms; the pool reports {waiting} waiting, {idle} idle of {total}; {} task(s) had not ended",
                        unfinished.len()
                    ),
                    None => "the pool did not answer".to_owned(),
                };
            }
            "application-wait-cycle" => {
                // A backend waits for a lock whose holder is a backend of the
                // same process.
                let base = self.app_name(
                    self.tasks
                        .iter()
                        .position(|t| t.done.is_none())
                        .unwrap_or(0),
                    1,
                );
                let base = base
                    .rsplit_once('#')
                    .map_or(base.clone(), |(b, _)| b.to_owned());
                let sql = format!(
                    "SELECT count(*) FROM pg_stat_activity w WHERE w.application_name LIKE {} AND w.wait_event_type = 'Lock' AND EXISTS (SELECT 1 FROM pg_stat_activity h WHERE h.pid = ANY(pg_blocking_pids(w.pid)) AND h.application_name = w.application_name)",
                    sql_text(&format!("{base}#%"))
                );
                let n = match self.cluster.query_value("postgres", &sql) {
                    Ok(Value::Number(n)) => n.as_u64().unwrap_or(0) as usize,
                    _ => 0,
                };
                evidence.lock_waits = n;
                evidence.confirmed = n >= 1;
                evidence.detail = format!(
                    "no statement was announced or completed for {quiet_ms} ms; {n} backend(s) wait for a lock held by a backend of the same process; PostgreSQL detects no deadlock (it sees no cycle)"
                );
            }
            other => {
                evidence.detail = format!("unknown stuck kind `{other}`");
            }
        }
        if !evidence.confirmed {
            self.problems.push(format!(
                "plan step {k}: the stuck state was not confirmed: {}",
                evidence.detail
            ));
            self.inconclusive = true;
        }
        self.stuck = Some(evidence);
    }

    /// A declared recovery mechanism starts a new execution: a new process
    /// with fresh connections (nothing of the previous one is reused).
    fn restart_step(&mut self, k: usize, r: &RestartStep) {
        let a = r.actor;
        let previous = self.slot(a);
        // The previous execution must have ended (the model retries only
        // after every task of the attempt ended).
        while !self.task_finished(a) {
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
            self.tasks[a]
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
        if self.workers[previous].actors.len() > 1 {
            self.problems.push(format!(
                "plan step {k}: {} shares its process with other tasks: a restart starts a new process, not one task of the old one",
                self.req.actors[a].name
            ));
            self.inconclusive = true;
            return;
        }
        match self.spawn(&[a], r.attempt) {
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
        let mut consumed: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
        for (k, step) in plan.iter().enumerate() {
            if self.diverged || self.inconclusive {
                return;
            }
            match step {
                PlanStep::Release(e) => self.release_step(k, e),
                PlanStep::Complete { actor, transition } => {
                    let a = *actor;
                    let Some(index) = self.tasks[a].running else {
                        self.problems.push(format!(
                            "plan step {k} (transition {transition}): {} has no waiting statement to resume",
                            self.req.actors[a].name
                        ));
                        self.diverged = true;
                        return;
                    };
                    loop {
                        if self.statements[index].completed || self.task_finished(a) {
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
                PlanStep::Crash(_) | PlanStep::InFlight(_) => {
                    if consumed.contains(&k) {
                        continue;
                    }
                    // Consecutive crash and in-flight steps of actors that
                    // share a process are one kill.
                    let actor = |step: &PlanStep| match step {
                        PlanStep::Crash(c) => Some(c.actor),
                        PlanStep::InFlight(i) => Some(i.actor),
                        _ => None,
                    };
                    let w = self.slot(actor(step).expect("crash step"));
                    let mut ks = vec![k];
                    for (j, next) in plan.iter().enumerate().skip(k + 1) {
                        match actor(next) {
                            Some(x) if self.slot(x) == w => ks.push(j),
                            _ => break,
                        }
                    }
                    consumed.extend(ks.iter().copied());
                    self.crash_group(&ks);
                }
                PlanStep::Restart(r) => self.restart_step(k, r),
                PlanStep::Stuck(st) => self.stuck_step(k, st),
                PlanStep::ServerComplete(c) => self.server_complete_step(k, c),
            }
        }
    }

    /// After the plan: let every worker finish, releasing whatever it
    /// still sends (recorded as `beyond-plan`, which the comparison treats
    /// as unexpected if the model's actor had finished).
    fn drain(&mut self) {
        if self.stuck.is_some() {
            // Nothing can finish: end the run.
            self.kill_all();
            return;
        }
        loop {
            self.release_outside();
            let mut progressed = false;
            for a in 0..self.tasks.len() {
                let w = self.slot(a);
                if self.tasks[a].running.is_some() || self.workers[w].killed {
                    continue;
                }
                if let Some(&index) = self.tasks[a].held.first() {
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
    if !plan
        .iter()
        .any(|s| matches!(s, PlanStep::Crash(_) | PlanStep::InFlight(_)))
    {
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

    // Phase 9: workers reach the database through the fault proxy when the
    // plan has an in-flight step; the proxy is the only thing that talks to
    // the cluster on their behalf.
    let mut worker_socket = cluster.socket_path();
    let proxy = if req.plan.iter().any(|s| matches!(s, PlanStep::InFlight(_))) {
        let name = worker_socket
            .file_name()
            .map(std::ffi::OsStr::to_os_string)
            .unwrap_or_default();
        let listen = root.path.join("px").join(name);
        match FaultProxy::start(&root.path, &listen, &cluster.socket_path()) {
            Ok(p) => {
                worker_socket = listen;
                Some(p)
            }
            Err(e) => return blocked(format!("the fault proxy could not start: {e}")),
        }
    } else {
        None
    };
    if let Err(e) = safety::check_database_target(&root.path, &worker_socket) {
        return blocked(e);
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
            worker_socket.display().to_string(),
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
        tasks: (0..req.actors.len())
            .map(|_| TaskState::default())
            .collect(),
        pool_max: BTreeMap::new(),
        pool_probe: None,
        rx,
        statements: Vec::new(),
        effects: Vec::new(),
        crashes: Vec::new(),
        restarts: Vec::new(),
        completions: Vec::new(),
        stuck: None,
        proxy,
        held_ops: BTreeMap::new(),
        released: 0,
        problems: Vec::new(),
        driver_internal: Vec::new(),
        diverged: false,
        inconclusive: false,
        deadline: Instant::now() + Duration::from_millis(manifest.timeout_ms),
    };
    let mut launched = vec![false; req.actors.len()];
    for i in 0..req.actors.len() {
        if launched[i] {
            continue;
        }
        let group = co.group_of(i);
        for g in &group {
            launched[*g] = true;
        }
        if let Err(e) = co.spawn(&group, 1) {
            co.kill_all();
            return blocked(e);
        }
    }
    co.check_pools();
    co.run_plan();
    // After the plan (or a divergence): let the workers finish.
    co.drain();
    // Reap.
    let mut attempts: Vec<ActorResult> = Vec::new();
    let mut last: Vec<usize> = vec![usize::MAX; req.actors.len()];
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
        for &a in &w.actors {
            let done = co.tasks[a].done.as_ref().or(w.done.as_ref());
            let (outcome, value, error) = match (done, w.killed) {
                (_, true) => ("killed".to_owned(), Value::Null, None),
                (Some(d), false) => (
                    d["outcome"].as_str().unwrap_or("unknown").to_owned(),
                    d["value"].clone(),
                    d["error"].as_str().map(str::to_owned),
                ),
                (None, false) => ("killed".to_owned(), Value::Null, None),
            };
            last[a] = attempts.len();
            attempts.push(ActorResult {
                name: req.actors[a].name.clone(),
                entry: req.actors[a].entry.clone(),
                outcome,
                value,
                error,
                stderr: stderr.clone(),
                attempt: w.attempt,
                pid: w.pid,
            });
        }
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
    let actors: Vec<ActorResult> = last.iter().map(|&i| attempts[i].clone()).collect();
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
        completions: std::mem::take(&mut co.completions),
        stuck: co.stuck.take(),
        proxy_log: co
            .proxy
            .as_ref()
            .map(FaultProxy::events)
            .unwrap_or_default(),
        proxy_log_truncated: co.proxy.as_ref().is_some_and(|p| p.dropped_events() > 0),
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

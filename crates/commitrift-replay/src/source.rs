//! Coordinated execution of original application code (Phase 7).
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    /// The source region of the model operation; the statement's innermost
    /// application frame must lie in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
    /// The model predicts the statement waits for a lock.
    pub expect_block: bool,
    /// Index of the model transition (for the report).
    pub transition: usize,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "step")]
pub enum PlanStep {
    /// Release the next statement of `actor`, which must match.
    Release(Expected),
    /// A statement of `actor` released earlier and still waiting must
    /// complete now (the model resumes it).
    Complete { actor: usize, transition: usize },
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

/// What became of one statement.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observed {
    pub actor: usize,
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
    /// `planned` (matched plan step `plan`), `outside-model`,
    /// `unexpected` or `beyond-plan`.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sqlstate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub completed: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorResult {
    pub name: String,
    pub entry: String,
    /// `returned`, `threw`, `killed`, `not-started`.
    pub outcome: String,
    #[serde(default)]
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr: String,
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
    pub actors: Vec<ActorResult>,
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
}

// ---------------------------------------------------------------------------
// Workers
// ---------------------------------------------------------------------------

struct Worker {
    child: Child,
    stream: Option<UnixStream>,
    hello: bool,
    done: Option<Value>,
    /// Statements announced and not yet released (in order).
    held: Vec<usize>,
    /// Released and not completed.
    running: Option<usize>,
    stderr_path: PathBuf,
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

fn send(stream: &mut UnixStream, message: &Value) -> std::io::Result<()> {
    let mut line = message.to_string();
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()
}

fn reader(index: usize, stream: UnixStream, tx: Sender<Message>) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(Message::Closed(index));
                    return;
                }
                Ok(_) => match serde_json::from_str::<Value>(line.trim()) {
                    Ok(v) => {
                        if tx.send(Message::Line(index, v)).is_err() {
                            return;
                        }
                    }
                    Err(_) => {
                        let _ = tx.send(Message::Closed(index));
                        return;
                    }
                },
            }
        }
    });
}

struct Coordinator<'r> {
    req: &'r RunRequest<'r>,
    cluster: &'r Cluster,
    workers: Vec<Worker>,
    rx: Receiver<Message>,
    statements: Vec<Observed>,
    released: usize,
    problems: Vec<String>,
    diverged: bool,
    inconclusive: bool,
    deadline: Instant,
}

impl Coordinator<'_> {
    fn in_scope(&self, actor: usize, frames: &[Frame]) -> Option<bool> {
        let scope = self.req.actors[actor].scope.as_ref()?;
        if frames.is_empty() {
            return None;
        }
        Some(frames.iter().any(|f| scope.contains(f)))
    }

    fn handle(&mut self, message: Message) {
        match message {
            Message::Closed(a) => {
                self.workers[a].stream = None;
            }
            Message::Line(a, v) => match v["type"].as_str().unwrap_or_default() {
                "hello" => self.workers[a].hello = true,
                "query" => {
                    let sql = v["sql"].as_str().unwrap_or_default().to_owned();
                    let (kind, table) = classify(&sql);
                    let frames: Vec<Frame> = v["site"]["frames"]
                        .as_array()
                        .map(|f| {
                            f.iter()
                                .filter_map(|x| serde_json::from_value(x.clone()).ok())
                                .collect()
                        })
                        .unwrap_or_default();
                    let index = self.statements.len();
                    self.statements.push(Observed {
                        actor: a,
                        seq: v["seq"].as_u64().unwrap_or_default(),
                        client: v["client"].as_u64().unwrap_or_default(),
                        backend_pid: v["backendPid"].as_i64(),
                        sql,
                        params: v["params"].clone(),
                        kind: Some(kind),
                        table,
                        frames,
                        role: String::new(),
                        ..Observed::default()
                    });
                    self.workers[a].held.push(index);
                }
                "result" | "error" => {
                    let seq = v["seq"].as_u64().unwrap_or_default();
                    if let Some(s) = self
                        .statements
                        .iter_mut()
                        .find(|s| s.actor == a && s.seq == seq)
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
                    if let Some(r) = self.workers[a].running
                        && self.statements[r].seq == seq
                    {
                        self.workers[a].running = None;
                    }
                }
                "unsupported" => {
                    self.problems.push(format!(
                        "{}: {}",
                        self.req.actors[a].name,
                        v["reason"].as_str().unwrap_or("unsupported query form")
                    ));
                    self.inconclusive = true;
                }
                "done" => self.workers[a].done = Some(v),
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

    fn finished(&self, a: usize) -> bool {
        self.workers[a].done.is_some() || self.workers[a].stream.is_none()
    }

    fn release(&mut self, a: usize, index: usize, role: &str, plan: Option<usize>) -> bool {
        self.workers[a].held.retain(|i| *i != index);
        self.released += 1;
        let s = &mut self.statements[index];
        s.role = role.to_owned();
        s.plan = plan;
        s.released = Some(self.released);
        self.workers[a].running = Some(index);
        match self.workers[a].stream.as_mut() {
            Some(stream) => send(stream, &json!({"type": "go"})).is_ok(),
            None => false,
        }
    }

    /// Release statements that lie outside the modeled region (a wrapper's
    /// own work before the callback). Writes outside the model make the
    /// run inconclusive: the model does not account for them.
    fn release_outside(&mut self) {
        for a in 0..self.workers.len() {
            if self.workers[a].running.is_some() {
                continue;
            }
            let Some(&index) = self.workers[a].held.first() else {
                continue;
            };
            if self.in_scope(a, &self.statements[index].frames) == Some(false) {
                if !matches!(self.statements[index].kind, Some(SqlKind::Select)) {
                    self.problems.push(format!(
                        "{} sent `{}` outside the modeled function: a write (or transaction control) the model does not represent",
                        self.req.actors[a].name,
                        short(&self.statements[index].sql)
                    ));
                    self.inconclusive = true;
                }
                self.release(a, index, "outside-model", None);
            }
        }
    }

    /// Wait until `index` completes, or PostgreSQL reports it waiting for a
    /// lock. Returns `Some(blocked)`, or `None` at the deadline.
    fn settle(&mut self, a: usize, index: usize) -> Option<bool> {
        let started = Instant::now();
        loop {
            if self.statements[index].completed {
                return Some(false);
            }
            if self.finished(a) {
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
            if started.elapsed() >= self.req.settle
                && let Some(pid) = self.statements[index].backend_pid
            {
                let sql = format!(
                    "SELECT coalesce(wait_event_type, '') || '/' || state FROM pg_stat_activity WHERE pid = {pid}"
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

    fn run_plan(&mut self) {
        for (k, step) in self.req.plan.iter().enumerate() {
            if self.diverged || self.inconclusive {
                return;
            }
            match step {
                PlanStep::Release(e) => {
                    let a = e.actor;
                    // Wait for the actor's next statement.
                    let index = loop {
                        self.release_outside();
                        if let Some(&i) = self.workers[a].held.first()
                            && self.workers[a].running.is_none()
                            && self.in_scope(a, &self.statements[i].frames) != Some(false)
                        {
                            break Some(i);
                        }
                        if self.finished(a) && self.workers[a].held.is_empty() {
                            self.problems.push(format!(
                                "plan step {k}: {} finished without sending the expected {}",
                                self.req.actors[a].name, e.label
                            ));
                            self.diverged = true;
                            break None;
                        }
                        if Instant::now() >= self.deadline {
                            self.problems.push(format!(
                                "plan step {k}: {} did not send the expected {} before the deadline",
                                self.req.actors[a].name, e.label
                            ));
                            self.inconclusive = true;
                            break None;
                        }
                        self.pump(Duration::from_millis(10));
                    };
                    let Some(index) = index else { return };
                    let s = &self.statements[index];
                    let kind_ok = s.kind == Some(e.kind)
                        && (e.table.is_none() || s.table.as_deref() == e.table.as_deref());
                    let site_ok = match (&e.region, s.frames.first()) {
                        (Some(region), Some(frame)) => region.contains(frame),
                        (Some(_), None) => true,
                        (None, _) => true,
                    };
                    if !(kind_ok && site_ok) {
                        self.problems.push(format!(
                            "plan step {k}: {} sent `{}`{} where the model expects {}",
                            self.req.actors[a].name,
                            short(&s.sql),
                            s.frames
                                .first()
                                .map(|f| format!(" (at {}:{}:{})", f.file, f.line, f.column))
                                .unwrap_or_default(),
                            e.label
                        ));
                        self.statements[index].role = "unexpected".to_owned();
                        self.diverged = true;
                        return;
                    }
                    if !self.release(a, index, "planned", Some(k)) {
                        self.problems
                            .push(format!("{}: the worker went away", self.req.actors[a].name));
                        self.inconclusive = true;
                        return;
                    }
                    match self.settle(a, index) {
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
                                if e.expect_block { "waits for a lock" } else { "completes" },
                                if blocked { "reported a lock wait" } else { "completed it" }
                            ));
                            self.diverged = true;
                        }
                        Some(_) => {}
                    }
                }
                PlanStep::Complete { actor, transition } => {
                    let a = *actor;
                    let Some(index) = self.workers[a].running else {
                        self.problems.push(format!(
                            "plan step {k} (transition {transition}): {} has no waiting statement to resume",
                            self.req.actors[a].name
                        ));
                        self.diverged = true;
                        return;
                    };
                    loop {
                        if self.statements[index].completed || self.finished(a) {
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
            for a in 0..self.workers.len() {
                if self.workers[a].running.is_some() {
                    continue;
                }
                if let Some(&index) = self.workers[a].held.first() {
                    let role = if self.diverged || self.inconclusive {
                        "after-divergence"
                    } else {
                        "beyond-plan"
                    };
                    self.release(a, index, role, None);
                    progressed = true;
                }
            }
            if (0..self.workers.len()).all(|a| self.finished(a)) {
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
    let cluster = match Cluster::start(bins, &root.path.join("db")) {
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
    let command = isolation.wrap(&manifest.command);
    let (tx, rx) = channel();
    let mut workers = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(manifest.timeout_ms);
    for (i, a) in req.actors.iter().enumerate() {
        let socket = root.path.join("runs").join(format!("a{i}.sock"));
        let listener = match UnixListener::bind(&socket) {
            Ok(l) => l,
            Err(e) => return blocked(format!("cannot create the coordination socket: {e}")),
        };
        let _ = listener.set_nonblocking(true);
        let stdout = root.path.join("runs").join(format!("a{i}.stdout"));
        let stderr = root.path.join("runs").join(format!("a{i}.stderr"));
        let (Ok(out), Ok(err)) = (
            std::fs::File::create(&stdout),
            std::fs::File::create(&stderr),
        ) else {
            return blocked("cannot create log files");
        };
        let mut cmd = Command::new(&command[0]);
        cmd.args(&command[1..])
            .current_dir(&req.harness.dir)
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k, v)))
            .env("COMMITRIFT_SOURCE_SOCKET", &socket)
            .env("COMMITRIFT_SOURCE_ACTOR", &a.name)
            .env("COMMITRIFT_SOURCE_ENTRY", &a.entry)
            .env("COMMITRIFT_SOURCE_INPUT", a.input.to_string())
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .process_group(0);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                for w in &mut workers {
                    let w: &mut Worker = w;
                    kill_group(&mut w.child);
                }
                return blocked(format!("cannot start `{}`: {e}", manifest.command[0]));
            }
        };
        let stream = loop {
            match listener.accept() {
                Ok((s, _)) => break Some(s),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if matches!(child.try_wait(), Ok(Some(_))) || Instant::now() >= deadline {
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(_) => break None,
            }
        };
        let Some(stream) = stream else {
            kill_group(&mut child);
            let text = std::fs::read_to_string(&stderr).unwrap_or_default();
            for w in &mut workers {
                let w: &mut Worker = w;
                kill_group(&mut w.child);
            }
            return blocked(format!(
                "worker `{}` did not connect: {}",
                a.name,
                safety::redact(text.trim(), &root.path)
            ));
        };
        let _ = stream.set_nonblocking(false);
        let read_half = match stream.try_clone() {
            Ok(r) => r,
            Err(e) => return blocked(format!("socket: {e}")),
        };
        reader(i, read_half, tx.clone());
        workers.push(Worker {
            child,
            stream: Some(stream),
            hello: false,
            done: None,
            held: Vec::new(),
            running: None,
            stderr_path: stderr,
        });
    }
    drop(tx);

    let mut co = Coordinator {
        req,
        cluster: &cluster,
        workers,
        rx,
        statements: Vec::new(),
        released: 0,
        problems: Vec::new(),
        diverged: false,
        inconclusive: false,
        deadline,
    };
    co.run_plan();
    // After the plan (or a divergence): let the workers finish.
    co.drain();
    // Reap.
    let mut actors = Vec::new();
    for (i, w) in co.workers.iter_mut().enumerate() {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            match w.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(5)),
                _ => {
                    kill_group(&mut w.child);
                    let _ = w.child.wait();
                    break;
                }
            }
        }
        let stderr = std::fs::read_to_string(&w.stderr_path).unwrap_or_default();
        let stderr: String = safety::redact(stderr.trim(), &root.path)
            .chars()
            .take(4000)
            .collect();
        let (outcome, value, error) = match &w.done {
            Some(d) => (
                d["outcome"].as_str().unwrap_or("unknown").to_owned(),
                d["value"].clone(),
                d["error"].as_str().map(str::to_owned),
            ),
            None => ("killed".to_owned(), Value::Null, None),
        };
        actors.push(ActorResult {
            name: req.actors[i].name.clone(),
            entry: req.actors[i].entry.clone(),
            outcome,
            value,
            error,
            stderr,
        });
    }
    let _ = cluster.wait_for_idle(DATABASE, Duration::from_secs(5));
    let mut tables = BTreeMap::new();
    for (name, sql) in req.final_queries {
        match cluster.query_value(DATABASE, sql) {
            Ok(Value::String(text)) => match serde_json::from_str::<Value>(&text) {
                Ok(Value::Array(rows)) => {
                    tables.insert(name.clone(), rows);
                }
                _ => co
                    .problems
                    .push(format!("final rows of `{name}` are not a JSON array")),
            },
            Ok(other) => co
                .problems
                .push(format!("final rows of `{name}`: unexpected {other}")),
            Err(e) => co.problems.push(format!("reading `{name}` failed: {e}")),
        }
    }
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
        ]
        .map(str::to_owned),
    );
    names.sort();
    SourceRun {
        status: status.to_owned(),
        problems,
        statements,
        actors,
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

//! A test-only, protocol-aware PostgreSQL proxy for in-flight fault
//! injection (Phase 9).
//!
//! The proxy sits between an authorized worker and the **disposable**
//! PostgreSQL cluster of one run (both reached over Unix sockets inside the
//! private run directory). It forwards every protocol message unchanged and
//! in order; it never edits, reorders or fabricates a message. What it can do
//! is *withhold*:
//!
//! * [`Fault::DropResponse`]: forward the next COMMIT to the server, let the
//!   server complete it, and withhold the server's answer (CommandComplete /
//!   ErrorResponse / ReadyForQuery) from the client. The client has sent the
//!   COMMIT and learns nothing; PostgreSQL's own answer was observed.
//! * [`Fault::HoldRequest`]: withhold the next COMMIT from the server. The
//!   client has sent it; the server has not received it. The held bytes can
//!   later be [delivered](FaultProxy::deliver) to the server (a COMMIT that was
//!   delayed in the network and arrives after the client is gone) or
//!   [discarded](FaultProxy::discard) (it never arrives; the server rolls the
//!   transaction back when the connection ends).
//!
//! Supported protocol subset: StartupMessage (an SSLRequest or GSSENCRequest
//! is answered with `N`; encryption is never negotiated), the simple query
//! protocol (`Q`) and the extended query protocol (`P`arse, `B`ind,
//! `E`xecute, `D`escribe, `C`lose, `S`ync, `H`/Flush), `X` (Terminate), and
//! the backend messages those produce. COPY, replication and function-call
//! messages are not supported: a connection that uses them is marked
//! **unsupported** (the proxy keeps forwarding but no fault is injected on it)
//! and the fact is reported — never a fabricated injection.
//!
//! Evidence and secrets. The proxy records a *redacted* message log: message
//! type, direction, length, and for transaction control (`BEGIN`, `COMMIT`,
//! `ROLLBACK`) the statement; any other SQL is recorded only as
//! `statement`. Bind parameters, result rows, authentication messages and
//! the backend's cancel key are never retained. At most [`MAX_EVENTS`]
//! events and [`MAX_FRAME`] bytes per message are handled; a larger message
//! makes the connection unsupported.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Largest protocol message the proxy buffers (bytes); larger messages make
/// the connection unsupported.
pub const MAX_FRAME: usize = 8 * 1024 * 1024;
/// Most events kept; later ones are counted in `dropped_events`.
pub const MAX_EVENTS: usize = 8192;
/// Most simultaneous client connections.
pub const MAX_CONNECTIONS: usize = 64;

/// What the proxy does to the next COMMIT of the matching connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Fault {
    DropResponse,
    HoldRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EventKind {
    Connected,
    BackendKey,
    /// A protocol message (redacted).
    Message,
    FaultArmed,
    /// The COMMIT was forwarded to the server.
    CommitForwarded,
    /// The COMMIT (all of its messages) is withheld from the server.
    CommitHeld,
    /// The server's answer to the COMMIT was observed (`detail`: command tag
    /// or SQLSTATE and transaction status).
    CommitCompleted,
    /// The server's answer, and everything after it, is withheld from the
    /// client.
    ResponseWithheld,
    Delivered,
    Discarded,
    ClientClosed,
    ServerClosed,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyEvent {
    pub seq: u64,
    pub conn: u64,
    pub at_ms: u64,
    pub kind: EventKind,
    pub detail: String,
}

/// A fault armed for the next COMMIT of connections with this
/// `application_name`.
#[derive(Clone, Debug)]
struct Rule {
    application_name: String,
    fault: Fault,
}

#[derive(Default)]
struct ConnState {
    application_name: String,
    backend_pid: Option<i64>,
    ready: bool,
    unsupported: Option<String>,
    /// Statement names (and the unnamed one) that are a COMMIT.
    commit_statements: BTreeMap<String, bool>,
    /// Portals bound to a COMMIT statement.
    commit_portals: BTreeMap<String, bool>,
    /// The armed COMMIT was forwarded; the next answer is the one to withhold.
    awaiting_commit_answer: bool,
    /// Server messages are withheld from the client from now on.
    withholding: bool,
    /// Client messages are held (not forwarded) from now on.
    holding: bool,
    held: Vec<u8>,
    /// The upstream connection kept open for a later delivery.
    upstream: Option<UnixStream>,
    client_gone: bool,
}

struct Inner {
    rules: Vec<Rule>,
    events: Vec<ProxyEvent>,
    dropped_events: u64,
    conns: BTreeMap<u64, ConnState>,
    next_conn: u64,
    seq: u64,
}

struct Shared {
    inner: Mutex<Inner>,
    changed: Condvar,
    stop: AtomicBool,
    started: Instant,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn event(&self, conn: u64, kind: EventKind, detail: impl Into<String>) {
        let mut g = self.lock();
        let seq = g.seq;
        g.seq += 1;
        if g.events.len() >= MAX_EVENTS {
            g.dropped_events += 1;
        } else {
            let at_ms = self.started.elapsed().as_millis() as u64;
            g.events.push(ProxyEvent {
                seq,
                conn,
                at_ms,
                kind,
                detail: detail.into(),
            });
        }
        drop(g);
        self.changed.notify_all();
    }
}

/// A running proxy; stopped (and its socket removed) on drop.
pub struct FaultProxy {
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
    path: PathBuf,
}

impl FaultProxy {
    /// Listen on `listen` and forward to the server socket `upstream`. Both
    /// must lie inside `root` (the private run directory).
    pub fn start(root: &Path, listen: &Path, upstream: &Path) -> Result<Self, String> {
        for p in [listen, upstream] {
            if !p.starts_with(root) {
                return Err(format!(
                    "the fault proxy only handles sockets inside the run directory ({})",
                    p.display()
                ));
            }
        }
        if let Some(dir) = listen.parent() {
            crate::safety::create_private_dir(dir)?;
        }
        let _ = std::fs::remove_file(listen);
        let listener = UnixListener::bind(listen)
            .map_err(|e| format!("cannot listen on the proxy socket: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("proxy socket: {e}"))?;
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                rules: Vec::new(),
                events: Vec::new(),
                dropped_events: 0,
                conns: BTreeMap::new(),
                next_conn: 1,
                seq: 0,
            }),
            changed: Condvar::new(),
            stop: AtomicBool::new(false),
            started: Instant::now(),
        });
        let upstream = upstream.to_path_buf();
        let s = shared.clone();
        let accept = std::thread::Builder::new()
            .name("commitrift-proxy".into())
            .spawn(move || accept_loop(&s, &listener, &upstream))
            .map_err(|e| format!("cannot start the proxy: {e}"))?;
        Ok(Self {
            shared,
            accept: Some(accept),
            path: listen.to_path_buf(),
        })
    }

    /// Arm `fault` for the next COMMIT of a connection whose
    /// `application_name` is `application_name`. One-shot.
    pub fn arm(&self, application_name: &str, fault: Fault) {
        self.shared.lock().rules.push(Rule {
            application_name: application_name.to_owned(),
            fault,
        });
        self.shared.event(
            0,
            EventKind::FaultArmed,
            format!("{fault:?} for application_name {application_name}"),
        );
    }

    /// Sequence number the next event will get (a cursor for [`Self::wait`]).
    pub fn cursor(&self) -> u64 {
        self.shared.lock().seq
    }

    /// Wait for an event at or after `since` accepted by `pick`.
    pub fn wait(
        &self,
        since: u64,
        timeout: Duration,
        pick: impl Fn(&ProxyEvent) -> bool,
    ) -> Option<ProxyEvent> {
        let deadline = Instant::now() + timeout;
        let mut g = self.shared.lock();
        loop {
            if let Some(e) = g.events.iter().find(|e| e.seq >= since && pick(e)) {
                return Some(e.clone());
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (guard, _) = self
                .shared
                .changed
                .wait_timeout(g, deadline - now)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            g = guard;
        }
    }

    pub fn events(&self) -> Vec<ProxyEvent> {
        self.shared.lock().events.clone()
    }

    pub fn dropped_events(&self) -> u64 {
        self.shared.lock().dropped_events
    }

    /// The backend process id of a connection (from BackendKeyData).
    pub fn backend_pid(&self, conn: u64) -> Option<i64> {
        self.shared.lock().conns.get(&conn)?.backend_pid
    }

    /// Why the proxy cannot inject faults on `conn`, if it cannot.
    pub fn unsupported(&self, conn: u64) -> Option<String> {
        self.shared.lock().conns.get(&conn)?.unsupported.clone()
    }

    /// Deliver the held COMMIT of `conn` to the server (the client may be
    /// gone). The server's answer arrives as a `CommitCompleted` event.
    pub fn deliver(&self, conn: u64) -> Result<(), String> {
        let (bytes, mut upstream) = {
            let mut g = self.shared.lock();
            let c = g.conns.get_mut(&conn).ok_or("no such connection")?;
            if !c.holding || c.held.is_empty() {
                return Err("nothing is held on this connection".into());
            }
            let up = c
                .upstream
                .as_ref()
                .ok_or("the server connection is not available (the client is still connected)")?
                .try_clone()
                .map_err(|e| e.to_string())?;
            (std::mem::take(&mut c.held), up)
        };
        upstream
            .write_all(&bytes)
            .and_then(|()| upstream.flush())
            .map_err(|e| format!("delivering the held COMMIT failed: {e}"))?;
        self.shared.event(
            conn,
            EventKind::Delivered,
            "held COMMIT delivered to the server",
        );
        Ok(())
    }

    /// Drop the held COMMIT and close the server connection: the COMMIT never
    /// arrives and the server rolls the transaction back.
    pub fn discard(&self, conn: u64) {
        let up = {
            let mut g = self.shared.lock();
            let Some(c) = g.conns.get_mut(&conn) else {
                return;
            };
            c.held.clear();
            c.upstream.take()
        };
        if let Some(u) = up {
            let _ = u.shutdown(std::net::Shutdown::Both);
        }
        self.shared
            .event(conn, EventKind::Discarded, "held COMMIT discarded");
    }

    /// Stop the proxy: close every connection and remove the socket.
    pub fn stop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        let ups: Vec<UnixStream> = {
            let mut g = self.shared.lock();
            g.conns
                .values_mut()
                .filter_map(|c| c.upstream.take())
                .collect()
        };
        for u in ups {
            let _ = u.shutdown(std::net::Shutdown::Both);
        }
        if let Some(h) = self.accept.take() {
            let _ = h.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for FaultProxy {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// Protocol parsing
// ---------------------------------------------------------------------------

/// What a transaction-control statement is, for fault matching.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Commit,
    Begin,
    Rollback,
    Other,
}

/// Classify SQL text: only plain `COMMIT`/`END` (optionally `TRANSACTION`,
/// `WORK`, `AND [NO] CHAIN`) is a COMMIT; `COMMIT PREPARED` is not.
pub fn classify_sql(sql: &str) -> Control {
    let mut text = sql.trim();
    loop {
        if let Some(rest) = text.strip_prefix("--") {
            text = rest.split_once('\n').map_or("", |(_, r)| r).trim_start();
        } else if let Some(rest) = text.strip_prefix("/*") {
            text = rest.split_once("*/").map_or("", |(_, r)| r).trim_start();
        } else {
            break;
        }
    }
    let lower = text
        .trim_end_matches([';', ' ', '\n', '\t', '\r'])
        .to_ascii_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    match words.as_slice() {
        ["commit" | "end"] => Control::Commit,
        ["commit" | "end", "transaction" | "work"] => Control::Commit,
        ["commit" | "end", "and", "chain"]
        | ["commit" | "end", "and", "no", "chain"]
        | ["commit" | "end", "transaction" | "work", "and", "chain"]
        | [
            "commit" | "end",
            "transaction" | "work",
            "and",
            "no",
            "chain",
        ] => Control::Commit,
        ["rollback" | "abort"] | ["rollback" | "abort", "transaction" | "work"] => {
            Control::Rollback
        }
        [first, ..] if *first == "begin" || *first == "start" => Control::Begin,
        _ => Control::Other,
    }
}

#[derive(Debug)]
struct Frame {
    tag: u8,
    body: Vec<u8>,
    raw: Vec<u8>,
}

/// (request code, parameters, raw bytes) of a startup-phase message.
type StartupMessage = (u32, Vec<u8>, Vec<u8>);

#[derive(Default)]
struct Framer {
    buf: Vec<u8>,
}

impl Framer {
    fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next typed message, or `None` if incomplete; `Err` for an invalid
    /// or oversized one.
    fn typed(&mut self) -> Result<Option<Frame>, String> {
        if self.buf.len() < 5 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
        if len < 4 {
            return Err(format!("invalid message length {len}"));
        }
        if len > MAX_FRAME {
            return Err(format!("a message of {len} bytes exceeds the proxy limit"));
        }
        if self.buf.len() < 1 + len {
            return Ok(None);
        }
        let raw: Vec<u8> = self.buf.drain(..1 + len).collect();
        Ok(Some(Frame {
            tag: raw[0],
            body: raw[5..].to_vec(),
            raw,
        }))
    }

    /// The next untyped startup-phase message (length, code, payload).
    fn startup(&mut self) -> Result<Option<StartupMessage>, String> {
        if self.buf.len() < 8 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if !(8..=MAX_FRAME).contains(&len) {
            return Err(format!("invalid startup message length {len}"));
        }
        if self.buf.len() < len {
            return Ok(None);
        }
        let raw: Vec<u8> = self.buf.drain(..len).collect();
        let code = u32::from_be_bytes([raw[4], raw[5], raw[6], raw[7]]);
        Ok(Some((code, raw[8..].to_vec(), raw)))
    }
}

fn cstr(bytes: &[u8]) -> (String, &[u8]) {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    (
        String::from_utf8_lossy(&bytes[..end]).into_owned(),
        bytes.get(end + 1..).unwrap_or(&[]),
    )
}

fn startup_application_name(payload: &[u8]) -> String {
    let mut rest = payload;
    loop {
        let (key, r) = cstr(rest);
        if key.is_empty() {
            return String::new();
        }
        let (value, r) = cstr(r);
        if key == "application_name" {
            return value;
        }
        rest = r;
    }
}

fn error_sqlstate(body: &[u8]) -> String {
    let mut rest = body;
    while let Some((&field, r)) = rest.split_first() {
        if field == 0 {
            break;
        }
        let (value, r) = cstr(r);
        if field == b'C' {
            return value;
        }
        rest = r;
    }
    String::new()
}

fn redacted_sql(sql: &str) -> String {
    match classify_sql(sql) {
        Control::Commit => "COMMIT".to_owned(),
        Control::Begin => "BEGIN".to_owned(),
        Control::Rollback => "ROLLBACK".to_owned(),
        Control::Other => "statement".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Connections
// ---------------------------------------------------------------------------

fn accept_loop(shared: &Arc<Shared>, listener: &UnixListener, upstream: &Path) {
    let mut handles: Vec<JoinHandle<()>> = Vec::new();
    while !shared.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((client, _)) => {
                let id = {
                    let mut g = shared.lock();
                    if g.conns.len() >= MAX_CONNECTIONS {
                        drop(g);
                        shared.event(0, EventKind::Unsupported, "too many connections");
                        continue;
                    }
                    let id = g.next_conn;
                    g.next_conn += 1;
                    g.conns.insert(id, ConnState::default());
                    id
                };
                let Ok(server) = UnixStream::connect(upstream) else {
                    shared.event(id, EventKind::ServerClosed, "cannot reach the server");
                    continue;
                };
                let _ = client.set_nonblocking(false);
                let _ = server.set_nonblocking(false);
                if let Err(e) = serve(shared, id, client, server, &mut handles) {
                    shared.event(id, EventKind::Unsupported, e);
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(_) => break,
        }
        handles.retain(|h| !h.is_finished());
    }
    for h in handles {
        let _ = h.join();
    }
}

fn serve(
    shared: &Arc<Shared>,
    id: u64,
    client: UnixStream,
    server: UnixStream,
    handles: &mut Vec<JoinHandle<()>>,
) -> Result<(), String> {
    let c2s_client = client.try_clone().map_err(|e| e.to_string())?;
    let c2s_server = server.try_clone().map_err(|e| e.to_string())?;
    let s2c_client = client;
    let s2c_server = server;
    let a = shared.clone();
    let b = shared.clone();
    handles.push(
        std::thread::Builder::new()
            .name(format!("px-c2s-{id}"))
            .spawn(move || client_to_server(&a, id, c2s_client, c2s_server))
            .map_err(|e| e.to_string())?,
    );
    handles.push(
        std::thread::Builder::new()
            .name(format!("px-s2c-{id}"))
            .spawn(move || server_to_client(&b, id, s2c_server, s2c_client))
            .map_err(|e| e.to_string())?,
    );
    Ok(())
}

fn mark_unsupported(shared: &Shared, id: u64, why: &str) {
    let first = {
        let mut g = shared.lock();
        match g.conns.get_mut(&id) {
            Some(c) if c.unsupported.is_none() => {
                c.unsupported = Some(why.to_owned());
                true
            }
            _ => false,
        }
    };
    if first {
        shared.event(id, EventKind::Unsupported, why);
    }
}

fn client_to_server(shared: &Shared, id: u64, mut client: UnixStream, mut server: UnixStream) {
    let _ = client.set_read_timeout(Some(Duration::from_millis(100)));
    let mut framer = Framer::default();
    let mut chunk = vec![0u8; 16 * 1024];
    let mut started = false;
    'outer: loop {
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        let n = match client.read(&mut chunk) {
            Ok(0) => {
                client_closed(shared, id, server);
                return;
            }
            Ok(n) => n,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => continue,
            Err(_) => {
                client_closed(shared, id, server);
                return;
            }
        };
        framer.push(&chunk[..n]);
        // Startup phase: untyped messages until a StartupMessage.
        while !started {
            match framer.startup() {
                Ok(None) => continue 'outer,
                Err(e) => {
                    mark_unsupported(shared, id, &e);
                    break 'outer;
                }
                Ok(Some((code, payload, raw))) => match code {
                    // SSLRequest, GSSENCRequest: encryption is not
                    // negotiated; the client continues in the clear.
                    80_877_103 | 80_877_104 => {
                        if client.write_all(b"N").is_err() {
                            break 'outer;
                        }
                    }
                    // CancelRequest: forwarded as is; its key is not kept.
                    80_877_102 => {
                        let _ = server.write_all(&raw);
                        break 'outer;
                    }
                    _ => {
                        let app = startup_application_name(&payload);
                        {
                            let mut g = shared.lock();
                            if let Some(c) = g.conns.get_mut(&id) {
                                c.application_name.clone_from(&app);
                            }
                        }
                        shared.event(id, EventKind::Connected, format!("application_name {app}"));
                        if server.write_all(&raw).is_err() {
                            break 'outer;
                        }
                        started = true;
                    }
                },
            }
        }
        loop {
            match framer.typed() {
                Ok(None) => break,
                Err(e) => {
                    mark_unsupported(shared, id, &e);
                    break 'outer;
                }
                Ok(Some(frame)) => {
                    if !forward_client_frame(shared, id, &frame, &mut server) {
                        break 'outer;
                    }
                }
            }
        }
    }
    client_closed(shared, id, server);
}

/// The client went away: end the connection to the server, unless a held
/// COMMIT is waiting to be delivered to it.
fn client_closed(shared: &Shared, id: u64, server: UnixStream) {
    let keep = {
        let mut g = shared.lock();
        match g.conns.get_mut(&id) {
            Some(c) => {
                c.client_gone = true;
                if c.holding && !c.held.is_empty() {
                    c.upstream = server.try_clone().ok();
                    true
                } else {
                    false
                }
            }
            None => false,
        }
    };
    shared.event(
        id,
        EventKind::ClientClosed,
        if keep {
            "client closed; server connection kept for the held COMMIT"
        } else {
            "client closed"
        },
    );
    if !keep {
        let _ = server.shutdown(std::net::Shutdown::Both);
    }
}

/// Forward (or hold) one client message. `false`: stop the connection.
fn forward_client_frame(shared: &Shared, id: u64, f: &Frame, server: &mut UnixStream) -> bool {
    // Message log (redacted) and COMMIT detection.
    let mut is_commit_frame = false;
    let mut detail = format!("{} bytes", f.raw.len());
    match f.tag {
        b'Q' => {
            let (sql, _) = cstr(&f.body);
            is_commit_frame = classify_sql(&sql) == Control::Commit;
            detail = redacted_sql(&sql);
        }
        b'P' => {
            let (name, rest) = cstr(&f.body);
            let (sql, _) = cstr(rest);
            let commit = classify_sql(&sql) == Control::Commit;
            detail = format!(
                "{} (statement name {})",
                redacted_sql(&sql),
                if name.is_empty() { "unnamed" } else { "named" }
            );
            if let Some(c) = shared.lock().conns.get_mut(&id) {
                c.commit_statements.insert(name, commit);
            }
            is_commit_frame = commit;
        }
        b'B' => {
            let (portal, rest) = cstr(&f.body);
            let (stmt, _) = cstr(rest);
            let mut g = shared.lock();
            if let Some(c) = g.conns.get_mut(&id) {
                let commit = c.commit_statements.get(&stmt).copied().unwrap_or(false);
                c.commit_portals.insert(portal, commit);
                is_commit_frame = commit;
            }
            detail = "parameters not recorded".into();
        }
        b'E' => {
            let (portal, _) = cstr(&f.body);
            let g = shared.lock();
            is_commit_frame = g
                .conns
                .get(&id)
                .and_then(|c| c.commit_portals.get(&portal).copied())
                .unwrap_or(false);
        }
        b'd' | b'c' | b'f' => {
            mark_unsupported(shared, id, "COPY is not supported by the fault proxy");
        }
        b'F' => mark_unsupported(
            shared,
            id,
            "function calls are not supported by the fault proxy",
        ),
        b'p' => detail = "authentication data not recorded".into(),
        _ => {}
    }
    shared.event(
        id,
        EventKind::Message,
        format!("client → server {} {detail}", f.tag as char),
    );
    // Is a fault armed for this COMMIT?
    let (hold_now, arm) = {
        let mut g = shared.lock();
        let supported;
        let app;
        let holding;
        {
            let Some(c) = g.conns.get(&id) else {
                return false;
            };
            supported = c.unsupported.is_none();
            app = c.application_name.clone();
            holding = c.holding;
        }
        if holding {
            (true, None)
        } else if is_commit_frame && supported {
            let at = g.rules.iter().position(|r| r.application_name == app);
            match at {
                Some(i) => {
                    let rule = g.rules.remove(i);
                    (rule.fault == Fault::HoldRequest, Some(rule.fault))
                }
                None => (false, None),
            }
        } else {
            (false, None)
        }
    };
    if let Some(fault) = arm {
        match fault {
            Fault::HoldRequest => {
                if let Some(c) = shared.lock().conns.get_mut(&id) {
                    c.holding = true;
                }
                shared.event(
                    id,
                    EventKind::CommitHeld,
                    "the COMMIT is withheld from the server",
                );
            }
            Fault::DropResponse => {
                if let Some(c) = shared.lock().conns.get_mut(&id) {
                    c.awaiting_commit_answer = true;
                }
                shared.event(
                    id,
                    EventKind::CommitForwarded,
                    "the COMMIT is forwarded; its answer will be withheld",
                );
            }
        }
    }
    if hold_now {
        let mut g = shared.lock();
        if let Some(c) = g.conns.get_mut(&id) {
            c.held.extend_from_slice(&f.raw);
            if c.held.len() > MAX_FRAME * 2 {
                c.unsupported = Some("held data exceeds the proxy limit".into());
                return false;
            }
        }
        return true;
    }
    server
        .write_all(&f.raw)
        .and_then(|()| server.flush())
        .is_ok()
}

fn server_to_client(shared: &Shared, id: u64, mut server: UnixStream, mut client: UnixStream) {
    let _ = server.set_read_timeout(Some(Duration::from_millis(100)));
    let mut framer = Framer::default();
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        let n = match server.read(&mut chunk) {
            Ok(0) => {
                shared.event(id, EventKind::ServerClosed, "server closed");
                let _ = client.shutdown(std::net::Shutdown::Both);
                return;
            }
            Ok(n) => n,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => continue,
            Err(_) => {
                let _ = client.shutdown(std::net::Shutdown::Both);
                return;
            }
        };
        framer.push(&chunk[..n]);
        // The server speaks typed messages from its first byte, except the
        // one-byte answer to an SSLRequest, which the proxy never forwards.
        loop {
            match framer.typed() {
                Ok(None) => break,
                Err(e) => {
                    mark_unsupported(shared, id, &e);
                    let _ = client.shutdown(std::net::Shutdown::Both);
                    return;
                }
                Ok(Some(f)) => {
                    if !forward_server_frame(shared, id, &f, &mut client) {
                        // The client is gone: keep reading (a delivered
                        // COMMIT's answer is still evidence).
                        continue;
                    }
                }
            }
        }
    }
}

fn forward_server_frame(shared: &Shared, id: u64, f: &Frame, client: &mut UnixStream) -> bool {
    let mut detail = format!("{} bytes", f.raw.len());
    let mut commit_answer: Option<String> = None;
    match f.tag {
        b'K' if f.body.len() >= 4 => {
            let pid = i64::from(u32::from_be_bytes([
                f.body[0], f.body[1], f.body[2], f.body[3],
            ]));
            if let Some(c) = shared.lock().conns.get_mut(&id) {
                c.backend_pid = Some(pid);
            }
            shared.event(
                id,
                EventKind::BackendKey,
                format!("backend pid {pid} (cancel key not kept)"),
            );
            detail = "BackendKeyData (key not recorded)".into();
        }
        b'R' => detail = "authentication (not recorded)".into(),
        b'C' => {
            let (tag, _) = cstr(&f.body);
            detail = format!("CommandComplete {tag}");
            if tag == "COMMIT" {
                commit_answer = Some("CommandComplete COMMIT".to_owned());
            }
        }
        b'E' => {
            let sqlstate = error_sqlstate(&f.body);
            detail = format!("ErrorResponse {sqlstate}");
            commit_answer = Some(format!("ErrorResponse {sqlstate}"));
        }
        b'Z' => {
            let status = f.body.first().map_or('?', |b| *b as char);
            detail = format!("ReadyForQuery status {status}");
            if let Some(c) = shared.lock().conns.get_mut(&id) {
                c.ready = true;
            }
        }
        b'G' | b'H' | b'W' | b'd' | b'c' => {
            mark_unsupported(shared, id, "COPY is not supported by the fault proxy");
        }
        b'D' | b'T' => detail = "result data (not recorded)".into(),
        _ => {}
    }
    shared.event(
        id,
        EventKind::Message,
        format!("server → client {} {detail}", f.tag as char),
    );
    let (withhold_now, answered) = {
        let mut g = shared.lock();
        let Some(c) = g.conns.get_mut(&id) else {
            return false;
        };
        if c.withholding {
            (true, None)
        } else if c.awaiting_commit_answer && commit_answer.is_some() {
            c.awaiting_commit_answer = false;
            c.withholding = true;
            (true, commit_answer.clone())
        } else {
            (false, None)
        }
    };
    if let Some(a) = answered {
        shared.event(
            id,
            EventKind::CommitCompleted,
            format!("the server answered the COMMIT: {a}"),
        );
        shared.event(
            id,
            EventKind::ResponseWithheld,
            "the answer and everything after it is withheld from the client",
        );
        return true;
    }
    if withhold_now {
        return true;
    }
    // A delivered COMMIT (client gone): its answer is evidence too.
    let gone_answer = {
        let g = shared.lock();
        g.conns.get(&id).is_some_and(|c| c.client_gone && c.holding)
    };
    if gone_answer && let Some(a) = commit_answer {
        shared.event(
            id,
            EventKind::CommitCompleted,
            format!("the server answered the delivered COMMIT: {a}"),
        );
        return false;
    }
    client
        .write_all(&f.raw)
        .and_then(|()| client.flush())
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_commit_is_a_commit() {
        for sql in [
            "COMMIT",
            "commit;",
            "  Commit  Transaction ;",
            "END",
            "commit work and no chain",
            "/* x */ commit",
            "-- c\ncommit",
        ] {
            assert_eq!(classify_sql(sql), Control::Commit, "{sql}");
        }
        for sql in [
            "COMMIT PREPARED 'x'",
            "select 'commit'",
            "insert into t values ('commit')",
            "committed",
            "",
        ] {
            assert_ne!(classify_sql(sql), Control::Commit, "{sql}");
        }
        assert_eq!(
            classify_sql("begin isolation level serializable"),
            Control::Begin
        );
        assert_eq!(classify_sql("ROLLBACK"), Control::Rollback);
    }

    #[test]
    fn frames_are_reassembled_from_arbitrary_chunks() {
        let mut f = Framer::default();
        let mut msg = vec![b'Q'];
        msg.extend_from_slice(&(4u32 + 7).to_be_bytes());
        msg.extend_from_slice(b"COMMIT\0");
        for byte in &msg[..msg.len() - 1] {
            f.push(&[*byte]);
            assert!(f.typed().unwrap().is_none());
        }
        f.push(&msg[msg.len() - 1..]);
        let frame = f.typed().unwrap().unwrap();
        assert_eq!(frame.tag, b'Q');
        assert_eq!(frame.raw, msg);
        assert_eq!(cstr(&frame.body).0, "COMMIT");
    }

    #[test]
    fn invalid_and_oversized_frames_are_refused() {
        let mut f = Framer::default();
        f.push(&[b'Q', 0, 0, 0, 2, 0]);
        assert!(f.typed().is_err());
        let mut f = Framer::default();
        f.push(&[b'Q', 0xff, 0xff, 0xff, 0xff]);
        assert!(f.typed().is_err());
    }

    #[test]
    fn startup_parameters_give_the_application_name_only() {
        let payload =
            b"user\0commitrift\0database\0app\0application_name\0commitrift-actor-A#1\0\0";
        assert_eq!(startup_application_name(payload), "commitrift-actor-A#1");
    }

    #[test]
    fn error_responses_yield_the_sqlstate_and_nothing_else() {
        let body = b"SERROR\0VERROR\0C40001\0Msecret detail\0\0";
        assert_eq!(error_sqlstate(body), "40001");
    }

    #[test]
    fn sql_other_than_transaction_control_is_never_recorded() {
        assert_eq!(
            redacted_sql("insert into t values ('card 4242')"),
            "statement"
        );
        assert_eq!(redacted_sql("COMMIT"), "COMMIT");
    }

    #[test]
    fn the_proxy_refuses_sockets_outside_the_run_directory() {
        let root = Path::new("/tmp/cr-root-x");
        let r = FaultProxy::start(
            root,
            Path::new("/tmp/elsewhere/.s.PGSQL.1"),
            &root.join("pg/.s.PGSQL.1"),
        );
        assert!(r.is_err());
    }
}

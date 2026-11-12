//! The fault proxy against a real, disposable PostgreSQL.
//!
//! The client is `psql` (simple query protocol) so that the test controls
//! exactly what is on the wire. Each test builds its own cluster in a private
//! temporary directory. Needs PostgreSQL 14+ binaries; prints SKIPPED
//! otherwise, unless `MIDWRITE_REQUIRE_REPLAY=1`.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use midwrite_replay::postgres::{APP_USER, Binaries, Cluster};
use midwrite_replay::proxy::{EventKind, Fault, FaultProxy, Target};
use midwrite_replay::safety::ReplayRoot;
use serde_json::Value;

const DB: &str = "app";
const WAIT: Duration = Duration::from_secs(15);

macro_rules! require {
    () => {
        match Binaries::discover() {
            Err(reason) => {
                assert!(
                    std::env::var_os("MIDWRITE_REQUIRE_REPLAY").is_none(),
                    "PostgreSQL required: {reason}"
                );
                eprintln!("SKIPPED: {reason}");
                return;
            }
            Ok(b) => b,
        }
    };
}

struct Rig {
    // Declaration order is drop order: the proxy and the cluster go before
    // the directory that holds them.
    proxy: FaultProxy,
    cluster: Cluster,
    bins: Binaries,
    root: ReplayRoot,
}

fn rig(bins: Binaries) -> Rig {
    let root = ReplayRoot::create(false).expect("private run directory");
    let cluster = Cluster::start(bins.clone(), &root.path.join("db")).expect("cluster");
    cluster.create_database(DB).expect("database");
    let schema = root.path.join("schema.sql");
    std::fs::write(
        &schema,
        "CREATE TABLE payments (id serial PRIMARY KEY, req text NOT NULL);",
    )
    .unwrap();
    cluster.apply_file(DB, &schema).expect("schema");
    let listen = root.path.join("px").join(".s.PGSQL.5432");
    let proxy = FaultProxy::start(&root.path, &listen, &cluster.socket_path()).expect("proxy");
    Rig {
        proxy,
        cluster,
        bins,
        root,
    }
}

impl Rig {
    /// A `psql` client of the proxy running `script` (statement by
    /// statement, over the simple query protocol).
    fn client(&self, application_name: &str, script: &str) -> Client {
        let file = self.root.path.join(format!("{application_name}.sql"));
        std::fs::write(&file, script).unwrap();
        let out = self.root.path.join(format!("{application_name}.out"));
        let child = Command::new(self.bins.dir.join("psql"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("PGAPPNAME", application_name)
            .arg("-X")
            .args(["-v", "ON_ERROR_STOP=1"])
            .arg("-h")
            .arg(self.root.path.join("px"))
            .args(["-p", "5432", "-U", APP_USER, "-d", DB])
            .arg("-f")
            .arg(&file)
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&out).unwrap())
            .stderr(Stdio::null())
            .spawn()
            .expect("psql");
        Client { child, out }
    }

    fn count(&self) -> i64 {
        match self
            .cluster
            .query_value(DB, "SELECT count(*) FROM payments")
            .unwrap()
        {
            Value::Number(n) => n.as_i64().unwrap(),
            other => panic!("unexpected {other}"),
        }
    }

    fn backend_state(&self, pid: i64) -> String {
        match self
            .cluster
            .query_value(
                "postgres",
                &format!("SELECT coalesce(state, 'gone') FROM pg_stat_activity WHERE pid = {pid}"),
            )
            .unwrap()
        {
            Value::String(s) => s,
            Value::Null => "gone".to_owned(),
            other => panic!("unexpected {other}"),
        }
    }
}

struct Client {
    child: Child,
    out: std::path::PathBuf,
}

impl Client {
    fn output(&self) -> String {
        std::fs::read_to_string(&self.out).unwrap_or_default()
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.kill();
    }
}

fn until(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !ok() {
        assert!(Instant::now() < deadline, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

const PAY: &str =
    "BEGIN;\nINSERT INTO payments (req) VALUES ('r1');\nCOMMIT;\nSELECT 'after commit';\n";

#[test]
fn a_dropped_response_leaves_a_committed_row_the_client_never_heard_about() {
    let bins = require!();
    let rig = rig(bins);
    rig.proxy.arm("w#1", Fault::DropResponse, Target::Commit);
    let since = rig.proxy.cursor();
    let mut client = rig.client("w#1", PAY);
    let done = rig
        .proxy
        .wait(since, WAIT, |e| e.kind == EventKind::OperationCompleted)
        .expect("the server answered the COMMIT");
    assert!(done.detail.contains("CommandComplete COMMIT"), "{done:?}");
    // The client is still waiting: it was not told.
    assert!(client.child.try_wait().unwrap().is_none());
    let out = client.output();
    assert!(
        !out.contains("COMMIT"),
        "the answer reached the client: {out}"
    );
    assert!(!out.contains("after commit"));
    client.kill();
    // PostgreSQL's own state, read independently: the transaction committed.
    assert_eq!(rig.count(), 1);
    let events = rig.proxy.events();
    assert!(events.iter().any(|e| e.kind == EventKind::ResponseWithheld));
    // Secrets and payloads are not in the evidence.
    let log = serde_json::to_string(&events).unwrap();
    assert!(!log.contains("r1"), "a parameter value leaked into the log");
    assert!(log.contains("INSERT") || log.contains("statement"));
}

#[test]
fn a_held_commit_is_not_on_the_server_until_it_is_delivered() {
    let bins = require!();
    let rig = rig(bins);
    rig.proxy.arm("w#1", Fault::HoldRequest, Target::Commit);
    let since = rig.proxy.cursor();
    let mut client = rig.client("w#1", PAY);
    let held = rig
        .proxy
        .wait(since, WAIT, |e| e.kind == EventKind::OperationHeld)
        .expect("the COMMIT was withheld");
    let conn = held.conn;
    let pid = rig.proxy.backend_pid(conn).expect("backend pid");
    // The client has sent COMMIT; the server has not received it: its
    // transaction is open (independently observed in pg_stat_activity).
    assert_eq!(rig.backend_state(pid), "idle in transaction");
    assert_eq!(rig.count(), 0);
    client.kill();
    // The client is gone but its COMMIT is still on its way: the server
    // session survives, the row is not there.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(rig.backend_state(pid), "idle in transaction");
    assert_eq!(rig.count(), 0);
    // The COMMIT arrives late: the server completes it.
    let since = rig.proxy.cursor();
    rig.proxy.deliver(conn).expect("deliver");
    let done = rig
        .proxy
        .wait(since, WAIT, |e| e.kind == EventKind::OperationCompleted)
        .expect("the server answered the delivered COMMIT");
    assert!(done.detail.contains("COMMIT"), "{done:?}");
    assert_eq!(rig.count(), 1);
}

#[test]
fn a_discarded_commit_never_commits_and_the_server_cleans_up() {
    let bins = require!();
    let rig = rig(bins);
    rig.proxy.arm("w#1", Fault::HoldRequest, Target::Commit);
    let since = rig.proxy.cursor();
    let mut client = rig.client("w#1", PAY);
    let held = rig
        .proxy
        .wait(since, WAIT, |e| e.kind == EventKind::OperationHeld)
        .unwrap();
    let pid = rig.proxy.backend_pid(held.conn).unwrap();
    client.kill();
    rig.proxy.discard(held.conn);
    until("the backend ended", || rig.backend_state(pid) == "gone");
    assert_eq!(rig.count(), 0, "the transaction was rolled back");
}

#[test]
fn only_the_armed_connection_is_touched_and_other_traffic_is_unchanged() {
    let bins = require!();
    let rig = rig(bins);
    rig.proxy
        .arm("victim#1", Fault::DropResponse, Target::Commit);
    // Another client through the same proxy is not affected.
    let mut other = rig.client("other#1", PAY);
    until("the other client finished", || {
        other.child.try_wait().unwrap().is_some()
    });
    let out = other.output();
    assert!(
        out.contains("COMMIT") && out.contains("after commit"),
        "{out}"
    );
    assert_eq!(rig.count(), 1);
    assert!(
        !rig.proxy
            .events()
            .iter()
            .any(|e| e.kind == EventKind::ResponseWithheld),
        "the fault was armed for another application_name"
    );
}

#[test]
fn a_rollback_or_other_statement_is_never_mistaken_for_a_commit() {
    let bins = require!();
    let rig = rig(bins);
    rig.proxy.arm("w#1", Fault::DropResponse, Target::Commit);
    let mut client = rig.client(
        "w#1",
        "BEGIN;\nINSERT INTO payments (req) VALUES ('commit');\nROLLBACK;\nSELECT 'done';\n",
    );
    until("the client finished", || {
        client.child.try_wait().unwrap().is_some()
    });
    assert!(client.output().contains("done"));
    assert_eq!(rig.count(), 0);
    assert!(
        !rig.proxy
            .events()
            .iter()
            .any(|e| e.kind == EventKind::OperationCompleted)
    );
}

#[test]
fn the_proxy_socket_is_private_and_removed() {
    let bins = require!();
    let path;
    {
        let rig = rig(bins);
        path = rig.root.path.join("px").join(".s.PGSQL.5432");
        assert!(path.exists());
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "the proxy directory is private");
    }
    assert!(!Path::new(&path).exists());
}

#[test]
fn an_autocommit_statement_can_be_answered_and_withheld() {
    let bins = require!();
    let rig = rig(bins);
    rig.proxy.arm("w#1", Fault::DropResponse, Target::Statement);
    let since = rig.proxy.cursor();
    let mut client = rig.client(
        "w#1",
        "INSERT INTO payments (req) VALUES ('auto');\nSELECT 'after';\n",
    );
    let done = rig
        .proxy
        .wait(since, WAIT, |e| e.kind == EventKind::OperationCompleted)
        .expect("the server answered the statement");
    assert!(done.detail.contains("INSERT"), "{done:?}");
    assert!(client.child.try_wait().unwrap().is_none());
    client.kill();
    assert_eq!(
        rig.count(),
        1,
        "autocommitted although the client never heard"
    );
}

#[test]
fn an_autocommit_statement_can_be_held_and_delivered_later() {
    let bins = require!();
    let rig = rig(bins);
    rig.proxy.arm("w#1", Fault::HoldRequest, Target::Statement);
    let since = rig.proxy.cursor();
    let mut client = rig.client("w#1", "INSERT INTO payments (req) VALUES ('auto');\n");
    let held = rig
        .proxy
        .wait(since, WAIT, |e| e.kind == EventKind::OperationHeld)
        .expect("held");
    client.kill();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(rig.count(), 0, "the statement has not run");
    rig.proxy.deliver(held.conn).unwrap();
    until("the late statement ran", || rig.count() == 1);
}

// ---------------------------------------------------------------------------
// The connection is broken while the client process is alive.
// ---------------------------------------------------------------------------

/// The client ended on its own (it was not killed): with the exit status and
/// what it printed.
fn ended_by_itself(client: &mut Client) -> (std::process::ExitStatus, String) {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(status) = client.child.try_wait().unwrap() {
            return (status, client.output());
        }
        assert!(Instant::now() < deadline, "the client did not notice");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_severed_connection_after_the_answer_leaves_the_commit_durable_and_the_client_alive() {
    let bins = require!();
    let rig = rig(bins);
    rig.proxy.arm("w#1", Fault::DropResponse, Target::Commit);
    let since = rig.proxy.cursor();
    let mut client = rig.client("w#1", PAY);
    let done = rig
        .proxy
        .wait(since, WAIT, |e| e.kind == EventKind::OperationCompleted)
        .expect("the server answered the COMMIT");
    assert!(done.detail.contains("CommandComplete COMMIT"), "{done:?}");
    let pid = rig.proxy.backend_pid(done.conn).unwrap();
    // Until now the client waits; nobody has touched it.
    assert!(client.child.try_wait().unwrap().is_none());
    rig.proxy.sever_client(done.conn).expect("sever");
    // The client process is still the one it was: it ends by itself, with an
    // error about its connection, never having been told the COMMIT's result.
    let (status, out) = ended_by_itself(&mut client);
    assert!(!status.success(), "{out}");
    assert!(!out.contains("after commit"), "{out}");
    until("the backend ended", || rig.backend_state(pid) == "gone");
    assert_eq!(
        rig.count(),
        1,
        "the COMMIT the client never heard about is durable"
    );
    let kinds: Vec<EventKind> = rig.proxy.events().iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&EventKind::ClientSevered));
    assert!(kinds.contains(&EventKind::ClientClosed));
}

#[test]
fn a_severed_connection_with_a_withheld_commit_keeps_the_transaction_for_a_late_delivery() {
    let bins = require!();
    let rig = rig(bins);
    rig.proxy.arm("w#1", Fault::HoldRequest, Target::Commit);
    let since = rig.proxy.cursor();
    let mut client = rig.client("w#1", PAY);
    let held = rig
        .proxy
        .wait(since, WAIT, |e| e.kind == EventKind::OperationHeld)
        .expect("the COMMIT was withheld");
    let pid = rig.proxy.backend_pid(held.conn).unwrap();
    rig.proxy.sever_client(held.conn).expect("sever");
    let (status, _) = ended_by_itself(&mut client);
    assert!(!status.success());
    // The client is gone from the connection; the server still has its open
    // transaction and no COMMIT.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(rig.backend_state(pid), "idle in transaction");
    assert_eq!(rig.count(), 0);
    let since = rig.proxy.cursor();
    rig.proxy.deliver(held.conn).expect("deliver");
    rig.proxy
        .wait(since, WAIT, |e| e.kind == EventKind::OperationCompleted)
        .expect("the server completed the late COMMIT");
    assert_eq!(rig.count(), 1);
}

#[test]
fn a_severed_connection_with_a_discarded_commit_rolls_the_transaction_back() {
    let bins = require!();
    let rig = rig(bins);
    rig.proxy.arm("w#1", Fault::HoldRequest, Target::Commit);
    let since = rig.proxy.cursor();
    let mut client = rig.client("w#1", PAY);
    let held = rig
        .proxy
        .wait(since, WAIT, |e| e.kind == EventKind::OperationHeld)
        .unwrap();
    let pid = rig.proxy.backend_pid(held.conn).unwrap();
    rig.proxy.sever_client(held.conn).expect("sever");
    rig.proxy.discard(held.conn);
    let (status, _) = ended_by_itself(&mut client);
    assert!(!status.success());
    until("the backend ended", || rig.backend_state(pid) == "gone");
    assert_eq!(rig.count(), 0);
}

#[test]
fn a_connection_can_only_be_severed_once_and_only_if_it_exists() {
    let bins = require!();
    let rig = rig(bins);
    assert!(rig.proxy.sever_client(99).is_err());
    rig.proxy.arm("w#1", Fault::HoldRequest, Target::Commit);
    let since = rig.proxy.cursor();
    let _client = rig.client("w#1", PAY);
    let held = rig
        .proxy
        .wait(since, WAIT, |e| e.kind == EventKind::OperationHeld)
        .unwrap();
    assert!(rig.proxy.sever_client(held.conn).is_ok());
    assert!(rig.proxy.sever_client(held.conn).is_err());
}

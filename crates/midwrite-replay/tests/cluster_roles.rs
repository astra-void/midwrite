//! The disposable cluster's roles: harnesses and project SQL connect as an
//! ordinary role, so they cannot make the (unsandboxed) server run commands
//! or write files outside the replay directory. Needs PostgreSQL binaries;
//! prints SKIPPED otherwise, unless `MIDWRITE_REQUIRE_REPLAY=1`.

use std::process::Command;

use midwrite_replay::postgres::{APP_USER, Binaries, Cluster, USER};
use midwrite_replay::safety::ReplayRoot;

fn psql(bins: &Binaries, cluster: &Cluster, user: &str, sql: &str) -> (bool, String) {
    let output = Command::new(bins.dir.join("psql"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .args(["-X", "-q", "-w", "-A", "-t", "-v", "ON_ERROR_STOP=1", "-h"])
        .arg(cluster.socket_dir())
        .args(["-p", "5432", "-U", user, "-d", "app", "-c", sql])
        .output()
        .expect("psql");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), text)
}

#[test]
fn harness_sessions_are_not_superusers_and_the_superuser_needs_its_password() {
    let bins = match Binaries::discover() {
        Ok(bins) => bins,
        Err(reason) => {
            assert!(
                std::env::var_os("MIDWRITE_REQUIRE_REPLAY").is_none(),
                "PostgreSQL required: {reason}"
            );
            eprintln!("SKIPPED: {reason}");
            return;
        }
    };
    let root = ReplayRoot::create(false).expect("private run directory");
    let cluster = Cluster::start(bins.clone(), &root.path.join("db")).expect("cluster");
    cluster.create_database("app").expect("database");
    let marker = root.path.join("EXECUTED");

    // The application role owns the database and can use it normally.
    let (ok, text) = psql(
        &bins,
        &cluster,
        APP_USER,
        "CREATE TABLE t (id int); INSERT INTO t VALUES (1); SELECT rolsuper FROM pg_roles WHERE rolname = current_user;",
    );
    assert!(ok, "{text}");
    assert_eq!(text.trim(), "f", "{text}");

    // It cannot make the server run a command or write a file.
    for sql in [
        format!("COPY (SELECT 1) TO PROGRAM 'touch {}'", marker.display()),
        format!("COPY t TO '{}'", marker.display()),
    ] {
        let (ok, text) = psql(&bins, &cluster, APP_USER, &sql);
        assert!(!ok, "accepted: {sql}: {text}");
    }
    assert!(!marker.exists());

    // Connecting as the superuser without the password fails: a harness
    // cannot simply pick the superuser's name.
    let (ok, text) = psql(&bins, &cluster, USER, "SELECT 1");
    assert!(!ok, "the superuser connected without a password: {text}");
    // Midwrite itself still can.
    assert_eq!(
        cluster
            .query_value("app", "SELECT count(*) FROM t")
            .expect("superuser query"),
        serde_json::json!(1)
    );
}

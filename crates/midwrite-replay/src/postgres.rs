//! A disposable PostgreSQL cluster for one replay invocation.
//!
//! The cluster lives in the replay's private temporary directory, listens on
//! a Unix socket in that directory only (`listen_addresses = ''`: no TCP),
//! and is stopped and deleted when the
//! [`Cluster`] is dropped. Every run gets a fresh database created from the
//! harness' setup files. No connection setting is ever read from the
//! environment: PostgreSQL client variables are cleared for every command.
//!
//! Two roles. [`USER`] is the bootstrap superuser, used only by Midwrite
//! itself (catalog reads, `pg_stat_activity`, creating databases); it needs
//! a random password that never leaves this process. [`APP_USER`] is an
//! ordinary role, trusted without a password, that owns every database
//! created here: harnesses connect as it, and project SQL (setup files,
//! migrations, observations) runs as it. A superuser session could make the
//! unsandboxed server run commands or write files (`COPY … PROGRAM`,
//! `COPY … TO '<file>'`), which would get around the harness isolation.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

pub const USER: &str = "midwrite";
/// The non-superuser role of harnesses and project SQL.
pub const APP_USER: &str = "midwrite_app";
const PORT: &str = "5432";

#[derive(Clone, Debug)]
pub struct Binaries {
    pub dir: PathBuf,
    pub version: String,
}

impl Binaries {
    /// Find `initdb`, `pg_ctl`, `postgres` and `psql` of one installation:
    /// `$MIDWRITE_PG_BIN`, else the directory of `postgres` on `PATH`
    /// (after resolving symlinks), else `pg_config --bindir`.
    pub fn discover() -> Result<Self, String> {
        let candidates: Vec<PathBuf> = std::env::var_os("MIDWRITE_PG_BIN")
            .map(PathBuf::from)
            .into_iter()
            .chain(
                find_on_path("postgres")
                    .and_then(|p| p.canonicalize().ok()?.parent().map(Path::to_path_buf)),
            )
            .collect();
        // `pg_config` only when nothing else was found, and only from an
        // absolute `PATH` entry.
        let pg_config = || {
            Command::new(find_on_path("pg_config")?)
                .arg("--bindir")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        };
        for dir in candidates
            .into_iter()
            .chain(std::iter::once_with(pg_config).flatten())
        {
            if ["initdb", "pg_ctl", "postgres", "psql"]
                .iter()
                .all(|tool| dir.join(tool).is_file())
            {
                let version = Command::new(dir.join("postgres"))
                    .arg("--version")
                    .output()
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                    .unwrap_or_default();
                return Ok(Self { dir, version });
            }
        }
        Err("PostgreSQL server binaries (initdb, pg_ctl, postgres, psql) were not found; install PostgreSQL or set MIDWRITE_PG_BIN".to_owned())
    }

    fn tool(&self, name: &str) -> Command {
        let mut command = Command::new(self.dir.join(name));
        command.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        command.env("LC_ALL", "C");
        command
    }
}

/// The major version in `postgres --version` output (`postgres (PostgreSQL)
/// 17.11` → 17); 0 when it cannot be read.
pub fn major_version(version: &str) -> u32 {
    version
        .split_whitespace()
        .find_map(|w| w.split('.').next().and_then(|m| m.parse().ok()))
        .unwrap_or(0)
}

/// The first `program` in an absolute `PATH` entry. Relative entries (`.`,
/// an empty entry, `node_modules/.bin`) are skipped: they resolve against
/// the working directory, which may be the repository being analyzed, so a
/// project could otherwise supply its own `initdb`, `psql` or `bwrap`.
pub fn find_on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// Refuse SQL that would make `psql` or the server run operating-system
/// commands: SQL files and observation queries are handed to `psql`, which
/// runs outside the harness isolation, as a superuser of the disposable
/// cluster. Refused: any `psql` meta-command (a backslash outside string
/// literals, quoted identifiers and comments: `\!`, `\o |cmd`, `\copy …
/// program`, `\i`), and `COPY … PROGRAM`. Nothing else is interpreted.
pub fn check_untrusted_sql(sql: &str, what: &str) -> Result<(), String> {
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut statement_has_copy = false;
    let mut word = String::new();
    let flush = |word: &mut String, copy: &mut bool| -> Result<(), String> {
        if word.eq_ignore_ascii_case("copy") {
            *copy = true;
        } else if *copy && word.eq_ignore_ascii_case("program") {
            return Err(format!(
                "{what}: `COPY … PROGRAM` runs an operating-system command and is refused"
            ));
        }
        word.clear();
        Ok(())
    };
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_alphanumeric() || c == b'_' {
            word.push(c as char);
            i += 1;
            continue;
        }
        // An `E'…'` string: backslashes escape inside it.
        let escape_string = word.eq_ignore_ascii_case("e") && c == b'\'';
        flush(&mut word, &mut statement_has_copy)?;
        match c {
            b'\\' => {
                return Err(format!(
                    "{what}: psql meta-commands (a backslash outside a string, such as `\\!`) are refused; they can run operating-system commands outside the isolation"
                ));
            }
            b';' => {
                statement_has_copy = false;
                i += 1;
            }
            b'\'' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' if escape_string => i += 2,
                        b'\'' if bytes.get(i + 1) == Some(&b'\'') => i += 2,
                        b'\'' => break,
                        _ => i += 1,
                    }
                }
                i += 1;
            }
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'"' if bytes.get(i + 1) == Some(&b'"') => i += 2,
                        b'"' => break,
                        _ => i += 1,
                    }
                }
                i += 1;
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let mut depth = 0;
                while i < bytes.len() {
                    if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        i += 2;
                    } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            b'$' => {
                // A dollar-quoted string `$tag$ … $tag$` (not a `$1` parameter).
                let rest = &sql[i + 1..];
                let tag_len = rest
                    .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                    .unwrap_or(rest.len());
                let starts_digit = rest.as_bytes().first().is_some_and(u8::is_ascii_digit);
                if !starts_digit && rest.as_bytes().get(tag_len) == Some(&b'$') {
                    let tag = &sql[i..i + tag_len + 2];
                    let body = i + tag.len();
                    i = sql[body..]
                        .find(tag)
                        .map_or(bytes.len(), |end| body + end + tag.len());
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    flush(&mut word, &mut statement_has_copy)
}

/// A running, private PostgreSQL cluster.
#[derive(Debug)]
pub struct Cluster {
    bins: Binaries,
    data: PathBuf,
    socket_dir: PathBuf,
    log: PathBuf,
    running: bool,
    /// The superuser's password (random per cluster, never in a harness'
    /// environment).
    password: String,
}

impl Cluster {
    /// Create and start a cluster under `root` (which must be private).
    pub fn start(bins: Binaries, root: &Path) -> Result<Self, String> {
        Self::start_with(bins, root, &[])
    }

    /// [`Self::start`] with extra server settings (`name=value`, names and
    /// values restricted to `[A-Za-z0-9_.]`).
    pub fn start_with(bins: Binaries, root: &Path, settings: &[String]) -> Result<Self, String> {
        // A cluster is a daemon: make Ctrl-C stop it through `Drop`.
        crate::cancel::install();
        for setting in settings {
            let ok = setting.split_once('=').is_some_and(|(k, v)| {
                !k.is_empty()
                    && !v.is_empty()
                    && setting
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '='))
            });
            if !ok {
                return Err(format!("refusing the server setting `{setting}`"));
            }
        }
        let data = root.join("pgdata");
        let socket_dir = root.join("pg");
        let log = root.join("postgres.log");
        std::fs::create_dir_all(&socket_dir).map_err(|e| e.to_string())?;
        let password = random_password();
        let pwfile = root.join("superuser.pw");
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&pwfile)
                .map_err(|e| format!("cannot write the password file: {e}"))?;
            writeln!(file, "{password}").map_err(|e| e.to_string())?;
        }
        let output = bins
            .tool("initdb")
            .args(["-D"])
            .arg(&data)
            .arg(format!("--pwfile={}", pwfile.display()))
            .args([
                "-U",
                USER,
                "--auth=scram-sha-256",
                "--no-sync",
                "--no-instructions",
                "-E",
                "UTF8",
                "--locale=C",
            ])
            .output()
            .map_err(|e| format!("cannot run initdb: {e}"))?;
        let _ = std::fs::remove_file(&pwfile);
        if !output.status.success() {
            return Err(format!("initdb failed: {}", tail(&output.stderr)));
        }
        // Only local (Unix-socket) connections: the superuser with its
        // password, the application role without one.
        std::fs::write(
            data.join("pg_hba.conf"),
            format!("local all {USER} scram-sha-256\nlocal all {APP_USER} trust\n"),
        )
        .map_err(|e| format!("cannot write pg_hba.conf: {e}"))?;
        let mut cluster = Self {
            bins,
            data,
            socket_dir,
            log,
            running: false,
            password,
        };
        let mut options = format!(
            "-c listen_addresses='' -k {} -p {PORT} -c max_connections=40",
            cluster.socket_dir.display()
        );
        for setting in settings {
            options.push_str(&format!(" -c {setting}"));
        }
        let output = cluster
            .bins
            .tool("pg_ctl")
            .arg("-D")
            .arg(&cluster.data)
            .arg("-l")
            .arg(&cluster.log)
            .args(["-w", "-t", "30", "-o", &options, "start"])
            .stdout(Stdio::null())
            .output()
            .map_err(|e| format!("cannot run pg_ctl: {e}"))?;
        cluster.running = true;
        if !output.status.success() {
            let log = std::fs::read_to_string(&cluster.log).unwrap_or_default();
            return Err(format!(
                "PostgreSQL did not start: {} {}",
                tail(&output.stderr),
                tail(log.as_bytes())
            ));
        }
        let mut command = cluster.psql("postgres", false);
        command.args([
            "-c",
            &format!(
                "CREATE ROLE {APP_USER} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS"
            ),
        ]);
        cluster.run(command)?;
        Ok(cluster)
    }

    /// The environment variable that authenticates a client as [`USER`]
    /// (`PGPASSWORD`). For Midwrite's own tools only; never hand it to a
    /// harness.
    pub fn superuser_env(&self) -> (&'static str, &str) {
        ("PGPASSWORD", &self.password)
    }

    pub fn version(&self) -> &str {
        &self.bins.version
    }

    pub fn socket_dir(&self) -> &Path {
        &self.socket_dir
    }

    /// Full path of the server's socket file.
    pub fn socket_path(&self) -> PathBuf {
        self.socket_dir.join(format!(".s.PGSQL.{PORT}"))
    }

    fn psql(&self, database: &str, read_only: bool) -> Command {
        let mut command = self.psql_as(USER, database, read_only);
        command.env("PGPASSWORD", &self.password);
        command
    }

    fn psql_as(&self, user: &str, database: &str, read_only: bool) -> Command {
        let mut command = self.bins.tool("psql");
        command
            .arg("-X")
            .arg("-q")
            // Never prompt for a password on the terminal.
            .arg("-w")
            .args(["-v", "ON_ERROR_STOP=1"])
            .arg("-h")
            .arg(&self.socket_dir)
            .args(["-p", PORT, "-U", user, "-d", database]);
        if read_only {
            command.env("PGOPTIONS", "-c default_transaction_read_only=on");
        }
        command
    }

    fn run(&self, mut command: Command) -> Result<Output, String> {
        let output = command
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("cannot run psql: {e}"))?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(tail(&output.stderr))
        }
    }

    pub fn create_database(&self, name: &str) -> Result<(), String> {
        let mut command = self.psql("postgres", false);
        command.args([
            "-c",
            &format!("CREATE DATABASE \"{name}\" OWNER {APP_USER}"),
        ]);
        self.run(command).map(|_| ())
    }

    pub fn drop_database(&self, name: &str) -> Result<(), String> {
        let mut command = self.psql("postgres", false);
        command.args([
            "-c",
            &format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"),
        ]);
        self.run(command).map(|_| ())
    }

    /// Run an SQL file as [`APP_USER`] (project SQL: setup files,
    /// migrations, seeds).
    pub fn apply_file(&self, database: &str, file: &Path) -> Result<(), String> {
        let mut command = self.psql_as(APP_USER, database, false);
        command.arg("-f").arg(file);
        self.run(command).map(|_| ())
    }

    /// Run a query in a read-only session and return its single value:
    /// `null` for no row, a number for an integer, otherwise the text.
    pub fn query_value(&self, database: &str, sql: &str) -> Result<Value, String> {
        let command = self.psql(database, true);
        self.single_value(command, sql)
    }

    /// [`Self::query_value`] as [`APP_USER`]: for queries written by the
    /// project (observations).
    pub fn query_value_as_app(&self, database: &str, sql: &str) -> Result<Value, String> {
        let command = self.psql_as(APP_USER, database, true);
        self.single_value(command, sql)
    }

    fn single_value(&self, mut command: Command, sql: &str) -> Result<Value, String> {
        command.args(["-A", "-t", "-F", "\u{1f}", "-c", sql]);
        let output = self.run(command)?;
        let text = String::from_utf8_lossy(&output.stdout);
        let rows: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
        match rows.as_slice() {
            [] => Ok(Value::Null),
            [row] if !row.contains('\u{1f}') => Ok(parse_value(row)),
            _ => Err("the query must return at most one row with one column".to_owned()),
        }
    }

    /// [`Self::query_value`] for several queries with one `psql` process
    /// (one read-only session, each query in its own statement). On any
    /// error, or if the output cannot be split unambiguously, the queries
    /// are run one by one so that the error names the failing query.
    /// Runs as [`APP_USER`]: the queries are the project's observations.
    pub fn query_values(&self, database: &str, queries: &[&str]) -> Result<Vec<Value>, String> {
        const SEPARATOR: &str = "\u{1e}midwrite-observation-separator\u{1e}";
        if queries.len() > 1 {
            let mut command = self.psql_as(APP_USER, database, true);
            command.args(["-A", "-t", "-F", "\u{1f}"]);
            for (index, sql) in queries.iter().enumerate() {
                if index > 0 {
                    command.args(["-c", &format!("\\echo {SEPARATOR}")]);
                }
                command.args(["-c", sql]);
            }
            if let Ok(output) = self.run(command) {
                let text = String::from_utf8_lossy(&output.stdout);
                let sections: Vec<&str> = text.split(&format!("{SEPARATOR}\n")).collect();
                if sections.len() == queries.len() {
                    let parsed: Option<Vec<Value>> = sections
                        .iter()
                        .map(|section| {
                            let rows: Vec<&str> =
                                section.lines().filter(|l| !l.is_empty()).collect();
                            match rows.as_slice() {
                                [] => Some(Value::Null),
                                [row] if !row.contains('\u{1f}') => Some(parse_value(row)),
                                _ => None,
                            }
                        })
                        .collect();
                    if let Some(values) = parsed {
                        return Ok(values);
                    }
                }
            }
        }
        queries
            .iter()
            .map(|sql| self.query_value_as_app(database, sql))
            .collect()
    }

    /// Wait until no session other than the caller's is connected to
    /// `database` (the killed harness' backends have exited and rolled back
    /// their open transactions).
    pub fn wait_for_idle(&self, database: &str, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let sql = format!(
            "SELECT count(*) FROM pg_stat_activity WHERE datname = '{database}' AND pid <> pg_backend_pid()"
        );
        loop {
            match self.query_value("postgres", &sql)? {
                Value::Number(n) if n.as_i64() == Some(0) => return Ok(()),
                _ if Instant::now() >= deadline => {
                    return Err(
                        "sessions of the harness were still open after the deadline".to_owned()
                    );
                }
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    pub fn stop(&mut self) {
        if !self.running {
            return;
        }
        self.running = false;
        let _ = self
            .bins
            .tool("pg_ctl")
            .arg("-D")
            .arg(&self.data)
            .args(["-m", "immediate", "-w", "-t", "30", "stop"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 32 random hexadecimal characters from `/dev/urandom`.
fn random_password() -> String {
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
    out.push_str(&format!("{nanos:x}"));
    out
}

fn parse_value(text: &str) -> Value {
    if let Ok(number) = text.parse::<i64>() {
        return Value::from(number);
    }
    Value::String(text.to_owned())
}

fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    let start = text
        .char_indices()
        .rev()
        .nth(1200)
        .map_or(0, |(index, _)| index);
    text[start..].to_owned()
}

#[cfg(test)]
mod untrusted_sql_tests {
    use super::check_untrusted_sql;

    #[test]
    fn shell_escapes_are_refused() {
        for sql in [
            "\\! touch /tmp/pwned",
            "select 1;\n\\! id",
            "create table t (a int);\\o |sh",
            "\\copy t to program 'id'",
            "\\i /etc/passwd",
            "COPY t TO PROGRAM 'touch /tmp/x'",
            "copy (select 1) to\n  program 'id';",
            "COPY t FROM PROGRAM 'curl example.com'",
        ] {
            assert!(check_untrusted_sql(sql, "t").is_err(), "accepted: {sql}");
        }
    }

    #[test]
    fn ordinary_migrations_and_quoted_backslashes_are_accepted() {
        for sql in [
            "CREATE TABLE \"orders\" (\"id\" serial PRIMARY KEY, \"note\" text DEFAULT 'a\\b');\n--> statement-breakpoint\nCREATE INDEX ON orders (id);",
            "select E'it\\'s \\\\ ok', 'x''\\y';",
            "insert into t values ($$ \\! not a command $$), ($tag$ \\o $tag$);",
            "select $1::int; -- \\! in a comment\n/* \\! nested /* \\x */ */ select 2;",
            "COPY t FROM STDIN; select 'program';",
            "create table program (id int); copy t to stdout; select program from program;",
        ] {
            check_untrusted_sql(sql, "t").unwrap_or_else(|e| panic!("refused {sql}: {e}"));
        }
    }
}

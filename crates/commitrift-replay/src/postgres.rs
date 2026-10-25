//! A disposable PostgreSQL cluster for one replay invocation.
//!
//! The cluster lives in the replay's private temporary directory, listens on
//! a Unix socket in that directory only (`listen_addresses = ''`: no TCP),
//! trusts the local user `commitrift`, and is stopped and deleted when the
//! [`Cluster`] is dropped. Every run gets a fresh database created from the
//! harness' setup files. No connection setting is ever read from the
//! environment: PostgreSQL client variables are cleared for every command.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

pub const USER: &str = "commitrift";
const PORT: &str = "5432";

#[derive(Clone, Debug)]
pub struct Binaries {
    pub dir: PathBuf,
    pub version: String,
}

impl Binaries {
    /// Find `initdb`, `pg_ctl`, `postgres` and `psql` of one installation:
    /// `$COMMITRIFT_PG_BIN`, else the directory of `postgres` on `PATH`
    /// (after resolving symlinks), else `pg_config --bindir`.
    pub fn discover() -> Result<Self, String> {
        let candidates: Vec<PathBuf> = std::env::var_os("COMMITRIFT_PG_BIN")
            .map(PathBuf::from)
            .into_iter()
            .chain(
                find_on_path("postgres")
                    .and_then(|p| p.canonicalize().ok()?.parent().map(Path::to_path_buf)),
            )
            .chain(
                Command::new("pg_config")
                    .arg("--bindir")
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim())),
            )
            .collect();
        for dir in candidates {
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
        Err("PostgreSQL server binaries (initdb, pg_ctl, postgres, psql) were not found; install PostgreSQL or set COMMITRIFT_PG_BIN".to_owned())
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

pub fn find_on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// A running, private PostgreSQL cluster.
#[derive(Debug)]
pub struct Cluster {
    bins: Binaries,
    data: PathBuf,
    socket_dir: PathBuf,
    log: PathBuf,
    running: bool,
}

impl Cluster {
    /// Create and start a cluster under `root` (which must be private).
    pub fn start(bins: Binaries, root: &Path) -> Result<Self, String> {
        let data = root.join("pgdata");
        let socket_dir = root.join("pg");
        let log = root.join("postgres.log");
        std::fs::create_dir_all(&socket_dir).map_err(|e| e.to_string())?;
        let output = bins
            .tool("initdb")
            .args(["-D"])
            .arg(&data)
            .args([
                "-U",
                USER,
                "--auth=trust",
                "--no-sync",
                "--no-instructions",
                "-E",
                "UTF8",
                "--locale=C",
            ])
            .output()
            .map_err(|e| format!("cannot run initdb: {e}"))?;
        if !output.status.success() {
            return Err(format!("initdb failed: {}", tail(&output.stderr)));
        }
        let mut cluster = Self {
            bins,
            data,
            socket_dir,
            log,
            running: false,
        };
        let options = format!(
            "-c listen_addresses='' -k {} -p {PORT} -c max_connections=40",
            cluster.socket_dir.display()
        );
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
        Ok(cluster)
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
        let mut command = self.bins.tool("psql");
        command
            .arg("-X")
            .arg("-q")
            .args(["-v", "ON_ERROR_STOP=1"])
            .arg("-h")
            .arg(&self.socket_dir)
            .args(["-p", PORT, "-U", USER, "-d", database]);
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
        command.args(["-c", &format!("CREATE DATABASE \"{name}\"")]);
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

    pub fn apply_file(&self, database: &str, file: &Path) -> Result<(), String> {
        let mut command = self.psql(database, false);
        command.arg("-f").arg(file);
        self.run(command).map(|_| ())
    }

    /// Run a query in a read-only session and return its single value:
    /// `null` for no row, a number for an integer, otherwise the text.
    pub fn query_value(&self, database: &str, sql: &str) -> Result<Value, String> {
        let mut command = self.psql(database, true);
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
    pub fn query_values(&self, database: &str, queries: &[&str]) -> Result<Vec<Value>, String> {
        const SEPARATOR: &str = "\u{1e}commitrift-observation-separator\u{1e}";
        if queries.len() > 1 {
            let mut command = self.psql(database, true);
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
            .map(|sql| self.query_value(database, sql))
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

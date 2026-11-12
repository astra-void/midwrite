//! The replay manifest (`midwrite-replay.json`): what a harness declares
//! about itself. Everything the supervisor executes comes from here, so it
//! is validated before anything runs, and validation errors never echo
//! values (a rejected database URL may contain a password).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use midwrite_core::verification::{FailureMode, VerificationLevel};

pub const MANIFEST_FILE_NAME: &str = "midwrite-replay.json";
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
const MAX_TIMEOUT_SECONDS: u64 = 300;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    #[serde(rename = "$schema", default)]
    pub schema: Option<String>,
    pub schema_version: u32,
    /// Identifier of the harness (`[a-z0-9-]+`).
    pub harness: String,
    #[serde(default)]
    pub description: String,
    /// The JavaScript runtime the command starts (optional). When declared,
    /// `command[0]` must be that runtime, its version is checked before
    /// anything runs, and the entry point must exist inside the harness
    /// directory (replay never installs packages or compiles code).
    #[serde(default)]
    pub runtime: Option<RuntimeKind>,
    /// Program and arguments, run from the harness directory without a
    /// shell.
    pub command: Vec<String>,
    /// Deadline of one harness process (seconds).
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub network: NetworkPolicy,
    pub database: DatabaseSpec,
    /// What to analyze statically to link cases to findings.
    #[serde(default)]
    pub analysis: Option<AnalysisSpec>,
    /// Extra environment variables for the harness (validated; never
    /// credentials or database settings).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub observations: Vec<ObservationSpec>,
    /// The application contract the harness author declares. A failure is a
    /// bug only when it violates one of these.
    pub invariants: Vec<InvariantSpec>,
    /// The scenario run once without injection to check that the invariants
    /// hold when nothing fails.
    pub control: ScenarioCall,
    pub cases: Vec<CaseSpec>,
}

fn default_timeout() -> u64 {
    30
}

/// A supported harness runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeKind {
    /// Node.js running compiled JavaScript (see docs/architecture/replay-protocol.md).
    Node,
    /// Bun running TypeScript directly.
    Bun,
}

impl RuntimeKind {
    pub fn program(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Bun => "bun",
        }
    }

    /// The oldest version Midwrite supports, as (major, minor).
    pub fn minimum(self) -> (u64, u64) {
        match self {
            Self::Node => (22, 0),
            Self::Bun => (1, 2),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkPolicy {
    /// Run the harness with external network access denied; refuse to run
    /// when the platform offers no way to deny it (unless overridden on the
    /// command line).
    #[default]
    Deny,
    /// The harness may use the network (not recommended).
    Allow,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseSpec {
    /// Only `ephemeral-postgres`: a PostgreSQL cluster the supervisor
    /// creates in a private temporary directory, reachable only through a
    /// Unix socket, and deletes afterwards.
    pub provider: String,
    /// SQL files (relative to the harness directory) applied to a fresh
    /// database before every run.
    #[serde(default)]
    pub setup: Vec<String>,
    // Fields that would point the harness at an existing database. They are
    // accepted by the parser only to be rejected with a clear message.
    #[serde(default)]
    url: Option<Value>,
    #[serde(default)]
    connection_string: Option<Value>,
    #[serde(default)]
    host: Option<Value>,
    #[serde(default)]
    port: Option<Value>,
    #[serde(default)]
    password: Option<Value>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnalysisSpec {
    /// The directory static analysis runs from, relative to the harness
    /// directory: the harness directory itself (default) or one of its
    /// ancestors (`".."` for a harness inside the project it tests). Finding
    /// fingerprints contain paths relative to this root, so it must be the
    /// directory `midwrite check`/`trace` runs in for the identities to
    /// match. Static analysis only reads files.
    #[serde(default)]
    pub root: Option<String>,
    /// Files or directories, relative to the analysis root.
    pub paths: Vec<String>,
    /// Configuration file relative to the analysis root (default: its
    /// `midwrite.json`, if present).
    #[serde(default)]
    pub config: Option<String>,
}

impl AnalysisSpec {
    /// The analysis root for a harness in `dir` (canonical). Validated to be
    /// `dir` or an ancestor of it.
    pub fn root_dir(&self, dir: &Path) -> Result<PathBuf, String> {
        let Some(root) = &self.root else {
            return Ok(dir.to_path_buf());
        };
        let path = Path::new(root);
        if root.is_empty()
            || !path.is_relative()
            || !path
                .components()
                .all(|c| matches!(c, Component::ParentDir | Component::CurDir))
        {
            return Err(format!(
                "analysis.root `{root}` must be `.` or a relative path made of `..` segments (the harness directory or one of its ancestors)"
            ));
        }
        let resolved = dir
            .join(path)
            .canonicalize()
            .map_err(|_| format!("analysis.root `{root}` does not exist"))?;
        if !dir.starts_with(&resolved) {
            return Err(format!(
                "analysis.root `{root}` is not an ancestor of the harness directory"
            ));
        }
        Ok(resolved)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservationSpec {
    pub name: String,
    /// A read-only query returning at most one row with one column.
    #[serde(default)]
    pub sql: Option<String>,
    /// A measure over the harness' event log.
    #[serde(default)]
    pub events: Option<EventMeasure>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventMeasure {
    /// Only events with this topic (all events when omitted).
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub measure: Measure,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Measure {
    /// Number of published events.
    #[default]
    Count,
    /// Number of distinct `id`s among them (events without an id count as
    /// distinct).
    DistinctIds,
    /// `count - distinctIds`: deliveries of an already delivered id.
    Duplicates,
    /// Number of events without an `id` (consumers cannot deduplicate them).
    WithoutId,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConditionSpec {
    pub observation: String,
    #[serde(default)]
    pub equals: Option<Value>,
    #[serde(default)]
    pub not_equals: Option<Value>,
    #[serde(default)]
    pub at_least: Option<i64>,
    #[serde(default)]
    pub at_most: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvariantSpec {
    pub id: String,
    pub description: String,
    /// The invariant applies only when all of these hold.
    #[serde(default)]
    pub when: Vec<ConditionSpec>,
    /// All of these must hold.
    #[serde(default)]
    pub require: Vec<ConditionSpec>,
    /// At least one of these must hold.
    #[serde(default)]
    pub require_any: Vec<ConditionSpec>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioCall {
    pub scenario: String,
    #[serde(default)]
    pub input: Value,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaseSpec {
    pub id: String,
    #[serde(default)]
    pub description: String,
    /// The static finding the case exercises.
    #[serde(default)]
    pub finding: Option<FindingSelector>,
    pub scenario: String,
    #[serde(default)]
    pub input: Value,
    /// Where and how to fail. Omitted: the scenario runs without injection.
    #[serde(default)]
    pub inject: Option<InjectSpec>,
    /// Checked against the database while the harness waits at the
    /// checkpoint, before injecting (e.g. that a write is visible from
    /// another session, i.e. committed). A failing check makes the run
    /// inconclusive instead of injecting.
    #[serde(default)]
    pub verify_before_injection: Vec<ConditionSpec>,
    /// The surviving state the trace predicts (observation -> value).
    #[serde(default)]
    pub predict: BTreeMap<String, Value>,
    /// Scenarios run afterwards without injection (a relay, a retry).
    #[serde(default)]
    pub recovery: Vec<ScenarioCall>,
    /// The level the author expects, for regression checks.
    #[serde(default)]
    pub expect: Option<VerificationLevel>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FindingSelector {
    /// A diagnostic fingerprint.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub function: Option<String>,
    /// File path relative to the analysis root (the harness directory
    /// unless `analysis.root` says otherwise).
    #[serde(default)]
    pub file: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InjectSpec {
    pub checkpoint: String,
    #[serde(default = "one")]
    pub occurrence: u32,
    #[serde(default)]
    pub label: Option<String>,
    pub mode: FailureMode,
}

fn one() -> u32 {
    1
}

/// A manifest read from disk and validated.
#[derive(Clone, Debug)]
pub struct LoadedHarness {
    pub dir: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: Manifest,
    pub manifest_hash: String,
}

impl LoadedHarness {
    /// Read and validate `<dir>/midwrite-replay.json`. Nothing is executed.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let manifest_path = dir.join(MANIFEST_FILE_NAME);
        if !manifest_path.is_file() {
            return Err(format!(
                "no replay harness at `{}`: `{MANIFEST_FILE_NAME}` not found (replay only runs harnesses that declare themselves)",
                dir.display()
            ));
        }
        let bytes = std::fs::read(&manifest_path)
            .map_err(|e| format!("cannot read `{}`: {e}", manifest_path.display()))?;
        if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
            let unfinished = placeholders(&value);
            if !unfinished.is_empty() {
                return Err(format!(
                    "the replay manifest `{}` still has {} unfinished placeholder(s) ({PLACEHOLDER_MARKER}) at {}; replace them with your application's observations, invariants and scenarios before running replay",
                    manifest_path.display(),
                    unfinished.len(),
                    unfinished.join(", ")
                ));
            }
        }
        let manifest: Manifest = serde_json::from_slice(&bytes)
            .map_err(|e| format!("invalid replay manifest `{}`: {e}", manifest_path.display()))?;
        let dir = dir
            .canonicalize()
            .map_err(|e| format!("cannot resolve `{}`: {e}", dir.display()))?;
        manifest
            .validate(&dir)
            .map_err(|e| format!("invalid replay manifest `{}`: {e}", manifest_path.display()))?;
        let hash = midwrite_core::diagnostic::stable_hash(&[&String::from_utf8_lossy(&bytes)]);
        Ok(Self {
            dir,
            manifest_path,
            manifest,
            manifest_hash: format!("{hash:016x}"),
        })
    }
}

/// Marks a value a developer must replace (written by `midwrite
/// init-replay`). A manifest that contains it anywhere is refused.
pub const PLACEHOLDER_MARKER: &str = "MIDWRITE-TODO";

/// JSON pointers of every string (key or value) containing the marker.
pub fn placeholders(value: &Value) -> Vec<String> {
    fn walk(value: &Value, pointer: &str, out: &mut Vec<String>) {
        match value {
            Value::String(text) if text.contains(PLACEHOLDER_MARKER) => {
                out.push(if pointer.is_empty() {
                    "/".to_owned()
                } else {
                    pointer.to_owned()
                });
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    walk(item, &format!("{pointer}/{index}"), out);
                }
            }
            Value::Object(map) => {
                for (key, item) in map {
                    let escaped = key.replace('~', "~0").replace('/', "~1");
                    let child = format!("{pointer}/{escaped}");
                    if key.contains(PLACEHOLDER_MARKER) {
                        out.push(child.clone());
                    }
                    walk(item, &child, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(value, "", &mut out);
    out
}

fn is_slug(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_name(text: &str) -> bool {
    midwrite_core::config::is_identifier(text)
}

/// Environment variable names a manifest may never set: database targets,
/// replay internals, and anything that looks like a credential.
pub fn forbidden_env_name(name: &str) -> Option<&'static str> {
    let upper = name.to_ascii_uppercase();
    if !upper.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        || !upper
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        || name != upper
    {
        return Some("names must match [A-Z][A-Z0-9_]*");
    }
    const PREFIXES: &[&str] = &[
        "PG", "DATABASE", "DB_", "MIDWRITE", "LD_", "DYLD_", "NODE_", "BUN_",
    ];
    if PREFIXES.iter().any(|p| upper.starts_with(p)) || upper == "PATH" || upper == "HOME" {
        return Some("database, loader and replay settings are provided by the supervisor");
    }
    const SECRETS: &[&str] = &[
        "SECRET",
        "TOKEN",
        "PASSWORD",
        "PASSWD",
        "KEY",
        "CREDENTIAL",
        "AUTH",
        "AWS_",
        "STRIPE",
        "RESEND",
        "SESSION",
        "COOKIE",
        "PRIVATE",
        "DSN",
        "URL",
        "URI",
    ];
    if SECRETS.iter().any(|s| upper.contains(s)) {
        return Some("credentials, connection strings and URLs are never passed to a harness");
    }
    None
}

/// Whether `relative` stays inside a directory: relative, without `..`.
fn is_contained(relative: &str) -> bool {
    let path = Path::new(relative);
    !relative.is_empty()
        && path.is_relative()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

impl Manifest {
    /// The script a declared runtime runs: the first argument that is not
    /// an option.
    pub fn entry_point(&self) -> Option<&str> {
        self.command[1..]
            .iter()
            .map(String::as_str)
            .find(|arg| !arg.starts_with('-'))
    }

    pub fn validate(&self, dir: &Path) -> Result<(), String> {
        if self.schema_version != MANIFEST_SCHEMA_VERSION {
            return Err(format!(
                "schemaVersion {} is not supported (expected {MANIFEST_SCHEMA_VERSION})",
                self.schema_version
            ));
        }
        if !is_slug(&self.harness) {
            return Err("`harness` must match [a-z0-9-]+".to_owned());
        }
        match self.command.first() {
            None => return Err("`command` is empty".to_owned()),
            Some(program) if program.trim().is_empty() || program.contains(char::is_whitespace) => {
                return Err("`command[0]` must be a program name or path without spaces; the command is not run through a shell".to_owned());
            }
            _ => {}
        }
        if self.command.iter().any(String::is_empty) {
            return Err("`command` contains an empty argument".to_owned());
        }
        if let Some(runtime) = self.runtime {
            let program = Path::new(&self.command[0])
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if program != runtime.program() {
                return Err(format!(
                    "`runtime` is `{}` but `command[0]` does not start it; use `[\"{}\", \"<entry point>\", ...]`",
                    runtime.program(),
                    runtime.program()
                ));
            }
            match self.entry_point() {
                Some(entry) if is_contained(entry) => {}
                _ => {
                    return Err(format!(
                        "`runtime`: the first argument after `{}` must be an entry point inside the harness directory (relative, without `..`)",
                        runtime.program()
                    ));
                }
            }
        }
        if self.timeout_seconds == 0 || self.timeout_seconds > MAX_TIMEOUT_SECONDS {
            return Err(format!(
                "`timeoutSeconds` must be between 1 and {MAX_TIMEOUT_SECONDS}"
            ));
        }
        self.database.validate(dir)?;
        if let Some(analysis) = &self.analysis {
            analysis.root_dir(dir)?;
            for path in analysis.paths.iter().chain(&analysis.config) {
                if !is_contained(path) {
                    return Err(format!(
                        "analysis path `{path}` must be relative to the analysis root and must not contain `..`"
                    ));
                }
            }
        }
        for (name, value) in &self.env {
            if let Some(reason) = forbidden_env_name(name) {
                return Err(format!("env: `{name}` is not allowed: {reason}"));
            }
            if value.contains("://") || value.len() > 256 {
                return Err(format!(
                    "env: the value of `{name}` is not allowed (no URLs, at most 256 characters)"
                ));
            }
        }

        let mut names = BTreeSet::new();
        for observation in &self.observations {
            if !is_name(&observation.name) {
                return Err(format!(
                    "observation `{}`: names must be identifiers",
                    observation.name
                ));
            }
            if !names.insert(observation.name.as_str()) {
                return Err(format!(
                    "observation `{}` is declared twice",
                    observation.name
                ));
            }
            match (&observation.sql, &observation.events) {
                (Some(sql), None) if !sql.trim().is_empty() => {
                    crate::postgres::check_untrusted_sql(
                        sql,
                        &format!("observation `{}`", observation.name),
                    )?;
                }
                (None, Some(_)) => {}
                _ => {
                    return Err(format!(
                        "observation `{}`: specify exactly one of `sql` or `events`",
                        observation.name
                    ));
                }
            }
        }
        let check = |condition: &ConditionSpec, context: &str| -> Result<(), String> {
            if !names.contains(condition.observation.as_str()) {
                return Err(format!(
                    "{context}: unknown observation `{}`",
                    condition.observation
                ));
            }
            let operators = usize::from(condition.equals.is_some())
                + usize::from(condition.not_equals.is_some())
                + usize::from(condition.at_least.is_some())
                + usize::from(condition.at_most.is_some());
            if operators != 1 {
                return Err(format!(
                    "{context}: a condition on `{}` needs exactly one of equals, notEquals, atLeast, atMost",
                    condition.observation
                ));
            }
            Ok(())
        };
        if self.invariants.is_empty() {
            return Err("declare at least one invariant: without an application contract a runtime result cannot be classified".to_owned());
        }
        let mut invariant_ids = BTreeSet::new();
        for invariant in &self.invariants {
            if !is_slug(&invariant.id) {
                return Err(format!(
                    "invariant `{}`: ids must match [a-z0-9-]+",
                    invariant.id
                ));
            }
            if !invariant_ids.insert(invariant.id.as_str()) {
                return Err(format!("invariant `{}` is declared twice", invariant.id));
            }
            if invariant.require.is_empty() && invariant.require_any.is_empty() {
                return Err(format!(
                    "invariant `{}`: `require` or `requireAny` must not be empty",
                    invariant.id
                ));
            }
            let context = format!("invariant `{}`", invariant.id);
            for condition in invariant
                .when
                .iter()
                .chain(&invariant.require)
                .chain(&invariant.require_any)
            {
                check(condition, &context)?;
            }
        }
        if !is_name(&self.control.scenario) {
            return Err("control: scenario names must be identifiers".to_owned());
        }
        if self.cases.is_empty() {
            return Err("no cases".to_owned());
        }
        let mut case_ids = BTreeSet::new();
        for case in &self.cases {
            let context = format!("case `{}`", case.id);
            if !is_slug(&case.id) {
                return Err(format!("{context}: ids must match [a-z0-9-]+"));
            }
            if !case_ids.insert(case.id.as_str()) {
                return Err(format!("{context} is declared twice"));
            }
            if !is_name(&case.scenario) {
                return Err(format!("{context}: scenario names must be identifiers"));
            }
            for recovery in &case.recovery {
                if !is_name(&recovery.scenario) {
                    return Err(format!(
                        "{context}: recovery scenario names must be identifiers"
                    ));
                }
            }
            if let Some(inject) = &case.inject {
                if inject.mode == FailureMode::None {
                    return Err(format!(
                        "{context}: `inject.mode` must be `exception` or `processKill` (omit `inject` for a run without injection)"
                    ));
                }
                if inject.occurrence == 0 {
                    return Err(format!("{context}: `inject.occurrence` counts from 1"));
                }
                if inject.checkpoint.is_empty() {
                    return Err(format!("{context}: empty checkpoint name"));
                }
            } else if !case.verify_before_injection.is_empty() {
                return Err(format!("{context}: `verifyBeforeInjection` needs `inject`"));
            }
            for condition in &case.verify_before_injection {
                check(condition, &context)?;
            }
            for name in case.predict.keys() {
                if !names.contains(name.as_str()) {
                    return Err(format!(
                        "{context}: prediction for unknown observation `{name}`"
                    ));
                }
            }
            if let Some(level) = case.expect
                && !level.is_runtime()
            {
                return Err(format!(
                    "{context}: `expect` must be a runtime level (RUNTIME_REPRODUCED, RUNTIME_NOT_REPRODUCED or INCONCLUSIVE)"
                ));
            }
        }
        Ok(())
    }
}

impl DatabaseSpec {
    fn validate(&self, dir: &Path) -> Result<(), String> {
        let external = [
            ("url", self.url.is_some()),
            ("connectionString", self.connection_string.is_some()),
            ("host", self.host.is_some()),
            ("port", self.port.is_some()),
            ("password", self.password.is_some()),
        ];
        if let Some((field, _)) = external.iter().find(|(_, present)| *present) {
            // The value is deliberately not echoed: it may hold a password.
            return Err(format!(
                "database.{field}: external database targets are refused; replay only runs against a disposable PostgreSQL cluster it creates (`\"provider\": \"ephemeral-postgres\"`)"
            ));
        }
        if self.provider != "ephemeral-postgres" {
            return Err(
                "database.provider must be `ephemeral-postgres` (the only supported provider)"
                    .to_owned(),
            );
        }
        for file in &self.setup {
            if !is_contained(file) {
                return Err(format!(
                    "database.setup: `{file}` must be relative to the harness directory and must not contain `..`"
                ));
            }
            let path = dir.join(file);
            let resolved = path
                .canonicalize()
                .map_err(|_| format!("database.setup: `{file}` does not exist"))?;
            if !resolved.starts_with(dir) {
                return Err(format!(
                    "database.setup: `{file}` resolves outside the harness directory"
                ));
            }
            let sql = std::fs::read_to_string(&resolved)
                .map_err(|e| format!("database.setup: `{file}`: {e}"))?;
            crate::postgres::check_untrusted_sql(&sql, &format!("database.setup `{file}`"))?;
            if sql.contains(PLACEHOLDER_MARKER) {
                return Err(format!(
                    "database.setup: `{file}` still contains {PLACEHOLDER_MARKER} placeholders; replace them with your application's schema and initial state"
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(database: &str, extra: &str) -> String {
        format!(
            r#"{{ "schemaVersion": 1, "harness": "t", "command": ["bun", "h.ts"], {extra}
              "database": {database},
              "observations": [{{ "name": "n", "sql": "select 1" }}],
              "invariants": [{{ "id": "i", "description": "d", "require": [{{ "observation": "n", "atMost": 1 }}] }}],
              "control": {{ "scenario": "s" }},
              "cases": [{{ "id": "c", "scenario": "s" }}] }}"#
        )
    }

    fn validate(text: &str) -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("midwrite-manifest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("schema.sql"), "").unwrap();
        let dir = dir.canonicalize().unwrap();
        let parsed: Manifest = serde_json::from_str(text).map_err(|e| e.to_string())?;
        parsed.validate(&dir)
    }

    const DB: &str = r#"{ "provider": "ephemeral-postgres", "setup": ["schema.sql"] }"#;

    #[test]
    fn a_minimal_manifest_is_valid() {
        validate(&manifest(DB, "")).unwrap();
    }

    #[test]
    fn external_databases_are_refused_without_echoing_them() {
        for field in ["url", "connectionString", "host", "password"] {
            let db = format!(
                r#"{{ "provider": "ephemeral-postgres", "{field}": "postgres://admin:hunter2@prod/app" }}"#
            );
            let error = validate(&manifest(&db, "")).unwrap_err();
            assert!(
                error.contains("external database targets are refused"),
                "{error}"
            );
            assert!(
                !error.contains("hunter2") && !error.contains("prod"),
                "{error}"
            );
        }
        let error = validate(&manifest(r#"{ "provider": "postgres" }"#, "")).unwrap_err();
        assert!(error.contains("ephemeral-postgres"), "{error}");
    }

    #[test]
    fn setup_files_must_stay_inside_the_harness() {
        for file in ["../x.sql", "/etc/passwd", "missing.sql"] {
            let db = format!(r#"{{ "provider": "ephemeral-postgres", "setup": ["{file}"] }}"#);
            assert!(validate(&manifest(&db, "")).is_err(), "{file}");
        }
    }

    #[test]
    fn credential_and_database_variables_are_refused() {
        for name in [
            "DATABASE_URL",
            "PGHOST",
            "STRIPE_SECRET_KEY",
            "GITHUB_TOKEN",
            "MIDWRITE_REPLAY_ROOT",
            "NODE_OPTIONS",
            "NODE_PATH",
            "lower",
        ] {
            let extra = format!(r#""env": {{ "{name}": "x" }},"#);
            assert!(validate(&manifest(DB, &extra)).is_err(), "{name}");
        }
        assert!(
            validate(&manifest(
                DB,
                r#""env": { "FEATURE_FLAG": "postgres://x" },"#
            ))
            .is_err()
        );
        validate(&manifest(DB, r#""env": { "FEATURE_FLAG": "on" },"#)).unwrap();
    }

    #[test]
    fn analysis_roots_are_the_harness_directory_or_an_ancestor() {
        for (root, ok) in [
            ("..", true),
            (".", true),
            ("../..", true),
            ("../sibling", false),
            ("/etc", false),
            ("src", false),
        ] {
            let extra = format!(r#""analysis": {{ "root": "{root}", "paths": ["src"] }},"#);
            assert_eq!(validate(&manifest(DB, &extra)).is_ok(), ok, "{root}");
        }
        let escape = r#""analysis": { "root": "..", "paths": ["../x"] },"#;
        assert!(validate(&manifest(DB, escape)).is_err());
    }

    #[test]
    fn declared_runtimes_must_match_the_command_and_name_an_entry_point() {
        let node = manifest(DB, r#""runtime": "node","#);
        assert!(validate(&node).unwrap_err().contains("does not start it"));
        let ok = node.replace(r#"["bun", "h.ts"]"#, r#"["node", "dist/h.js"]"#);
        validate(&ok).unwrap();
        let flags = node.replace(
            r#"["bun", "h.ts"]"#,
            r#"["/usr/local/bin/node", "--enable-source-maps", "dist/h.js"]"#,
        );
        validate(&flags).unwrap();
        for bad in [
            r#"["node"]"#,
            r#"["node", "../outside.js"]"#,
            r#"["node", "/abs.js"]"#,
        ] {
            let text = node.replace(r#"["bun", "h.ts"]"#, bad);
            assert!(validate(&text).is_err(), "{bad}");
        }
        let bun = manifest(DB, r#""runtime": "bun","#);
        validate(&bun).unwrap();
        assert!(validate(&manifest(DB, r#""runtime": "deno","#)).is_err());
    }

    #[test]
    fn placeholders_are_found_in_keys_and_values() {
        let value: Value = serde_json::from_str(
            r#"{ "a": "x", "b": ["ok", "MIDWRITE-TODO: sql"], "c": { "MIDWRITE-TODO": 1 } }"#,
        )
        .unwrap();
        assert_eq!(placeholders(&value), ["/b/1", "/c/MIDWRITE-TODO"]);
    }

    #[test]
    fn commands_are_not_shell_strings_and_unknown_fields_fail() {
        let shell = manifest(DB, "").replace(r#"["bun", "h.ts"]"#, r#"["bun h.ts"]"#);
        assert!(validate(&shell).is_err());
        let unknown = manifest(DB, r#""autoRun": true,"#);
        assert!(validate(&unknown).is_err());
        let no_invariants = manifest(DB, "").replace(
            r#""invariants": [{ "id": "i", "description": "d", "require": [{ "observation": "n", "atMost": 1 }] }]"#,
            r#""invariants": []"#,
        );
        assert!(validate(&no_invariants).unwrap_err().contains("invariant"));
    }
}

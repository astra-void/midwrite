//! User configuration (`midwrite.json`).
//!
//! The configuration tells Midwrite which identifiers denote the Drizzle
//! database object and which calls are external side effects. Matching is
//! syntactic: Midwrite trusts these declarations and does not verify them
//! with a type checker. See `docs/ARCHITECTURE.md` for the trust model.

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ir::{ExternalCategory, Recovery};

/// File name looked up when no configuration path is given explicitly.
pub const CONFIG_FILE_NAME: &str = "midwrite.json";

/// Configuration file names used before the project was renamed (CommitRift,
/// then CrashProof), newest first. Read only when [`CONFIG_FILE_NAME`] does
/// not exist.
pub const LEGACY_CONFIG_FILE_NAMES: [&str; 2] = ["commitrift.json", "crashproof.json"];

/// The configuration file found in a directory (see [`discover_config`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredConfig {
    pub path: std::path::PathBuf,
    /// The file is one of the [`LEGACY_CONFIG_FILE_NAMES`].
    pub legacy: bool,
    /// A legacy file that exists next to the file used and is **not** read.
    /// The files are never merged.
    pub ignored: Option<std::path::PathBuf>,
}

/// Find the configuration file of `dir`: `midwrite.json` if it exists,
/// otherwise the first existing legacy name (`commitrift.json`, then
/// `crashproof.json`). Only one file is ever used; another candidate that
/// also exists is reported in `ignored`.
pub fn discover_config(dir: &Path) -> Option<DiscoveredConfig> {
    let mut found = std::iter::once(CONFIG_FILE_NAME)
        .chain(LEGACY_CONFIG_FILE_NAMES)
        .map(|name| (name != CONFIG_FILE_NAME, dir.join(name)))
        .filter(|(_, path)| path.is_file());
    let (legacy, path) = found.next()?;
    Some(DiscoveredConfig {
        path,
        legacy,
        ignored: found.next().map(|(_, path)| path),
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    /// Ignored; allows editors to attach a JSON schema.
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Identifiers that denote a Drizzle database object.
    #[serde(default = "default_database_receivers")]
    pub database_receivers: Vec<ReceiverRule>,
    /// Methods on a database object that start a callback transaction.
    #[serde(default = "default_transaction_methods")]
    pub transaction_methods: Vec<String>,
    /// Calls that perform an effect outside the database.
    #[serde(default = "default_external_effects")]
    pub external_effects: Vec<ExternalEffectRule>,
    /// Glob patterns of files not to analyze when a directory is scanned
    /// (`tests/**`, `**/*.test.ts`, `__generated__`). A pattern with a `/`
    /// is matched against the path relative to the scanned directory; one
    /// without is matched against every file and directory name. Files named
    /// explicitly on the command line are always analyzed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Recovery mechanisms outside a function that the application asserts
    /// for it (a job runner that re-executes the function after an error or
    /// a crash, a reconciliation process). Asserted, never inferred and not
    /// verified: a covered finding stays reported, with the contract and its
    /// cited evidence attached, and is informational under `recommended`.
    /// See [`crate::recovery`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recovery_contracts: Vec<crate::recovery::RecoveryContract>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema: None,
            database_receivers: default_database_receivers(),
            transaction_methods: default_transaction_methods(),
            external_effects: default_external_effects(),
            exclude: Vec::new(),
            recovery_contracts: Vec::new(),
        }
    }
}

fn default_database_receivers() -> Vec<ReceiverRule> {
    vec![ReceiverRule::Name("db".to_owned())]
}

fn default_transaction_methods() -> Vec<String> {
    vec!["transaction".to_owned()]
}

fn default_external_effects() -> Vec<ExternalEffectRule> {
    vec![ExternalEffectRule {
        receiver: Some("queue".to_owned()),
        method: Some("publish".to_owned()),
        function: None,
        category: ExternalCategory::MessagePublish,
        imported_from: None,
        first_argument: None,
        http_methods: None,
        best_effort: false,
        recovery: None,
    }]
}

/// A database receiver: either a bare name (`"db"`) or a name that must be
/// imported from a specific module (`{"name": "db", "importedFrom": "@/db"}`).
/// The name may also be a member path such as `"this.db"` or `"ctx.db"`
/// (see [`is_receiver_path`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReceiverRule {
    Name(String),
    Detailed(DetailedReceiver),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DetailedReceiver {
    pub name: String,
    #[serde(default)]
    pub imported_from: Option<String>,
}

impl ReceiverRule {
    pub fn name(&self) -> &str {
        match self {
            Self::Name(name) => name,
            Self::Detailed(detailed) => &detailed.name,
        }
    }

    pub fn imported_from(&self) -> Option<&str> {
        match self {
            Self::Name(_) => None,
            Self::Detailed(detailed) => detailed.imported_from.as_deref(),
        }
    }

    /// Whether the name is a member path (`this.db`, `ctx.db`) rather than a
    /// plain identifier.
    pub fn is_member_path(&self) -> bool {
        self.name().trim_end_matches("()").contains('.')
    }

    /// Whether the name denotes the value a call returns (`getDb()`).
    pub fn is_call(&self) -> bool {
        self.name().ends_with("()")
    }
}

/// An external effect: either `receiver.method(...)` (the receiver may be a
/// member path such as `stripe.charges` or `this.queue`) or a plain
/// `function(...)` call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExternalEffectRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    pub category: ExternalCategory,
    /// When set, the root identifier must be imported from this module
    /// specifier (compared verbatim, e.g. `"@/lib/queue"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<String>,
    /// When set, the rule matches only calls whose first argument is this
    /// string literal (`triggerJob("notification", ...)`). Rules are tried
    /// in order, so a narrow rule goes before a general one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_argument: Option<String>,
    /// Restrict an HTTP effect to a literal `method` in its second argument
    /// (fetch-style options). Unresolved options do not match this rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_methods: Option<Vec<String>>,
    /// The application accepts that this effect is sometimes not performed
    /// (a best-effort notification). Under the `recommended` policy, a
    /// finding whose only claim is that this effect may not happen after an
    /// earlier operation (MW1001, MW1005) is reported as `info`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub best_effort: bool,
    /// A recovery property the application asserts for this effect
    /// (`bestEffort`, `reconciled`, `idempotentConsumer`; see
    /// [`Recovery`]). Not verified. `"bestEffort": true` is the same as
    /// `"recovery": "bestEffort"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<Recovery>,
}

impl ExternalEffectRule {
    /// The declared recovery property, folding in `bestEffort`.
    pub fn effective_recovery(&self) -> Option<Recovery> {
        self.recovery
            .or(self.best_effort.then_some(Recovery::BestEffort))
    }

    /// Whether the effect is declared best-effort, either way.
    pub fn is_best_effort(&self) -> bool {
        self.effective_recovery() == Some(Recovery::BestEffort)
    }
}

/// The call shape an [`ExternalEffectRule`] matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalTarget<'c> {
    Method { receiver: &'c str, method: &'c str },
    Function { name: &'c str },
}

impl ExternalEffectRule {
    /// The validated call shape. Only meaningful after [`Config::validate`].
    pub fn target(&self) -> Option<ExternalTarget<'_>> {
        match (&self.receiver, &self.method, &self.function) {
            (Some(receiver), Some(method), None) => {
                Some(ExternalTarget::Method { receiver, method })
            }
            (None, None, Some(name)) => Some(ExternalTarget::Function { name }),
            _ => None,
        }
    }
}

impl fmt::Display for ExternalEffectRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.target() {
            Some(ExternalTarget::Method { receiver, method }) => write!(f, "{receiver}.{method}")?,
            Some(ExternalTarget::Function { name }) => write!(f, "{name}")?,
            None => return f.write_str("<invalid rule>"),
        }
        match &self.first_argument {
            Some(argument) => write!(f, "({argument:?}, …)"),
            None => Ok(()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read configuration file `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid configuration in `{origin}`: {source}")]
    Parse {
        origin: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid configuration in `{origin}`: {message}")]
    Invalid { origin: String, message: String },
}

impl Config {
    /// Parse and validate a configuration document. `origin` names the source
    /// in error messages.
    pub fn from_json(text: &str, origin: &str) -> Result<Self, ConfigError> {
        let config: Self = serde_json::from_str(text).map_err(|source| ConfigError::Parse {
            origin: origin.to_owned(),
            source,
        })?;
        config.validate().map_err(|message| ConfigError::Invalid {
            origin: origin.to_owned(),
            message,
        })?;
        Ok(config)
    }

    /// Read, parse and validate a configuration file.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let origin = path.display().to_string();
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: origin.clone(),
            source,
        })?;
        Self::from_json(&text, &origin)
    }

    /// Check structural rules serde cannot express.
    pub fn validate(&self) -> Result<(), String> {
        for rule in &self.database_receivers {
            let name = rule.name();
            if !is_receiver_path(name.strip_suffix("()").unwrap_or(name)) {
                return Err(format!(
                    "databaseReceivers: `{name}` must be a JavaScript identifier or a member path such as `this.db` or `ctx.db`, optionally followed by `()` for the value a call returns (`getDb()`)"
                ));
            }
            match rule.imported_from() {
                Some("") => {
                    return Err(format!(
                        "databaseReceivers: `{name}` has an empty `importedFrom`"
                    ));
                }
                Some(_) if starts_with_this(name) => {
                    return Err(format!(
                        "databaseReceivers: `{name}` starts with `this`, which cannot be imported; remove `importedFrom`"
                    ));
                }
                _ => {}
            }
        }
        for method in &self.transaction_methods {
            if !is_identifier(method) {
                return Err(format!(
                    "transactionMethods: `{method}` is not a JavaScript identifier"
                ));
            }
        }
        for (index, rule) in self.external_effects.iter().enumerate() {
            match rule.target() {
                Some(ExternalTarget::Method { receiver, method }) => {
                    if !is_receiver_path(receiver) {
                        return Err(format!(
                            "externalEffects[{index}]: receiver `{receiver}` must be an identifier or a dotted path of identifiers (optionally starting with `this`)"
                        ));
                    }
                    if !is_identifier(method) || method == THIS {
                        return Err(format!(
                            "externalEffects[{index}]: method `{method}` is not a JavaScript identifier"
                        ));
                    }
                    if rule.imported_from.is_some() && starts_with_this(receiver) {
                        return Err(format!(
                            "externalEffects[{index}]: receiver `{receiver}` starts with `this`, which cannot be imported; remove `importedFrom`"
                        ));
                    }
                }
                Some(ExternalTarget::Function { name }) => {
                    if !is_identifier(name) || name == THIS {
                        return Err(format!(
                            "externalEffects[{index}]: function `{name}` is not a JavaScript identifier"
                        ));
                    }
                }
                None => {
                    return Err(format!(
                        "externalEffects[{index}]: specify either `receiver` and `method`, or `function`"
                    ));
                }
            }
            if rule.imported_from.as_deref().is_some_and(str::is_empty) {
                return Err(format!("externalEffects[{index}]: empty `importedFrom`"));
            }
            if rule.first_argument.as_deref().is_some_and(str::is_empty) {
                return Err(format!("externalEffects[{index}]: empty `firstArgument`"));
            }
            if let Some(methods) = &rule.http_methods
                && (rule.category != ExternalCategory::HttpRequest
                    || methods.is_empty()
                    || methods.iter().any(|method| {
                        method.is_empty() || !method.bytes().all(|byte| byte.is_ascii_uppercase())
                    }))
            {
                return Err(format!(
                    "externalEffects[{index}]: `httpMethods` requires category `http-request` and a nonempty list of uppercase HTTP methods"
                ));
            }
            if rule.best_effort
                && rule
                    .recovery
                    .is_some_and(|recovery| recovery != Recovery::BestEffort)
            {
                return Err(format!(
                    "externalEffects[{index}]: `bestEffort: true` conflicts with `recovery`; declare one recovery property"
                ));
            }
        }
        for pattern in &self.exclude {
            if pattern.is_empty() || pattern.starts_with('/') {
                return Err(format!(
                    "exclude: `{pattern}` must be a non-empty relative glob pattern"
                ));
            }
        }
        crate::recovery::validate_contracts(&self.recovery_contracts)
    }

    /// Whether `relative` (a path relative to a scanned directory, with `/`
    /// separators) matches an `exclude` pattern.
    pub fn is_excluded(&self, relative: &str) -> bool {
        let segments: Vec<&str> = relative.split('/').filter(|s| !s.is_empty()).collect();
        self.exclude.iter().any(|pattern| {
            if pattern.contains('/') {
                let pattern: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
                glob_segments(&pattern, &segments)
            } else {
                segments.iter().any(|segment| glob(pattern, segment))
            }
        })
    }
}

/// Match path segments against pattern segments; `**` matches any number of
/// segments.
fn glob_segments(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| glob_segments(rest, &path[skip..])),
        Some((first, rest)) => path
            .split_first()
            .is_some_and(|(segment, tail)| glob(first, segment) && glob_segments(rest, tail)),
    }
}

/// Match one segment: `*` matches any run of characters, `?` one character.
fn glob(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(position) = star {
            p = position + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

const THIS: &str = "this";

/// Whether `text` is a receiver path: an identifier, or `this`, followed by
/// zero or more `.identifier` segments (`db`, `ctx.db`, `this.db`,
/// `stripe.charges`). A bare `this` is not a receiver path.
pub fn is_receiver_path(text: &str) -> bool {
    let mut segments = text.split('.');
    let Some(first) = segments.next() else {
        return false;
    };
    let rest: Vec<&str> = segments.collect();
    is_identifier(first)
        && (first != THIS || !rest.is_empty())
        && rest
            .iter()
            .all(|segment| is_identifier(segment) && *segment != THIS)
}

fn starts_with_this(path: &str) -> bool {
    path.split('.').next() == Some(THIS)
}

/// Whether `text` is a plain (ASCII) JavaScript identifier.
pub fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_db_and_queue_publish() {
        let config = Config::default();
        assert_eq!(config.database_receivers[0].name(), "db");
        assert_eq!(config.transaction_methods, ["transaction"]);
        assert_eq!(
            config.external_effects[0].target(),
            Some(ExternalTarget::Method {
                receiver: "queue",
                method: "publish"
            })
        );
        assert!(config.validate().is_ok());
    }

    #[test]
    fn parses_the_documented_schema() {
        let config = Config::from_json(
            r#"{
              "databaseReceivers": ["db", { "name": "database", "importedFrom": "@/db" }],
              "transactionMethods": ["transaction"],
              "externalEffects": [
                { "receiver": "queue", "method": "publish", "category": "message-publish" },
                { "receiver": "stripe.charges", "method": "create", "category": "payment" },
                { "function": "sendEmail", "category": "email", "importedFrom": "@/lib/email" }
              ]
            }"#,
            "test",
        )
        .expect("valid config");
        assert_eq!(config.database_receivers[1].imported_from(), Some("@/db"));
        assert_eq!(config.external_effects.len(), 3);
        assert_eq!(config.external_effects[2].to_string(), "sendEmail");
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let config = Config::from_json("{}", "test").expect("valid config");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn rejects_unknown_fields() {
        let error = Config::from_json(r#"{ "databaseReceiver": ["db"] }"#, "test").unwrap_err();
        assert!(matches!(error, ConfigError::Parse { .. }), "{error}");
    }

    #[test]
    fn rejects_ambiguous_external_rules() {
        let error = Config::from_json(
            r#"{ "externalEffects": [{ "receiver": "queue", "function": "send", "category": "other" }] }"#,
            "test",
        )
        .unwrap_err();
        assert!(error.to_string().contains("either `receiver` and `method`"));
    }

    #[test]
    fn rejects_non_identifiers() {
        for name in [
            "this",
            "db.this",
            "a..b",
            "1db",
            "ctx.",
            "ctx[db]",
            "",
            "getDb(x)",
            "getDb()()",
        ] {
            let error =
                Config::from_json(&format!(r#"{{ "databaseReceivers": ["{name}"] }}"#), "test")
                    .unwrap_err();
            assert!(
                error.to_string().contains("member path such as `this.db`"),
                "{name}: {error}"
            );
        }
        let error = Config::from_json(
            r#"{ "externalEffects": [{ "function": "this", "category": "other" }] }"#,
            "test",
        )
        .unwrap_err();
        assert!(error.to_string().contains("not a JavaScript identifier"));
    }

    #[test]
    fn accepts_member_paths() {
        let config = Config::from_json(
            r#"{
              "databaseReceivers": ["db", "this.db", "ctx.db", { "name": "services.db", "importedFrom": "@/services" }],
              "externalEffects": [
                { "receiver": "this.queue", "method": "publish", "category": "message-publish" },
                { "receiver": "ctx.clients.mailer", "method": "send", "category": "email" }
              ]
            }"#,
            "test",
        )
        .expect("valid config");
        assert!(!config.database_receivers[0].is_member_path());
        assert!(config.database_receivers[1].is_member_path());
        assert_eq!(config.external_effects[0].to_string(), "this.queue.publish");
    }

    #[test]
    fn rejects_imported_this_paths() {
        let error = Config::from_json(
            r#"{ "databaseReceivers": [{ "name": "this.db", "importedFrom": "@/db" }] }"#,
            "test",
        )
        .unwrap_err();
        assert!(error.to_string().contains("cannot be imported"), "{error}");
        let error = Config::from_json(
            r#"{ "externalEffects": [{ "receiver": "this.queue", "method": "publish", "category": "other", "importedFrom": "@/q" }] }"#,
            "test",
        )
        .unwrap_err();
        assert!(error.to_string().contains("cannot be imported"), "{error}");
    }

    #[test]
    fn receiver_path_check() {
        assert!(is_receiver_path("db"));
        assert!(is_receiver_path("this.db"));
        assert!(is_receiver_path("ctx.db"));
        assert!(is_receiver_path("stripe.charges"));
        assert!(!is_receiver_path("this"));
        assert!(!is_receiver_path("ctx.this"));
        assert!(!is_receiver_path("a..b"));
        assert!(!is_receiver_path(".db"));
    }

    #[test]
    fn http_method_filters_are_validated_and_round_trip() {
        let config = Config::from_json(
            r#"{"externalEffects":[{"function":"fetch","category":"http-request","httpMethods":["POST","DELETE"]}]}"#,
            "test",
        ).unwrap();
        assert_eq!(
            Config::from_json(&serde_json::to_string(&config).unwrap(), "round trip").unwrap(),
            config
        );
        for rule in [
            r#"{"function":"fetch","category":"http-request","httpMethods":[]}"#,
            r#"{"function":"fetch","category":"http-request","httpMethods":["post"]}"#,
            r#"{"function":"fetch","category":"http-request","httpMethods":[""]}"#,
            r#"{"function":"fetch","category":"email","httpMethods":["POST"]}"#,
        ] {
            assert!(
                Config::from_json(&format!(r#"{{"externalEffects":[{rule}]}}"#), "test").is_err(),
                "{rule}"
            );
        }
    }

    #[test]
    fn external_rules_can_be_narrowed_and_marked_best_effort() {
        let config = Config::from_json(
            r#"{ "externalEffects": [
                { "function": "triggerJob", "firstArgument": "notification", "category": "message-publish", "bestEffort": true }
            ] }"#,
            "test",
        )
        .unwrap();
        let rule = &config.external_effects[0];
        assert!(rule.best_effort);
        assert_eq!(rule.to_string(), "triggerJob(\"notification\", …)");
        assert!(
            Config::from_json(
                r#"{ "externalEffects": [{ "function": "f", "firstArgument": "", "category": "other" }] }"#,
                "test"
            )
            .is_err()
        );
        let calls = Config::from_json(
            r#"{ "databaseReceivers": ["getDb()", "ctx.db()"] }"#,
            "test",
        )
        .unwrap();
        assert!(calls.database_receivers[0].is_call());
    }

    #[test]
    fn recovery_declarations_parse_and_conflicts_are_rejected() {
        let config = Config::from_json(
            r#"{ "externalEffects": [
                { "function": "a", "category": "email", "recovery": "reconciled" },
                { "function": "b", "category": "email", "bestEffort": true },
                { "function": "c", "category": "email", "recovery": "idempotentConsumer" }
            ] }"#,
            "test",
        )
        .unwrap();
        let rules = &config.external_effects;
        assert_eq!(rules[0].effective_recovery(), Some(Recovery::Reconciled));
        assert!(!rules[0].is_best_effort());
        assert_eq!(rules[1].effective_recovery(), Some(Recovery::BestEffort));
        assert!(rules[1].is_best_effort());
        assert_eq!(
            rules[2].effective_recovery(),
            Some(Recovery::IdempotentConsumer)
        );
        let error = Config::from_json(
            r#"{ "externalEffects": [{ "function": "a", "category": "email", "bestEffort": true, "recovery": "reconciled" }] }"#,
            "test",
        )
        .unwrap_err();
        assert!(error.to_string().contains("conflicts"), "{error}");
        assert!(
            Config::from_json(
                r#"{ "externalEffects": [{ "function": "a", "category": "email", "recovery": "retried" }] }"#,
                "test",
            )
            .is_err(),
            "unknown declarations are rejected"
        );
    }

    #[test]
    fn exclude_patterns_match_paths_and_names() {
        let config = Config::from_json(
            r#"{ "exclude": ["tests/**", "**/*.test.ts", "__generated__", "*.spec.ts"] }"#,
            "test",
        )
        .unwrap();
        assert!(config.is_excluded("tests/integration/a.ts"));
        assert!(config.is_excluded("src/a.test.ts"));
        assert!(config.is_excluded("a.test.ts"));
        assert!(config.is_excluded("src/__generated__/x.ts"));
        assert!(config.is_excluded("lib/x.spec.ts"));
        assert!(!config.is_excluded("src/tests.ts"));
        assert!(!config.is_excluded("src/lib/testsuite/a.ts"));
        assert!(!config.is_excluded("src/a.ts"));
        assert!(Config::from_json(r#"{ "exclude": ["/abs"] }"#, "test").is_err());
    }

    #[test]
    fn discovery_prefers_the_new_name_and_never_merges() {
        let dir = std::env::temp_dir().join(format!("midwrite-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let [commitrift, crashproof] = LEGACY_CONFIG_FILE_NAMES;
        for name in [CONFIG_FILE_NAME, commitrift, crashproof] {
            let _ = std::fs::remove_file(dir.join(name));
        }
        assert_eq!(discover_config(&dir), None);

        std::fs::write(dir.join(crashproof), "{}").unwrap();
        let found = discover_config(&dir).unwrap();
        assert!(found.legacy);
        assert_eq!(found.path, dir.join(crashproof));
        assert_eq!(found.ignored, None);

        std::fs::write(dir.join(commitrift), "{}").unwrap();
        let found = discover_config(&dir).unwrap();
        assert!(found.legacy);
        assert_eq!(found.path, dir.join(commitrift));
        assert_eq!(found.ignored, Some(dir.join(crashproof)));

        std::fs::write(dir.join(CONFIG_FILE_NAME), "{}").unwrap();
        let found = discover_config(&dir).unwrap();
        assert!(!found.legacy);
        assert_eq!(found.path, dir.join(CONFIG_FILE_NAME));
        assert_eq!(found.ignored, Some(dir.join(commitrift)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn identifier_check() {
        assert!(is_identifier("db"));
        assert!(is_identifier("$db_1"));
        assert!(!is_identifier("1db"));
        assert!(!is_identifier(""));
        assert!(!is_identifier("a.b"));
    }
}

//! Developer-editable verification contracts (`commitrift-contracts.json`).
//!
//! Everything discovery cannot read from source is supplied here, and every
//! fact taken from this file is reported as declared, never as derived:
//!
//! * external effects (which callee is an effect, on which channel, whether
//!   the channel deduplicates by key);
//! * how an entry is invoked (wrapper invocation and preconditions, at-least-
//!   once delivery, client retries after a crash);
//! * business invariants, as templates instantiated with the synthesized
//!   values;
//! * transaction helpers whose retry loop is not analyzed;
//! * how a source harness is called for each entry (original-code
//!   verification; executing it still needs `--allow-execution`).
//!
//! The file is optional. Unknown fields are errors.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use commitrift_model::invariant::Formula;
use commitrift_model::value::Value;

pub const CONTRACTS_FILE: &str = "commitrift-contracts.json";
pub const CONTRACTS_KIND: &str = "commitrift.contracts";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Contracts {
    pub schema_version: u32,
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub externals: Vec<ExternalContract>,
    /// Keyed by entry selector.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entries: BTreeMap<String, EntryContract>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invariants: Vec<DeclaredInvariant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transaction_helpers: Vec<HelperContract>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<HarnessContract>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExternalContract {
    /// Callee as written in source (`chargeCard`, `queue.publish`).
    pub callee: String,
    pub channel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_argument: Option<usize>,
    /// `none` (every call is an effect) or `byKey` (the channel ignores a
    /// repeated idempotency key).
    #[serde(default = "dedup_none")]
    pub dedup: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

fn dedup_none() -> String {
    "none".to_owned()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntryContract {
    /// How a wrapper invokes the callback: `awaited` or `fire-and-forget`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preconditions: Vec<String>,
    /// `at-least-once`: a delivery can be redelivered after its
    /// acknowledgement is lost, and deliveries can overlap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<String>,
    /// `on-crash`, `on-failure` or `on-failure-or-crash`: the caller repeats
    /// the operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<String>,
    /// Whether two invocations can run at the same time (default: yes for
    /// Server Actions, route handlers and at-least-once consumers; stated
    /// as an assumption otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrent: Option<bool>,
    /// Positional arguments to use instead of synthesized ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<Value>>,
    /// Values for parameter paths (`data.role`) that replace synthesized
    /// ones: what validation code the model does not run accepts.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// A business invariant. `entries` restricts it to candidates of these
/// selectors (empty: every candidate whose tables or channel it names).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeclaredInvariant {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<String>,
    pub property: Template,
}

/// Templates instantiated with the values a scenario synthesizes.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "template", deny_unknown_fields)]
pub enum Template {
    /// At most one committed row of `table` per value of `key` among rows
    /// matching `where`.
    AtMostOne {
        table: String,
        key: Vec<String>,
        #[serde(default, rename = "where")]
        filter: BTreeMap<String, Value>,
    },
    /// At least `min` committed rows matching `where` per value of
    /// `groupBy`.
    AtLeast {
        table: String,
        #[serde(default, rename = "groupBy")]
        group_by: Vec<String>,
        #[serde(default, rename = "where")]
        filter: BTreeMap<String, Value>,
        min: i64,
    },
    /// `column` of every committed row is >= 0.
    NonNegative { table: String, column: String },
    /// At most `max` performed effects on `channel`.
    MaxExternal { channel: String, max: i64 },
    /// A literal invariant formula (`docs/phase6/INVARIANTS.md`).
    Formula { assert: Formula },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HelperContract {
    pub callee: String,
    #[serde(default)]
    pub callback_argument: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options_argument: Option<usize>,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// How the authorized source harness is called for each entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarnessContract {
    /// The harness manifest, relative to the project root.
    pub manifest: String,
    /// Keyed by entry selector.
    pub entries: BTreeMap<String, HarnessEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarnessEntry {
    /// The entry name in the manifest.
    pub entry: String,
    /// The harness input. Strings of the form `$argN` or `$argN.path` are
    /// replaced by the actor's synthesized argument (or a field of it).
    pub input: serde_json::Value,
}

impl Contracts {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("`{}`: {e}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let contracts: Self = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if contracts.schema_version != 1 {
            return Err(format!(
                "unsupported schemaVersion {}; this build reads 1",
                contracts.schema_version
            ));
        }
        if contracts.kind != CONTRACTS_KIND {
            return Err(format!("`kind` must be `{CONTRACTS_KIND}`"));
        }
        for e in &contracts.externals {
            if !matches!(e.dedup.as_str(), "none" | "byKey") {
                return Err(format!(
                    "external `{}`: dedup `{}` (expected `none` or `byKey`)",
                    e.callee, e.dedup
                ));
            }
        }
        for (selector, entry) in &contracts.entries {
            if let Some(d) = &entry.delivery
                && d != "at-least-once"
            {
                return Err(format!(
                    "entry `{selector}`: delivery `{d}` (expected `at-least-once`)"
                ));
            }
            if let Some(r) = &entry.retry
                && !matches!(
                    r.as_str(),
                    "on-crash" | "on-failure" | "on-failure-or-crash"
                )
            {
                return Err(format!(
                    "entry `{selector}`: retry `{r}` (expected `on-crash`, `on-failure` or `on-failure-or-crash`)"
                ));
            }
            if let Some(i) = &entry.invocation
                && !matches!(i.as_str(), "awaited" | "fire-and-forget")
            {
                return Err(format!(
                    "entry `{selector}`: invocation `{i}` (expected `awaited` or `fire-and-forget`)"
                ));
            }
        }
        Ok(contracts)
    }

    pub fn entry(&self, selector: &str) -> Option<&EntryContract> {
        self.entries.get(selector)
    }

    pub fn channel_dedup(&self, channel: &str) -> Option<&str> {
        self.externals
            .iter()
            .find(|e| e.channel == channel)
            .map(|e| e.dedup.as_str())
    }

    /// A stable digest of the whole file (cache keys).
    pub fn digest(&self) -> u64 {
        let text = serde_json::to_string(self).unwrap_or_default();
        commitrift_core::diagnostic::stable_hash(&[&text])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contracts_parse_and_reject_unknown_values() {
        let ok = r#"{
            "schemaVersion": 1, "kind": "commitrift.contracts",
            "externals": [{"callee": "chargeCard", "channel": "card", "keyArgument": 1, "dedup": "byKey"}],
            "entries": {"src/jobs.ts#run": {"delivery": "at-least-once"}},
            "invariants": [
                {"name": "duty", "property": {"template": "atLeast", "table": "shifts", "groupBy": ["storeId"], "where": {"onDuty": true}, "min": 1}},
                {"name": "one", "property": {"template": "atMostOne", "table": "t", "key": ["a"]}}
            ]
        }"#;
        let c = Contracts::parse(ok).unwrap();
        assert_eq!(c.channel_dedup("card"), Some("byKey"));
        assert!(matches!(
            c.invariants[0].property,
            Template::AtLeast { min: 1, .. }
        ));
        for bad in [
            r#"{"schemaVersion": 2, "kind": "commitrift.contracts"}"#,
            r#"{"schemaVersion": 1, "kind": "other"}"#,
            r#"{"schemaVersion": 1, "kind": "commitrift.contracts", "surprise": 1}"#,
            r#"{"schemaVersion": 1, "kind": "commitrift.contracts", "entries": {"x#y": {"retry": "always"}}}"#,
            r#"{"schemaVersion": 1, "kind": "commitrift.contracts", "externals": [{"callee": "a", "channel": "b", "dedup": "maybe"}]}"#,
            r#"{"schemaVersion": 1, "kind": "commitrift.contracts", "invariants": [{"name": "x", "property": {"template": "nonNegative", "table": "t", "column": "c", "extra": 1}}]}"#,
        ] {
            assert!(Contracts::parse(bad).is_err(), "{bad}");
        }
    }
}

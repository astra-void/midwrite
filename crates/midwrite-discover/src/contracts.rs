//! Developer-editable verification contracts (`midwrite-contracts.json`).
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

use midwrite_model::invariant::Formula;
use midwrite_model::provider::ReplicaFactoryBinding;
use midwrite_model::value::Value;

pub const CONTRACTS_FILE: &str = "midwrite-contracts.json";
pub const CONTRACTS_KIND: &str = "midwrite.contracts";
/// The contracts file name before the rename to Midwrite; read only when
/// [`CONTRACTS_FILE`] does not exist.
pub const LEGACY_CONTRACTS_FILE: &str = "commitrift-contracts.json";
/// The contracts `kind` before the rename; accepted on input.
pub const LEGACY_CONTRACTS_KIND: &str = "commitrift.contracts";

/// The contracts file of the project at `root`: `midwrite-contracts.json`,
/// else the legacy `commitrift-contracts.json`.
pub fn find_contracts_file(root: &std::path::Path) -> Option<std::path::PathBuf> {
    [CONTRACTS_FILE, LEGACY_CONTRACTS_FILE]
        .into_iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file())
}

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
    /// Phase 13: project functions declared to build a replicated database
    /// with drizzle-orm `withReplicas` routing (`module#export`). Their
    /// bodies are not interpreted; each declaration is reported as an
    /// assumption.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replica_factories: Vec<ReplicaFactoryBinding>,
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
    /// Phase 14: at most one successful call (of `function`, every actor
    /// when absent) was told `returned` ("at most one caller is told it
    /// claimed the task", "a handle is reserved for one caller", "no
    /// duplicate success after a retry").
    AtMostOneSuccess {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        function: Option<String>,
        returned: Value,
    },
    /// Phase 14: exactly one successful call (of `function`) at the end:
    /// one acknowledged logical operation.
    ExactlyOneSuccess {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        function: Option<String>,
    },
    /// Phase 14: every successful call (of `function`) was told `returned`,
    /// or its own argument number `argument` (read-your-writes: the value
    /// the caller wrote).
    EverySuccessReturns {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        function: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        returned: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        argument: Option<String>,
    },
}

impl Template {
    /// Phase 14: the formula of a return-value template.
    pub fn returned_formula(&self) -> Option<Formula> {
        use midwrite_model::invariant::{ActorsRef, OutcomeKind, Term, TermOp};
        let ok = |function: &Option<String>, returned: Option<&Value>| {
            Term::Op(TermOp::Actors(ActorsRef {
                function: function.clone(),
                outcome: OutcomeKind::Ok,
                returned: returned.cloned(),
            }))
        };
        Some(match self {
            Self::AtMostOneSuccess { function, returned } => {
                Formula::Le([ok(function, Some(returned)), Term::Int(1)])
            }
            Self::ExactlyOneSuccess { function } => Formula::Eq([ok(function, None), Term::Int(1)]),
            Self::EverySuccessReturns {
                function,
                returned: Some(returned),
                argument: None,
            } => Formula::Eq([ok(function, Some(returned)), ok(function, None)]),
            _ => return None,
        })
    }
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
        if contracts.kind != CONTRACTS_KIND && contracts.kind != LEGACY_CONTRACTS_KIND {
            return Err(format!("`kind` must be `{CONTRACTS_KIND}`"));
        }
        // Phase 14: `everySuccessReturns` names either a value or an
        // argument, not both and not neither.
        for inv in &contracts.invariants {
            if let Template::EverySuccessReturns {
                returned, argument, ..
            } = &inv.property
                && returned.is_some() == argument.is_some()
            {
                return Err(format!(
                    "invariant `{}`: `everySuccessReturns` needs exactly one of `returned` and `argument`",
                    inv.name
                ));
            }
        }
        for e in &contracts.externals {
            if !matches!(e.dedup.as_str(), "none" | "byKey") {
                return Err(format!(
                    "external `{}`: dedup `{}` (expected `none` or `byKey`)",
                    e.callee, e.dedup
                ));
            }
        }
        for f in &contracts.replica_factories {
            if f.factory
                .rsplit_once('#')
                .is_none_or(|(m, e)| m.is_empty() || e.is_empty())
            {
                return Err(format!(
                    "replica factory `{}`: expected `module#export`",
                    f.factory
                ));
            }
            if f.reason.trim().is_empty() {
                return Err(format!(
                    "replica factory `{}`: `reason` is required (why the routing holds)",
                    f.factory
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
        midwrite_core::diagnostic::stable_hash(&[&text])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_value_templates_become_formulas_and_need_a_value() {
        let c = Contracts::parse(
            r#"{"schemaVersion":1,"kind":"midwrite.contracts","invariants":[
              {"name":"one-claim","entries":["a.ts#claim"],"property":{"template":"atMostOneSuccess","function":"claim","returned":true}},
              {"name":"ryw","entries":["a.ts#rename"],"property":{"template":"everySuccessReturns","argument":"name"}}]}"#,
        )
        .unwrap();
        let f = c.invariants[0].property.returned_formula().unwrap();
        assert_eq!(
            serde_json::to_value(&f).unwrap(),
            serde_json::json!({"le": [{"actors": {"function": "claim", "outcome": "ok", "returned": true}}, 1]})
        );
        // An argument is instantiated per scenario, not a fixed formula.
        assert!(c.invariants[1].property.returned_formula().is_none());
        let bad = Contracts::parse(
            r#"{"schemaVersion":1,"kind":"midwrite.contracts","invariants":[
              {"name":"x","entries":["a.ts#f"],"property":{"template":"everySuccessReturns"}}]}"#,
        );
        assert!(bad.unwrap_err().contains("exactly one of"));
    }

    #[test]
    fn contracts_parse_and_reject_unknown_values() {
        let ok = r#"{
            "schemaVersion": 1, "kind": "midwrite.contracts",
            "externals": [{"callee": "chargeCard", "channel": "card", "keyArgument": 1, "dedup": "byKey"}],
            "entries": {"src/jobs.ts#run": {"delivery": "at-least-once"}},
            "invariants": [
                {"name": "duty", "property": {"template": "atLeast", "table": "shifts", "groupBy": ["storeId"], "where": {"onDuty": true}, "min": 1}},
                {"name": "one", "property": {"template": "atMostOne", "table": "t", "key": ["a"]}}
            ]
        }"#;
        let c = Contracts::parse(ok).unwrap();
        assert_eq!(c.channel_dedup("card"), Some("byKey"));
        let replicas = Contracts::parse(
            r#"{"schemaVersion": 1, "kind": "midwrite.contracts", "replicaFactories": [{"factory": "packages/db/src/replicas.ts#withReplicas", "primaryOnlyMethods": ["usePrimaryOnly"], "primaryProperties": ["$primary"], "reason": "read from source"}]}"#,
        )
        .unwrap();
        assert_eq!(replicas.replica_factories[0].primary_argument, 0);
        assert_eq!(
            replicas.replica_factories[0].primary_only_methods,
            vec!["usePrimaryOnly".to_owned()]
        );
        assert!(matches!(
            c.invariants[0].property,
            Template::AtLeast { min: 1, .. }
        ));
        for bad in [
            r#"{"schemaVersion": 2, "kind": "midwrite.contracts"}"#,
            r#"{"schemaVersion": 1, "kind": "other"}"#,
            r#"{"schemaVersion": 1, "kind": "midwrite.contracts", "surprise": 1}"#,
            r#"{"schemaVersion": 1, "kind": "midwrite.contracts", "entries": {"x#y": {"retry": "always"}}}"#,
            r#"{"schemaVersion": 1, "kind": "midwrite.contracts", "externals": [{"callee": "a", "channel": "b", "dedup": "maybe"}]}"#,
            r#"{"schemaVersion": 1, "kind": "midwrite.contracts", "invariants": [{"name": "x", "property": {"template": "nonNegative", "table": "t", "column": "c", "extra": 1}}]}"#,
            r#"{"schemaVersion": 1, "kind": "midwrite.contracts", "replicaFactories": [{"factory": "src/db.ts", "reason": "r"}]}"#,
            r#"{"schemaVersion": 1, "kind": "midwrite.contracts", "replicaFactories": [{"factory": "src/db.ts#make"}]}"#,
            r#"{"schemaVersion": 1, "kind": "midwrite.contracts", "replicaFactories": [{"factory": "src/db.ts#make", "reason": "r", "routing": "x"}]}"#,
        ] {
            assert!(Contracts::parse(bad).is_err(), "{bad}");
        }
    }
}

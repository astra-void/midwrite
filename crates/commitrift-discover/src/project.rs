//! The project index: modules, entry candidates, Drizzle schema, table
//! bindings, declared externals and transaction helpers.
//!
//! Table bindings are not computed here: a probe scenario that reads every
//! table from the schema module is loaded by the model checker's own loader,
//! and the bindings are read from the model it builds, so extraction during
//! discovery uses exactly the columns a generated scenario will model.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::json;

use commitrift_analyzer::entries::{EntryFact, ModuleFacts, index_project};
use commitrift_model::provider::{ExternalBinding, TableBinding, TransactionRetryBinding};
use commitrift_model::schema_facts::{ExtractedSchema, SchemaTable};

use crate::contracts::Contracts;
use crate::extract::{Bindings, MemoProvider};

/// One table the model can represent, with the schema module it comes from.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeledTable {
    pub binding: String,
    pub name: String,
    pub schema_file: String,
    /// Model columns (TypeScript property names).
    pub columns: Vec<String>,
    /// Columns of the source the model does not represent (types it does not
    /// model), by property name.
    pub unmodeled_columns: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperNote {
    pub name: String,
    pub location: String,
    /// `source-derived (no loop: one attempt)`, `declared (N attempts)` or
    /// `not bound: ...`.
    pub status: String,
}

pub struct ProjectIndex {
    pub root: PathBuf,
    pub modules: Vec<ModuleFacts>,
    pub schema_files: Vec<String>,
    /// Every table extracted from the schema modules, with its file.
    pub schema: BTreeMap<String, ExtractedSchema>,
    pub tables: Vec<ModeledTable>,
    /// Tables the loader refused, with the reason.
    pub refused_tables: Vec<(String, String)>,
    pub bindings: Bindings,
    pub helpers: Vec<HelperNote>,
    pub receivers: Vec<String>,
    pub contracts: Contracts,
    /// Notes about the index (schema modules not understood, ...).
    pub notes: Vec<String>,
}

impl ProjectIndex {
    pub fn entries(&self) -> impl Iterator<Item = (&ModuleFacts, &EntryFact)> {
        self.modules
            .iter()
            .flat_map(|m| m.entries.iter().map(move |e| (m, e)))
    }

    pub fn table_by_name(&self, name: &str) -> Option<&ModeledTable> {
        self.tables.iter().find(|t| t.name == name)
    }

    pub fn source_table(&self, name: &str) -> Option<&SchemaTable> {
        self.schema
            .values()
            .flat_map(|s| s.tables.iter())
            .find(|t| t.name == name)
    }

    /// The project files `file` can read through imports (itself included).
    pub fn import_closure(&self, file: &str) -> BTreeSet<String> {
        let by_file: BTreeMap<&str, &ModuleFacts> =
            self.modules.iter().map(|m| (m.file.as_str(), m)).collect();
        let mut seen = BTreeSet::new();
        let mut todo = vec![file.to_owned()];
        while let Some(f) = todo.pop() {
            if !seen.insert(f.clone()) {
                continue;
            }
            if let Some(m) = by_file.get(f.as_str()) {
                todo.extend(m.imports.iter().cloned());
            }
        }
        seen
    }
}

fn receivers(root: &Path) -> Vec<String> {
    let mut out = vec!["db".to_owned()];
    let config = root.join("commitrift.json");
    if config.is_file()
        && let Ok(config) = commitrift_core::config::Config::load(&config)
    {
        let names: Vec<String> = config
            .database_receivers
            .iter()
            .map(|r| r.name().to_owned())
            .filter(|n| {
                n.chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
            })
            .collect();
        if !names.is_empty() {
            out = names;
        }
    }
    out
}

/// Build the index of the project at `root`.
pub fn index(
    root: &Path,
    contracts: Contracts,
    provider: &MemoProvider,
) -> Result<ProjectIndex, String> {
    let receivers = receivers(root);
    let modules = index_project(root, &receivers)?;
    let mut notes = Vec::new();
    let schema_files: Vec<String> = modules
        .iter()
        .filter(|m| m.declares_tables && m.parse_error.is_none())
        .map(|m| m.file.clone())
        .collect();
    let mut schema = BTreeMap::new();
    let mut tables = Vec::new();
    let mut refused = Vec::new();
    for file in &schema_files {
        let extracted =
            match commitrift_model::provider::ActorProvider::extract_schema(provider, root, file) {
                Ok(s) => s,
                Err(e) => {
                    notes.push(format!("schema module `{file}` was not read: {e}"));
                    continue;
                }
            };
        for u in &extracted.unsupported {
            notes.push(format!("`{file}`: not understood: {u}"));
        }
        let (ok, bad) = probe_tables(root, file, &extracted, provider);
        tables.extend(ok);
        refused.extend(bad);
        schema.insert(file.clone(), extracted);
    }
    tables.sort_by(|a, b| a.name.cmp(&b.name));

    let table_bindings: Vec<TableBinding> = tables
        .iter()
        .map(|t| TableBinding {
            identifier: t.binding.clone(),
            table: t.name.clone(),
            columns: t.columns.clone(),
        })
        .collect();
    let externals = contracts
        .externals
        .iter()
        .map(|e| ExternalBinding {
            callee: e.callee.clone(),
            channel: e.channel.clone(),
            key_argument: e.key_argument,
        })
        .collect();
    let mut helpers = Vec::new();
    let mut retries = Vec::new();
    for declared in &contracts.transaction_helpers {
        retries.push(TransactionRetryBinding {
            callee: declared.callee.clone(),
            callback_argument: declared.callback_argument,
            options_argument: declared.options_argument,
            attempts: declared.attempts.max(1),
        });
        helpers.push(HelperNote {
            name: declared.callee.clone(),
            location: "commitrift-contracts.json".to_owned(),
            status: format!("declared ({} attempts)", declared.attempts.max(1)),
        });
    }
    for m in &modules {
        for h in &m.transaction_helpers {
            if contracts
                .transaction_helpers
                .iter()
                .any(|d| d.callee == h.name)
            {
                continue;
            }
            let location = format!("{}:{}", h.file, h.line);
            if h.has_loop {
                helpers.push(HelperNote {
                    name: h.name.clone(),
                    location,
                    status: "not bound: the helper contains a loop (it may run the callback again); declare its attempts in commitrift-contracts.json".to_owned(),
                });
                continue;
            }
            retries.push(TransactionRetryBinding {
                callee: h.name.clone(),
                callback_argument: h.callback_argument,
                options_argument: h.options_argument,
                attempts: 1,
            });
            helpers.push(HelperNote {
                name: h.name.clone(),
                location,
                status:
                    "source-derived: runs the callback in one transaction (no loop: one attempt)"
                        .to_owned(),
            });
        }
    }
    retries.sort_by(|a, b| a.callee.cmp(&b.callee));
    retries.dedup_by(|a, b| a.callee == b.callee);
    Ok(ProjectIndex {
        root: root.to_path_buf(),
        modules,
        schema_files,
        schema,
        tables,
        refused_tables: refused,
        bindings: Bindings {
            root: root.to_path_buf(),
            tables: table_bindings,
            externals,
            transaction_retries: retries,
        },
        helpers,
        receivers,
        contracts,
        notes,
    })
}

/// Load a scenario that reads every table of `file` with the model checker's
/// loader; tables it refuses are retried one by one to name the reason.
fn probe_tables(
    root: &Path,
    file: &str,
    schema: &ExtractedSchema,
    provider: &MemoProvider,
) -> (Vec<ModeledTable>, Vec<(String, String)>) {
    let load = |bindings: &[&str]| {
        let doc = json!({
            "schemaVersion": 1,
            "name": "commitrift-discover-probe",
            "extraction": { "root": ".", "schema": file },
            "database": {
                "tables": bindings.iter().map(|b| json!({ "from": b })).collect::<Vec<_>>(),
            },
            "actors": [{ "name": "probe", "function": "probe", "codes": [[{
                "op": { "op": "return" },
                "src": { "origin": "declared" }
            }]] }],
            "invariants": [{ "name": "probe", "assert": { "eq": [0, 0] } }],
        });
        commitrift_model::scenario::load_str(&doc.to_string(), root, provider)
    };
    let all: Vec<&str> = schema.tables.iter().map(|t| t.binding.as_str()).collect();
    let mut accepted: Vec<&str> = Vec::new();
    let mut refused = Vec::new();
    if load(&all).is_ok() {
        accepted = all;
    } else {
        for b in &all {
            match load(&[b]) {
                Ok(_) => accepted.push(b),
                Err(e) => refused.push((format!("{file}#{b}"), e.to_string())),
            }
        }
    }
    let mut out = Vec::new();
    let loaded = load(&accepted);
    if let Err(e) = &loaded {
        refused.push((file.to_owned(), e.to_string()));
    }
    if let Ok((model, report)) = loaded {
        for facts in &report.schema {
            let Some(binding) = &facts.binding else {
                continue;
            };
            let Some(table) = model.schema.tables.iter().find(|t| t.name == facts.table) else {
                continue;
            };
            let source = schema.table(binding);
            let columns: Vec<String> = table.columns.iter().map(|c| c.name.clone()).collect();
            let unmodeled = source
                .map(|s| {
                    s.columns
                        .iter()
                        .filter(|c| !columns.contains(&c.property))
                        .map(|c| c.property.clone())
                        .collect()
                })
                .unwrap_or_default();
            out.push(ModeledTable {
                binding: binding.clone(),
                name: table.name.clone(),
                schema_file: file.to_owned(),
                columns,
                unmodeled_columns: unmodeled,
            });
        }
    }
    (out, refused)
}

//! Comparing schema facts extracted from Drizzle source with the catalog of
//! a database initialized by the application's migrations (Phase 8,
//! Milestone 3.6).
//!
//! Every extracted fact is checked: present with the same meaning
//! (`agree`), different (`drift`), or not comparable (`not-compared`, with
//! the reason). Objects the database has and the source does not declare
//! are drift too. The comparison never modifies anything: a drift is
//! reported, the schema is not "fixed" to make the model agree.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::schema_facts::{ExtractedSchema, SchemaDefault, SchemaLiteral, SchemaTable};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FidelityItem {
    pub fact: String,
    /// `agree`, `drift`, `not-compared`.
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableFidelity {
    pub table: String,
    pub items: Vec<FidelityItem>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaFidelity {
    /// `agree` (no drift), `drift`, `not-compared`.
    pub status: String,
    pub tables: Vec<TableFidelity>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub postgres_version: String,
    pub note: String,
}

const NOTE: &str = "Schema facts extracted statically from the Drizzle source, compared with the catalog of a disposable PostgreSQL database initialized by the application's migrations. A drift means the model (built from source) and the database the application runs on disagree; nothing was changed to make them agree.";

fn s(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The catalog type name `format_type` prints for an extracted column, or
/// `None` when the extractor's type is not mapped.
fn expected_type(t: &SchemaTable, builder: &str, sql_type: &str) -> Option<String> {
    let _ = t;
    Some(match builder {
        "serial" | "integer" => "integer".to_owned(),
        "bigserial" | "bigint" => "bigint".to_owned(),
        "smallserial" | "smallint" => "smallint".to_owned(),
        "text" => "text".to_owned(),
        "boolean" => "boolean".to_owned(),
        "uuid" => "uuid".to_owned(),
        "json" => "json".to_owned(),
        "jsonb" => "jsonb".to_owned(),
        "date" => "date".to_owned(),
        "real" => "real".to_owned(),
        "doublePrecision" => "double precision".to_owned(),
        "numeric" | "decimal" => "numeric".to_owned(),
        "timestamp" if sql_type.ends_with("with time zone") => {
            "timestamp with time zone".to_owned()
        }
        "timestamp" => "timestamp without time zone".to_owned(),
        "varchar" => sql_type
            .strip_prefix("varchar")
            .map(|rest| format!("character varying{rest}"))?,
        "char" => sql_type
            .strip_prefix("char")
            .map(|rest| format!("character{rest}"))?,
        _ => {
            let name = sql_type.strip_prefix("enum ")?;
            name.to_owned()
        }
    })
}

/// Whether the catalog default expression means the extracted default.
fn default_agrees(
    expected: Option<&SchemaDefault>,
    catalog: Option<&str>,
    identity: Option<&str>,
) -> Result<(), String> {
    let strip_cast = |text: &str| -> String {
        // `'pending'::text`, `'member'::member_role`, `'x'::character varying`
        match text.rfind("::") {
            Some(i) if text.starts_with('\'') => text[..i].to_owned(),
            _ => text.to_owned(),
        }
    };
    match (expected, catalog) {
        (None | Some(SchemaDefault::Application { .. }), None) if identity.is_none() => Ok(()),
        (Some(SchemaDefault::Constant { value }), Some(c)) => {
            let want = match value {
                SchemaLiteral::Integer(n) => n.to_string(),
                SchemaLiteral::Text(t) => format!("'{}'", t.replace('\'', "''")),
                SchemaLiteral::Boolean(b) => b.to_string(),
                SchemaLiteral::Null => "NULL".to_owned(),
            };
            let got = strip_cast(c);
            if got == want || got.trim_matches(|ch| ch == '(' || ch == ')') == want {
                Ok(())
            } else {
                Err(format!("source DEFAULT {want}, database DEFAULT {c}"))
            }
        }
        (Some(SchemaDefault::Sequence { generated }), c) => match generated.as_str() {
            "generatedAlwaysAsIdentity" if identity == Some("a") => Ok(()),
            "generatedByDefaultAsIdentity" if identity == Some("d") => Ok(()),
            "serial" | "bigserial" | "smallserial"
                if c.is_some_and(|c| c.starts_with("nextval(")) =>
            {
                Ok(())
            }
            _ => Err(format!(
                "source: generated by {generated}; database default {:?}, identity {:?}",
                c, identity
            )),
        },
        (Some(SchemaDefault::Expression { text }), Some(c)) => {
            let norm = |t: &str| t.replace(' ', "").to_lowercase();
            if norm(text) == norm(c) {
                Ok(())
            } else {
                Err(format!(
                    "source DEFAULT {text}, database DEFAULT {c} (expressions compared as text)"
                ))
            }
        }
        (None, Some(c)) => Err(format!(
            "the source declares no default; the database has DEFAULT {c}"
        )),
        (Some(SchemaDefault::Application { text }), Some(c)) => Err(format!(
            "the source computes the value in the application (`{text}`); the database also has DEFAULT {c}"
        )),
        (Some(d), None) => Err(format!(
            "the source declares {d:?}; the database has no default"
        )),
        (None, None) => Err(format!(
            "identity {:?} in the database, none in the source",
            identity
        )),
    }
}

fn item(fact: String, status: &str, detail: impl Into<String>) -> FidelityItem {
    FidelityItem {
        fact,
        status: status.to_owned(),
        detail: detail.into(),
    }
}

fn same_set(a: &[String], b: &[String]) -> bool {
    let mut a = a.to_vec();
    let mut b = b.to_vec();
    a.sort();
    b.sort();
    a == b
}

/// Compare the extracted tables named in `tables` (SQL names; empty: all)
/// with `catalog` (from `commitrift_replay::catalog::read`).
pub fn compare(schema: &ExtractedSchema, catalog: &Value, tables: &[String]) -> SchemaFidelity {
    let catalog_tables = catalog.as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for t in &schema.tables {
        if !tables.is_empty() && !tables.contains(&t.name) {
            continue;
        }
        let schema_name = t.schema.clone().unwrap_or_else(|| "public".to_owned());
        let mut items = Vec::new();
        let Some(ct) = catalog_tables
            .iter()
            .find(|c| s(c, "name") == t.name && s(c, "schema") == schema_name)
        else {
            out.push(TableFidelity {
                table: t.name.clone(),
                items: vec![item(
                    format!("table {schema_name}.{}", t.name),
                    "drift",
                    "declared in the source, absent from the database",
                )],
            });
            continue;
        };
        let columns = ct["columns"].as_array().cloned().unwrap_or_default();
        for c in &t.columns {
            let fact = format!("column {}", c.sql_name);
            let Some(cc) = columns.iter().find(|x| s(x, "name") == c.sql_name) else {
                items.push(item(fact, "drift", "absent from the database"));
                continue;
            };
            match expected_type(t, &c.builder, &c.sql_type) {
                Some(want) if want == s(cc, "type") => {
                    items.push(item(format!("{fact} type {want}"), "agree", ""));
                }
                Some(want) => items.push(item(
                    format!("{fact} type"),
                    "drift",
                    format!("source {want}, database {}", s(cc, "type")),
                )),
                None => items.push(item(
                    format!("{fact} type"),
                    "not-compared",
                    format!("source type `{}` is not mapped", c.sql_type),
                )),
            }
            let nn = cc["notNull"].as_bool().unwrap_or(false);
            items.push(if nn == c.not_null {
                item(
                    format!("{fact} {}", if nn { "NOT NULL" } else { "nullable" }),
                    "agree",
                    "",
                )
            } else {
                item(
                    format!("{fact} nullability"),
                    "drift",
                    format!(
                        "source {}, database {}",
                        if c.not_null { "NOT NULL" } else { "nullable" },
                        if nn { "NOT NULL" } else { "nullable" }
                    ),
                )
            });
            match default_agrees(
                c.default.as_ref(),
                cc["default"].as_str(),
                cc["identity"].as_str(),
            ) {
                Ok(()) => items.push(item(format!("{fact} default"), "agree", "")),
                Err(detail) => items.push(item(format!("{fact} default"), "drift", detail)),
            }
        }
        for cc in &columns {
            let name = s(cc, "name");
            if t.column_by_sql(&name).is_none() {
                items.push(item(
                    format!("column {name}"),
                    if t.unsupported.is_empty() { "drift" } else { "not-compared" },
                    if t.unsupported.is_empty() {
                        "in the database, not declared in the source".to_owned()
                    } else {
                        "in the database; the source has constructs the extractor did not understand (see unsupported facts)".to_owned()
                    },
                ));
            }
        }
        let constraints = ct["constraints"].as_array().cloned().unwrap_or_default();
        let indexes = ct["indexes"].as_array().cloned().unwrap_or_default();
        // Primary key.
        let pk = constraints.iter().find(|c| s(c, "kind") == "p");
        match (pk, t.primary_key.is_empty()) {
            (Some(pk), false) if strings(&pk["columns"]) == t.primary_key => items.push(item(
                format!("primary key ({})", t.primary_key.join(", ")),
                "agree",
                "",
            )),
            (Some(pk), _) => items.push(item(
                "primary key".to_owned(),
                "drift",
                format!(
                    "source ({}), database ({})",
                    t.primary_key.join(", "),
                    strings(&pk["columns"]).join(", ")
                ),
            )),
            (None, false) => items.push(item(
                "primary key".to_owned(),
                "drift",
                format!("source ({}), database none", t.primary_key.join(", ")),
            )),
            (None, true) => {}
        }
        // Uniqueness: constraints and unique indexes (not the primary key).
        let mut catalog_unique: Vec<(String, Vec<String>, Option<String>, bool)> = Vec::new();
        for i in &indexes {
            if i["unique"].as_bool() == Some(true) && i["primary"].as_bool() != Some(true) {
                catalog_unique.push((
                    s(i, "name"),
                    strings(&i["columns"]),
                    i["predicate"].as_str().map(str::to_owned),
                    i["nullsNotDistinct"].as_bool().unwrap_or(false),
                ));
            }
        }
        let mut matched = vec![false; catalog_unique.len()];
        for u in &t.unique {
            let fact = format!(
                "unique ({}){}{}",
                u.columns.join(", "),
                u.predicate
                    .as_ref()
                    .map(|p| format!(" WHERE {p}"))
                    .unwrap_or_default(),
                if u.nulls_not_distinct {
                    " NULLS NOT DISTINCT"
                } else {
                    ""
                }
            );
            let found = catalog_unique
                .iter()
                .enumerate()
                .find(|(k, c)| !matched[*k] && same_set(&c.1, &u.columns));
            match found {
                None => items.push(item(fact, "drift", "absent from the database")),
                Some((k, c)) => {
                    matched[k] = true;
                    let partial_ok = u.predicate.is_some() == c.2.is_some();
                    if !partial_ok {
                        items.push(item(
                            fact,
                            "drift",
                            match &c.2 {
                                Some(p) => format!("the database index `{}` is partial (WHERE {p}); the source's is global", c.0),
                                None => format!("the database index `{}` is GLOBAL; the source's is partial", c.0),
                            },
                        ));
                    } else if u.nulls_not_distinct != c.3 {
                        items.push(item(
                            fact,
                            "drift",
                            format!(
                                "NULLS NOT DISTINCT: source {}, database {}",
                                u.nulls_not_distinct, c.3
                            ),
                        ));
                    } else {
                        items.push(item(
                            fact,
                            "agree",
                            match (&u.predicate, &c.2) {
                                (Some(_), Some(p)) => format!(
                                    "database predicate {p} (predicates compared by presence only)"
                                ),
                                _ => String::new(),
                            },
                        ));
                    }
                }
            }
        }
        for (k, c) in catalog_unique.iter().enumerate() {
            if !matched[k] {
                items.push(item(
                    format!("unique ({})", c.1.join(", ")),
                    "drift",
                    format!("the database has `{}`; the source does not declare it", c.0),
                ));
            }
        }
        // Foreign keys.
        let fks: Vec<&Value> = constraints.iter().filter(|c| s(c, "kind") == "f").collect();
        let mut fk_matched = vec![false; fks.len()];
        for fk in &t.foreign_keys {
            let target = schema
                .table(&fk.table)
                .map_or_else(|| fk.table.clone(), |x| x.name.clone());
            let foreign: Vec<String> = fk
                .foreign_columns
                .iter()
                .map(|p| {
                    schema
                        .table(&fk.table)
                        .and_then(|x| x.column(p))
                        .map_or_else(|| p.clone(), |c| c.sql_name.clone())
                })
                .collect();
            let fact = format!(
                "foreign key ({}) → {target}({})",
                fk.columns.join(", "),
                foreign.join(", ")
            );
            let found = fks.iter().enumerate().find(|(k, c)| {
                !fk_matched[*k]
                    && strings(&c["columns"]) == fk.columns
                    && s(c, "foreignTable") == target
                    && strings(&c["foreignColumns"]) == foreign
            });
            match found {
                Some((k, _)) => {
                    fk_matched[k] = true;
                    items.push(item(fact, "agree", ""));
                }
                None => items.push(item(fact, "drift", "absent from the database")),
            }
        }
        for (k, c) in fks.iter().enumerate() {
            if !fk_matched[k] {
                items.push(item(
                    format!("foreign key `{}`", s(c, "name")),
                    "drift",
                    "in the database, not declared in the source",
                ));
            }
        }
        // CHECK constraints: by count (expressions are not compared).
        let checks = constraints.iter().filter(|c| s(c, "kind") == "c").count();
        if checks != t.checks.len() {
            items.push(item(
                "CHECK constraints".to_owned(),
                "drift",
                format!("source {}, database {checks}", t.checks.len()),
            ));
        } else if checks > 0 {
            items.push(item(
                format!("{checks} CHECK constraint(s)"),
                "agree",
                "expressions not compared",
            ));
        }
        out.push(TableFidelity {
            table: t.name.clone(),
            items,
        });
    }
    let status = if out
        .iter()
        .flat_map(|t| &t.items)
        .any(|i| i.status == "drift")
    {
        "drift"
    } else {
        "agree"
    };
    SchemaFidelity {
        status: status.to_owned(),
        tables: out,
        postgres_version: String::new(),
        note: NOTE.to_owned(),
    }
}

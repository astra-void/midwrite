//! Schema facts extracted statically from Drizzle definitions (Phase 8).
//!
//! The data types live here, in the model crate, so that the scenario loader
//! can turn them into the model's [`crate::db::TableSchema`] and the source
//! verifier can compare them with a PostgreSQL catalog; the analyzer
//! (`commitrift_analyzer::schema`) produces them without executing code.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A literal default value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum SchemaLiteral {
    Integer(i64),
    Text(String),
    Boolean(bool),
    Null,
}

/// What a column takes when an INSERT omits it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum SchemaDefault {
    /// `.default(<literal>)`: a database default.
    Constant { value: SchemaLiteral },
    /// `serial`/`bigserial`/`smallserial` or an identity column.
    Sequence { generated: String },
    /// `.defaultNow()`, `.default(sql\`...\`)`: a database expression.
    Expression { text: String },
    /// `.$defaultFn(...)`/`.$default(...)`: computed by Drizzle in the
    /// application, not by the database.
    Application { text: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaColumn {
    /// The TypeScript property (`teamId`).
    pub property: String,
    /// The SQL column name (`team_id`).
    pub sql_name: String,
    /// The Drizzle builder (`integer`, `varchar`, `serial`, an enum builder).
    pub builder: String,
    /// PostgreSQL type as Drizzle declares it (`integer`, `varchar(255)`,
    /// `text`, `boolean`, `timestamp`, `enum role`).
    pub sql_type: String,
    pub not_null: bool,
    pub primary_key: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<SchemaDefault>,
    /// Allowed values of an enum column.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<String>,
    /// `.references(() => table.column)`: (table binding, column property).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<(String, String)>,
    pub line: u32,
}

/// A uniqueness rule: a column `.unique()`, `unique().on(...)`,
/// `uniqueIndex().on(...)`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaUnique {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// SQL column names.
    pub columns: Vec<String>,
    /// `constraint` or `index`.
    pub kind: String,
    /// `NULLS NOT DISTINCT`.
    #[serde(default)]
    pub nulls_not_distinct: bool,
    /// The index predicate of a PARTIAL unique index (`.where(...)`), as
    /// written. A partial index is NOT a global uniqueness constraint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
    pub line: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaForeignKey {
    /// SQL column names of this table.
    pub columns: Vec<String>,
    /// The referenced table binding and column properties.
    pub table: String,
    pub foreign_columns: Vec<String>,
    pub line: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaTable {
    /// The exported binding (`invitations`).
    pub binding: String,
    /// The SQL table name.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub columns: Vec<SchemaColumn>,
    /// SQL column names (a column `.primaryKey()` or `primaryKey({...})`).
    pub primary_key: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unique: Vec<SchemaUnique>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreign_keys: Vec<SchemaForeignKey>,
    /// Non-unique indexes (names), for the catalog comparison.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexes: Vec<String>,
    /// CHECK constraints as written (not interpreted).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<String>,
    /// What the extractor saw but does not model, with the line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsupported: Vec<String>,
    pub file: String,
    pub line: u32,
}

impl SchemaTable {
    pub fn column(&self, property: &str) -> Option<&SchemaColumn> {
        self.columns.iter().find(|c| c.property == property)
    }

    pub fn column_by_sql(&self, sql: &str) -> Option<&SchemaColumn> {
        self.columns.iter().find(|c| c.sql_name == sql)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedSchema {
    pub tables: Vec<SchemaTable>,
    /// Enum declarations: binding → (SQL type name, values).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub enums: BTreeMap<String, (String, Vec<String>)>,
    /// Declarations in the module that look like schema but were not
    /// understood (dynamic table names, spread columns, helpers).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsupported: Vec<String>,
}

impl ExtractedSchema {
    pub fn table(&self, binding_or_name: &str) -> Option<&SchemaTable> {
        self.tables
            .iter()
            .find(|t| t.binding == binding_or_name)
            .or_else(|| self.tables.iter().find(|t| t.name == binding_or_name))
    }
}

/// Where a fact of a model table comes from (Phase 8). The categories are
/// never conflated in reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FactOrigin {
    /// Read from the application's Drizzle schema (statically).
    ExtractedFromSource,
    /// Written in the scenario and consistent with the source.
    DeclaredByScenario,
    /// Written in the scenario and DIFFERENT from the source, with a reason.
    ScenarioOverride,
    /// Present in the source, not represented by the model.
    NotModeled,
    /// Present in the source, not understood by the extractor.
    Unknown,
    /// Confirmed by the disposable database's catalog.
    ConfirmedByRuntimeCatalog,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaFact {
    pub fact: String,
    pub origin: FactOrigin,
    /// `file:line` of the declaration, or the scenario path.
    pub source: String,
}

/// The facts behind one model table (Phase 8).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableFacts {
    pub table: String,
    /// The Drizzle binding it was read from (`from`), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
    pub facts: Vec<SchemaFact>,
}

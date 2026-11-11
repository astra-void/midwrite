//! `midwrite schema`: the PostgreSQL schema facts Midwrite reads from a
//! project's Drizzle definitions. Static by default; with
//! `--migrations ... --allow-execution` the migrations are applied to a
//! disposable PostgreSQL and the extracted facts are compared with its
//! catalog (schema drift). The application's code is never executed.

use std::path::PathBuf;

use clap::{Args, ValueEnum};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum SchemaFormat {
    Text,
    Json,
}

#[derive(Debug, Args)]
pub struct SchemaArgs {
    /// Project root.
    pub root: PathBuf,
    /// The Drizzle schema module, relative to the root.
    #[arg(long)]
    pub file: String,
    /// Migration SQL files to apply to a disposable PostgreSQL database
    /// whose catalog is compared with the extracted facts.
    #[arg(long = "migrations", value_name = "SQL", num_args = 1..)]
    pub migrations: Vec<PathBuf>,
    /// Required with --migrations: the migration SQL is executed (on a
    /// private, disposable cluster).
    #[arg(long)]
    pub allow_execution: bool,
    #[arg(long, value_enum, default_value_t = SchemaFormat::Text)]
    pub format: SchemaFormat,
}

/// Exit 0: extracted (and no drift); 1: drift; 2: usage or extraction error.
pub fn schema(args: &SchemaArgs) -> Result<u8, String> {
    let extracted = midwrite_analyzer::schema::extract_schema(&args.root, &args.file)?;
    let fidelity = if args.migrations.is_empty() {
        None
    } else {
        Some(compare(&extracted, args)?)
    };
    match args.format {
        SchemaFormat::Json => {
            let doc = serde_json::json!({
                "kind": "midwrite.schema",
                "schemaVersion": 1,
                "file": args.file,
                "schema": extracted,
                "fidelity": fidelity,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?
            );
        }
        SchemaFormat::Text => {
            for t in &extracted.tables {
                println!(
                    "table {}{} (binding `{}`, {}:{})",
                    t.schema
                        .as_ref()
                        .map(|s| format!("{s}."))
                        .unwrap_or_default(),
                    t.name,
                    t.binding,
                    t.file,
                    t.line
                );
                for c in &t.columns {
                    println!(
                        "  {:<24} {:<28} {}{}",
                        c.sql_name,
                        c.sql_type,
                        if c.not_null { "NOT NULL" } else { "NULL" },
                        c.default
                            .as_ref()
                            .map(|d| format!(" default {d:?}"))
                            .unwrap_or_default()
                    );
                }
                println!("  primary key ({})", t.primary_key.join(", "));
                for u in &t.unique {
                    println!(
                        "  unique {} ({}){}{}",
                        u.kind,
                        u.columns.join(", "),
                        u.predicate
                            .as_ref()
                            .map(|p| format!(" PARTIAL WHERE {p}"))
                            .unwrap_or_default(),
                        if u.nulls_not_distinct {
                            " NULLS NOT DISTINCT"
                        } else {
                            ""
                        }
                    );
                }
                for f in &t.foreign_keys {
                    println!(
                        "  foreign key ({}) → {}({})",
                        f.columns.join(", "),
                        f.table,
                        f.foreign_columns.join(", ")
                    );
                }
                for u in &t.unsupported {
                    println!("  unsupported: {u}");
                }
            }
            for u in &extracted.unsupported {
                println!("unsupported: {u}");
            }
            if let Some(f) = &fidelity {
                println!(
                    "\nschema fidelity ({}): {}",
                    f.postgres_version,
                    f.status.to_uppercase()
                );
                for t in &f.tables {
                    for i in &t.items {
                        if i.status != "agree" {
                            println!("  {} {}: {} {}", t.table, i.fact, i.status, i.detail);
                        }
                    }
                }
            }
        }
    }
    Ok(match &fidelity {
        Some(f) if f.status == "drift" => 1,
        _ => 0,
    })
}

#[cfg(feature = "replay")]
fn compare(
    extracted: &midwrite_model::schema_facts::ExtractedSchema,
    args: &SchemaArgs,
) -> Result<midwrite_model::schema_fidelity::SchemaFidelity, String> {
    let (catalog, version) =
        midwrite_replay::catalog::read_migrations(&args.migrations, args.allow_execution)?;
    let mut fidelity = midwrite_model::schema_fidelity::compare(extracted, &catalog, &[]);
    fidelity.postgres_version = version;
    Ok(fidelity)
}

#[cfg(not(feature = "replay"))]
fn compare(
    _: &midwrite_model::schema_facts::ExtractedSchema,
    _: &SchemaArgs,
) -> Result<midwrite_model::schema_fidelity::SchemaFidelity, String> {
    Err("this build does not include PostgreSQL support".to_owned())
}

//! Reading the schema of a disposable database from PostgreSQL's catalog
//!: tables, columns (type, NOT NULL, default, identity),
//! constraints (primary key, unique, foreign key, check) and indexes
//! (unique or not, columns, partial-index predicate, NULLS NOT DISTINCT).
//!
//! Read-only (`psql` sessions run with `default_transaction_read_only`),
//! on a cluster this crate created. The result is plain JSON; the model
//! crate compares it with the schema facts extracted from source.

use serde_json::Value;

use crate::postgres::{Cluster, major_version};

fn catalog_sql(nulls_not_distinct: bool) -> String {
    let nnd = if nulls_not_distinct {
        "i.indnullsnotdistinct"
    } else {
        "false"
    };
    format!(
        "SELECT coalesce(jsonb_agg(t ORDER BY t.schema, t.name), '[]'::jsonb)::text FROM (
  SELECT n.nspname AS schema, c.relname AS name,
    (SELECT coalesce(jsonb_agg(jsonb_build_object(
        'name', a.attname,
        'type', format_type(a.atttypid, a.atttypmod),
        'notNull', a.attnotnull,
        'default', pg_get_expr(d.adbin, d.adrelid),
        'identity', nullif(a.attidentity::text, '')) ORDER BY a.attnum), '[]'::jsonb)
      FROM pg_attribute a LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
      WHERE a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped) AS columns,
    (SELECT coalesce(jsonb_agg(jsonb_build_object(
        'name', con.conname,
        'kind', con.contype::text,
        'columns', (SELECT coalesce(jsonb_agg(a.attname ORDER BY k.ord), '[]'::jsonb)
                    FROM unnest(con.conkey) WITH ORDINALITY k(attnum, ord)
                    JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum),
        'foreignTable', (SELECT relname FROM pg_class WHERE oid = con.confrelid),
        'foreignColumns', (SELECT coalesce(jsonb_agg(a.attname ORDER BY k.ord), '[]'::jsonb)
                    FROM unnest(con.confkey) WITH ORDINALITY k(attnum, ord)
                    JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.attnum),
        'definition', pg_get_constraintdef(con.oid)) ORDER BY con.conname), '[]'::jsonb)
      FROM pg_constraint con WHERE con.conrelid = c.oid) AS constraints,
    (SELECT coalesce(jsonb_agg(jsonb_build_object(
        'name', ic.relname,
        'unique', i.indisunique,
        'primary', i.indisprimary,
        'columns', (SELECT coalesce(jsonb_agg(a.attname ORDER BY k.ord), '[]'::jsonb)
                    FROM unnest(i.indkey::int2[]) WITH ORDINALITY k(attnum, ord)
                    JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = k.attnum),
        'predicate', pg_get_expr(i.indpred, i.indrelid),
        'nullsNotDistinct', {nnd},
        'constraint', EXISTS (SELECT 1 FROM pg_constraint x WHERE x.conindid = i.indexrelid))
        ORDER BY ic.relname), '[]'::jsonb)
      FROM pg_index i JOIN pg_class ic ON ic.oid = i.indexrelid WHERE i.indrelid = c.oid) AS indexes
  FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
  WHERE c.relkind IN ('r', 'p')
    AND n.nspname NOT IN ('pg_catalog', 'information_schema')
    AND n.nspname NOT LIKE 'pg_toast%'
) t"
    )
}

/// The catalog of `database` as a JSON array of tables.
pub fn read(cluster: &Cluster, database: &str) -> Result<Value, String> {
    let sql = catalog_sql(major_version(cluster.version()) >= 15);
    match cluster.query_value(database, &sql)? {
        Value::String(text) => serde_json::from_str(&text).map_err(|e| e.to_string()),
        other => Err(format!("unexpected catalog result {other}")),
    }
}

/// Start a disposable cluster, apply `migrations` (SQL files) to a fresh
/// database, and read its catalog. Executes the SQL of the files (never
/// application code); callers require explicit authorization.
pub fn read_migrations(
    migrations: &[std::path::PathBuf],
    allow_execution: bool,
) -> Result<(Value, String), String> {
    if !allow_execution {
        return Err(
            "reading a database catalog applies the project's migration SQL to a disposable PostgreSQL: it requires the explicit --allow-execution flag"
                .to_owned(),
        );
    }
    let bins = crate::postgres::Binaries::discover()?;
    let root = crate::safety::ReplayRoot::create(false)?;
    let cluster = Cluster::start(bins, &root.path.join("db"))
        .map_err(|e| format!("the disposable PostgreSQL cluster failed: {e}"))?;
    crate::safety::check_database_target(&root.path, &cluster.socket_path())?;
    cluster.create_database("schema_check")?;
    for m in migrations {
        cluster
            .apply_file("schema_check", m)
            .map_err(|e| format!("applying `{}` failed: {e}", m.display()))?;
    }
    let catalog = read(&cluster, "schema_check")?;
    Ok((catalog, cluster.version().to_owned()))
}

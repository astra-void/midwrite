# Schema fidelity: source vs database catalog

`crates/commitrift-replay/src/catalog.rs` (catalog reader),
`crates/commitrift-model/src/schema_fidelity.rs` (comparison).

## When it runs

* `commitrift schema ROOT --file schema.ts --migrations a.sql ... --allow-execution`:
  the migration SQL is applied to a private, disposable cluster (Unix
  socket only, removed afterwards) and its catalog is compared with the
  extracted facts. Without `--allow-execution` this is refused; without
  `--migrations` the command is static.
* `model-check --verify-source --allow-execution` for a scenario that reads
  tables from source: the run database's catalog — after the application's
  own migrations, before the seed — is compared, and the result is in
  `sourceVerification.schemaFidelity`.

The application's code is never executed for this; the database is never
changed to make the model agree. A difference is reported as **drift**.

## What is compared

From `pg_attribute`, `pg_attrdef`, `pg_constraint`, `pg_index` (read-only
sessions; JSON):

| fact | rule |
| --- | --- |
| table present (with schema) | else drift |
| column present; columns in the database the source does not declare | drift (or not-compared when the source has constructs the extractor did not understand, such as spread columns) |
| type | `format_type` vs the builder's type (`varchar(255)` ↔ `character varying(255)`, `timestamp` ↔ `timestamp without time zone`, enum ↔ its type name, `serial` ↔ `integer`); unmapped types not-compared |
| NOT NULL | exact |
| default | constant (casts stripped: `'pending'::text`), sequence (`nextval(...)` for serial, `attidentity` `a`/`d` for identity), expression (text, whitespace/case-normalized), application default ↔ no database default |
| primary key | column list |
| unique | constraints and unique indexes (not the primary key), as column sets; partial vs global must match; NULLS NOT DISTINCT must match (PostgreSQL 15+); predicates compared by presence only; unique objects the source does not declare are drift |
| foreign keys | columns, referenced table, referenced columns; extra ones are drift |
| CHECK | count (expressions not compared) |

## Results

| source | migration | PostgreSQL 17.11 | facts |
| --- | --- | --- | --- |
| `fixtures/phase8/shop-app` | `drizzle/0000_init.sql` | **agree** | 8 tables |
| `fixtures/phase7/team-app` | `drizzle/0000_init.sql` | **agree** | 5 tables |
| `fixtures/phase8/schemas` (edge cases) | `drizzle/0000_init.sql` | **agree** (one spread column not-compared) | 4 tables |
| `fixtures/phase8/schemas` | `drizzle/drift.sql` (drift injected on purpose) | **drift: 8 items, all injected** — nullable `email`, default `'viewer'` vs `'member'`, missing `UNIQUE(email)`, a **global** index where the source's is partial, missing `NULLS NOT DISTINCT` constraint, two missing foreign keys, missing CHECK | |
| saas-starter (external) | not run | — | static extraction only (not authorized to execute its migrations) |

Every source-verification run of a `from` scenario also compared its run
database (for example `checkout-crash-after-charge`: 38 facts, agree).

Tests: `extracted_schemas_agree_with_the_databases_their_migrations_create`,
`schema_drift_between_source_and_database_is_reported_never_repaired`,
`reading_a_schema_is_static_and_comparing_it_needs_authorization`, and the
`schemaFidelity` assertion of the checkout crash test.

# Drizzle schema extraction

`crates/commitrift-analyzer/src/schema.rs` (extractor),
`crates/commitrift-model/src/schema_facts.rs` (facts and origins),
`crates/commitrift-model/src/scenario.rs` (`from`, overrides),
`commitrift schema` (CLI).

## Reading, not executing

The extractor parses the schema module with Oxc and reads declarations;
the module is never executed (a module that throws at load is read the
same way — test `schema_extraction_never_executes_the_module`). Only
literal names and the documented Drizzle builders are understood:

| source | facts |
| --- | --- |
| `pgTable("name", { ... }, (t) => [...])`, also `(t) => ({ ... })` | table, binding, line |
| `pgSchema("s")` + `s.table(...)` | schema namespace |
| `pgEnum("name", ["a", "b"])` | enum type and values (columns built with it) |
| column builders `serial`, `bigserial`, `smallserial`, `integer`, `smallint`, `bigint`, `text`, `varchar({length})`, `char`, `boolean`, `uuid`, `timestamp({withTimezone})`, `date`, `numeric`, `json`, `jsonb`, `real`, `doublePrecision`, enum builders | SQL column name, PostgreSQL type |
| `.notNull()`, `.primaryKey()`, `.unique(name?, {nulls})` | nullability, primary key, unique (NULLS NOT DISTINCT) |
| `.default(literal)` | constant database default |
| `.default(sql…)`, `.defaultNow()`, `.defaultRandom()` | database **expression** default (kept as text) |
| `.$defaultFn(...)`, `.$default(...)` | **application** default (Drizzle computes it, the database has none) |
| `serial*`, `.generatedAlwaysAsIdentity()`, `.generatedByDefaultAsIdentity()` | generated identifier (sequence) |
| `.references(() => t.col)` | foreign key |
| `primaryKey({columns: [...]})`, `primaryKey(t.a, t.b)` | composite primary key |
| `unique("n").on(...)`, `uniqueIndex("n").on(...)`, `.nullsNotDistinct()`, `.where(sql…)` | composite uniqueness; **partial** indexes keep their predicate |
| `index("n").on(...)` | non-unique index (for the catalog comparison) |
| `foreignKey({columns, foreignColumns})` | composite foreign key |
| `check("n", sql…)` | CHECK, kept as text (not interpreted) |

Everything else becomes an **unsupported** fact with its line: a dynamic
table name (`pgTable(prefix + "x", ...)`), spread columns (`...timestamps`),
a column without an explicit SQL name (Drizzle's `casing` option could
change it), unknown builders and modifiers, non-literal enum values.

## Semantics that must not be generalized

| | rule |
| --- | --- |
| partial unique index | never a global uniqueness constraint: the model does not get it; the report lists it as `not-modeled` with its predicate |
| nullable unique columns | modeled with PostgreSQL's NULLS DISTINCT semantics (the model already skips uniqueness for keys containing NULL) |
| `NULLS NOT DISTINCT` on a nullable column | not modeled (the model treats NULLs as distinct) — reported |
| expression and application defaults | not modeled values; a column relying on one is excluded and reported; a NOT NULL column without any default that is not modeled is flagged ("an INSERT the model accepts may fail") |
| enum | modeled as text; its domain is reported as not enforced |
| foreign keys, CHECK | not modeled: a violation (23503, 23514) is not predicted — reported |

## Scenarios that read tables from source

```json
"extraction": { "root": "../shop-app", "schema": "src/lib/db/schema.ts" },
"database": { "tables": [
  { "from": "orders", "columns": ["id", "customerEmail", "amount", "status"] },
  { "from": "jobs", "overrides": { "reason": "what if UNIQUE(order_id) existed?", "unique": [["orderId"]] } }
] }
```

* every fact comes from the source; `columns` selects (by TypeScript
  property) — an explicitly selected column the model cannot represent is
  an error;
* a declared fact (name, nullability, SQL name, type, default, primary key,
  unique sets) that **contradicts** the source is an **error** — values
  are never merged silently;
* an intended difference is an **override** with a `reason`
  (`unique`, `primaryKey`, `notNull`), listed in the report as
  `scenario-override`;
* the actor extractor learns the Drizzle binding of every `from` table
  (no `extraction.tables` mapping needed).

Every fact in the report (`schemaFacts`, report schema 3) carries its
origin — `extracted-from-source`, `declared-by-scenario`,
`scenario-override`, `not-modeled`, `unknown`, and, after a catalog
comparison, `confirmed-by-runtime-catalog` — and its `file:line`.

The 12 shop-app scenarios now read their tables from source; each yields
exactly the model the hand-declared version produced (same verdict, states,
transitions and table definitions).

## Tests

`commitrift-analyzer/tests/phase8.rs`: every supported fact with its line
(single and composite primary keys, composite and nullable uniques, partial
index, NULLS NOT DISTINCT, identity and serial ids, constant/expression/
application defaults, enum, `pgSchema`, foreign keys, CHECK), refusal of
dynamic declarations, spread columns and casing-dependent names, no
execution, object-form constraints; scenario linking with origins,
conflicts, overrides, unknown bindings, unmodelable explicit columns; partial
and NULLS NOT DISTINCT uniques not generalized.

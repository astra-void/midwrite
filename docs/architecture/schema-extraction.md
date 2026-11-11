# Drizzle schema extraction and schema fidelity

Code: `crates/midwrite-analyzer/src/schema.rs` and `generators.rs`
(extractor), `crates/midwrite-model/src/schema_facts.rs` (facts and
origins), `crates/midwrite-model/src/scenario.rs` (`from`, overrides),
`crates/midwrite-replay/src/catalog.rs` and
`crates/midwrite-model/src/schema_fidelity.rs` (catalog comparison),
`midwrite schema` (CLI, [reference](../reference/cli.md)).

Tables reach the model in one of two ways: declared in a scenario, or
read from the application's Drizzle schema modules. Discovery always reads
them from source. How the model then treats keys, foreign keys, defaults
and unique rules is in the [PostgreSQL model](postgres-model.md).

## Reading, not executing

The extractor parses schema modules with Oxc and reads declarations; a
module is never executed (one that throws at load is read the same way).
Only literal names and the documented Drizzle builders are understood:

| source | facts |
| --- | --- |
| `pgTable("name", { ... }, (t) => [...])`, also `(t) => ({ ... })` | table, binding, line |
| `pgSchema("s")` + `s.table(...)` | schema namespace |
| `pgEnum("name", ["a", "b"])` | enum type and values (modeled as text; the domain is reported as not enforced) |
| column builders `serial`, `bigserial`, `smallserial`, `integer`, `smallint`, `bigint`, `text`, `varchar({length})`, `char`, `boolean`, `uuid`, `timestamp({withTimezone})`, `date`, `numeric`, `json`, `jsonb`, `real`, `doublePrecision`, enum builders | SQL column name, PostgreSQL type (time and JSON columns are modeled only with the scenario features `timestamps` / `json`) |
| `.notNull()`, `.primaryKey()`, `.unique(name?, { nulls })` | nullability, primary key, unique (with `NULLS NOT DISTINCT`) |
| `.default(literal)`, `.default(sql…)`, `.defaultNow()`, `.defaultRandom()`, `.$defaultFn(...)`, `.$default(...)` | defaults, classified below |
| `serial*`, `.generatedAlwaysAsIdentity()`, `.generatedByDefaultAsIdentity()` | generated identifier (sequence) |
| `.references(() => t.col, { onDelete, onUpdate })`, `foreignKey({ columns, foreignColumns })` | foreign keys with referential actions, resolved by declaration |
| `primaryKey({ columns })`, `primaryKey(t.a, t.b)` | composite primary key |
| `unique("n").on(...)`, `uniqueIndex("n").on(...)`, `.nullsNotDistinct()`, `.where(sql…)` | composite uniqueness; **partial** indexes keep their predicate |
| `index("n").on(...)` | non-unique index (for the catalog comparison) |
| `check("n", sql…)` | CHECK, kept as text (not interpreted) |

Everything else becomes an **unsupported** fact with its line: a dynamic
table name, spread columns (`...timestamps`), helper-built columns, custom
types, unknown builders and modifiers, non-literal enum values. A column
without an explicit SQL name depends on Drizzle's `casing` option; the
extractor assumes the property name and records the assumption.

## Defaults and generated values

Every column default is classified (`schema_facts::GeneratorClass`,
reported as `SchemaColumn.generation`):

| category | source forms | model |
| --- | --- | --- |
| `database-generated` | `uuid().defaultRandom()`, `.default(sql\`gen_random_uuid()\`)` (`sql` from `drizzle-orm`), `.defaultNow()` | a fresh identifier for the UUID forms; `now()` is a clock reading with the `timestamps` feature |
| `application-generated` | `$defaultFn`/`$default` with a **recognized** generator | a fresh identifier |
| `sequence-generated`, `identity-generated` | `serial*`, identity columns | the next sequence value (non-transactional, allocated before constraint checks) |
| `constant-default` | `.default(<literal>)` | the constant |
| `unknown-default` | any other callback or SQL expression | not executed, not given a value: the column is not modeled (a primary key with it refuses the table) |

A recognized application generator is a callback that calls, with no
other effect and no arguments, one of: `crypto.randomUUID()` (the WebCrypto
global when nothing local is named `crypto`, or an import of
`crypto`/`node:crypto`), `randomUUID` from `crypto`, `createId` from
`@paralleldrive/cuid2`, `nanoid` from `nanoid`, `v4`/`v7` from `uuid`,
`ulid` from `ulid`/`ulidx`, `uuidv7` from `uuidv7` — **established by its
import**, never by name. Accepted shapes: `() => g()`, `() => { return
g(); }`, `function () { return g(); }`, a reference to such an import, or a
helper in the same module or a project module. Refused with the reason:
`customIdService.generate()`, `process.env.X`, a local function merely
named like a generator, `crypto.randomUUID` passed unbound, async
callbacks, callbacks with parameters. With both a database default and
`$defaultFn`, the column is application-generated (drizzle-orm 0.45.4
calls `defaultFn` first). `$onUpdate` is a not-modeled fact.

Generated values are identities, not concrete values: the model's
semantics of `Value::Gen` are in the
[PostgreSQL model](postgres-model.md).

## Rules that must not be generalized

| | rule |
| --- | --- |
| partial unique index | never a global uniqueness constraint: a recorded protection only; a counterexample on a table with one is INCONCLUSIVE |
| nullable unique columns | NULLS DISTINCT semantics unless the source declares `NULLS NOT DISTINCT`, which is enforced |
| expression and application defaults | modeled only for the categories above; a column relying on another one is excluded and reported; a NOT NULL column without a modeled default is flagged ("an INSERT the model accepts may fail") |
| foreign keys | enforced only with the scenario feature `foreign-keys` (`NO ACTION`/`RESTRICT`, immediate); `CASCADE`/`SET NULL`/`SET DEFAULT` and deferred constraints are refused when a statement would reach them |
| CHECK, enum domains | kept as text / not enforced; a violation is not predicted (scenarios may declare simple CHECKs themselves) |

## Scenarios that read tables from source

```json
"extraction": { "root": "../shop-app", "schema": "src/lib/db/schema.ts" },
"database": { "tables": [
  { "from": "orders", "columns": ["id", "customerEmail", "amount", "status"] },
  { "from": "jobs", "overrides": { "reason": "what if UNIQUE(order_id) existed?", "unique": [["orderId"]] } }
] }
```

* Every fact comes from the source; `columns` selects by TypeScript
  property, and an explicitly selected column the model cannot represent
  is an error.
* A declared fact that **contradicts** the source is an error — values are
  never merged silently. An intended difference is an **override** with a
  `reason` (`unique`, `primaryKey`, `notNull`), reported as
  `scenario-override`.
* The actor extractor learns the Drizzle binding of every `from` table.

**Several schema modules.** `extraction.schema` may be a list; a table's
`from` is `binding` (unique across the listed modules) or
`module#binding`. Errors: an unqualified binding declared by several
modules, a module listed twice, two used declarations with one SQL name.
With a list, an identifier denotes a table only if it **resolves** —
through imports, aliases and barrels — to that declaration: a local
variable or another declaration with the same name is not the table, an
import alias is; `db.query.<key>` is restricted to declarations the
querying module reaches. Foreign keys are resolved by declaration too.
Discovery indexes multi-module projects this way, refuses a second
declaration of an SQL name, and emits scenarios listing exactly the
modules of a candidate's table closure (the tables its actors touch plus
foreign-key parents).

Every fact in a report (`schemaFacts`) carries its origin —
`extracted-from-source`, `declared-by-scenario`, `scenario-override`,
`not-modeled`, `unknown`, and after a catalog comparison
`confirmed-by-runtime-catalog` — and its `file:line`.

## Schema fidelity: source vs. database catalog

* `midwrite schema ROOT --file schema.ts --migrations a.sql ...
  --allow-execution` applies the migration SQL to a private, disposable
  cluster (Unix socket only, removed afterwards) and compares its catalog
  with the extracted facts. Without `--allow-execution` this is refused;
  without `--migrations` the command is static.
* `model-check --verify-source --allow-execution` on a scenario that reads
  tables from source compares the run database's catalog (after the
  application's own migrations, before the seed); the result is
  `sourceVerification.schemaFidelity`.

Application code is never executed for this, and the database is never
changed to make the model agree: a difference is reported as **drift**.
Compared (from `pg_attribute`, `pg_attrdef`, `pg_constraint`, `pg_index`,
read-only sessions):

| fact | rule |
| --- | --- |
| tables, columns | present on both sides; extra database columns are drift (not compared when the source has constructs the extractor did not understand) |
| type | `format_type` vs. the builder's type (`varchar(255)` ↔ `character varying(255)`, `serial` ↔ `integer`, enum ↔ its type name); unmapped types not compared |
| NOT NULL, primary key | exact |
| default | constant (casts stripped), sequence (`nextval`, identity `a`/`d`), expression (normalized text), application default ↔ no database default |
| unique | constraints and unique indexes as column sets; partial vs. global and NULLS NOT DISTINCT must match; predicates compared by presence only |
| foreign keys | columns, referenced table and columns; extra ones are drift |
| CHECK | count only (expressions not compared) |

Tests: `crates/midwrite-analyzer/tests/drizzle_schema.rs` (every
supported fact with its line, refusals, no execution, scenario linking,
overrides), `crates/midwrite-analyzer/tests/schema_modules.rs` (several
modules, generator classification),
`crates/midwrite-cli/tests/crash_retry.rs` (catalog comparison and drift),
`crates/midwrite-model/tests/generated_values.rs`; fixtures
[`fixtures/apps/drizzle-schemas`](../../fixtures/apps/drizzle-schemas)
(edge cases and an injected-drift migration),
[`fixtures/apps/shop-app`](../../fixtures/apps/shop-app),
[`fixtures/apps/workspace-app`](../../fixtures/apps/workspace-app),
[`fixtures/apps/defaults-probe`](../../fixtures/apps/defaults-probe).

# Generated values, Drizzle defaults and RETURNING

This document covers Milestones 1, 2.3, 8.2 and the generator part of 4
(`GENERATED-VALUE-SEMANTICS`, `DRIZZLE-DEFAULTS` and `RETURNING-SEMANTICS` of
the plan, consolidated).

## 1. Generator categories (`schema_facts::GeneratorClass`)

Every column default the extractor reads is classified; the category is a
schema fact with its provenance (`SchemaColumn.generation`, serialized only
when present, so earlier serialized schemas are unchanged).

| category | source forms | model |
| --- | --- | --- |
| `database-generated` | `uuid().defaultRandom()`, `.default(sql\`gen_random_uuid()\`)` with `sql` from `drizzle-orm`, `.defaultNow()` | a fresh identifier for the UUID forms (`ColumnDefault::Generated`, side `database`); `now()` stays *not modeled* (timestamps are not model types) |
| `application-generated` | `$defaultFn`/`$default` with a **recognized** callback (below) | a fresh identifier (side `application`) |
| `sequence-generated` | `serial`, `bigserial`, `smallserial` | `ColumnDefault::Sequence` (Phase 7, unchanged) |
| `identity-generated` | `generatedAlwaysAsIdentity()`, `generatedByDefaultAsIdentity()` | `ColumnDefault::Sequence` |
| `constant-default` | `.default(<literal>)` | `ColumnDefault::Value` |
| `unknown-default` | any other callback or SQL expression | not executed, not given a value: the column is *not modeled* (a primary key with it refuses the table, as before) |

When a column has both a database default and `$defaultFn`, it is classified
as application-generated: when the INSERT omits the column, drizzle-orm
0.45.4's `buildInsertQuery` calls `defaultFn` first and emits `default` only
without one (`node_modules/drizzle-orm/pg-core/dialect.js:378`, read in the
pinned package). The combination itself was not probed at runtime (the
probes cover each side alone). The same code shows two shapes the model does
not represent: a callback returning an `SQL` value is inlined as SQL (not a
recognized generator), and `$onUpdate` runs on INSERT for a column without a
default (`$onUpdate` stays a *not modeled* fact).

### Recognized application generators (`analyzer::generators`)

A callback is recognized only when it calls, with no other effect and no
arguments, one of these, **and the binding is established by its import**:
`crypto.randomUUID()` (the WebCrypto global when nothing local is named
`crypto`, or the default/namespace import of `crypto`/`node:crypto`),
`randomUUID` from `crypto`/`node:crypto`, `createId` from
`@paralleldrive/cuid2`, `nanoid` from `nanoid` (optionally with a numeric
length), `v4`/`v7` from `uuid`, `ulid` from `ulid`/`ulidx`, `uuidv7` from
`uuidv7`. Accepted shapes: `() => g()`, `() => { return g(); }`,
`function () { return g(); }`, a reference `g` to such an import, or a
helper declared in the same module (one level) or exported by a project
module (`$defaultFn(createId)` with `createId` from `src/lib/ids.ts`,
resolved through the shared project context).

Refused, with the reason recorded: `customIdService.generate()`,
`process.env.X`, a local function merely named `uuid4` (or one with a side
effect), `crypto.randomUUID` passed **unbound** (the WebCrypto method throws
"Illegal invocation" without its receiver), a local `crypto` shadowing the
global, async callbacks, callbacks with parameters, template strings around
a generator. Tests: `analyzer/tests/phase12.rs`
(`defaults_are_classified_by_mechanism_and_only_known_generators_are_fresh`,
`a_callback_imported_from_a_project_module_is_classified_by_its_definition`).

## 2. Generated values in the model (`value::Value::Gen`)

```rust
pub enum Value { Null, Bool(bool), Int(i64), Str(String), Gen(GenId), Row(Row) }
pub struct GenId { generator: u16, actor: u16, n: u32 }  // JSON: {"$generated": g, "actor": a, "n": n}
```

* **Identity = the generation event**: which generator (`Model::generators`
  index), which actor, that actor's generation counter `gen_seq`. The counter
  is per actor and never reset — not by a rollback, not by a retry — so every
  event has its own identity and the identity does not depend on how other
  actors are scheduled (no shared state, no footprint entry: unlike a
  sequence, generation does not make inserts of different actors dependent).
* **Equality is identity**: a generated value equals itself and nothing else;
  it is distinct from every concrete value. This is the explicit modeling
  assumption **`generated-values-distinct`**, printed with every result of a
  model that has generators. Collisions of random identifiers are not
  explored; see `LIMITATIONS.md`.
* **No order**: `<`, `<=`, … on a generated value are an evaluation error
  (unknown). Where a `LIMIT 1` read needs an order among rows with generated
  keys, rows are ordered by generation event; that this differs from
  PostgreSQL (which orders random UUIDs by value) is printed as an
  assumption.
* **Execution boundary**: a generated default is assigned when the INSERT
  *starts executing* (`exec::apply_defaults`, `!p.started`) — the point where
  Drizzle builds the statement (application side) and where PostgreSQL
  evaluates defaults (database side). A statement that waits for a lock and
  resumes keeps its values; a new execution (a retried request, a retried
  transaction, a resubmission) generates new ones.
* **Initial state**: generated values are never initial data. The loader
  refuses them in `initialState`; synthesis never requires a row whose key
  the execution generates.
* **State identity**: the generation counters are part of the canonical
  state, written only when non-zero (encodings of earlier models are
  unchanged).
* **Serialization**: `{"$generated": …, "actor": …, "n": …}` (a reserved key);
  rendering `gen#g:a.n`.

### What the two sides differ in, and what the model keeps

| question (Milestone 1.3) | database (`gen_random_uuid()`) | application (`$defaultFn`) | model |
| --- | --- | --- | --- |
| when evaluated | while PostgreSQL executes the INSERT | when Drizzle builds the statement, once **per row**, at **every** execution of the builder (a builder awaited twice generates twice) | at the statement's first execution step |
| sent to the database | `default` keyword | a bound parameter | — |
| visible to the application before completion | only via `RETURNING` | no (internal to Drizzle) | only via `RETURNING` |
| failed INSERT | value discarded | callback **still runs**, value discarded | counter consumed, nothing observable |
| rollback | not reused | not reused | counter not reset |
| application retry / resubmission | new value | new value | new event, new value |
| side effects of the callback | — | they happen (a counter in the probe was incremented on a failing insert) | only recognized side-effect-free generators are modeled |

Every row of this table except the model column is a measured result
(`POSTGRESQL-DIFFERENTIAL.md`).

## 3. RETURNING and propagation

* `insert(t).values(...).returning()` returns the inserted row (Phase 7);
  **`returning({ alias: t.col, ... })`** (Phase 12) is lowered as the INSERT
  with `RETURNING col, ...` (`StmtKind::Insert.returning_columns`, the
  columns Drizzle sends) and a local projection
  `out = row == null ? null : { alias: row.col }`. A generated key returned
  this way is the generation event's value; storing it in another table
  (`values({ ownerId: created.id })`) stores that same value, so foreign keys
  keep identity.
* `update ... returning()` keeps the Phase 7–11 treatment: only the row count
  (and truthiness of the first row) is modeled, which is exact for the
  `rows.length === 0` / `if (!row)` decisions; returned row *values* of an
  UPDATE are not modeled.
* A result that an unsupported operation would have assigned is not given a
  value: the sealed `Op::Unknown` records the locals it would have assigned
  (`defines`), so the scenario loads and the path ends as unsupported where it
  reaches the operation (the saas-starter `signUp` regression).

## 4. PostgreSQL oracle and original-code verification

* The lockstep oracle sends a generated value as a deterministic UUID that
  encodes the event (`00000000-0000-4000-8ggg-aaaannnnnnnn`) and reads it back
  as the same event: constraints, visibility and blocking are checked with
  generated keys; generation itself is checked by the Drizzle probes.
* Original-code verification matches model generated values to the run's
  concrete values **bijectively per run** (`source_verify::GenMatch`): a
  generated value matches the first text it is compared with and only that
  text afterwards; two generated values never match one text. Statement
  results, final tables (as multisets) and crash checkpoints use the same
  correspondence.

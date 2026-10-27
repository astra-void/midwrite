# Phase 12 limitations

What the model does **not** represent, what it approximates, and what was
not measured. Supported semantics are in `GENERATED-VALUE-SEMANTICS.md` and
`ARCHITECTURE.md`.

## Assumptions printed with results

* **`generated-values-distinct`**: two generation events never produce the
  same value, and a generated value equals no pre-existing concrete value.
  Collisions of random identifiers (UUIDv4, nanoid, cuid2) are not explored;
  there is no collision mode. A property whose verdict would depend on a
  collision is therefore not checked against it.
* **Order of generated values**: where a `LIMIT 1` read needs an order among
  rows with generated keys, rows are ordered by generation event; PostgreSQL
  orders them by their random value. (Ordering comparisons on generated
  values are an evaluation error, never a guessed result.)

## Not modeled (reported as unknown or as not-modeled facts)

* Unrecognized default callbacks (`customIdService.generate()`,
  `process.env.X`, callbacks with side effects, callbacks returning an `SQL`
  value), `$onUpdate` (which drizzle-orm 0.45.4 also runs on INSERT for a
  column without a default), `default(sql…)` other than `gen_random_uuid()`,
  `uuid_generate_v4()` (an extension), triggers, RLS, unsupported CHECK
  expressions, dynamic schema declarations, Drizzle `alias()` self-joins,
  timestamp/JSON/array column types (columns are dropped with a fact; a
  primary key of such a type refuses its table).
* Multi-row inserts, re-executing a stored builder, `sql\`default\`` and
  other SQL values in `values(...)` — their runtime behavior was measured by
  the probes, but extraction reports them as unknown operations.
* `update/delete ... returning()` row **values** (row counts and presence
  are modeled).
* **Return values of the main task.** A hazard whose only observable effect
  is what the function returns (a lost claim that both callers report as
  successful: evaluation case `claimTask`) produces no candidate.
* **A crash between the last statement and the response.** The response is
  not a model event; a single-statement entry with a declared crash retry
  cannot exhibit the duplicate its resubmission causes (evaluation case
  `recordSignup`). Entries with a later operation can (`addWorkspace`).
  Adding a response event would change the state spaces of every earlier
  scenario and was not done in this phase.
* **Foreign-key locks.** PostgreSQL's foreign-key check locks the
  referenced row `FOR KEY SHARE`; the model has no foreign keys, so an
  INSERT that PostgreSQL makes wait for another transaction's `FOR UPDATE`
  on the parent proceeds in the model (measured divergence,
  `POSTGRESQL-DIFFERENTIAL.md` §5). Candidates where this can happen carry
  the `foreign-key-locks` assumption.
* Read replicas (`withReplicas`, midday): reads that may be stale are not
  modeled; database handles whose declared type is computed
  (`Awaited<ReturnType<typeof f>>`) are not recognized, so their operations
  stay unknown (measured category "database handle of an unresolved type").
* Union handle types (`Database | Transaction` parameters of exported
  helpers) are not bound when the helper is an entry (the connection kind
  depends on the caller); they are bound when the helper is inlined with a
  known handle.
* Methods of a class with `extends`, constructors that do more than store
  their parameters, objects modified anywhere in their module, `this` of an
  entry that is itself a method (no instance), dynamic dispatch.
* Calls into packages (`bcryptjs.hash` blocks saas-starter's `signUp`).
* A Drizzle `casing` option changes SQL column names; the extractor assumes
  the property name and records the assumption (midday uses
  `casing: "snake_case"`).

## Not measured / not independent

* The Phase 12 evaluation set is fresh and frozen but written by the
  implementation's author (`INDEPENDENT-EVALUATION.md`); there was still
  **no independent human review**, and no corpus finding is claimed as a
  defect of a corpus project.
* Original-code reproduction was measured for the first-party workspace
  application only; the evaluation application has no harness.
* Memory: peak RSS per run is measured; per-structure sizes (index, cache,
  schema graph) are not instrumented.

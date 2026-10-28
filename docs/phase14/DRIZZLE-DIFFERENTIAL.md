# Phase 14 Track D: Drizzle and PostgreSQL value semantics, differentially tested

Multi-row inserts, predicates, timestamps and JSON columns: what the model
does, what it refuses, and how it was compared with PostgreSQL 17.11 and
the pinned drizzle-orm 0.45.4. Code: `model/src/{ir,exec,engine,db,state,
oracle,sqlfn,scenario,source_verify}.rs`, `analyzer/src/{concurrency,
schema}.rs`. Tests: `model/tests/phase14_drizzle.rs` (19),
`model/tests/pg_matrix.rs` (3 new lockstep tests),
`analyzer/tests/phase14_drizzle.rs` (8), fixture `fixtures/phase14/drizzle-app`.

## D1. Multi-row INSERT

`db.insert(t).values([{…}, {…}])` (at most 8 rows) is **one** statement:

| behavior | model (measured on PostgreSQL 17.11 where noted) |
| --- | --- |
| atomicity | any error aborts the statement (its transaction): no row of it stays |
| defaults | Drizzle's application defaults (`$defaultFn`, clock) are computed for all rows when the statement is built; database defaults (sequences, `gen_random_uuid()`, `now()`) are drawn per row as the row is formed — measured: `('k'),('l')` with `k` existing draws 1 sequence value, `('l'),('l')` draws 2, `('a'),('b'),('k')` draws 3 |
| waits | a row that waits for a lock keeps the earlier rows written; the statement resumes at that row |
| unique | a conflict between two rows of the statement is 23505 |
| foreign keys | checked per row at the end of the statement (Phase 13 RI semantics) |
| `ON CONFLICT DO NOTHING` | a row conflicting with an earlier row of the same statement is skipped (measured: `(1,'x'),(2,'x')` inserts one row) |
| `ON CONFLICT DO UPDATE` | reaching a row the statement already inserted or updated is SQLSTATE 21000 (measured) |
| `returning()` | a list of the rows actually returned |

IR: `StmtKind::Insert` gains `more_rows` (omitted when empty: single-row
scenarios and reports serialize byte-identically). Refused: `values([])`
(Drizzle throws), spreads or holes, more than 8 rows, a multi-row
`returning({ alias: col })` with renamed aliases, and multi-row statements
in flight (a lost acknowledgment of a multi-row autocommit statement).
`values(obj)` with an object that is not a literal or `const` is unknown
(which keys a caller passes is not known; Drizzle turns a missing key into
`DEFAULT`).

## D2. Timestamps (feature `timestamps`)

`timestamp`, `timestamptz` and `date` columns in their default Drizzle modes.
Clock assumptions, stated with every scenario that uses them:

* `now()` (`defaultNow()`, ``sql`now()` ``) is the **transaction's start
  time**: one value for every statement of a transaction (PostgreSQL's
  `now()` is stable within a transaction).
* `new Date()` written in a statement's arguments, `$defaultFn(() => new
  Date())`, and `$onUpdate(() => new Date())` are one **application clock
  reading** per occurrence and execution.
* `$onUpdate` columns are added to `UPDATE … SET`, to an upsert's
  `DO UPDATE SET`, and to inserts that omit the column (the probe confirms
  each).
* Clock readings are **opaque**: equality is identity
  (`clock-readings-distinct`: distinct readings differ from each other and
  from explicit values); an order comparison is unsupported, never guessed.
* Explicit values must be canonical text (`YYYY-MM-DDTHH:MM:SS.sssZ`, what a
  Drizzle `Date` serializes to; `YYYY-MM-DD` for `date`) and compare by
  value. Anything else, integers included, is unsupported.

Not modeled: `mode: "string"` timestamps, `date` with `mode: "date"`, a
clock into a `date` column, `.default(sql\`now()\`)` (only `defaultNow()`
is recognized), and a `new Date()` held in a local and used by several
statements (one reading shared by statements needs an engine operation that
does not exist yet; such code stays unknown).

## D3. JSON (feature `json`)

`json`/`jsonb` columns hold objects, arrays, integers, strings and
booleans, nested at most 4 deep: writes, read-back and `isNull` work. Every
comparison of a JSON column is unsupported, and property access (`->>`)
stays refused. This is not PostgreSQL's JSONB semantics.

## D4. Predicates

`and()`, `or()`, `not()`, `isNull()`, `isNotNull()`, `inArray(col, list)`,
`notInArray` (as `NOT IN`), with SQL three-valued logic exactly: an empty
`inArray` is `false` and an empty `notInArray` is `true` (read from
drizzle-orm 0.45.4 `sql/expressions/conditions.js`); `and`/`or` drop
`undefined` operands as Drizzle does. A conditional predicate
`test ? predA : predB` becomes `(which AND A) OR (NOT which AND B)` with
`which = !!test` computed in JavaScript, and each branch's operands
computed only under that branch. Refused: nested conditional predicates,
`inArray` over a subquery or a computed list, comparisons with object or
array literals.

## D5. Differential evidence

**Lockstep oracle** (every schedule of the model replayed against
PostgreSQL 17.11, comparing blocking, results, SQLSTATEs and final rows; 3
isolation levels):

| family | workload pairs × levels | schedules | disagreements |
| --- | --- | ---: | ---: |
| multi-row inserts (unique, mid-statement waits, deadlocks between multi-row transactions, DO NOTHING + RETURNING, per-row FK, sequences, multi-row upsert, 21000) | 16 × 3 | 420 | 0 |
| predicates (IN with NULLs, NOT/OR/IS NULL with NULLs, conditional, empty lists) | 7 × 3 | 159 | 0, plus 1 known one-way SSI difference |
| timestamps and JSON (`now()` stability, `new Date()`, `$onUpdate`, explicit values, `jsonb` read-back) | 5 × 3 | 90 | 0 |

The one extra case, `detach×prune-top-tx` at SERIALIZABLE: PostgreSQL alone
raised 40001, because its unindexed sequential scan takes a relation-level
SIREAD lock. That is the documented one-way granularity difference
(Phase 6, pinned by `ssi_granularity_differences_only_go_one_way`); the
test counts exactly that case and fails on any other.

**Drizzle SQL probe** (`fixtures/phase14/drizzle-app/probe/sql-probe.mjs`,
`drizzle.mock().toSQL()`, no database; output
`reports/phase14/drizzle-sql/probe.json`): multi-row inserts send `default`
for omitted columns; `values([])` throws; empty `inArray` is `false`, empty
`notInArray` `true`; `not` is emitted without parentheses; `undefined`
operands are dropped; `$onUpdate` reaches `SET`, upsert `SET` and inserts;
an upsert `set` uses one clock parameter for all rows; `jsonb` values are
sent as `JSON.stringify` text.

**Predicate agreement** (`reports/phase14/drizzle-sql/oracle-comparison.txt`):
for 11 predicate forms, Drizzle's SQL on PostgreSQL, the oracle's rendering
of the extracted predicate on PostgreSQL, and the model select identical
rows from a NULL-rich table. No mismatch; the predicates are kept as
regression data (`probe/extracted-predicates.json`).

## D6/D7. Reductions and versioning

86 models (all workloads × 3 levels, plus two three-actor multi-row
models): the independence audit is clean and sleep sets, classic DPOR and
Source-DPOR (stateful+sleep, stateless+sleep) match the unreduced search on
all 344 runs (three timestamp models are unsupported by design: a stale
read ordering a clock reading). 17 Phase 6–13 scenarios give identical
report JSON and canonical state bytes against the pre-change build
(`earlier_scenarios_keep_their_reports_and_state_bytes`, golden in
`analyzer/tests/phase14/versioning-golden.json`).

## End to end

From source (`fixtures/phase14/drizzle-app`): `welcome-race` finds a
counterexample; `welcome-once`, `tags-race` and `place-order` hold.
On original code (`fixtures/phase14/request-app`, `EVALUATION.md`): the
two-row INSERT whose response is lost and resubmitted duplicates order
lines, reproduced 3/3 on PostgreSQL 17.11 (`original-code-crash-reproduced`);
the keyed variant holds.

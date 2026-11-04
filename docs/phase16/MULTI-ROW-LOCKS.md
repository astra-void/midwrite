# Phase 16: multi-row SELECT and row locks taken one row at a time

Track C. Before Phase 16 a `SELECT` was always `ORDER BY <key> LIMIT 1`: row
locking needed `.limit(1)` ("a multi-row lock is not modeled", Phase 15 set
E10) and a result bound to a variable needed `.limit(1)`. This note covers
the multi-row mode: what Drizzle sends, what PostgreSQL does, what the
model does, how it was checked, and what remains approximate.

## 1. What Drizzle sends (drizzle-orm 0.45.4)

`toSQL()` of the pinned version (`fixtures/replay/drizzle-node`):

| source | SQL | params |
| --- | --- | --- |
| `select().from(accounts).where(inArray(accounts.id, [2, 1])).for("update")` | `select "id", "balance" from "accounts" where "accounts"."id" in ($1, $2) for update` | `[2, 1]` |
| `... .limit(2).for("update", { noWait: true })` | `... in ($1, $2) limit $3 for update nowait` | `[2, 1, 2]` |
| `... .for("update", { skipLocked: true })` | `... for update skip locked` | |
| `... .for("share")` | `... for share` | |
| `select({ id }).from(accounts).where(eq(accounts.id, 2)).for("no key update")` | `select "id" from "accounts" where "accounts"."id" = $1 for no key update` | `[2]` |
| `inArray(accounts.id, [])` | `... where false for update` | `[]` |

There is no `ORDER BY` unless the source writes `.orderBy(...)`; then it
precedes `limit` and the lock clause.

## 2. What PostgreSQL does (17.11, measured)

* **Order.** A locking `SELECT` is `LockRows` above the scan (below `Limit`).
  Rows are locked one at a time in the order the plan produces them; a
  statement waiting on its k-th row holds the first k-1 locks.
  * Without `ORDER BY` the order is the plan's. On the 3-row test table
    (no statistics, physical order 3, 1, 2) `id IN (2, 3, 1) FOR UPDATE` is
    `LockRows -> Bitmap Heap Scan` and returns/locks **3, 1, 2** (physical
    order); `v = 0 FOR UPDATE` is a sequential scan, also 3, 1, 2.
  * On a 10 000-row table inserted in descending key order and analyzed, the
    same statement is `LockRows -> Index Scan using big_pkey` and locks
    **1, 2, 3** (key order).
  * With `ORDER BY id` (Drizzle `.orderBy(t.id)`), `LockRows` runs above the
    sort or the ordered index scan: key order whatever the physical order.
  (`postgresql_locks_in_the_plan_order_without_order_by_and_in_key_order_with_it`.)
* **Waits and deadlocks**: a conflicting lock makes the statement wait while
  holding its prefix; PostgreSQL's detector aborts a wait-for cycle with
  40P01.
* **NOWAIT**: 55P03 at the first conflicting row. The transaction is
  aborted at that error: the rows the statement had locked are free at
  once, although the session stays "idle in transaction (aborted)" until
  the client sends ROLLBACK (a third session locked them with `NOWAIT`
  while the failed session had not rolled back;
  `postgresql_releases_the_locks_of_a_failed_nowait_statement_at_the_error`).
  The task statement expected the prefix to stay locked until ROLLBACK; the
  measurement says otherwise, and the model follows the measurement.
* **SKIP LOCKED** skips rows another transaction locks, never waits.
* **READ COMMITTED**: after a wait (or for a row whose newest committed
  version is newer than the statement snapshot) the predicate is re-checked
  on the newest version (EvalPlanQual): a row that no longer matches is
  skipped, a deleted row is skipped; `LIMIT n` counts only rows returned.
* **REPEATABLE READ / SERIALIZABLE**: a row changed (or deleted) after the
  snapshot makes the locking statement fail with 40001.

## 3. The model

* IR: `StmtKind::Select` has a defaulted field `many: Option<ManyRows>`
  (`#[serde(default, skip_serializing_if = "Option::is_none")]`;
  `ManyRows { limit: Option<u32>, order_by: Option<String> }`, both skipped
  when absent). Absent = the Phase 6-15 `LIMIT 1` statement, unchanged in
  memory, in serialization and in the engine (the closed form is a new
  variant `ClosedStmt::SelectMany`, so the canonical state encoding of
  single-row selects is untouched). The scenario DSL writes the mode
  `let rows = select all t where ... [order by @id] [limit n] [for ...]`
  (and `inArray(@col, [..])`).
* Result: the list of returned rows (`StmtResult::Rows`, a JavaScript array
  of row objects; projections apply per row). Bounded by the rows of the
  model's finite world (and `limit`).
* Order: primary-key order, always. The loader accepts `order_by` only for
  the single-column primary key; any other order is refused by the
  extractor.
* Locks (`exec.rs`, the `run_rows` loop shared with `UPDATE`/`DELETE`): one
  row at a time in scan order. Each row: conflict check against other
  holders (`RowLock` compatibility, Phase 13); `SKIP LOCKED` skips it,
  `NOWAIT` fails with 55P03, otherwise the statement returns `Blocked`
  **keeping the locks it already granted** and resumes later with the same
  snapshot and the remaining candidates. Then the version check: READ
  COMMITTED re-evaluates the predicate on the newest version (skip if it no
  longer matches or was deleted), REPEATABLE READ / SERIALIZABLE fail with
  40001; a row the transaction wrote itself is used as is. The lock is
  granted, the row appended to the result, and the statement stops when
  `limit` rows were returned. Deadlocks: the existing wait-for graph and
  victim choice (40P01).
* SERIALIZABLE read set: the predicate, or, when `limit` stopped the scan,
  the rows up to the last returned one in key order (as `LIMIT 1`).
* Footprints (POR): unchanged access kinds, recorded per row: `Pred` for the
  scan, `LockCheck(row)` for every row examined, `LockChange(row)` for every
  lock granted, `RowRead(row)` for a re-read newest version, `Wait` and
  `Snapshot` when blocking. A step that takes a prefix and blocks therefore
  conflicts with every transition that tests or changes a lock of the same
  rows (locking reads, updates, deletes, foreign-key `KEY SHARE` checks) and
  with writes its predicate sees.
* Statement atomicity (`docs/phase6/LIMITATIONS.md`): a statement runs
  atomically until it waits. Interleavings between two of its row visits
  that do not involve a wait are not explored (as for multi-row `UPDATE`).
  The lockstep oracle can only check this granularity: PostgreSQL cannot be
  paused between two rows of one statement.

## 4. Extraction (`crates/commitrift-analyzer/src/concurrency.rs`, `select`)

The multi-row mode needs JavaScript values (`ExtractRequest::js_values`,
scenario feature `js-values`: the result is an array). With `js_values`:

* `.for(..)` without `.limit(1)` → multi-row lock;
  `const [row] = await ...for("update")` reads its first element (`undefined`
  when empty, `Builtin::At`); `const rows = await ...` binds the array
  (`rows.length`, `rows[i]`, `for (const r of rows)` are the Phase 14 list
  forms); an awaited select whose result is not used is still executed (it
  takes its locks).
* `.limit(n)` with a literal `n ≥ 0`; `.orderBy(t.col)` / `.orderBy(asc(t.col))`
  on the table's own column (the loader then requires the primary key);
  `desc`, several keys, computed orders are refused.
* Joins, counts and replicas keep their Phase 15 rules (a locking read on a
  join or a replica is refused).
* Without `js_values` the Phase 15 refusals are unchanged (byte-identical
  extraction).
* Assumption text: with `.orderBy(key)` "rows ... locked one row at a time,
  in key order (PostgreSQL locks above the sort)"; without it "modeled in
  primary-key order ... PostgreSQL uses its plan's order: key order for an
  index scan, physical order for a bitmap or sequential scan ..., so
  lock-order outcomes (waits, deadlocks) of such a statement are
  approximate".

## 5. Oracle and replay SQL

`oracle::stmt_sql` sends a multi-row select as
`SELECT row_to_json(t) FROM (SELECT * FROM "t" WHERE <pred> ORDER BY <pk>
[LIMIT n] <lock>) t;` (one JSON row per result row; with a projection
`SELECT json_build_object(..) FROM ... ORDER BY <pk> [LIMIT n] <lock>`): the
model's order stated. `SelectForm::Drizzle` (`PgEnv::set_select_form`)
sends what Drizzle sends instead: no `ORDER BY` unless the source had
`.orderBy`. Original-code verification compares each returned row's
cells in order (`source_verify::expected_cells`).

## 6. Evidence

All on PostgreSQL 17.11 (Homebrew), private disposable clusters
(`autovacuum=off`), `deadlock_timeout` 500 ms.

```
COMMITRIFT_REQUIRE_REPLAY=1 cargo test -p commitrift-model --test phase16_locks -- --nocapture
```

* **Lockstep differential matrix**
  (`the_model_agrees_with_postgresql_on_multi_row_locks`): 12 two-transaction
  workloads × READ COMMITTED / REPEATABLE READ / SERIALIZABLE, *every*
  schedule the model enumerates replayed in lockstep: overlapping row sets,
  reversed lock order (multi-row lock against two single-row locks: 40P01),
  a missing row (with a concurrent insert of it), concurrent UPDATE that
  makes a row stop matching / keeps it matching, concurrent DELETE, NOWAIT,
  SKIP LOCKED (then an UPDATE that waits), FOR SHARE then UPDATE by both
  (deadlock), FOR SHARE vs FOR UPDATE, `LIMIT 2` after a wait, zero rows.
  Three passes: (a) the oracle's form (with `ORDER BY`), all 36
  configurations; (b) Drizzle's statements without `ORDER BY` for the 9
  primary-key workloads, rows inserted in key order; (c) Drizzle's
  statements with `.orderBy(t.id)` for the same 9 workloads, rows inserted
  out of key order (physical order 3, 1, 2).
  **90 configurations, 2 124 schedules (792 + 666 + 666), 2 124 agreeing,
  0 disagreements**; 7 878 statements compared, 1 296 lock waits observed,
  SQLSTATEs 40001 × 420, 40P01 × 270, 55P03 × 63, 0 deadlock-victim retries.
  Exact SQL of every multi-row statement is printed by the test.
* **The approximation, measured**
  (`without_order_by_postgresql_locks_in_physical_order_which_the_model_does_not_follow`):
  Drizzle's statements without `ORDER BY` over rows in physical order 3, 1, 2
  (READ COMMITTED, workloads reversed-order and overlapping-sets): **24 of
  51 schedules disagree** (rows returned in physical order, a different row
  waited on). The test asserts that disagreements exist.
* **PostgreSQL facts** (§2): plans and lock order; NOWAIT releasing at the
  error.
* **Reduction** (`cargo test -p commitrift-model --test phase16_locks reductions`):
  unreduced exploration against sleep sets, classic DPOR and source-set DPOR
  (stateful+sleep, stateless+sleep, stateful), terminal states, verdict,
  counterexample existence and replay, stuck states, unsupported behavior:
  the 36 differential models (180 comparisons, 23 models with a
  counterexample, 0 discrepancies) and 120 generated multi-row lock
  scenarios (2-3 transactions over 4 rows and a child table with a foreign
  key; multi-row locks of every strength/option with random key sets and
  limits, single-row locks, key and non-key updates, deletes, child inserts;
  random isolation): 120 models, 73 with a counterexample, 599 comparisons,
  0 discrepancies. Sensitivity: with every `LockCheck`/`LockChange` access
  deleted from the footprints, sleep sets and source-set DPOR lose terminal
  states or counterexamples on 4 of the 12 workloads (8 workload ×
  algorithm pairs).
* **Discovery** (`cargo test -p commitrift-discover --test phase16_locks`,
  `fixtures/phase16/locks`): two single-row locks in argument order →
  MODEL_COUNTEREXAMPLE, a 40P01 deadlock schedule (against the synthesized
  `no-request-fails` hypothesis: a failure, not a broken balance);
  `.orderBy(accounts.id)` multi-row lock with crossed arguments →
  NO_VIOLATION_WITHIN_BOUNDS; the same without `.orderBy` →
  NO_VIOLATION_WITHIN_BOUNDS **in the model's key order** (see §7).
  Phase 15 set: E10 (`wallets.ts#transfer`) is now extracted and reported as
  MODEL_COUNTEREXAMPLE with a 40P01 schedule; E11 (`transferOrdered`) stays
  NO_VIOLATION_WITHIN_BOUNDS.

Discovery needed two small changes in `crates/commitrift-discover/src/synth.rs`
(not a Track C file): a multi-row lock that locked nothing does not count as
reaching a lock-order target, and an empty multi-row read seeds rows like an
empty single-row read; with swapped keys a locking read's key is varied too
(before, only writes were, so E10-style candidates ran with equal keys).

## 7. Limitations

* **Lock order without `ORDER BY` is approximate.** The model always locks
  in key order. PostgreSQL locks in physical order under a bitmap or
  sequential scan (small or unanalyzed tables, non-key predicates). Two
  transactions whose scans see different physical positions of a row (one
  sees the version before a committed update, the other after it) can lock
  in different orders and deadlock where the model reports none; with
  equal positions they lock in the same order as the model. A
  NO_VIOLATION for a multi-row lock without `.orderBy(key)` is therefore
  conditional on key-order locking (stated in the assumptions).
* Statement atomicity between non-waiting row visits (§3).
* `orderBy` other than the single-column primary key ascending, joins with
  the multi-row mode, `limit` that is not a literal, multi-row results
  without `js_values`: refused (`Op::Unknown` with the construct named).
* `FOR SHARE` / `FOR KEY SHARE` / `FOR NO KEY UPDATE` with `skipLocked` or
  `noWait`: refused (Drizzle accepts them; not modeled).
* Plans with `LockRows` below a sort or aggregate are not produced by the
  supported forms and are not modeled.

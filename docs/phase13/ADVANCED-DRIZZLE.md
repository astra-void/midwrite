# Advanced Drizzle constructs (Track E)

Consolidates the plan's `ADVANCED-DRIZZLE` and `RETURNING-AND-UPSERT`.
Track E was prioritized by what the corpus and Tracks A–D needed; what is
not listed here is unchanged from Phase 12.

## 1. `INSERT … ON CONFLICT (target) DO UPDATE` — implemented

Drizzle (pinned 0.45.4, `pg-core/query-builders/insert.js`):
`onConflictDoUpdate({ target, set, setWhere })` sends `ON CONFLICT (target)
DO UPDATE SET … WHERE …`; `where` is the deprecated name of `setWhere`;
`targetWhere` (a partial-index arbiter) is not modeled and makes the
statement unknown.

Model (`exec.rs::upsert_row`), as PostgreSQL's `ExecOnConflictUpdate`:

* the arbiter is checked first: a row with the same target key — committed,
  or the transaction's own — is the conflicting row; an uncommitted insert
  or update of that key by another transaction makes the statement **wait**,
  then the whole statement runs again;
* under REPEATABLE READ / SERIALIZABLE a conflicting row the snapshot does
  not see as it is now is **40001** (also when it was only updated);
* the row is locked (`FOR UPDATE` when `SET` changes a key column, else `FOR
  NO KEY UPDATE`) **before** `WHERE` is evaluated, so a rejected update
  still holds the lock;
* `SET` and `WHERE` read the existing row; `excluded.c` (Drizzle:
  ``sql`excluded.c` ``) is the proposed row; ``sql`${t.c} + 1` ``-style
  arithmetic as before;
* a conflict on another unique key is still 23505; foreign-key checks fire
  as for an `UPDATE`; `RETURNING` returns the updated row (nothing when
  `WHERE` rejected it).

Evidence:

| | result |
| --- | --- |
| lockstep matrix (`pg_matrix::upsert_workloads`): 6 workload pairs (counter increments, `excluded`, conditional `WHERE`, transactions with an earlier snapshot) × with/without an existing row × 3 levels | **396 schedules, 0 disagreements** (waits, results, `RETURNING` rows, 40001, final rows) |
| ground truth replay (`LOCK-DIFFERENTIAL.md` §7a) | the 12 `uncommitted-parent` cells with `ON CONFLICT DO NOTHING / DO UPDATE SET name = excluded.name` (wait for the uncommitted same-key insert; 40001 at RR/SER) agree; the 7 `upsert` cells use `name \|\| '+a'` (string concatenation, not in the value layer) and are not replayed |
| original code (`request-app`, `reserve-race`: two `reserveHandle` take a handle over with `RETURNING`, both told they reserved it) | model counterexample, **reproduced 3/3** |
| safe counterparts | `reserve-if-free` (`setWhere: isNull(owner)`) and `count-views` (`hits + 1`) hold |
| reductions | upsert models agree with the unreduced search under every reduction; audit clean |

## 2. Row-lock strengths — implemented

`.for("update" | "no key update" | "share" | "key share")` now lowers to the
four strengths (`POSTGRESQL-FK-LOCKS.md` §1); `skipLocked`/`noWait` remain
supported with `update` only. Discovery counts only `FOR UPDATE` and `FOR NO
KEY UPDATE` as a protection of check-then-act, read-modify-write and status
transitions (two `FOR SHARE` readers do not exclude each other: ground truth
"guard" cells). Found through the frozen evaluation set: `createTeamShared`
(`FOR SHARE`) was an unknown operation (post-feedback change, scored as
such).

## 3. `UPDATE`/`DELETE … RETURNING` — partly

Row counts and presence through `returning()` are modeled (Phase 7–12):
`rows.length`, `rows.length > 0`, `rows[0]` truthiness. **Row values** of
`UPDATE/DELETE … RETURNING` (`const [row] = …; row.col`) are still unknown:
PostgreSQL's `RETURNING` order of a multi-row update is not the key order,
and a single-row form was not needed by any measured path. A conditional
`UPDATE … WHERE … RETURNING` under concurrency is exact for its count: the
model re-evaluates the predicate on the newest version (EvalPlanQual), so
the earlier `SELECT` is never assumed to predict it (`claim-atomic`,
`settleOrderOnce`).

## 4. Not done in Phase 13

* Multi-row `values([...])` (the evaluation set's `placeOrder` uses one):
  still an unknown operation. Atomicity, per-row defaults and per-row
  conflicts would need a pending statement with a row cursor; not started.
* Savepoints / nested transaction helpers, transaction-callback return
  values beyond what the extractor already inlines: unchanged.
* Application retry wrappers given as higher-order functions
  (`withRetry(() => …)` in the evaluation set): unknown (see
  `SOURCE-RESOLUTION.md`).

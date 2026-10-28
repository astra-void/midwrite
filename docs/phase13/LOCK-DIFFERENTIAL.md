# Phase 13 lock differential: PostgreSQL row locks and foreign keys

> **Final (Phase 13 closeout, 2026-10-28).** §1–§6 are the PostgreSQL
> **ground truth**, produced without reading the Phase 13 implementation;
> §7 is the comparison with the CommitRift model (707 cells, 2 726 labeled
> steps, 0 disagreements; 543 lockstep schedules, 0 disagreements). This
> header said "DRAFT" while §7 was still to be written.

## 1. Method

* `scripts/phase13-lock-matrix.py` (Python 3 standard library only) starts a
  **disposable** PostgreSQL cluster with the server binaries of
  `/opt/homebrew/opt/postgresql@17/bin` (`initdb`, `pg_ctl`, `postgres`, and
  `psql` for bootstrap). The cluster lives under a short `/tmp/plm.*`
  directory, has `listen_addresses = ''` (unix socket only) and
  `deadlock_timeout = 100ms` (detection latency only). Durability is off. The
  script stops and deletes the cluster at exit, and it connects to no other
  server.
* Server: **PostgreSQL 17.11 (Homebrew)**, `server_version_num` 170011,
  aarch64-apple-darwin.
* Sessions A and B are separate connections speaking the wire protocol, so
  every error carries its SQLSTATE, message, detail and server routine. A
  third, observer connection decides whether a statement is **blocked**.
  After each statement is sent, the script polls until either its response
  arrives or the observer sees the backend with an ungranted lock in
  `pg_locks` (locktype, mode, `pg_blocking_pids`). No sleep is used as
  synchronization. A statement that does neither within 10 s (or 15 s while
  draining a deadlock) would be recorded as INCONCLUSIVE.
* Step semantics: steps run in the listed order. When a step's session is
  blocked, the step runs as soon as that session's statement returns, as a
  client would. A step can run conditionally on an earlier result (used for
  check-then-insert).
* Each cell starts from fresh tables and seed, both recorded in the cell. The
  observer also records the row-lock modes held after each step, with
  `pgrowlocks`, and each cell records the final rows.
* Schema `base`: `users(id int primary key, name text)` and
  `workspaces(id int primary key, owner_id int references users(id), name text)`,
  seeded with users 1 and 2 and workspace 10 owned by user 2. Unless noted,
  every action targets user 1, which has no child row. The variants are:
  * `email`: adds `users.email text UNIQUE`.
  * `cascade`: `ON DELETE CASCADE`, and user 1 owns workspace 11.
  * `deferred`: `DEFERRABLE INITIALLY DEFERRED`.
  * `nulls`: a `folders` table with `UNIQUE(owner_id, parent_id, name)`.
* The definition of every cell, enough for the model side to replay it:
  `fixtures/phase13/lock-matrix/cells.json`. Results, one object per cell
  (SQL per session, blocked or not and on which lock, results with SQLSTATE,
  transaction outcomes, final rows, `server_version`):
  `reports/phase13/lock-matrix/results.json`. Full per-cell tables:
  `reports/phase13/lock-matrix/summary.md`.

**837 cells, 0 INCONCLUSIVE.** A second run, replayed from `cells.json`,
gave identical blocking, results, transaction outcomes and final rows in all
837 cells. Every observed wait was a `ShareLock` on the holder's
`transactionid`, and the blocker was always the expected session.

Rerun (about 15 s):

```sh
python3 scripts/phase13-lock-matrix.py                    # regenerate cells.json, run, write results + summary
python3 scripts/phase13-lock-matrix.py --from-cells fixtures/phase13/lock-matrix/cells.json
python3 scripts/phase13-lock-matrix.py --only main/base/rc/ --only fk-cycle/   # subsets by id prefix
```

## 2. Lock modes taken (pgrowlocks, users row 1)

| statement | row-lock mode |
| --- | --- |
| `SELECT … FOR UPDATE` / `FOR NO KEY UPDATE` / `FOR SHARE` / `FOR KEY SHARE` | For Update / For No Key Update / For Share / For Key Share |
| `UPDATE users SET name = …` (non-key) | No Key Update |
| `UPDATE users SET id = 3` (referenced key) | Update |
| `DELETE FROM users …` | Update |
| `UPDATE users SET email = 'new'` (`email` UNIQUE, referenced by no FK) | **Update** |
| `UPDATE users SET email = <same value>, name = …` | **No Key Update** |
| child `INSERT INTO workspaces (…, owner_id = 1)` (FK check) | For Key Share |

## 3. Main matrix: A holds a lock on the parent, then B acts

Schedule: A: `BEGIN` → A: lock → B: `BEGIN` → **B: action** (does it wait?)
→ A: `COMMIT` or `ROLLBACK` → B: `COMMIT`. A and B run at the same isolation
level. 8 A-locks × 7 B-actions × 2 endings × 3 isolation levels (base), plus
3 A-locks on the `email` variant: 462 cells.

**Blocking** (`W` = B waited on A's transaction; `-` = no wait). It is
identical at READ COMMITTED, REPEATABLE READ and SERIALIZABLE:

| A holds \ B does | INSERT child | UPDATE child FK → 1 | FOR KEY SHARE | FOR SHARE | FOR UPDATE | UPDATE name | DELETE parent |
| --- | --- | --- | --- | --- | --- | --- | --- |
| nothing (plain SELECT) | - | - | - | - | - | - | - |
| FOR UPDATE | W | W | W | W | W | W | W |
| FOR NO KEY UPDATE | - | - | - | W | W | W | W |
| FOR SHARE | - | - | - | - | W | W | W |
| FOR KEY SHARE | - | - | - | - | W | - | W |
| UPDATE name (non-key) | - | - | - | W | W | W | W |
| UPDATE id (key) | W | W | W | W | W | W | W |
| DELETE | W | W | W | W | W | W | W |
| UPDATE email (unique, unreferenced) | W | W | W | W | W | W | W |
| UPDATE email := same value, name | - | - | - | W | W | W | W |

**B's result.** When B did not wait, its statement succeeds at every
isolation level. When B waited and A then **rolled back**, B's statement
succeeds at every isolation level. When B waited and A **committed**, B's
result is as follows (SERIALIZABLE is identical to REPEATABLE READ in all 154
cell pairs):

| A committed | B's waiting action | READ COMMITTED | REPEATABLE READ / SERIALIZABLE |
| --- | --- | --- | --- |
| a lock only (FOR UPDATE / NO KEY UPDATE / SHARE / KEY SHARE) | any | ok | ok |
| UPDATE name | FOR SHARE, FOR UPDATE, UPDATE, DELETE | ok (on the new version) | **40001** |
| UPDATE id, or DELETE | INSERT child, UPDATE child FK | **23503** | **40001** |
| UPDATE id, or DELETE | FOR KEY SHARE, FOR SHARE, FOR UPDATE, UPDATE, DELETE | ok, **0 rows** | **40001** |
| UPDATE email (unique) | all seven, including INSERT child | ok | **40001** |
| UPDATE email := same value | FOR SHARE, FOR UPDATE, UPDATE, DELETE | ok | **40001** |

## 4. Special cases

**4.1 Child of an uncommitted parent.** A: `BEGIN; INSERT users(3)`; then B
acts; then A commits or rolls back.

| B does | wait? | result (RC / RR / SER) |
| --- | --- | --- |
| `INSERT` child with `owner_id = 3` | **no** | **23503 immediately**, whether A later commits or rolls back |
| `SELECT … WHERE id = 3 FOR KEY SHARE` | no | 0 rows |
| `INSERT users(3)` (same key) | W | A commits: 23505 / A rolls back: ok |
| `… ON CONFLICT (id) DO NOTHING RETURNING` | W | A commits: RC ok with **0 rows**, RR/SER **40001** / A rolls back: 1 row |
| `… ON CONFLICT (id) DO UPDATE … RETURNING` | W | A commits: RC ok (updates A's row, 1 row returned), RR/SER **40001** / A rolls back: inserts, 1 row |

**4.2 A acts on the parent while B holds an uncommitted child insert**
(B already holds FOR KEY SHARE on user 1). Then B commits or rolls back, and
A continues.

| A does | A waits? | B commits → A | B rolls back → A |
| --- | --- | --- | --- |
| FOR UPDATE | W | ok | ok |
| FOR NO KEY UPDATE, FOR SHARE, FOR KEY SHARE, UPDATE name | - | ok | ok |
| UPDATE id | W | **23503** (RC, RR, SER) | ok |
| DELETE (NO ACTION) | W | **23503** (RC, RR, SER), not 40001 | ok |
| DELETE (ON DELETE CASCADE) | W | RC: ok, the cascade also deletes B's just-committed child; RR/SER: **40001** | ok |
| UPDATE email (unique) | W | ok | ok |
| UPDATE email := same value | - | ok | ok |

**4.3 A change committed after B's snapshot** (the REPEATABLE READ
question). B: `BEGIN; SELECT count(*) FROM users` takes the snapshot; then A
commits the change in autocommit; then B acts. B never waits.

| A committed | INSERT child / UPDATE child FK | FOR KEY SHARE | FOR SHARE / FOR UPDATE / UPDATE / DELETE |
| --- | --- | --- | --- |
| UPDATE name | **ok at RC, RR and SER** | ok at RC, RR and SER | RC ok; RR/SER 40001 |
| UPDATE email := same value | ok at RC, RR and SER | ok | RC ok; RR/SER 40001 |
| UPDATE email (unique) | RC ok; RR/SER **40001** | RC ok; RR/SER 40001 | RC ok; RR/SER 40001 |
| UPDATE id / DELETE | RC 23503; RR/SER **40001** | RC 0 rows; RR/SER 40001 | RC 0 rows; RR/SER 40001 |
| SELECT … FOR UPDATE / FOR KEY SHARE (a lock only) | ok | ok | ok |

**4.4 `DEFERRABLE INITIALLY DEFERRED`.**

* A holds a lock, then B: `INSERT` child, then `COMMIT`. B's **INSERT never
  waits**. The FK check and its FOR KEY SHARE run at **B's COMMIT**, which
  waits only behind FOR UPDATE, UPDATE id and DELETE, at every level. After A
  commits a key UPDATE or a DELETE, B's COMMIT fails with 23503 (RC) or
  40001 (RR, SER). After a rollback of A, or after any of the other A-locks,
  B's COMMIT succeeds.
* B first inserts the child (uncommitted), then A deletes the parent. A's
  DELETE **does not wait**, and A's COMMIT succeeds, because the uncommitted
  child is invisible to A's deferred check. B's COMMIT then fails with 23503
  (RC) or 40001 (RR, SER). If A rolls back instead, B commits.

**4.5 `ON DELETE CASCADE`.** A: `BEGIN; DELETE users 1`, which cascades to
workspace 11. Every B action on user 1 waits, and so does every B action on
workspace 11: UPDATE, FOR KEY SHARE, DELETE. After A commits, RC gives 23503
for INSERT child and UPDATE child FK, and the other actions affect 0 rows;
RR and SER give 40001 for every action. After A rolls back, every action
succeeds.

**4.6 Foreign-key lock cycles** (ground truth for eval E05/E06). Both
transactions insert a child of user 1, then:

| then each does | RC | RR | SER |
| --- | --- | --- | --- |
| `SELECT user FOR UPDATE` | **deadlock: one gets 40P01**, the other commits | same | same |
| `SELECT user FOR NO KEY UPDATE` | the second waits for the first; both commit | same | same |
| `UPDATE users SET name = …` (non-key) | the second waits; both commit | the second gets 40001 | the second gets 40001 |
| `SELECT user FOR SHARE` | no wait; both commit | same | same |
| lock FOR UPDATE **first**, then insert | the second waits at the lock; both commit | same | same |

**4.7 Check-then-insert of a child under a parent lock** (eval E01–E04).
Each transaction locks user 1, counts its workspaces, and inserts one if the
count is 0. Workspaces of user 1 at the end:

| lock | RC | RR | SER |
| --- | --- | --- | --- |
| none, FOR KEY SHARE, FOR SHARE | **2** | 2 | 1 (B's INSERT: 40001) |
| FOR UPDATE, FOR NO KEY UPDATE | **1** (B waits, then sees A's row) | **2**: B's snapshot is taken by its blocked lock statement, before A commits | 1 (B's INSERT: 40001) |
| mixed: B checks with no lock (autocommit), A runs the FOR UPDATE version | **2**: B's INSERT **waits** behind A's FOR UPDATE (FK check), then commits | – | – |

The mixed row reproduces the Phase 12 `createWorkspace` ×
`createWorkspaceLocked` divergence (Phase 12 POSTGRESQL-DIFFERENTIAL §5). The
unlocked INSERT waits on a lock the Phase 12 model lacks, and the invariant
is still violated.

**4.8 Unique keys and upserts.** Two transactions that insert
`UNIQUE(owner_id, parent_id, name)` rows with `parent_id NULL` both succeed
without waiting (NULLS DISTINCT). With a non-null parent the second one
waits, then gets 23505, or succeeds if the first rolls back. Two
autocommitted `INSERT … ON CONFLICT DO UPDATE … RETURNING` of the same
existing key **both return a row**. Two concurrent ones: the second waits,
then at RC applies its increment on top of the first; at RR and SER it gets
40001 if the first committed.

## 5. Where the observations depart from the common description

The common description is the manual's row-lock conflict table, with
FOR KEY SHARE conflicting only with FOR UPDATE, a non-key UPDATE taking
FOR NO KEY UPDATE, and a key UPDATE or a DELETE taking FOR UPDATE. Read that
way, it predicts **blocking** correctly in **all 462 main cells (0
disagreements)**, and pgrowlocks shows exactly those modes. The surprises are
in what "key" means, in when the lock is taken, and in the results after a
wait:

1. **"Key" means any column of a usable unique index, not the referenced
   column.** Changing `users.email` (UNIQUE, referenced by no foreign key)
   takes FOR UPDATE. Child INSERTs referencing `users.id` then wait (RC) and
   fail with 40001 (RR, SER), although `id` never changed.
2. **Key-ness is decided by whether the value changes, not by the SET
   list.** `SET email = <same value>` takes FOR NO KEY UPDATE. An ORM that
   writes every column blocks FK inserts only when a key value actually
   changes.
3. **REPEATABLE READ distinguishes key from non-key updates.** After a
   concurrent, committed non-key UPDATE of the parent, a child INSERT, an FK
   UPDATE or an explicit FOR KEY SHARE still **succeeds**. FOR SHARE,
   FOR UPDATE, UPDATE and DELETE of the same row fail with 40001. After a
   committed key UPDATE or DELETE, the child INSERT fails with **40001 at
   RR/SER but 23503 at RC**. A committed lock-only (`SELECT … FOR UPDATE`)
   never causes 40001.
4. **An uncommitted parent is invisible, not locked.** A child INSERT that
   references a parent inserted by an open transaction fails with **23503
   immediately, without waiting**, even if that transaction later commits. A
   same-key INSERT does wait.
5. **ON CONFLICT is not error-free at REPEATABLE READ.** `DO NOTHING` and
   `DO UPDATE` wait for a concurrent inserter, and when it commits they fail
   with **40001** instead of doing nothing or updating.
6. **A parent DELETE or key UPDATE that waits for an uncommitted child
   insert, which then commits, fails with 23503 at every level (not
   40001).** With ON DELETE CASCADE, the same DELETE succeeds at RC (and
   removes the new child) but fails with 40001 at RR/SER.
7. **Deferred foreign keys move the lock to COMMIT.** The child INSERT never
   waits. COMMIT waits, and it is COMMIT that fails. A parent DELETE does not
   wait for an uncommitted deferred child, and wins.
8. **Insert-child-then-lock-parent deadlocks only with FOR UPDATE.** With
   FOR NO KEY UPDATE or a non-key UPDATE there is no cycle, because
   FOR KEY SHARE and FOR NO KEY UPDATE do not conflict.
9. **A FOR UPDATE guard serializes check-then-insert only at READ
   COMMITTED.** At REPEATABLE READ the waiting transaction's snapshot is
   older than the commit it waited for, so it inserts a duplicate. At
   SERIALIZABLE it gets 40001.

## 6. Consequences for the Phase 13 evaluation labels

The evaluation labels (`fixtures/phase13/eval-labels.json`) were frozen
before this matrix was run. The cells that bear on them agree with all of
them, and **no correction was recorded**:

| label | cell(s) | observed |
| --- | --- | --- |
| E01 positive, E04 positive (FOR SHARE) | `guard/rc/none`, `guard/rc/for_share` | 2 workspaces |
| E02 negative (FOR UPDATE, RC), and its RR caveat | `guard/rc/for_update`, `guard/rr/for_update` | 1 at RC; 2 at RR |
| E03 positive (Phase 12 divergence pattern) | `guard/rc/mixed_unlocked_vs_for_update` | INSERT waits, then 2 workspaces |
| E05 positive (40P01) | `fk-cycle/*/insert_then_for_update` | one 40P01 at every level |
| E06 negative at RC | `fk-cycle/rc/insert_then_update_nonkey` | no deadlock; the RR caveat (40001) also observed |
| E07 negative (no orphan with FK) | `held-child-insert/base/rc/delete/*`, `main/base/rc/delete/insert_child/*` | 23503 on whichever comes second |
| E21 negative, E25 negative (DO NOTHING) | `uncommitted-parent/base/rc/insert_same_key_do_nothing/*` | waits, then 0 rows |
| E22 negative (upsert increment) | `upsert/rc/concurrent_do_update_returning/*` | both increments applied |
| E23 positive (DO UPDATE RETURNING) | `upsert/rc/sequential_do_update_returning` | both calls get a row |
| E26 positive (NULLS DISTINCT) | `nulls/rc/parent_null/*` | both inserts commit |

## 7. Model comparison

Two independent comparisons, both on PostgreSQL 17.11.

**(a) The recorded ground truth replayed in the model.**
`scripts/phase13-lock-replay-plan.py` translates every cell of
`reports/phase13/lock-matrix/results.json` into two declared programs (the
scenario DSL) plus the order in which PostgreSQL received the statements (a
session sends its next statement only after its previous one completed:
`sent_after_step`), and the model test `lock_ground_truth` steps the model
in that order, resuming a waiting statement as soon as what it waits for has
ended. At every labeled step it compares whether the statement waited and
its outcome (rows affected or returned, the counted value, or the
SQLSTATE).

| | cells | labeled steps |
| --- | ---: | ---: |
| replayed | **707** of 837 | **2 726** compared |
| disagreements | **0** | **0** |
| not replayed (listed with the reason in `model-replay.json`) | 130 | — |

The 130 not replayed: 102 use `ON DELETE CASCADE` or `DEFERRABLE INITIALLY
DEFERRED` (the model refuses scenarios that would run them, §5 of
`POSTGRESQL-FK-LOCKS.md`; this said "§9" until the closeout); 15 are the foreign-key deadlock cells (which
transaction PostgreSQL aborts is a timer race; the model explores both
victims, and the lockstep matrix below steers them); 7 use string
concatenation in `SET` (not in the value layer); 4 use a bare `BEGIN` of the
`nulls` family; 2 send a statement only if an earlier result satisfies a
condition (application logic, not a schedule).

Two first runs disagreed in the `guard` family; both causes were in the
replay harness, not in the model, and were fixed there: (1) for `SELECT
count(*)` the replay compared the counted value with the number of result
rows; (2) it replayed steps in list order instead of PostgreSQL's send order
(`B.check` was sent after `A.end`). **Sensitivity:** with the model's
compatibility table mutated (KEY SHARE no longer conflicting with UPDATE),
the same test reports 109 disagreements.

**(b) Every schedule of the model, in lockstep against PostgreSQL**
(`pg_matrix::foreign_key_lock_workloads`): 20 workload pairs (parent `FOR
UPDATE`/`FOR NO KEY UPDATE`/`FOR SHARE`/`FOR KEY SHARE`, non-key, key and
other-unique-key updates, deletes, rollbacks, child inserts and re-pointing,
a transaction that took its snapshot before the parent changed) × 3
isolation levels: **543 schedules, 0 disagreements** — blocking
(`pg_stat_activity` reports a lock wait exactly where the model blocks),
every result and SQLSTATE, and the final rows.

**Mismatches found and fixed in the model** (both before this matrix was
used for scoring anything):

* A parent `DELETE`/key `UPDATE` under REPEATABLE READ that finds a
  referencing row committed after its snapshot: first modeled as 40001
  (from `ri_PerformCheck`'s cross-check snapshot); PostgreSQL reports
  23503 — the cross-check applies to cascading writes, not to the `NO
  ACTION` probe's `FOR KEY SHARE`. Found by (b).
* A child insert under REPEATABLE READ after a committed `UPDATE` of
  **another unique column** of the parent (§3 "UPDATE email"): first modeled
  as success (the referenced key was unchanged); PostgreSQL reports 40001,
  because any unique-index column change sets the "keys updated" bit. Found
  by this ground truth (§5), then covered by (b) with three new workloads.

# Foreign keys and row-lock modes in the model (Track A)

Consolidates the plan's `POSTGRESQL-FK-LOCKS` and `ROW-LOCK-COMPATIBILITY`.
Every rule below is checked against PostgreSQL 17.11: by the independent
ground truth (`LOCK-DIFFERENTIAL.md` §1–§6, replayed in the model in §7a) and
by replaying every schedule of 20 workloads × 3 isolation levels in lockstep
(§7b). Implementation: `model/src/db.rs` (`RowLock`, `ForeignKeySchema`,
lock holders), `model/src/exec.rs` (`run_rows`, `run_fk`, `upsert_row`).

## 1. Lock modes

| statement | row lock |
| --- | --- |
| `SELECT … FOR UPDATE` (also `SKIP LOCKED`, `NOWAIT`), `DELETE`, an `UPDATE` that **changes a key column** | `FOR UPDATE` |
| `SELECT … FOR NO KEY UPDATE`, an `UPDATE` that changes no key column | `FOR NO KEY UPDATE` |
| `SELECT … FOR SHARE` | `FOR SHARE` |
| `SELECT … FOR KEY SHARE`, a foreign-key check on the referenced row, the `NO ACTION` probe on referencing rows | `FOR KEY SHARE` |

"Key column" is any column of the primary key or of a unique set — not only
the referenced one — and it counts as changed only if the new value differs
(PostgreSQL compares the values: an ORM that writes every column with the
same values takes `FOR NO KEY UPDATE`). An `UPDATE`'s mode is computed from
the version it updates (the newest one under READ COMMITTED, as after
EvalPlanQual).

| requested \ held | KEY SHARE | SHARE | NO KEY UPDATE | UPDATE |
| --- | --- | --- | --- | --- |
| KEY SHARE | | | | waits |
| SHARE | | | waits | waits |
| NO KEY UPDATE | | waits | waits | waits |
| UPDATE | waits | waits | waits | waits |

A row has a list of holders (one per transaction, its strongest mode) in
acquisition order. A waiting statement waits for the **first** conflicting
holder; when that one ends it re-checks (and may wait for the next) — as
`MultiXactIdWait` waits for members one at a time, so the deadlock
detector's wait-for edge is to one holder. A compatible request is granted
at once even while another request waits (PostgreSQL takes the tuple lock
only when it must sleep). Locks are released when the transaction ends
(commit, rollback, an aborting error, the connection or process ending).

## 2. Referencing side: `RI_FKey_check` (INSERT, UPDATE of the foreign key)

At the end of the statement (PostgreSQL's AFTER ROW triggers fire then),
for every foreign key whose columns are all non-null (`MATCH SIMPLE`) — on an
`UPDATE` only if the key changed or the old version was written by the same
transaction:

1. find the referenced row: on a **new snapshot** under READ COMMITTED, on
   the **transaction's snapshot** otherwise; none → **23503**. A parent
   inserted by another, uncommitted transaction is not visible: 23503 at
   once, no wait (ground truth §4).
2. lock it `FOR KEY SHARE`; a holder of `FOR UPDATE` (a `SELECT … FOR
   UPDATE`, a `DELETE`, a key-changing `UPDATE`) makes the statement **wait**
   — the child row is already written (uncommitted) while it waits.
3. after the wait (or if the row changed after the snapshot), the newest
   version decides: READ COMMITTED — gone or re-keyed → 23503; REPEATABLE
   READ / SERIALIZABLE — any key-column change or deletion → **40001**; an
   update that changed no key column → the check succeeds (also at RR).

A resumed check re-examines the row it found (PostgreSQL's lock-rows node
follows that row's update chain) rather than scanning again.

## 3. Referenced side: `NO ACTION` / `RESTRICT` (DELETE, key UPDATE)

At the end of the statement, for every referencing foreign key and each
deleted or re-keyed row: scan the referencing table on the newest snapshot
(the candidate list is fixed when the scan starts), lock each candidate `FOR
KEY SHARE` (waiting for a holder of `FOR UPDATE`), skip one whose newest
version no longer refers; a remaining one is **23503** — at every isolation
level, including a referencing row committed after the transaction's
snapshot (§7 of `LOCK-DIFFERENTIAL.md`). The `DELETE`'s own `FOR UPDATE`
first waits for every uncommitted child insert (they hold `FOR KEY SHARE`).

## 4. Where foreign keys come from

* Declared scenario tables: `"foreignKeys": [{ "columns", "references",
  "referencedColumns", "onDelete"?, "onUpdate"?, "initiallyDeferred"? }]`;
  the referenced columns must be the primary key or a unique set.
* Tables read from the Drizzle schema, with the scenario feature
  `foreign-keys`: `.references(() => t.c, { onDelete, onUpdate })` and
  `foreignKey({ columns, foreignColumns }).onDelete(…)`, the target resolved
  by declaration across modules. A foreign key whose referenced table or
  columns are not modeled stays a `not-modeled` fact.
* The PostgreSQL oracle creates the constraints (`ALTER TABLE … ADD
  CONSTRAINT … FOREIGN KEY`) and seeds parents first.

## 5. Not modeled (refused, never approximated)

| behavior | handling |
| --- | --- |
| `ON DELETE`/`ON UPDATE` `CASCADE`, `SET NULL`, `SET DEFAULT` | the scenario is refused if a statement deletes a parent (or updates its referenced key) |
| `DEFERRABLE INITIALLY DEFERRED` (check at COMMIT; ground truth §4) | refused if the referencing table is written |
| `SET CONSTRAINTS` | not an operation of the IR |
| `MATCH FULL`, partial-index arbiters, self-referencing multi-row statements | not extracted |
| `SKIP LOCKED` / `NOWAIT` with a weaker strength | an unknown operation |

## 6. State identity and reductions (Track G1/G3)

Lock modes and holder lists enter the canonical encoding only when the model
can request a shared mode; otherwise every two locks conflict and the Phase
6–12 encoding is kept (Phase 6–9 scenarios: identical states and
transitions, `COMPATIBILITY` in `FINAL-REPORT.md`). A pending foreign-key
check is part of the pending statement's encoding. Footprints: a check reads
the referenced table's key predicate (`Pred`), the row (`RowRead`), checks
and changes its lock (`LockCheck`/`LockChange`), and records `Wait` when it
blocks. On all 20 workloads × 3 levels, plus a three-actor workload, the
independence audit finds no violation and the sleep-set search, classic DPOR
and Source-DPOR (stateful and stateless, with sleep sets) produce the same
terminal states, verdicts and stuck states as the unreduced search
(`model/tests/phase13.rs`).

## 7. The Phase 12 divergence (Vertical slice 1)

`createWorkspace` (unlocked) × `createWorkspaceLocked` (`SELECT … FOR
UPDATE` of the owner) and the same with the Server Action: in Phase 12 the
model let the unlocked `INSERT` proceed while the owner was locked, and the
original code diverged 0/3. With the feature on, the model's counterexample
has the `INSERT` **blocked** on the locked transaction; the original code
follows that schedule — PostgreSQL reports the lock wait at the same
statement — and violates the invariant: **3/3 runs for both pairs**
(`original-code-reproduced`; raw runs in `reports/phase13/fk-slice/`, and
the closeout rerun of every slice in `reports/phase13/evidence/`). Discovery no
longer emits the `foreign-key-locks` limitation, and the Phase 12 test that
pinned the divergence now pins the reproduction.

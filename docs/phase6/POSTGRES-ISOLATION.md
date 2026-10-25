# PostgreSQL isolation semantics in the model

Target: PostgreSQL 15–17 (validated on 17.11 and 15.19). Every rule below has
differential tests against a real server
(`crates/commitrift-model/tests/pg_matrix.rs`, results in
[`POSTGRES-VALIDATION.md`](POSTGRES-VALIDATION.md)); rules found wrong by
those tests were changed and the change is recorded here.

## READ COMMITTED

* **Snapshot per statement.** Each statement sees the data committed before
  it started (plus its own transaction's writes); successive statements of
  one transaction may see different data.
* **Waiting.** A statement that must lock or write a row locked by another
  active transaction waits for that transaction to end. The waiting
  statement keeps its snapshot.
* **Re-evaluation (EvalPlanQual).** When the row a statement is about to
  lock/update/delete has a newer committed version than the statement's
  snapshot, PostgreSQL re-checks the `WHERE` predicate on the newest version:
  if it still matches, the statement acts on the newest version (so `SET
  stock = stock - 1` decrements the *new* value), otherwise the row is
  skipped (0 rows). A deleted newest version is skipped. Rows that did not
  match in the snapshot are never reconsidered.
* **Commit** makes the transaction's writes visible to later statements.

Consequences tested on PostgreSQL: a read-modify-write purchase in a
transaction still oversells (the second `UPDATE` writes the stale computed
value); the atomic `UPDATE … SET stock = stock - 1 WHERE stock > 0` and
`SELECT … FOR UPDATE` do not.

## REPEATABLE READ

* **One snapshot per transaction**, taken at its *first statement* (not at
  `BEGIN`).
* **First-updater-wins.** Locking, updating or deleting a row that a
  transaction committed after the snapshot raises 40001 ("could not
  serialize access due to concurrent update"), after waiting if that
  transaction was still running.
* **Write skew is allowed**: two transactions that read overlapping data and
  write *different* rows both commit (tested with the doctors workload).
* `INSERT … ON CONFLICT DO NOTHING` whose conflicting row committed after the
  snapshot raises 40001; a visible conflicting row means "do nothing".
* Generic lock-based "repeatable read" is **not** used: no read locks exist.

## SERIALIZABLE

PostgreSQL implements Serializable Snapshot Isolation. The model does not
implement SSI internals; it uses a **conservative serialization-graph
abstraction**:

* Transactions run as REPEATABLE READ.
* Each SERIALIZABLE transaction records its *logical* read set: the
  predicate of every scan (`count`, `UPDATE`/`DELETE` scans), and for
  `SELECT … ORDER BY pk LIMIT 1` the predicate restricted to the key-ordered
  prefix up to the returned row (exactly the rows the result depends on).
* Edges: wr (reading a version written by T), ww (overwriting T's version),
  rw (T read a predicate whose matching set a later, invisible write
  changes) — added when either the read or the write happens.
* After every statement and at commit, a SERIALIZABLE transaction that
  would close a cycle through committed SERIALIZABLE transactions fails with
  40001.

What this means:

1. The model aborts only on *true* cycles of the logical graph; a correct
   SSI implementation must abort at least those, so the histories the model
   allows **include** those PostgreSQL allows (over-approximation). A
   `NO_COUNTEREXAMPLE_WITHIN_BOUNDS` under SERIALIZABLE is therefore
   meaningful for the modeled workload; a counterexample is a candidate that
   PostgreSQL may prevent by aborting more (replay it).
2. PostgreSQL's predicate locks are plan-dependent (relation locks for
   sequential scans, page locks for index ranges): **PostgreSQL may abort
   more**. Measured: two transactions reading disjoint rows through an
   unindexed column and writing other disjoint rows — PostgreSQL raised 40001
   in 60 of 70 schedules, the model in none. The test asserts that every
   disagreement goes in this direction only.
3. **Timing**: PostgreSQL raises 40001 at the statement that completes the
   dangerous structure, not necessarily at `COMMIT`; the model checks after
   every statement (changed after the differential tests showed the second
   write-skew writer failing at its `UPDATE`).
4. The model distinguishes a *candidate nonserializable schedule* (model
   counterexample), a *serialization failure observed at runtime*
   (`runtime.sqlstates`), and *a schedule PostgreSQL allows* (a confirmed
   replay). SSI behaviors outside the graph abstraction (false positives from
   page/relation locks, read-only transaction optimizations, deferrable
   transactions) are not modeled and are reported as a difference, not
   hidden.

Change made because of a measured mismatch: `SELECT … FOR UPDATE SKIP LOCKED
LIMIT 1` was first modeled as reading the whole predicate; PostgreSQL did
not abort (60 of 70 schedules disagreed), because the scan stops at the
first lockable row. The read set became the key-ordered prefix.

## Locks, waiting and deadlocks

* Row locks are taken by `UPDATE`, `DELETE` and `SELECT … FOR UPDATE` and
  held until commit or rollback (crash included). `FOR SHARE` and other lock
  strengths are not supported.
* A waiting statement is a pending operation, never an immediately
  successful write; it resumes when the holder ends (`Resume` transition).
* An insert or key-changing update waits for an in-progress writer of the
  same unique key and then fails (if it committed) or proceeds (if it
  rolled back).
* `SKIP LOCKED` skips locked rows; `NOWAIT` raises 55P03.
* **Deadlock**: when the wait-for graph has a cycle, a `Deadlock(victim)`
  transition is enabled for each member; the victim's statement fails with
  40P01 and its transaction is rolled back. Which member PostgreSQL picks
  depends on which backend's `deadlock_timeout` expires after the cycle
  formed; the model allows each. The oracle steers PostgreSQL to the
  modeled victim by delaying the closing statement past the earlier
  waiter's timer, and all 84 deadlock schedules agreed (both victims).
* Lock timeouts (`lock_timeout`, `statement_timeout`) are not modeled.

## Autocommit

Statements on the root handle run as single-statement transactions at the
scenario's default isolation level and commit in the same step.

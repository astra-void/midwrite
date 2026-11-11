# PostgreSQL model

`crates/midwrite-model/src/{db,exec,state,value}.rs`. The model is a
small, explicitly bounded multi-version store, not a SQL engine. **It does
not model every PostgreSQL feature**; what it models was checked against
PostgreSQL 17.11 (macOS) and 15.19 (Linux container) by lockstep replay,
and the lock and foreign-key rules also against an independently produced
ground truth ([validation](#validation-against-postgresql)).

## Modeled

| area | behavior | details |
| --- | --- | --- |
| storage | multi-version rows, snapshots, visibility of committed vs uncommitted versions | [storage and statements](#storage-and-statements) |
| statements | point/predicate `SELECT … LIMIT 1` (primary-key order, an assumption the report lists), `count(*)`, `UPDATE`, `INSERT` (single- and multi-row), `DELETE`; inner/left joins with column equality and projections; `RETURNING`; `ON CONFLICT DO NOTHING`; `ON CONFLICT (target) DO UPDATE SET … WHERE …` with `excluded` | [storage and statements](#storage-and-statements) |
| READ COMMITTED, REPEATABLE READ | PostgreSQL snapshot rules, EvalPlanQual re-evaluation on the newest version, unique-key waits, 40001 at REPEATABLE READ | [isolation](#isolation) |
| SERIALIZABLE | a conservative serialization-graph abstraction (every history whose logical serialization graph is acyclic), **not SSI** | [isolation](#isolation) |
| constraints | primary keys, unique sets (no partial or expression indexes), `NULLS NOT DISTINCT`, NOT NULL, simple CHECK | [constraints](#constraints) |
| row locks | `FOR UPDATE`, `FOR NO KEY UPDATE`, `FOR SHARE`, `FOR KEY SHARE` with PostgreSQL's compatibility table; implicit modes of `UPDATE` (key vs non-key change, by value) and `DELETE`; holders in acquisition order, waiting on the first conflicting holder (multixact order); `SKIP LOCKED`/`NOWAIT` with `FOR UPDATE` | [row locks](#row-locks) |
| multi-row reads and locks | `SELECT … FOR UPDATE/SHARE` over several rows locks them one at a time in key order (waits keep the prefix, 40P01, `NOWAIT` 55P03 releasing at the error, `SKIP LOCKED`, `LIMIT n` after a wait); lock order without `ORDER BY` is approximate (PostgreSQL uses scan order) | [multi-row locks](#multi-row-reads-and-locks) |
| foreign keys | `RI_FKey_check` on the referencing side, `NO ACTION`/`RESTRICT` on the referenced side, immediate constraints, `MATCH SIMPLE` — with the scenario feature `foreign-keys` | [foreign keys](#foreign-keys) |
| deadlocks | detected on the wait-for graph; any member of a cycle may be the victim (over-approximation of PostgreSQL's timer race) | [row locks](#row-locks) |
| sequences, defaults | non-transactional sequence allocation (`serial`, identity), constant defaults; values concrete relative to a declared start | [value semantics](../concepts/value-semantics.md) |
| generated values | application- and database-generated keys as values whose identity is the generation event; assumption `generated-values-distinct` | [value semantics](../concepts/value-semantics.md#generated-values) |
| timestamps, JSON | `now()` per transaction, clock readings as opaque values; JSON read-back only (features `timestamps`, `json`) | [value semantics](../concepts/value-semantics.md) |
| read replica | one asynchronous hot standby replaying the primary's commits in order with unbounded lag; replica reads take no locks and never wait — with the scenario feature `replicas`; **model only** | [read replica](#read-replica) |
| in-flight operations | a COMMIT or autocommit write whose acknowledgment is lost may be completed by the server later (`ServerComplete`) | [failure model](../concepts/failure-model.md#lost-acknowledgments) |
| connection pools | bounded FIFO leases | [model semantics](../concepts/model-semantics.md#processes-tasks-connections) |

Statements run atomically unless they wait. A conflicting statement that
waits is resumed when what it waits for ends, and re-checks.

## Storage and statements

A row id is `(table, inserting actor, per-actor counter)` (initial rows:
`(table, initial, index)`). Each row has a chain of versions in commit
order; at most one is uncommitted (its creator holds the row lock). A
snapshot `(sequence, own transaction)` sees the newest version that is its
own or committed at or before the sequence. Schema and constraints come
from the scenario or from the Drizzle schema, never from application logic;
initial rows are validated against every constraint and omitted nullable
columns are `null`.

All reads go through *candidate rows*: rows whose visible image matches
the predicate, in primary-key order.

| statement | behavior |
| --- | --- |
| `SELECT … LIMIT 1` | first candidate; no locks |
| `SELECT count(*)` | number of candidates |
| `SELECT … FOR <mode>` | per candidate in order: wait if another active transaction holds a conflicting lock (`SKIP LOCKED`: skip; `NOWAIT`: 55P03); if a newer committed version exists, READ COMMITTED re-checks the predicate on it (skip or use it), the other levels raise 40001; lock and return |
| `UPDATE … SET … WHERE` | as a locking read for every candidate, then the new image from the target version (`SET stock = stock - 1` reads the target, which after a re-check is the newest version; `SET stock = <local>` writes what the application computed); NOT NULL / CHECK / unique keys (waiting on an in-progress writer of the same key); a new uncommitted version |
| `DELETE … WHERE` | as `UPDATE`, writing a deletion |
| `INSERT` | NOT NULL / CHECK, then unique keys in declaration order (primary key first): a key written by an in-progress transaction waits for it; a committed live row with the key raises 23505, or with `ON CONFLICT DO NOTHING` inserts nothing — except at REPEATABLE READ / SERIALIZABLE when that row is not visible to the snapshot (40001) |
| multi-row `INSERT` (≤ 8 rows) | one statement: any error aborts it; database defaults are drawn per row as the row is formed, application defaults for all rows when the statement is built; a row that waits keeps the earlier rows written and the statement resumes there; a conflict between two of its rows is 23505 (`DO NOTHING` skips the later one; `DO UPDATE` reaching a row the statement already wrote is 21000); foreign keys are checked per row at the end |
| `ON CONFLICT (target) DO UPDATE` | arbiters are the unique sets with exactly the target's columns; a match updates (with `excluded`) under the `WHERE`; an uncommitted match waits; at REPEATABLE READ / SERIALIZABLE a match committed after the snapshot is 40001 |

Commit stamps the transaction's versions with a new sequence (only if it
wrote) and releases its locks; abort removes its uncommitted versions and
releases its locks. A statement error aborts the whole transaction. Query
plans and their locking side effects are not modeled beyond these rules.

## Constraints

| constraint | model | SQLSTATE |
| --- | --- | --- |
| primary key | unique + NOT NULL | 23505 / 23502 |
| UNIQUE (single or multi-column) | checked against the **newest** version of every row, regardless of snapshot; a key with a NULL never conflicts | 23505 |
| UNIQUE `NULLS NOT DISTINCT` (PostgreSQL ≥ 15) | NULL equals NULL for this rule, everywhere uniqueness is checked: single- and multi-row `INSERT`, `UPDATE`, `ON CONFLICT` (a NULL key matches through an NND arbiter), with the same waits as a non-NULL key | 23505 |
| NOT NULL | every new image | 23502 |
| CHECK (expression over columns; fails only when false) | every new image | 23514 |
| foreign keys | with the feature `foreign-keys` ([below](#foreign-keys)) | 23503 |

A unique rule with an unmodeled column, and every partial unique index, is
not enforced; discovery reports a counterexample on such a table as
`INCONCLUSIVE`, naming the rule ([scenario
synthesis](scenario-synthesis.md)).

## Isolation

* **READ COMMITTED** — a snapshot per statement. A statement that must lock
  or write a row locked by another transaction waits and keeps its
  snapshot. When the row has a newer committed version, the `WHERE` is
  re-checked on the newest version (EvalPlanQual): still matching → act on
  it (`SET stock = stock - 1` decrements the new value); otherwise, or if
  deleted, skip. Rows that did not match in the snapshot are never
  reconsidered.
* **REPEATABLE READ** — one snapshot per transaction, taken at its first
  statement (not at `BEGIN`). Locking, updating or deleting a row committed
  after the snapshot raises 40001 (after waiting if it was still running).
  Write skew is allowed. `INSERT … ON CONFLICT` whose conflicting row
  committed after the snapshot raises 40001. No read locks exist.
* **SERIALIZABLE** — REPEATABLE READ plus a serialization-graph check.
  Each SERIALIZABLE transaction records its *logical* read set: the
  predicate of every scan, and for a key-ordered `LIMIT` read only the
  prefix up to the last returned row (the rows the result depends on).
  Edges wr, ww and rw are added when either the read or the write happens;
  after **every statement** and at commit, a transaction that would close a
  cycle through committed SERIALIZABLE transactions fails with 40001
  (PostgreSQL raises it at the statement completing the dangerous
  structure, not necessarily at `COMMIT`).

What the SERIALIZABLE abstraction means:

1. The model aborts only on true cycles of the logical graph; any correct
   SSI implementation aborts at least those, so the model's histories
   **include** PostgreSQL's. A `NO_COUNTEREXAMPLE_WITHIN_BOUNDS` under
   SERIALIZABLE holds for the model's histories, which include
   PostgreSQL's — argued and consistent with every measurement, not proven.
2. PostgreSQL's predicate locks are plan-dependent (relation locks for
   sequential scans, page locks for index ranges), so **PostgreSQL may abort
   more**: measured, 60 of 70 schedules of a workload reading disjoint rows
   through an unindexed column were aborted by PostgreSQL and allowed by the
   model, never the reverse; through the primary-key index the two agree. A
   SERIALIZABLE counterexample may therefore not be realizable — replay it.
3. Read-only and deferrable transaction optimizations are not modeled.
   Mixed isolation levels in one scenario follow the same rules per
   transaction (SERIALIZABLE edges connect only SERIALIZABLE transactions)
   and were not differentially tested.

Autocommit statements are single-statement transactions at the scenario's
default level and commit in their step.

## Row locks

Lock taken per statement:

| statement | row lock |
| --- | --- |
| `SELECT … FOR UPDATE` (also `SKIP LOCKED`, `NOWAIT`), `DELETE`, an `UPDATE` that **changes a key column** | `FOR UPDATE` |
| `SELECT … FOR NO KEY UPDATE`, an `UPDATE` that changes no key column | `FOR NO KEY UPDATE` |
| `SELECT … FOR SHARE` | `FOR SHARE` |
| `SELECT … FOR KEY SHARE`, a foreign-key check on the referenced row, the `NO ACTION` probe on referencing rows | `FOR KEY SHARE` |

| requested \ held | KEY SHARE | SHARE | NO KEY UPDATE | UPDATE |
| --- | --- | --- | --- | --- |
| KEY SHARE | | | | waits |
| SHARE | | | waits | waits |
| NO KEY UPDATE | | waits | waits | waits |
| UPDATE | waits | waits | waits | waits |

* "Key column" is any column of the primary key or of a unique set — not
  only a referenced one — and it counts as changed only when the new value
  differs (an ORM that writes every column with the same values takes `FOR
  NO KEY UPDATE`). An `UPDATE`'s mode is computed from the version it
  updates.
* A row keeps a list of holders (one per transaction, its strongest mode)
  in acquisition order. A waiting statement waits for the **first**
  conflicting holder and re-checks when it ends; a compatible request is
  granted at once even while another waits. Locks are released when the
  transaction ends (commit, rollback, an aborting error, the connection or
  process ending).
* **Deadlock**: when the wait-for graph has a cycle, a `Deadlock` transition
  is enabled for each member; the victim's statement fails with 40P01 and
  its transaction rolls back. Which member PostgreSQL picks depends on
  which backend's `deadlock_timeout` fires after the cycle formed, so the
  model allows each.
* `lock_timeout`, `statement_timeout` and advisory locks are not modeled.

Behaviors measured on PostgreSQL that the rules above follow (and that
differ from a common reading of the manual):

1. Changing any unique column of a parent (not only the referenced key)
   takes `FOR UPDATE`; child inserts then wait (READ COMMITTED) or fail with
   40001 (REPEATABLE READ, SERIALIZABLE).
2. Key-ness is decided by whether the value changes, not by the `SET` list.
3. At REPEATABLE READ, after a committed **non-key** update of the parent a
   child insert, a foreign-key update or `FOR KEY SHARE` still succeeds;
   after a committed key update or delete the child insert fails with 40001
   (23503 at READ COMMITTED). A committed lock-only `SELECT … FOR UPDATE`
   never causes 40001.
4. An uncommitted parent is invisible, not locked: a child insert
   referencing it fails with 23503 at once, without waiting.
5. `ON CONFLICT DO NOTHING / DO UPDATE` wait for a concurrent inserter and,
   at REPEATABLE READ, fail with 40001 when it commits.
6. A parent `DELETE` or key `UPDATE` that waits for an uncommitted child
   insert that then commits fails with 23503 at every level.
7. Inserting a child and then locking the parent deadlocks only with `FOR
   UPDATE`; `FOR KEY SHARE` and `FOR NO KEY UPDATE` do not conflict.
8. A `FOR UPDATE` guard serializes check-then-insert only at READ
   COMMITTED: at REPEATABLE READ the waiting transaction's snapshot predates
   the commit it waited for and it inserts a duplicate; at SERIALIZABLE it
   gets 40001.

## Multi-row reads and locks

`select().from(t).where(…)` without `.limit(1)` returns a list of rows
(bounded by the model's finite world and `limit`), in primary-key order;
`order_by` is accepted only for the single-column primary key. With a lock
clause, rows are locked **one at a time in scan order**: each row is
conflict-checked; `SKIP LOCKED` skips it; `NOWAIT` fails with 55P03 and the
transaction is aborted at that error, so the rows it had locked are free at
once (measured; the session stays "idle in transaction (aborted)" until
`ROLLBACK`); otherwise the statement blocks **keeping the locks it already
granted** and later resumes with the same snapshot and the remaining
candidates. The version check is as for single rows (READ COMMITTED
re-evaluates and skips, the other levels raise 40001); the statement stops
when `limit` rows were returned. A SERIALIZABLE read set is the predicate,
or the key-ordered prefix up to the last returned row when `limit` stopped
the scan.

The model always locks in key order. PostgreSQL locks in the plan's order:
key order under an index scan or with `ORDER BY key`, physical order under
a bitmap or sequential scan (small or unanalyzed tables, non-key
predicates). A `NO_COUNTEREXAMPLE_WITHIN_BOUNDS` for a multi-row lock
without `.orderBy(key)` is therefore conditional on key-order locking, and
the report states it. Weaker strengths combined with `skipLocked`/`noWait`,
non-literal `limit`, other orders and joins in multi-row mode are refused.

## Foreign keys

With the scenario feature `foreign-keys` (or declared `foreignKeys` on
scenario tables; the referenced columns must be the primary key or a unique
set). Discovery turns the feature on when the tables carry foreign keys.

**Referencing side** (`INSERT`, `UPDATE` of the foreign key) — at the end
of the statement, for every foreign key whose columns are all non-null (on
an `UPDATE`, only if the key changed or the old version is the
transaction's own):

1. find the referenced row on a **new snapshot** at READ COMMITTED, on the
   transaction's snapshot otherwise; none → 23503 (an uncommitted parent is
   not visible: 23503 at once);
2. lock it `FOR KEY SHARE`; a `FOR UPDATE` holder makes the statement wait
   with the child row already written (uncommitted);
3. after the wait (or if the row changed after the snapshot) the newest
   version decides: READ COMMITTED — gone or re-keyed → 23503; REPEATABLE
   READ / SERIALIZABLE — any key-column change or deletion → 40001; a change
   of no key column → the check succeeds. A resumed check re-examines the
   row it found rather than scanning again.

**Referenced side** (`NO ACTION`/`RESTRICT` on `DELETE` or key `UPDATE`) —
at the end of the statement, for each deleted or re-keyed row: scan the
referencing table on the newest snapshot, lock each candidate `FOR KEY
SHARE` (waiting for a `FOR UPDATE` holder), skip one that no longer refers;
a remaining one is 23503 at every isolation level. The `DELETE`'s own `FOR
UPDATE` first waits for every uncommitted child insert.

Refused, never approximated: `ON DELETE`/`ON UPDATE` `CASCADE`, `SET NULL`,
`SET DEFAULT` when a statement would run them; `DEFERRABLE INITIALLY
DEFERRED` when the referencing table is written; `SET CONSTRAINTS`, `MATCH
FULL`, partial-index arbiters, self-referencing multi-row statements;
`SKIP LOCKED`/`NOWAIT` with a weaker strength. A foreign key whose target is
not modeled stays a `not-modeled` schema fact.

## Read replica

Stated with every replica scenario:

* **asynchronous streaming replication** to one hot standby, replaying the
  primary's commits **in commit order** — a replica read sees a prefix of
  the commit sequence;
* **unbounded lag** — no lag distribution, no timing guarantee, no
  synchronous replication (`remote_apply`);
* **one replica** — Drizzle's `withReplicas` may pick a different replica
  per read; several replicas with different prefixes are not modeled;
* **reads only** — writes and row locks on a replica are refused.

`State.replica_seq` is the replayed prefix (initial data is visible); the
optional environment transition `Replicate` replays the next commit; a
replica statement runs outside any primary transaction on that prefix,
takes no locks and never waits. In the canonical encoding the replica is one
more snapshot holder and every committed version carries its distance from
the prefix. A replica read records `Snapshot` and `Replicate` records
`CommitWrite`, so they are dependent. Without the feature a replica read is
refused, never bound to the primary. Discovery carries the project's
declared `replicaFactories` and the feature into synthesized scenarios and
checks that the loader re-extracts the same replica reads (the round trip in
[scenario synthesis](scenario-synthesis.md)).

Replica counterexamples are **model-only**: the lockstep oracle has one
server and refuses replica models, and original-code verification refuses
schedules that depend on replication. A stale read is reported only against
a declared property (read-your-writes); heuristic properties include every
replica progress among their serial outcomes.

## Refused or not modeled

Refused (the scenario does not load): the foreign-key actions above;
replica reads without the `replicas` feature; writes or row locks on a
replica; generated values in `initialState`.

Not modeled: subqueries, aggregates other than `count`, `ORDER BY` other
than the key, `LIMIT` other than 1 except in multi-row mode, row values of
`UPDATE`/`DELETE … RETURNING`, partial-index upsert arbiters
(`targetWhere`), `SET CONSTRAINTS`, `MATCH FULL`, triggers, row-level
security, database functions other than the recognized defaults,
savepoints and nested transactions, `lock_timeout`/`statement_timeout`,
advisory locks, `LISTEN`/`NOTIFY`, decimals, string concatenation in SQL
`SET`, JSON comparisons, several replicas with different lags, synchronous
replication, hot-standby query cancellation, server crashes that lose
committed data, collations. Complete, deduplicated list:
[limitations § unsupported PostgreSQL behavior](../reference/limitations.md#unsupported-postgresql-behavior).

## Validation against PostgreSQL

Every result compares the model with a real server, never with another
model; the first disagreement is recorded as observed, and model
predictions are never adjusted to what PostgreSQL did. Measured counts:
[evidence index](../research/evidence-index.md).

### The lockstep oracle

`model-check --validate-postgres` (and the `pg_matrix` tests):

1. the model executes the schedule and records, per transition, the
   physical actions it implies (`BEGIN ISOLATION LEVEL …`, a statement with
   its expected outcome, `COMMIT`, an abort, a deadlock victim, a crash);
2. a private, disposable cluster (Unix socket in a 0700 directory, no TCP,
   `autovacuum=off`, removed afterwards) gets the scenario's schema (DDL
   from the declared constraints, foreign keys added after the tables,
   parents seeded first) and the initial rows;
3. each actor gets its own session; autocommit statements of a task get a
   pooled session at the default level;
4. the schedule decides which session sends next. A statement counts as
   **blocked** only when the server says so (`pg_stat_activity` wait event
   `Lock`), and `pg_blocking_pids` must name the session the model says it
   waits for; a statement expected to finish may show a lock wait for at
   most 250 ms. Results are compared exactly: rows, counts, affected rows,
   SQLSTATEs;
5. crashes kill the session's process and wait for the backend to exit;
6. deadlock victims are steered by delaying the closing statement past the
   earlier waiter's `deadlock_timeout` — the only timing-based coordination
   (a slow machine can make it disagree; reported, never forced);
7. afterwards the committed contents of every table are compared.

The oracle sends the **model's** SQL (labeled model-derived or
fixture-declared); it does not run the application or Drizzle. Generated
values are sent as deterministic UUIDs that encode their event. It refuses
replica models. Schedules *inside* one statement (between its snapshot and
its row visits) cannot be forced on a real server and are not compared.

### Differential matrices

Every schedule of a workload (enumerated without reduction,
`witness::enumerate_schedules`) is replayed at each isolation level: lost
updates, `FOR UPDATE`, conditional updates, write skew, check-then-act with
and without a unique key, `ON CONFLICT`, inserts waiting on uncommitted
inserts, CHECK constraints, deletes against updates, job claims with `SKIP
LOCKED`/`NOWAIT`, deadlocks (both victims), crash after commit, transaction
retry, concurrent tasks; foreign keys and lock modes; upserts; multi-row
inserts; predicates with NULLs; timestamps and JSON; multi-row locks;
`NULLS NOT DISTINCT`. The one accepted difference is the one-way SSI
granularity case above (`ssi_granularity_differences_only_go_one_way`
fails on any other disagreement); the measured lock-order difference of
multi-row locks without `ORDER BY` is asserted to exist.

### Independent ground truth for locks and foreign keys

A standalone script (`scripts/lock-matrix.py`, Python standard library
only; `scripts/lock-replay-plan.py` turns its cells into declared programs)
produced a matrix of
PostgreSQL row-lock and foreign-key behavior **without reading the model's
implementation**: two wire-protocol sessions and an observer connection
that decides blocking from `pg_locks` (no sleeps as synchronization), fresh
tables per cell, lock modes recorded with `pgrowlocks`. The model test
`lock_ground_truth` replays the recorded cells
([`fixtures/model/lock-matrix/`](../../fixtures/model/lock-matrix/)) in
PostgreSQL's send order and compares, at every labeled step, whether the
statement waited and its outcome. Cells that use refused features
(cascades, deferred constraints), timer-dependent deadlock victims, SQL
string concatenation or application conditions are not replayed and are
listed with the reason. Sensitivity check: mutating the compatibility table
makes the replay disagree.

### Drizzle probes

The pinned drizzle-orm's SQL generation and default evaluation are checked
by probes (what Drizzle sends with which parameters, how often callbacks
ran, the final rows) and by running the same TypeScript through Drizzle on
PostgreSQL and through extraction in the model, comparing final rows column
by column with generated values matched one-to-one. Shapes whose runtime
behavior was observed but not modeled extract to unknown operations.
Kept probe output: [`fixtures/reports/drizzle-sql/probe.json`](../../fixtures/reports/drizzle-sql/probe.json).

### Limits of this validation

Two PostgreSQL versions (17.11, 15.19; `NULLS NOT DISTINCT` needs ≥ 15,
16 was not run), both aarch64; small workloads (≤ 3 actors, a few rows);
plan-dependent behavior only where named above; not covered: triggers,
`lock_timeout`, savepoints, prepared transactions, read-only/deferrable
transactions, replication.

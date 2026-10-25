# PostgreSQL validation

`crates/commitrift-model/src/oracle.rs`, tests
`crates/commitrift-model/tests/pg_matrix.rs` and `postgres.rs`, CLI
`commitrift model-check --validate-postgres`. PostgreSQL tested: **17.11
(Homebrew), macOS arm64** and **15.19 (Debian 12), Linux aarch64** (the same
test suites in `scripts/linux-replay-docker.sh`). Raw output:
`reports/phase6/postgres-matrix-macos.txt`, `reports/phase6/linux/`.

## How a schedule is replayed

1. The model executes the schedule first and records, per transition, the
   physical actions it implies (`BEGIN ISOLATION LEVEL …`, a statement with
   its expected outcome, `COMMIT`, an abort/`ROLLBACK`, a deadlock victim,
   a crash).
2. A private, disposable cluster (the Phase 4 `commitrift-replay`
   infrastructure: Unix socket in a 0700 directory, no TCP, removed on drop)
   gets a database with the scenario's schema (DDL from the declared
   constraints) and, before each run, its initial rows.
3. Each actor gets its own connection — a separate `psql` process; autocommit
   statements of a task get their own pooled session at the scenario's
   default isolation level.
4. The schedule decides which session sends its next statement. A statement
   counts as *blocked* only when the server says so (`pg_stat_activity`
   `wait_event_type = 'Lock'`), and `pg_blocking_pids` must name the session
   of the actor the model says it waits for. A statement expected to finish
   must finish; a lock wait has to persist 250 ms before it counts against
   it (a session that was just released can still show `Lock` briefly).
   Results are compared exactly: the JSON row of `SELECT … LIMIT 1`, counts,
   affected rows (`WITH … RETURNING 1`), and SQLSTATEs (verbose errors).
5. Crashes kill the session's process; the oracle waits until the backend
   is gone (the server rolled back) before continuing.
6. Deadlock victims: PostgreSQL picks the backend whose `deadlock_timeout`
   (set to 500 ms) expires after the cycle exists. When the model's victim is
   the transaction that closed the cycle, the oracle sends the closing
   statement only after the earlier waiter's timer has expired
   (750 ms after it started waiting); otherwise it sends it at once. This is
   the only timing-based coordination.
7. After the schedule, the committed contents of every table are read and
   compared with the model's committed state.

The first disagreement stops the replay; it is recorded as observed. Model
predictions are never adjusted to what PostgreSQL did.

SQL is generated from the model's statements and labeled per actor as
*model-derived (extracted from source)* or *fixture-declared*. The original
TypeScript/Drizzle code is not executed by this oracle.

## The differential matrix

Every schedule of every configuration (enumerated without reduction) was
replayed:

| Workload | Isolation | Constraint / variant | Schedules | Disagreements |
| --- | --- | --- | --- | --- |
| lost update: read-modify-write, autocommit | RC, RR, SER | — | 3 × 16 | 0 |
| lost update in a transaction | RC, RR, SER | — | 3 × 92 | 0 |
| `SELECT … FOR UPDATE` then update | RC, RR, SER | — | 3 × 36 | 0 |
| concurrent conditional `UPDATE … WHERE stock > 0` | RC, RR, SER | autocommit / transaction | 3 × (4 + 24) | 0 |
| write skew (doctors) | RC, RR, SER | — | 3 × 70 | 0 |
| check-then-act redemption | RC, RR, SER | without / with UNIQUE | 6 × 6 | 0 |
| `INSERT … ON CONFLICT DO NOTHING` | RC, RR, SER | plain / after a read | 3 × (14 + 50) | 0 |
| insert waiting on an uncommitted insert (rollback / commit) | RC, RR, SER | UNIQUE | 3 × (14 + 14) | 0 |
| CHECK constraint under concurrent decrement | RC, RR, SER | CHECK | 3 × 14 | 0 |
| `DELETE` vs `UPDATE` (re-check / serialization failure) | RC, RR, SER | — | 3 × 14 | 0 |
| job claim: `FOR UPDATE`, `SKIP LOCKED` (1 and 2 jobs), `NOWAIT` | RC, RR, SER | — | 3 × (24 + 24 + 70 + 24) | 0 |
| deadlock (opposite update order; both victims) | RC, RR | — | 2 × 42 | 0 |
| crash after commit; crash then client retry | RC | — | 4 + 7 | 0 |
| transaction retry after serialization failure | RR | `retry 2` | 50 | 0 |
| concurrent tasks of one request (`Promise.all`) | RC | — | 2 | 0 |
| **total** | | **60 configurations** | **1 695** | **0** |

255 s for the whole matrix on macOS (release build, one test thread). On
Linux with PostgreSQL 15.19 the same matrix passed (every configuration
asserts zero disagreements; 47 s, debug build, parallel test threads);
per-configuration counts are identical because the schedules are
enumerated from the same model.

Reference cases required by the specification and where they are covered:
lost update (purchase workloads), write skew, check-then-act with and without
a unique constraint, concurrent conditional UPDATE, `SELECT FOR UPDATE`,
serialization failure (RR purchase, SER write skew, RR `ON CONFLICT`),
retry after serialization failure (`tx-retry`), crash after commit.

## Mismatches found, and what was done

| Found | Cause | Resolution |
| --- | --- | --- |
| SER write skew: PostgreSQL raised 40001 at the second writer's `UPDATE`, the model at its `COMMIT` (18 of 60 sampled schedules; final rows equal) | the model checked the serialization graph only at commit | the graph is checked after every statement of a SERIALIZABLE transaction; 0 disagreements since |
| SER `FOR UPDATE SKIP LOCKED LIMIT 1` with two jobs: the model raised 40001, PostgreSQL committed both (60 of 70 schedules) | the model treated `LIMIT 1` as reading every matching row; the scan stops at the first lockable row | the read set of `ORDER BY pk LIMIT 1` is the key-ordered prefix up to the returned row; 0 disagreements since |
| `begin isolation repeatable-read` in declared programs | a DSL tokenizer bug (`-`) | fixed (scenario rejected before, never silently wrong) |

## Known, accepted difference

**SSI predicate-lock granularity** (`ssi_granularity_differences_only_go_one_way`):
two SERIALIZABLE transactions reading disjoint rows through an unindexed
column and updating other disjoint rows. PostgreSQL's sequential scans take
relation-level predicate locks and it aborted one transaction (40001) in
**60 of 70** schedules; the model, which uses the logical read sets, aborted
in none. The test requires every disagreement to be of this kind
(PostgreSQL stricter). With the same reads through the primary-key index,
PostgreSQL agreed with the model in all 70 schedules. The model therefore
over-approximates the behaviors PostgreSQL allows under SERIALIZABLE; a
model counterexample under SERIALIZABLE may be prevented by PostgreSQL
(replay it).

## Command-line validation of the fixtures

All seven counterexamples of `fixtures/concurrency/scenarios/` were replayed
with `--validate-postgres`: all `postgres-confirmed-schedule`
(`reports/phase6/model-check/*.json`).

## Limits of this validation

* Two PostgreSQL versions (17.11 on macOS, 15.19 on Linux), both aarch64.
* Small workloads (≤ 3 actors, ≤ 4 rows); plan-dependent behavior (index vs
  sequential scans) is only covered by the SSI case above.
* Not covered: `FOR SHARE`/`FOR NO KEY UPDATE`, foreign keys, triggers,
  `lock_timeout`, multi-row statements racing within one statement,
  savepoints, prepared transactions, read-only/deferrable transactions.
* Schedules inside one statement (between snapshot and row visit) cannot be
  forced on a real server and are not compared.

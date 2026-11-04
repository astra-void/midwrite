# Phase 16 PostgreSQL differential and original-code evidence

(Track H's H4–H5 and the brief's §8.2, PostgreSQL part.)

Every result below compares the model with a real PostgreSQL server, never
with another model. Server: PostgreSQL 17.11 (Homebrew,
`/opt/homebrew/opt/postgresql@17/bin`), private disposable clusters on Unix
sockets that CommitRift creates and removes (`autovacuum=off`). The Linux
run uses PostgreSQL 15 in the container
([QUALITY-GATES.md](QUALITY-GATES.md#linux)).

## 1. Lockstep matrices of the new semantics

In lockstep, each schedule is replayed statement by statement on the
server in the model's interleaving. The harness compares statement results,
SQLSTATEs, which backend waits for which (`pg_blocking_pids`) and the final
tables.

| semantics | test | configurations | schedules replayed | disagreements | details |
| --- | --- | ---: | ---: | ---: | --- |
| `UNIQUE … NULLS NOT DISTINCT` (Track D) | `crates/commitrift-model/tests/phase16_nulls.rs` | 90 (15 workloads × NULLS [NOT] DISTINCT × RC/RR/SER) | 1 542 | **0** | 291 schedules end in 23505 under NULLS NOT DISTINCT, none on a NULL key under NULLS DISTINCT; `ON CONFLICT DO NOTHING/DO UPDATE` wait, then 0 rows / update (RC) or 40001 (RR/SER) ([NULLS-NOT-DISTINCT.md](NULLS-NOT-DISTINCT.md#evidence)) |
| multi-row `SELECT … FOR UPDATE/SHARE`, NOWAIT, SKIP LOCKED, LIMIT (Track C) | `crates/commitrift-model/tests/phase16_locks.rs` | 90 (12 workloads × RC/RR/SER, three statement forms) | 2 124 | **0** | 7 878 statements, 1 296 lock waits, SQLSTATEs 40001 × 420, 40P01 × 270, 55P03 × 63 ([MULTI-ROW-LOCKS.md](MULTI-ROW-LOCKS.md#6-evidence)) |
| lock order **without** `ORDER BY` (the approximation, measured) | same file | 2 workloads, RC, rows in physical order 3, 1, 2 | 51 | **24 (expected)** | PostgreSQL locks in scan order; the model locks in key order. The test asserts that disagreements exist; a NO_VIOLATION for such a lock is stated as conditional on key-order locking |
| caught database errors (Track B) | `crates/commitrift-cli/tests/phase16_loops.rs` | drizzle-orm 0.45.4 on node-postgres and postgres-js | — | — | which property of the thrown error holds the SQLSTATE (`DrizzleQueryError.cause.code`, `code` undefined) was established against the real driver stack, not assumed ([LOOP-SEMANTICS.md](LOOP-SEMANTICS.md#how-drizzle-orm-and-the-drivers-surface-database-errors-verified)) |

## 2. Discovered counterexamples, replayed on PostgreSQL

`scripts/phase16-evidence.py reports/phase16/evidence --repeat 3`
(`reports/phase16/evidence/SUMMARY.md`): every scenario that discovery
emits on ten applications (the Phase 13–16 evaluation applications, the
Phase 15 worlds application and the five Phase 16 first-party
applications); every counterexample replayed with `commitrift
model-check --validate-postgres`.

| | result |
| --- | --- |
| counterexample schedules | 54 |
| confirmed on PostgreSQL in lockstep | **49** |
| database outcomes compared | 264 |
| disagreements | **0** |
| not replayable (runtime-inconclusive) | 5, all **read-replica** counterexamples (one each in the Phase 13, 14, 15 and 16 evaluation applications and `fixtures/phase16/genid`'s `createInvite`) |

The lockstep harness runs a single primary. A schedule in which an actor
reads a replica that has not yet replayed a commit has no counterpart
there, so these 5 stay **model-only**, and the report says so: "the
counterexample is a model counterexample only". The replica model itself
(one asynchronous replica, unbounded lag) is unchanged since Phase 13.

## 3. Original code (authorized harnesses, first-party only)

`--verify-source --allow-execution` ran only on the first-party fixtures
whose harnesses ship with the repository. **No external application was
executed.**

| application | entry | class | world | evidence | runs |
| --- | --- | --- | --- | --- | --- |
| `fixtures/phase16/retry` | `placeOrder` (exported function, `for` retry loop) | ambiguous-commit | base | `original-code-connection-loss-reproduced`: the proxy broke the connection after PostgreSQL committed the INSERT; node-postgres reported an error; the original loop inserted again; two `orders` rows with one request id | **3/3** |
| `fixtures/phase16/retry` | `POST` (Next.js route handler, `request.json()`, then `placeOrder`) | ambiguous-commit | base | same, through the route handler | **3/3** |
| `fixtures/phase15/worlds-app` | `createFolder` | check-then-act | `null:parentId` | `original-code-reproduced` | 3/3 |
| `fixtures/phase15/worlds-app` | `startExport` | check-then-act | `stored-boundary:quotas.remaining=1` | `original-code-reproduced` | 3/3 |

Safe counterparts, on the original code along the model's own schedules
(`crates/commitrift-cli/tests/phase16_loops.rs`,
`crates/commitrift-discover/tests/phase16_nulls.rs`):

| entry | what ran | result |
| --- | --- | --- |
| `recordPayment` (retry loop, `INSERT … ON CONFLICT DO NOTHING` on the reused idempotency key) | "COMMIT acknowledgment lost, PostgreSQL commits, the loop runs the transaction again" | `ORIGINAL_CODE_RECOVERY_REPRODUCED`: one `payments` row, invariant holds, 2/2 |
| `withdraw` (SERIALIZABLE, re-run on `cause.code === "40001"`) | "one withdrawal commits, the other gets 40001, then re-runs" | `invariant-held`: the original loop recognized the wrapped SQLSTATE and re-ran the transaction |
| `createLabel` (Phase 15's NULLS NOT DISTINCT safe counterpart) | the model's candidate schedule | invariant held, 3/3; the candidate moved from INCONCLUSIVE to NO_VIOLATION_WITHIN_BOUNDS |

**At least one new first-party source-level counterexample reproduced
against PostgreSQL:** `placeOrder` and its route handler (DoD 15). **A safe
counterpart verified within explicit bounds:** `recordPayment`, `withdraw`
and `createLabel` (DoD 16): NO_VIOLATION_WITHIN_BOUNDS in the model, and
the invariant held on the original code along the model's schedules.
Neither is a proof outside those bounds.

## 4. Where the model is approximate (stated in every affected report)

* Lock order of a multi-row lock without `.orderBy(primaryKey)`: key order
  in the model, scan order in PostgreSQL (§1, measured).
* Read replicas: model-only counterexamples (§2).
* Generated identifiers: which actor generated a value is not observable in
  an original-code run, so a property using a Phase 16 generated-identity
  term is reported `not-evaluated` on original code, never "held" or
  "violated" there ([GENERATED-ID-INVARIANTS.md](GENERATED-ID-INVARIANTS.md#5-original-code-and-postgresql-replay)).

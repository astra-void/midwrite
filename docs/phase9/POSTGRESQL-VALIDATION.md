# PostgreSQL validation (Phase 9)

What the in-flight, shared-process and pool scenarios established when the
**original TypeScript** of the first-party fixture `fixtures/phase9/ledger-app`
(drizzle-orm 0.45.4 on node-postgres `pg` 8.23.1) ran against a disposable
PostgreSQL through the test-only protocol proxy. Everything here is
first-party: the fixture was written to contain these patterns. No external
application was executed.

Final run: `scripts/phase9-evidence.py reports/phase9/evidence 5` with the
final release binary (sha256 `c2f4d6d4…`, source digest `df9f2a6d…`), macOS
arm64, PostgreSQL 17.11 (Homebrew), Node 22.23.2. Reports:
`reports/phase9/evidence/*.json`; matrix:
`reports/phase9/final/evidence-matrix.md`. Linux (PostgreSQL 15,
bubblewrap): `FINAL-REPORT.md` § Cross-platform.

## Fault-injection cases

The proxy realizes three fates of an operation that is on the wire when the
worker is SIGKILLed (`PROTOCOL-FAULT-INJECTION.md`):

| fate | proxy action | what the server sees |
| --- | --- | --- |
| `not-reached` | holds the operation's frames, then discards them and closes the server connection | the transaction (or statement) never arrives; the session ends and PostgreSQL rolls back |
| `answered-and-withheld` | forwards the operation, observes the server's answer, withholds it and everything after it from the client | the operation completes before the client dies |
| `held-for-later-delivery` | holds the frames and keeps the server connection open after the client dies; delivers them at the model's `ServerComplete` step | the operation arrives later, after the restarted client has acted |

## Evidence matrix (5 runs per scenario)

| scenario | in-flight operation: model fate = observed | model verdict | runtime evidence | runs followed / matched | inconclusive | differing SQL | durable rows right after the kill | final rows | client ack | invariant |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| ledger-blind-commit-in-flight | COMMIT: answered-and-withheld = answered-and-withheld | COUNTEREXAMPLE_FOUND | ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED | 5/5 / 5/5 | 0 | 0 | payments 1 | payments 2 | none | violated |
| ledger-autocommit-blind-in-flight | INSERT: answered-and-withheld = answered-and-withheld | COUNTEREXAMPLE_FOUND | ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED | 5/5 / 5/5 | 0 | 0 | payments 1 | payments 2 | none | violated |
| ledger-reconciled-commit-in-flight | COMMIT: held-for-later-delivery = held-for-later-delivery | COUNTEREXAMPLE_FOUND | ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED | 5/5 / 5/5 | 0 | 0 | payments 0 | payments 2 | none | violated |
| ledger-idempotent-commit-not-reached | COMMIT: not-reached = not-reached | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | ORIGINAL_CODE_RECOVERY_REPRODUCED | 5/5 / 5/5 | 0 | 0 | ledger 0 | ledger 1 | none | holds |
| ledger-idempotent-commit-answered | COMMIT: answered-and-withheld = answered-and-withheld | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | ORIGINAL_CODE_RECOVERY_REPRODUCED | 5/5 / 5/5 | 0 | 0 | ledger 1 | ledger 1 | none | holds |
| ledger-idempotent-commit-held | COMMIT: held-for-later-delivery = held-for-later-delivery | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | ORIGINAL_CODE_RECOVERY_REPRODUCED | 5/5 / 5/5 | 0 | 0 | ledger 0 | ledger 1 | none | holds |
| ledger-settle-update-answered | UPDATE orders: answered-and-withheld = answered-and-withheld | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | ORIGINAL_CODE_RECOVERY_REPRODUCED | 5/5 / 5/5 | 0 | 0 | `orders.status = paid` | `paid` | none | holds |
| ledger-settle-update-not-reached | UPDATE orders: not-reached = not-reached | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | ORIGINAL_CODE_RECOVERY_REPRODUCED | 5/5 / 5/5 | 0 | 0 | `orders.status = unpaid` | `unpaid` | none | holds |
| ledger-shared-process-crash | C's INSERT: answered-and-withheld = answered-and-withheld (A, B at checkpoints) | COUNTEREXAMPLE_FOUND | ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED | 5/5 / 5/5 | 0 | 0 | payments 2 | payments 2 | none | violated |
| ledger-pool-starvation | — (stuck: pool starvation) | COUNTEREXAMPLE_FOUND | original-code-reproduced | 5/5 / 5/5 | 0 | 0 | — | ledger 0, audit_log 0 | — | termination violated |
| ledger-pool-of-two | — | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | **not run** (no counterexample and no fault to probe) | 0/0 | — | — | — | — | — | — |

50 original-code runs across the 10 scenarios that have something to run, every one followed by the
coordinator and matched to the model; 0 inconclusive, 0 diverged, 0 SQL
statements that differ from or fall outside the model. The interrupted run
from earlier the same day (`reports/phase9/evidence-interrupted-run/`, 4
scenarios × 5) and the run before the finalization fix
(`reports/phase9/final/pre-fix/evidence/`, 11 × 5) gave the same table.

## The three required ambiguous outcomes

### A. COMMIT not received by PostgreSQL (`ledger-idempotent-commit-not-reached`)

Wire log of run 1: `FaultArmed: HoldRequest on the next Commit` →
`OperationHeld: the Commit is withheld from the server` → worker killed →
`ClientClosed`, `Discarded: held COMMIT discarded`, `ServerClosed`.
`pg_stat_activity` showed the backend `idle in transaction` before the kill;
PostgreSQL ended the session 16 ms after the kill (run 1) (`sessionsEnded`), and
the ledger read from PostgreSQL right after the kill was **empty** — the
uncommitted insert did not become durable. The client received no
acknowledgment (`clientAcknowledged: false`, it was killed waiting). The
restarted client (new pid) recorded the payment once. The model never takes
absence of an acknowledgment as "rolled back"; this run is the
`notCommitted` world, established by the proxy and by PostgreSQL, not
assumed.

The same fate for an autocommit `UPDATE` (`ledger-settle-update-not-reached`)
leaves `orders.status = 'unpaid'`.

### B. COMMIT completed, response withheld (`ledger-blind-commit-in-flight`)

`FaultArmed: DropResponse on the next Commit` → `OperationForwarded` →
`OperationCompleted: the server answered: CommandComplete COMMIT` →
`ResponseWithheld`. The proxy saw the server's answer; the application did
not. Backend state before the kill: `idle` (the transaction had ended
server-side). The worker was SIGKILLed (pid 13961); PostgreSQL ended the
session after 17 ms; the durable ledger read **independently through
PostgreSQL** after the session ended held **1 payment** — the commit
survived the termination. The retry (new pid 13971) inserted a second row:
2 payments for one request id, `recorded-at-most-once` violated in 5/5 runs.

The autocommit variant (`ledger-autocommit-blind-in-flight`, answer
`CommandComplete INSERT 0 1`) and the autocommit `UPDATE`
(`ledger-settle-update-answered`, `orders.status = 'paid'` durable although
the client was killed) show the same: an answered-and-withheld write is
durable.

### C. COMMIT forwarded later, after recovery started (`ledger-reconciled-commit-in-flight`)

The application (`recordPaymentReconciled`) reads the ledger for its
request id and inserts only if it finds nothing. Run 1, in order:

| t | what happened | observed by |
| --- | --- | --- |
| 1–3 | read: `no row`; BEGIN; INSERT (1 row, uncommitted) | worker statements |
| 4 | COMMIT announced and released; `OperationHeld`; backend `idle in transaction`; worker SIGKILLed (pid 15708); **the server connection stays open**; durable ledger **empty** | proxy, `pg_stat_activity`, PostgreSQL read |
| 5 | restart (pid 15720) | coordinator |
| 6 | reconciliation read: **`no row`** | worker statement |
| 7 | the held COMMIT is delivered: `Delivered`, `OperationCompleted: CommandComplete COMMIT`; session ended; durable ledger now **1 row** | proxy, PostgreSQL read |
| 8–10 | the retry's BEGIN, INSERT, COMMIT | worker statements |
| end | **2 payments** for one request id | PostgreSQL read |

The reconciliation read is correct when it runs — the first COMMIT has not
reached the server — and the retry is still a duplicate, because the
original COMMIT completes after the read. This is the distinct outcome the
model represents with an orphan completed by a later `ServerComplete`; it is
not collapsed into "already committed" (the read would have seen the row)
or "aborted" (the row would never appear). 5/5 runs reproduced this order.
The model's counterexample has the same shape in all three search modes:
`step step step crash-in-flight retry step server-complete step step step`
(`reports/phase9/final/reduction-modes.md`).

`ledger-idempotent-commit-held` is the same timing with
`UNIQUE(request_id)` + `ON CONFLICT DO NOTHING`: the restarted client
begins, the held COMMIT is delivered and completes (durable ledger: 1 row),
and the retry's insert then reports `0 row(s)` (conflict): 1 row, invariant
holds, 5/5. The run covers the order the model's probe chose (completion
before the retry's insert); the model explores the other orders.

## Recovery strategies

"Model" is the bounded exhaustive search of the stated scenario
(`fixtures/phase9/model-scenarios`, `fixtures/phase8/scenarios` with
`--max-in-flight 1`); "original code" is a run of the first-party fixture,
one schedule per run. Results are the same under full search, sleep sets and
DPOR (`reports/phase9/final/reduction-modes.md`).

| strategy | model (bounds: 1 client unless noted, ≤1 crash/lost ack, ≤1 retry) | original code |
| --- | --- | --- |
| blind retry | counterexample: `ack-lost-committed, server-complete, retry` → 2 rows | duplicated, 5/5 (B; also autocommit) |
| lookup-then-insert | counterexample: `ack-lost-committed, retry, read, server-complete, insert` | duplicated, 5/5 (C) |
| lookup-then-insert with a concurrent second delivery | counterexample without any fault (plain check-then-act race) | — |
| unique idempotency key + `ON CONFLICT DO NOTHING` | no counterexample (19 states) | holds in all three fates, 5/5 each |
| lookup + unique key, concurrent second delivery | no counterexample (305 states) | — |
| transactional outbox (`shipping-outbox-no-loss`) | no counterexample with an in-flight fault (72 states) | Phase 8 crash probe only (`reports/phase9/final/phase8-regression/`) |
| claim row, then effect (`job-claimed-effect-redelivery`) | no counterexample (90 states) | Phase 8 crash probe only |

A "no counterexample" is bounded by the scenario's actors, faults and step
bound; a recovery run is one schedule. Neither is a proof of the strategy.

## Shared process and pool

`ledger-shared-process-crash`: tasks A, B, C of one Node process (pid
16747 in run 1) with a pool of one connection (the harness pool's `max`
was checked against the scenario). A committed its payment and released the
connection to C; C's autocommit INSERT was forwarded and answered with the
answer withheld; A and B were at announced checkpoints (an external
confirmation, never sent). One SIGKILL of the process group ended all three
tasks (same pid in all three crash records); PostgreSQL ended the session
after 15 ms; 2 payments durable, no confirmation published. Tasks are
attributed by the harness's task context (`AsyncLocalStorage`), not by
function name (`PROCESS-AND-TASK-MODEL.md`).

`ledger-pool-starvation` (`settleWithAudit`): the transaction holds the
process's only connection and, inside it, writes an audit row through the
pool (`db`, not `tx`) and awaits it; the audit insert waits for a connection
the open transaction holds.
At runtime: no statement announced or completed for 803 ms, the pool
reports 1 waiting, 0 idle of 1, 0 lock waits (`pg_blocking_pids`), 1 task
unfinished — classified `pool-starvation`, agreeing with the model, 5/5.
Nothing was committed.

Not executed on original code: a pool of capacity 2 (model only:
`ledger-pool-of-two`, no counterexample; the same code terminates), an
application-level wait cycle (model tests
`a_lock_wait_on_the_applications_own_transaction_is_an_application_wait_cycle`),
and a real PostgreSQL deadlock in the ledger app. PostgreSQL deadlocks are
covered by the Phase 6 model-SQL replay matrix (`pg_matrix`
`deadlock_workloads`: the server's detector aborts a victim; the model's
`Deadlock` transition is never a stuck state, test
`a_database_deadlock_is_not_pool_starvation`). A slow operation is not
reported as stuck: stuck-state confirmation requires quiescence **and** a
pool or lock explanation; otherwise the run is `Unexplained`/INCONCLUSIVE.

## Protocol data retained

The wire log keeps event names, connection numbers, the backend pid, the
`application_name` and transaction-control words; no parameter values, no
result rows, no SQL text of ordinary statements, no authentication data (the
cluster uses trust; `p` frames are never recorded), no cancel key. In the 11
final reports the request id `req-1` appears 0 times in any `wireLog`
(it appears in the statement and table sections, which hold the
application's own test data). Test: `the_wire_log_names_events_but_carries_no_payloads`.

## Driver coverage

| driver | in-flight on original code |
| --- | --- |
| node-postgres `pg` 8.23.1 via drizzle-orm 0.45.4 | **covered** (above) |
| postgres-js | **not supported**: no proxy integration; its in-flight scenarios are not reproduced. Its Phase 8 crash/retry scenarios still reproduce (`pgjs-*` rows of `reports/phase9/final/phase8-regression.log`) |

## Mismatches and inconclusive results

None in the final run, the pre-fix run or the interrupted run. The
`LostCommitted`/`LostNotCommitted` fates (live process, broken connection)
have no runtime realization and are refused as UNSUPPORTED, never
approximated; no scenario in the evidence set needs them.

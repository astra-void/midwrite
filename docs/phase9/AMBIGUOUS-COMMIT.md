# Ambiguous COMMIT

```ts
await db.transaction(async (tx) => { await tx.insert(payments).values(payment); });
await publishConfirmation();
```

The process may stop after sending COMMIT and before it receives the result.
At that moment two physical histories are possible and the client cannot tell
them apart:

| | history A | history B |
| --- | --- | --- |
| COMMIT | never reaches the server (or is rolled back) | reaches the server |
| PostgreSQL | transaction rolled back | commits — **at some later point, possibly after the restarted client acted** |
| model | `LostNotCommitted` / `CrashInFlight` bit 0 | `LostCommitted` / `CrashInFlight` bit 1, then `ServerComplete` |
| runtime fate | `not-reached` (the proxy withholds the COMMIT and closes the server connection) | `answered-and-withheld` (the server completes it, the answer is withheld) **or** `held-for-later-delivery` (the COMMIT is delivered after the retry has run) |

Absence of an acknowledgment is not read as rollback anywhere in the model,
the planner or the report.

## What each scenario shows

All scenarios are first-party (`fixtures/phase9/`); "model" is the explicit-state
search (`fixtures/phase9/model-scenarios/` for declared programs,
`fixtures/phase9/scenarios/` for the TypeScript of `ledger-app`); "original
code" is a run of that TypeScript against PostgreSQL 17.11 through the proxy
(`--verify-source --allow-execution`, 2 runs per test).

### A — COMMIT succeeds, acknowledgment lost

`ledger-blind-commit-in-flight` (`recordPayment`: transaction insert, commit,
publish a confirmation; a client resubmits after a crash).

* model (`--fault-mode in-flight`): `begin, insert, crash-in-flight(COMMIT
  reaches the server), server-complete, retry, begin, insert, commit, publish`
  — two payments for one request (`recorded-at-most-once` violated). Twin
  analysis: in the other world the invariant holds; the application's
  observations are identical in both for the rest of the schedule ("it can
  never learn which one it is in").
* original code: `ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED`, 2 of 2 runs:
  proxy saw `COMMIT`, PostgreSQL answered `CommandComplete COMMIT` (observed),
  the answer was withheld, the worker was killed by SIGKILL (new pid for the
  retry), the durable rows right after the kill were 1 payment (read from
  PostgreSQL), 2 at the end. Compare the Phase 8 checkpoint: the COMMIT was
  already *acknowledged* when the process was killed.

### B — COMMIT does not succeed

The same code with fate `not-reached` (`ledger-idempotent-commit-not-reached`
probe, same bounds): the proxy withheld the COMMIT (backend `idle in
transaction`, observed), the worker was killed, PostgreSQL ended the session
and rolled back (`sessionsEnded`, 15–50 ms), the durable ledger was empty,
the idempotent retry recorded the payment once. Not conflated with A: the fate
is a field of the evidence and the run diverges if the established fate
differs from the model's.

### C — in-flight autocommit

* `ledger-autocommit-blind-in-flight`: an autocommit `INSERT` whose answer is
  withheld; the row is durable, the retry duplicates it —
  `ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED`.
* `ledger-settle-update-answered` / `-not-reached`: two autocommit writes (the
  ledger row, then `UPDATE orders SET status = 'paid'`), the process killed
  with the `UPDATE` in flight, no retry. The durable `orders.status` after the
  kill is `paid` when the server answered and `unpaid` when it never received
  the statement — *the update does not roll back because the client was
  killed*, and the client can see neither.

### D, E

Shared-process crash and pool starvation: `PROCESS-AND-TASK-MODEL.md`,
`CONNECTION-POOL.md`.

## Recovery strategies

Each is evaluated within its stated assumptions: a bounded model of one
client (and, where said, a concurrent second delivery) with at most one
lost acknowledgment and one crash.

| strategy | result | why |
| --- | --- | --- |
| **blind retry** (fresh row per attempt) | unsafe — model, original code | the COMMIT may have completed; the retry inserts again |
| **query durable state first** (`recordPaymentReconciled`) | **unsafe** — model, original code (`held-for-later-delivery`) | the read finds nothing because the server has not completed the COMMIT *yet*; the retry inserts; the server then completes the held COMMIT. The read does not establish "the first attempt will never commit". Both reads in the run returned `no row`. |
| **idempotency key**: `UNIQUE(request_id)` + `INSERT … ON CONFLICT DO NOTHING` | safe within the bounds, in every fate — model, original code probes (`not-reached`, `answered`, `held`) | the database serializes the two inserts whichever order the server's completion has |
| **lookup + the same unique key** (concurrent second delivery) | safe — model (305 states) | the lookup is an optimization; the constraint is what keeps the invariant |
| **lookup only, with a concurrent second delivery** | unsafe even *without* ambiguity — model | a plain check-then-act race (`ambiguous-commit-lookup-then-insert-concurrent`) |
| **transactional outbox** (Phase 8 `shipping-outbox-no-loss`) | still safe with in-flight faults (72 states vs 29) — model | state change and event are one transaction |
| **claim row, then effect** (Phase 8 `job-claimed-effect-redelivery`) | no violation of its at-most-once-effect invariant (90 vs 21 states) — model | the unique claim decides |
| **idempotent provider key** (Phase 8 `checkout-idempotent-crash-after-charge`) | still safe (83 vs 30 states) — model, under the *declared* `byKey` contract of the provider double | an assumption about the provider |

Phase 8's unsafe scenarios (`checkout-crash-after-charge`,
`job-redelivery-duplicate`, `shipping-outbox-relay-duplicate`) stay unsafe,
and their counterexamples now include the in-flight form when the bound is
on: `job-redelivery-duplicate` is found as `ack-lost-committed, retry,
server-complete`.

A recovery strategy is not safe because it performs an extra read: the row 
"query durable state first" is the counter-example, and the reason is a
property of the server (the COMMIT is still on its way), not of the query.

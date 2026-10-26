# Original-code crash and recovery verification

First-party applications written for this phase; their business logic is
ordinary TypeScript on Drizzle ORM, and the harnesses import it unchanged.
Every run executes the **original** functions in worker processes against a
disposable PostgreSQL, with real SIGKILLs and new processes for retries
([`CRASH-RETRY-SEMANTICS.md`](CRASH-RETRY-SEMANTICS.md)).

* `fixtures/phase8/shop-app` (node-postgres 8.23.1, Drizzle 0.45.4):
  checkout (`src/app/checkout.ts`), shipping with dual write and
  transactional outbox + relay (`shipping.ts`), a queue consumer
  (`jobs.ts`), stock reservations with an application-level transaction
  retry helper (`inventory.ts`, `lib/db/retry.ts`). Provider seams for the
  payment provider, the event bus, the mailer and the warehouse
  (`lib/payments/provider.ts`, `lib/messaging/bus.ts`); the harness
  (`verify/harness.ts`) implements them with coordinator-hosted test doubles.
* `fixtures/phase8/pgjs-app` (postgres-js 3.4.5, Drizzle 0.45.4): the
  team-app actions, the checkout and the reservations unchanged, with the
  client created at module load from `POSTGRES_URL` as in saas-starter.
* `fixtures/phase7/team-app` (Phase 7 application): lock waits with crashes.

Test doubles' guarantees are **declared contracts**, not proofs about
Stripe, a queue or a mail provider.

## Scenario A — payment accepted, response lost

`checkout-crash-after-charge` (vulnerable `payOrder`): select order, insert
an attempt, `chargeCard` (no idempotency key), **crash before** `UPDATE
orders SET status = 'paid'` (announced, never sent), the client retries
(declared `onCrash`), the retry charges again. Model: counterexample (9
transitions). Original code: the worker was SIGKILLed with the UPDATE held;
PostgreSQL ended its session (≈ 18 ms); the committed rows after the crash
were exactly the model's (order still `unpaid`, one attempt); a new process
(new pid, attempt 2) charged again; final effect log `card-charge ×2`.
**`ORIGINAL_CODE_CRASH_REPRODUCED`, 5/5.**

`checkout-idempotent-crash-after-charge` (`payOrderIdempotent`: request id
recorded with `ON CONFLICT DO NOTHING` and sent as idempotency key; provider
contract `byKey`): no counterexample within bounds; the recovery probe
(charge → crash → retry → charge) ran on the original code: the retry's
insert affected 0 rows, the second charge was deduplicated by the double,
one effective charge. **`ORIGINAL_CODE_RECOVERY_REPRODUCED`, 5/5.**

`checkout-idempotent-two-requests` — found by the Phase 8 model benchmark,
then verified: two concurrent requests with **different** request ids for
the same order are both charged; the key protects one request, not the
order. **Counterexample, original code reproduced 5/5.** The idempotent
variant is safe *for the declared schedule class* (one request, a crash, a
retry), not in general.

## Scenario B — commit, then publish

| scenario | result | runs |
| --- | --- | --- |
| `shipping-dual-write-crash` | shipped status committed, crash before `publishEvent` (held, never sent); no recovery mechanism is declared, none is invented: the event is lost | `ORIGINAL_CODE_CRASH_REPRODUCED` 5/5 |
| `shipping-outbox-relay-duplicate` | outbox row written in the same transaction; the relay publishes (message id = row id), crashes before marking the row published; its supervisor (declared, guaranteed) restarts it; the event is published twice — **at-least-once**, not exactly-once | `ORIGINAL_CODE_CRASH_REPRODUCED` 5/5 |
| `shipping-outbox-no-loss` | same schedule class: no shipped event is lost (published, or pending in the outbox) | `ORIGINAL_CODE_RECOVERY_REPRODUCED` 5/5 |

The relay's retry ran in the harness (a new process), so the modeled
recovery mechanism actually executed. No eventual-delivery claim is made:
a guaranteed retry is a declared fairness assumption bounded by
`maxRetries`.

## Scenario C — job redelivery

| scenario | result | runs |
| --- | --- | --- |
| `job-redelivery-duplicate` | consumer sends the receipt, crashes before acknowledging (`UPDATE jobs ... done` held); the queue redelivers (declared at-least-once): two receipts | `ORIGINAL_CODE_CRASH_REPRODUCED` 5/5 |
| `job-concurrent-claims-duplicate` | the same job delivered to two consumers concurrently: both pass the status check, two receipts | `original-code-reproduced` 5/5 |
| `job-claimed-effect-redelivery` | claim row with `UNIQUE(job_id, effect)` first; crash after the e-mail, redelivery finds the claim and skips: at most one receipt | `ORIGINAL_CODE_RECOVERY_REPRODUCED` 5/5 |
| `job-claimed-effect-concurrent` | two concurrent consumers: only the one whose claim insert succeeded sends | `ORIGINAL_CODE_INVARIANT_HELD` 5/5 |

At-least-once delivery and exactly-once effects are kept apart: the claim
makes the effect at-most-once (a crash between claim and e-mail loses the
receipt), not exactly-once.

## Scenario D — retry after a serialization failure

`withTransactionRetry` runs the work in a new transaction after SQLSTATE
40001/40P01 (`attempts` total); bound in scenarios with
`extraction.transactionRetries`, so the model's `Begin { retry }` predicts
the loop — the loop itself is the application's code and **runs** in the
verification.

| scenario | result | runs |
| --- | --- | --- |
| `inventory-retry-duplicate-notice` | REPEATABLE READ; R2's UPDATE fails with a **real 40001**, Drizzle rolls back, the helper begins a fresh transaction, R2 commits; the warehouse notification sent inside the failed transaction is **not undone** by the rollback: 2 reservations, 3 notices | `original-code-reproduced` 5/5 |
| `inventory-notify-after-commit` | notification after commit: one notice per reservation through the same real 40001 and retry | `ORIGINAL_CODE_INVARIANT_HELD` 5/5 |
| `inventory-retry-exhausted` | `attempts: 1`: the 40001 is final, R2 fails (`threw`), its changes never commit, stock accounted | `ORIGINAL_CODE_INVARIANT_HELD` 5/5 |

The retried BEGIN is attributed to the modeled call site even though the
helper is called with `return withTransactionRetry(...)` (no `await`),
across which V8 loses the caller's frame: the worker restores the chain from
an earlier stack of the same execution when it is unambiguous, and the
report says so ("call chain restored across an un-awaited return").

## Lock waits and crashes

| scenario | result | runs |
| --- | --- | --- |
| `accept-crash-while-waiting` | Accept2's UPDATE waits for Accept1's row lock (confirmed in `pg_stat_activity`), Accept2 is killed **while waiting**; its session ends (≈ 47 ms with `client_connection_check_interval`), Accept1 commits | `ORIGINAL_CODE_RECOVERY_REPRODUCED` 5/5 |
| `accept-crash-lock-holder` | Accept1 holds the lock, is killed before COMMIT (held); PostgreSQL rolls it back, Accept2's waiting UPDATE proceeds and commits | `ORIGINAL_CODE_RECOVERY_REPRODUCED` 5/5 |

## postgres-js

`pgjs-*` repeat the checkout crash, the invite double submit, the accept
race, both lock-wait crashes and the 40001 retry on postgres-js; all
reproduce 5/5 ([`POSTGRES-JS-ADAPTER.md`](POSTGRES-JS-ADAPTER.md)).

## Totals

21 scenarios, 5 runs each on PostgreSQL 17.11 (macOS): 105 original-code
runs, all followed the plan and matched the model
(`reports/phase8/model-check/SUMMARY.md`); 3 runs each on PostgreSQL 15.19
in the Linux container: 63 runs, all followed and matched, with the same
evidence level per scenario (`reports/phase8/linux/evidence.txt`). Negative
tests: a harness input the model does not expect diverges at the crash
point and is never reported as reproduced; nothing runs without
`--allow-execution`; no retry exists without a declared mechanism.

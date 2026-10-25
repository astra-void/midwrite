# Crashes, retries and idempotency

## Crash transitions

`Crash(process)` is enabled while some actor of the process is running and
has performed an operation, at most `maxCrashes` times per schedule, at
**every boundary between transitions** — before a statement, while a
statement waits, after `COMMIT` and before the next operation, after an
external effect.

| State | After a crash |
| --- | --- |
| Committed database state | unchanged (a crash never rolls back a commit) |
| Uncommitted transaction state (open transactions, waiting statements) | rolled back; locks released (the server notices the lost connection) |
| Performed external effects | remain in the log (never undone) |
| In-flight external effect | an effect is a single transition: a crash *before* it means it did not happen, *after* it that it happened and the actor never learned (equivalent to a lost acknowledgement) |
| Local process memory | lost: the actor's locals and tasks; in-process variables of the process reset |
| Pending jobs | other actors are unaffected; a crashed actor runs again only through a retry |

A crash of a process stops all its actors (`process` in the scenario). An
unhandled rejection of an unawaited task also crashes the process (not
counted against `maxCrashes`). Validated on PostgreSQL by killing the
session's process and waiting for its backend to exit
(`crash-after-commit`, `crash-then-client-retry`: 11 schedules agree).

## Retry mechanisms (not interchangeable)

| Mechanism | Model |
| --- | --- |
| **Re-executing a transaction** | `Begin { retry: n }` (DSL `begin … retry n`): after 40001/40P01 in the transaction, the main task restarts at `BEGIN` with the locals saved at `BEGIN`, up to *n* times, counted against `maxRetries`. Deterministic: part of the failing transition. Tested on PostgreSQL (`tx-retry`, 50 schedules). |
| **Re-executing the whole operation** (client retry, scheduled job re-run) | actor `retry: { "trigger": "onFailure" \| "onCrash" \| "onFailureOrCrash" }`: a `Retry` transition starts a new attempt from the beginning with the original arguments, once all tasks of the previous attempt ended. |
| **Queue redelivery** (at-least-once) | `"trigger": "atLeastOnce"`: also after a *successful* attempt (lost acknowledgement). Exactly-once execution is never assumed. |
| **Retrying a single failed SQL statement** | not supported (no IR form); a scenario cannot express it. |
| **Scheduled reconciliation** | not built in; model it as its own actor with a retry policy. |

Retries are **optional** environment choices (the client may also give up)
unless declared `"guaranteed": true` (a queue that redelivers until
acknowledged): then a state in which the retry is still enabled is not
quiescent, and final invariants are evaluated only after it ran (or
`maxRetries` is exhausted). Every retry and crash consumes the per-schedule
budgets; both appear in the witness.

## Idempotency

An idempotency key is **not sufficient by name**. External effects carry the
key expression found in source (`extraction.externals[].keyArgument`) or
declared; the scenario must state who enforces it:

* `channels.<name>.dedup: "none"` — the provider ignores keys: every call is
  a visible effect;
* `"byKey"` — the provider enforces uniqueness of the key: a repeated key is
  logged but marked deduplicated (an *assumption about the provider*,
  listed in the report);
* database-enforced idempotency (an idempotency-key row with a unique
  constraint, `ON CONFLICT DO NOTHING`) is modeled by the database semantics
  themselves.

Example (`fixtures/concurrency/scenarios/payment-crash-retry*.json`, code in
`fixtures/concurrency/project/src/payments.ts`): the payment is charged,
the server crashes before recording it, the client retries. With `dedup:
none` the model finds a double charge in 5 transitions; with `byKey` no
violation exists within the bounds — under that declared assumption only.

## State-space effect of each dimension

Measured in [`PERFORMANCE.md`](PERFORMANCE.md) (states and transitions for
the same workload with 0/1/2 crashes and 0/1/2 retries, with and without
reduction).

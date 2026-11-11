# Failure model

Midwrite asks one question in two settings: **what state is left when a
process, connection or call fails between two side effects that are durable
or visible independently of each other?** The static analyzer answers it
for one function at a time over its paths; the model checker answers it for
bounded concurrent executions with explicit faults.

## Static analyzer (`check`, `trace`)

A function's operations are replayed along each path with a stack of open
transactions. Two kinds of **points** matter:

* a **commit**: a write on the root database handle (PostgreSQL autocommit),
  or the successful end of a root transaction that staged writes;
* a **visible** effect: a configured external effect (message publish,
  HTTP request, email, payment, file write, other).

A failure between two *adjacent* points leaves the first done and the
second not. Each adjacent pair is classified:

| first → second | code |
| --- | --- |
| commit → external effect | `MW1001` |
| external effect → commit | `MW1002` |
| external effect while a transaction with writes is open | `MW1003` |
| commit → commit (no shared transaction, or `db` used inside a `tx` callback) | `MW1004` |
| external effect → external effect | `MW1005` |

Transaction semantics used:

* a write outside a recognized transaction commits when it executes;
* a write on the handle of `db.transaction(async (tx) => …)` is staged and
  becomes durable only when the callback completes normally; an exception,
  `tx.rollback()` or a `throw` rolls it back;
* a nested `tx.transaction(…)` is a savepoint that commits with the
  outermost transaction;
* a write on `db` inside a transaction callback runs on another connection
  and commits on its own.

Every awaited operation may fail; the crash simulation in a trace models a
process crash only (exception handlers are not executed in the
simulation). Unknown calls are never assumed to be effects or to be safe:
they become `MW9001` when they sit next to recognized effects. Details:
[`docs/ARCHITECTURE.md` § failure-path analysis](../ARCHITECTURE.md#failure-path-analysis-midwrite-coreanalysis),
[trace format](../reference/trace-format.md).

## Model checker (`model-check`, `discover --model-check`)

Faults are explicit, bounded transitions of the model
([model semantics](model-semantics.md#transitions)):

| fault | model | bounded by | runtime realization against original code |
| --- | --- | --- | --- |
| process crash at a transition boundary | the process's tasks stop; uncommitted work rolls back; committed data stays | `maxCrashes` | SIGKILL at a verified checkpoint |
| retry / redelivery | an operation or whole transaction runs again (`onCrash`, at-least-once delivery, transaction retry after 40001/40P01); optional unless declared guaranteed | `maxRetries` | a new process per attempt |
| lost acknowledgment of a COMMIT or autocommit write | an in-flight operation becomes an orphan the server may complete before, during or after recovery (`ServerComplete`); the client's knowledge is tracked separately from the physical state | `maxInFlight`, `--fault-mode` | protocol proxy: not-reached, answered-and-withheld, held-for-later-delivery |
| broken connection under a live process | `LostCommitted` / `LostNotCommitted` | `maxInFlight` | closed socket for a COMMIT or autocommit write |
| crash after the last statement, before the response | the response is a separate visible step for actors with `response: true` | `maxCrashes` | response checkpoint |
| deadlock | a `Deadlock` transition aborts a member of the cycle (any member: over-approximation of PostgreSQL's timer race) | — | covered by the model-SQL replay matrix, not by original code |
| connection-pool exhaustion | bounded FIFO pools; stuck states classified as `PoolStarvation`, `ApplicationWaitCycle` or `Unexplained` | pool capacity | pool counters and quiescence at runtime |

Runtime realizations: [runtime verification](../architecture/runtime-verification.md),
[fault injection](../architecture/fault-injection.md).

Not modeled: server crashes that lose committed data (committed data is
durable), partial statements or partially performed effects (equivalent to
"before" or "after"), timeouts (`statement_timeout`,
`idle_in_transaction_session_timeout`, pool acquisition timeouts), network
delay, reordering, duplication or partial frames, pipelined statements,
synchronous replication.

### Crashes

A crash is enabled at **every boundary between transitions** — before a
statement, while a statement waits, after `COMMIT` and before the next
operation, after an external effect — while some task of the process has
started, at most `maxCrashes` times per schedule.

| after a crash | |
| --- | --- |
| committed rows | unchanged (a crash never rolls back a commit) |
| open transactions, waiting statements | rolled back; locks released (the server notices the lost connection; the model releases at once) |
| performed external effects | remain (killing a process does not cancel a charge or a publication) |
| an external effect | a single transition: a crash before it means it did not happen, after it that it happened and the actor never learned |
| an operation on the wire | resolved per the crash's in-flight mask (below) |
| locals, tasks, in-process variables, the process's pool | lost / reset / emptied |
| other actors | unaffected; a crashed actor runs again only through a retry |

A crash stops every actor of the process. An unhandled rejection of an
unawaited task also crashes the process (not counted against `maxCrashes`).

### Retries (not interchangeable)

| mechanism | model |
| --- | --- |
| re-executing a transaction | `Begin { retry: n }` (DSL `begin … retry n`): after 40001/40P01 the main task restarts at `BEGIN` with the locals saved there, up to *n* times, within `maxRetries`; part of the failing transition |
| re-executing the whole operation (client retry, job re-run) | actor `retry: { "trigger": "onFailure" \| "onCrash" \| "onFailureOrCrash" }`: a `Retry` transition starts a new attempt with the original arguments once every task of the previous attempt ended |
| queue redelivery (at-least-once) | `"trigger": "atLeastOnce"`: also after a *successful* attempt (lost acknowledgment); exactly-once execution is never assumed |
| a retry loop written in the application | unrolled source code ([loops and caught errors](../architecture/loops-and-errors.md)) |
| retrying a single failed SQL statement | not expressible |
| scheduled reconciliation | not built in: model it as its own actor |

Retries are optional environment choices (the client may also give up)
unless declared `"guaranteed": true` — then a state in which the retry is
still enabled is not quiescent, and final invariants are evaluated only
after it ran or the budget is exhausted. A retry of an actor that shares a
process with others cannot be realized at runtime (it needs a new process)
and is refused for original-code verification.

### Idempotency

An idempotency key is **not sufficient by name**. External effects carry the
key expression found in source or declared; the scenario states who
enforces it: `channels.<name>.dedup: "none"` (every call is a visible
effect) or `"byKey"` (the provider deduplicates; an assumption about the
provider, listed in the report). Database-enforced idempotency (a unique
key and `ON CONFLICT DO NOTHING`) is modeled by the database semantics
themselves.

## Lost acknowledgments

An operation whose acknowledgment can be lost is a COMMIT or a write outside
a transaction (autocommit). Reads and statements inside a transaction have
no durable outcome of their own; a lost answer there ends the transaction
with the connection, which a crash or failure already represents. An
explicit `ROLLBACK` has the same outcome either way, and multi-row
autocommit statements are not put in flight.

```text
Prepared ─► Submitted ─► ServerReceived ─► Executing ─► ServerCompleted ─► ResponsePending ─► ClientAcknowledged
   └───────────┴──────────────┴─────────────────┴───────────────┴──────────────────┴──► OutcomeUnknown
```

| stage | in the model | at runtime |
| --- | --- | --- |
| Prepared | the task's next visible operation | the worker announces the operation — observed |
| Submitted | a `LostX` / `CrashInFlight` transition | the proxy saw the frames — observed |
| ServerReceived | an orphan exists, or the operation executed | forwarded frames, `idle in transaction` — observed; a *held* COMMIT is observed as not yet forwarded |
| ServerCompleted | `ServerComplete` | the server's answer frame; durable rows read from PostgreSQL |
| ResponsePending | the answer exists, undelivered | the proxy withholds it — observed |
| ClientAcknowledged | the task continues | the worker's result |
| OutcomeUnknown | an ambiguity in the actor's knowledge | **inferred** (the application sent and got nothing) — never reported as observed |

Two physical histories remain after a lost acknowledgment, and the client
cannot tell them apart:

| | not committed | committed |
| --- | --- | --- |
| COMMIT | never reaches the server (or is rolled back) | reaches the server |
| PostgreSQL | rolled back | commits — **at some later point, possibly after the restarted client acted** |
| model | `LostNotCommitted` / mask bit 0 | `LostCommitted` / mask bit 1, then `ServerComplete` |
| runtime fate | `not-reached` | `answered-and-withheld`, or `held-for-later-delivery` (the COMMIT is delivered after the retry ran) |

Absence of an acknowledgment is never read as rollback. The pending
`ServerComplete` exists because of the **reconciliation hazard**: a client
that, after a crash, reads the table and inserts only when it finds nothing
can still be raced by the COMMIT the server has not completed yet. If
"reached the server" meant "committed at once", that interleaving — which
PostgreSQL permits — would be missing and the reconciliation would look
safe. A `ServerComplete` of an autocommit statement is enabled only when the
statement would not wait; whether a backend completes a *blocked* statement
of a client that died is not modeled. Orphans complete atomically;
server-side timeouts are not modeled beyond "the server may complete at any
later time".

What recovery strategies give under this model (one client, and where said a
concurrent second delivery; at most one lost acknowledgment and one crash):

| strategy | result | why |
| --- | --- | --- |
| blind retry (a fresh row per attempt) | unsafe — model and original code | the COMMIT may have completed; the retry inserts again |
| query durable state first, then retry | **unsafe** — model and original code (`held-for-later-delivery`) | the read finds nothing because the server has not completed the COMMIT *yet* |
| idempotency key: a unique request id + `INSERT … ON CONFLICT DO NOTHING` | safe within the bounds in every fate | the database serializes the two inserts whatever order the server's completion takes |
| lookup + the same unique key, concurrent second delivery | safe — model | the constraint, not the lookup, keeps the invariant |
| lookup only, concurrent second delivery | unsafe even without ambiguity | a plain check-then-act race |
| transactional outbox; claim row then effect | safe — model | state change and event (or claim) commit together |
| idempotent provider key | safe — model, under the declared `byKey` contract | an assumption about the provider |

A recovery strategy is not made safe by an extra read: the hazard is a
property of the server (the COMMIT is still on its way), not of the query.
Fixtures: [`fixtures/model/in-flight/`](../../fixtures/model/in-flight/) (declared
programs), [`fixtures/scenarios/in-flight/`](../../fixtures/scenarios/in-flight/)
and [`fixtures/scenarios/connection-loss/`](../../fixtures/scenarios/connection-loss/)
(TypeScript of [`fixtures/apps/ledger-app`](../../fixtures/apps/ledger-app/) and
its live variants).

## Physical state vs knowledge

The model keeps apart what happened (the store, which processes run,
whether the response was sent) and what a client can know (the completion
and its return value). A request whose `INSERT` committed and whose
response was lost is a crashed request with the row committed — never a
failed transaction. States that differ only in unresolved outcomes are not
merged, and knowledge is kept per attempt. For a counterexample the report
explains, per lost acknowledgment, whether the application could ever tell
the worlds apart and whether the invariant differs between them (twin
analysis, [model semantics](model-semantics.md#possible-worlds-and-knowledge)).

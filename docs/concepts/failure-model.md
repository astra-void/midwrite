# Failure model

CommitRift asks one question in two settings: **what state is left when a
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
| commit → external effect | `CP1001` |
| external effect → commit | `CP1002` |
| external effect while a transaction with writes is open | `CP1003` |
| commit → commit (no shared transaction, or `db` used inside a `tx` callback) | `CP1004` |
| external effect → external effect | `CP1005` |

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
they become `CP9001` when they sit next to recognized effects. Details:
[`docs/ARCHITECTURE.md` § failure-path analysis](../ARCHITECTURE.md#failure-path-analysis-commitrift-coreanalysis),
[Phase 3 trace format](../phase3/TRACE-FORMAT.md).

## Model checker (`model-check`, `discover --model-check`)

Faults are explicit, bounded transitions of the model:

| fault | model | bounded by | runtime realization against original code |
| --- | --- | --- | --- |
| process crash at a transition boundary | the process's tasks stop; uncommitted work rolls back; committed data stays | `maxCrashes` | SIGKILL at a verified checkpoint (Phase 8) |
| retry / redelivery | an operation or whole transaction runs again (`onCrash`, at-least-once delivery, transaction retry after 40001/40P01); optional unless declared guaranteed | `maxRetries` | a new process per attempt (Phase 8) |
| lost acknowledgment of a COMMIT or autocommit write | an in-flight operation becomes an orphan the server may complete before, during or after recovery (`ServerComplete`); the client's knowledge is tracked separately from the physical state | `maxInFlight`, `--fault-mode` | protocol proxy: not-reached, answered-and-withheld, held-for-later-delivery (Phase 9) |
| broken connection under a live process | `LostCommitted` / `LostNotCommitted` | `maxInFlight` | closed socket for a COMMIT or autocommit write (Phase 10) |
| crash after the last statement, before the response | the response is a separate visible step for actors with `response: true` | `maxCrashes` | response checkpoint (Phase 13) |
| deadlock | a `Deadlock` transition aborts a member of the cycle (any member: over-approximation of PostgreSQL's timer race) | — | covered by the model-SQL replay matrix, not by original code |
| connection-pool exhaustion | bounded FIFO pools; stuck states classified as `PoolStarvation`, `ApplicationWaitCycle` or `Unexplained` | pool capacity | pool of 1 confirmed at runtime (Phase 9) |

Not modeled: server crashes that lose committed data (committed data is
durable), partial statements or partially performed effects (equivalent to
"before" or "after"), timeouts (`statement_timeout`,
`idle_in_transaction_session_timeout`), network delay, reordering,
duplication or partial frames, synchronous replication.

Sources: [Phase 6 failure and retry](../phase6/FAILURE-AND-RETRY.md),
[Phase 8 crash/retry semantics](../phase8/CRASH-RETRY-SEMANTICS.md),
[Phase 9 epistemic semantics](../phase9/EPISTEMIC-SEMANTICS.md),
[Phase 9 operation lifecycle](../phase9/OPERATION-LIFECYCLE.md),
[Phase 10 live connection failures](../phase10/LIVE-CONNECTION-FAILURES.md),
[Phase 13 return values and responses](../phase13/RETURN-VALUES-AND-RESPONSES.md).

## Physical state vs knowledge

Since Phase 9 the model keeps apart what happened (the store, which
processes run, whether the response was sent) and what a client can know
(the completion and its return value). A request whose `INSERT` committed
and whose response was lost is a crashed request with the row committed —
never a failed transaction. States that differ only in unresolved outcomes
are not merged. See [Phase 9 epistemic semantics](../phase9/EPISTEMIC-SEMANTICS.md)
and [Phase 13 § 4](../phase13/RETURN-VALUES-AND-RESPONSES.md#4-epistemic-separation-b7).

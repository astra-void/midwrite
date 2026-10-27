# Live-process connection failures

Phase 9 modeled `LostCommitted` / `LostNotCommitted` (the connection breaks
while the application process lives) but refused to run them against
original code: only a process kill was realized. Phase 10 realizes them.

## Fault model

```text
Node.js process: alive throughout ───────────────────────────────────────────►
  COMMIT (or an autocommit write) submitted
      │ proxy applies the fate on the wire:
      │   answered-and-withheld  forward it, observe the server's answer, withhold it
      │   not-reached            withhold it from the server
      │   held-for-later         withhold it, keep the server connection open
      ▼
  proxy closes the CLIENT's socket (`FaultProxy::sever_client`) — nothing is
  written to the client first, no signal is sent to the process
      ▼
  driver error → application code (try/catch) → new connection → retry
      ▼
  server side: answered → already durable, session ends; not-reached → proxy
  ends the server connection, PostgreSQL rolls back; held → session stays
  `idle in transaction` until a later `ServerComplete` delivers the COMMIT
```

The process lifecycle and the connection lifecycle are separate facts and are
established separately: the proxy's own events for the wire, the operating
system (`waitid(..., WNOWAIT)` on the worker) for the process,
`pg_stat_activity` and the durable rows for PostgreSQL, the worker's report
for the driver's error.

## Plan and evidence

`source_verify.rs::plan` turns a `LostCommitted`/`LostNotCommitted` transition
into `PlanStep::ConnectionLoss` (fate: not-reached; answered-and-withheld when
the model's next transition is the server completing that operation; held
otherwise, with a later `ServerComplete` step). Only a COMMIT or an autocommit
write qualifies; the actor must have one running task. Drizzle's `ROLLBACK`
after the failure goes to the broken client; node-postgres rejects it without
sending it, and the worker reports such statements as `dead-connection`
(recognized by node-postgres's own state, `_queryable`/`_connectionError`/
`_ending`, verified on pg 8.23.1) so they are never held and never matched
against the plan.

Per loss the report records (`sourceVerification.recovery.losses[]`, schema 5):
model and observed fate, mechanism, forwarded/withheld, the server's answer as
seen on the wire, `clientSevered`, the worker pid and `workerAliveAfter`
(operating system), the **driver's error quoted as reported** (run directory
redacted), the backend's state before the break, whether its session ended or
stays open, the statements issued on the broken client, the backends of the
connections used afterwards, the redacted wire events, the application's
knowledge and the physical outcome (kept apart), and the durable rows compared
with the model's.

New evidence level: `original-code-connection-loss-reproduced`
(`ORIGINAL_CODE_CONNECTION_LOSS_REPRODUCED`): the run followed the plan, every
loss check agreed (process alive, driver error observed, fate, server state,
durable rows), every statement and outcome matched, and the invariant is
violated on the observed state. A process that dies when the model predicts it
lives is a **divergence** (`DIVERGED`), with the worker's stderr quoted.

## Observed driver behavior

| | node-postgres 8.23.1 (Drizzle 0.45.4) | postgres-js 3.4.5 (Drizzle 0.45.4) |
| --- | --- | --- |
| error the application gets | `Connection terminated unexpectedly` | `write CONNECTION_CLOSED <replay-root>/px/.s.PGSQL.5432` |
| SQLSTATE | none | none |
| a checked-out client's `error` event | emitted; **unhandled, it ends the Node process** (`Unhandled 'error' event`) | not applicable |
| Drizzle's ROLLBACK after the failure | sent to the broken client, rejected without reaching the wire: `Client has encountered a connection error and is not queryable` | none observed |
| connection for the retry | the pool discards the broken client and opens a new one (a new backend pid; also with a pool of one) | a new connection (postgres-js does not expose the backend pid to the worker) |
| process shutdown | normal | `sql.end()` without a timeout did not return through the proxy, with or without a broken connection (not with the Phase 8 harness, which has no proxy); the harness uses `end({ timeout: 5 })` and records when it was needed |
| fault on an autocommit statement | the INSERT | the INSERT; the proxy initially armed postgres-js's own `pg_type` catalog read on the new connection — statement faults now target only data-modifying statements |

These are observations of these versions in these runs, not documented
guarantees of the drivers.

### The unhandled `error` event

The first run of scenario A failed in an informative way: the worker died.
Its stderr: `Unhandled 'error' event … Error: Connection terminated unexpectedly
at Connection.<anonymous> (pg/lib/client.js:204:73)`. node-postgres emits
`error` on a checked-out client whose connection ends, and neither pg-pool nor
Drizzle's `transaction()` attaches a listener while the client is checked out.
An application whose entry point does not attach one cannot run its recovery
code at all: its process ends. The Phase 10 node-postgres harness attaches a
logging listener on every new client (`verify/harness.ts`); the scenario
`live-blind-retry-unhandled-error-event` runs the same code without it and is
expected to — and does — **diverge**, quoting the stderr.

## Scenarios (fixtures/phase10/scenarios)

| scenario | driver | what it shows |
| --- | --- | --- |
| A `live-blind-retry-lost-commit` | node-postgres | COMMIT durable, connection lost, the in-process blind retry records the payment twice |
| B `live-blind-retry-commit-not-durable` (probe) | node-postgres | COMMIT not durable, same error; the same retry is right here |
| C `live-autocommit-retry-lost-write` / `-write-not-reached` | node-postgres | an autocommit INSERT: both outcomes |
| D `live-idempotent-retry-lost-commit` / `-commit-not-durable` (probes) | node-postgres | a unique request key with `ON CONFLICT DO NOTHING` keeps one row in both outcomes |
| — `live-reconciled-retry-lost-commit` | node-postgres | look-then-insert after the error races the COMMIT the server still holds |
| E `live-blind-retry-pool-of-one` | node-postgres | a pool of one: the broken connection is replaced, no starvation |
| — `live-blind-retry-unhandled-error-event` | node-postgres | no client error handler: the process dies; DIVERGED |
| `pgjs-*` (6) | postgres-js | the same application code on the other driver |

Evidence (five runs each): `reports/phase10/evidence/runtime/SUMMARY.md`, see
`RUNTIME-VERIFICATION.md`.

## Not covered

* Errors other than a closed socket: timeouts, `57P01` admin shutdown, TLS
  errors, partial frames, a server that closes first.
* Statements other than a COMMIT or an autocommit write losing their
  connection (a broken connection in the middle of a transaction).
* Several tasks of one process sharing the broken pool (one running task per
  actor is required at the loss).
* Retry *of an actor* in a shared process (still refused, as in Phase 9).

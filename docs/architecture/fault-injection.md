# Fault injection against original code: in-flight operations and lost connections

How a model counterexample with a lost acknowledgment or a broken
connection becomes a run of the application's own code. Code:
`crates/midwrite-replay/src/proxy.rs` (protocol proxy), driven by
`crates/midwrite-replay/src/source.rs`; plans from
`crates/midwrite-model/src/source_verify.rs`. The surrounding coordinator,
crashes at checkpoints and retries are in
[runtime verification](runtime-verification.md); the fault model itself in
[failure model](../concepts/failure-model.md).

## Why a protocol proxy

A driver-level checkpoint holds a statement *before* the driver sends it;
that proves nothing about an operation that was sent. To exercise "the
COMMIT is on the wire and the application will never hear" the
coordinator needs control over the bytes between driver and server.
Server-side synchronization (hooks, `pg_sleep`, advisory locks) cannot
withhold a response or delay a COMMIT request without changing the
statement. So a test-only proxy forwards frames unchanged, withholds them,
and never edits them; the evidence is hybrid: proxy events say what
crossed the wire, `pg_stat_activity` and the durable rows say what
PostgreSQL did.

```text
worker (driver) ──► <run>/px/.s.PGSQL.5432 ──proxy──► <run>/pg/.s.PGSQL.5432 ──► PostgreSQL
```

* It starts only when the plan contains an in-flight or connection-loss
  step; other runs connect straight to the cluster.
* It refuses to start unless both sockets are inside the run directory,
  and the coordinator checks that the socket handed to the worker is
  inside it too. It never opens a TCP socket.
* It is not a PostgreSQL implementation: it parses message frames (tag,
  length, body) and recognizes only what a fault needs.

## Protocol subset

Frontend: `StartupMessage`; `SSLRequest`/`GSSENCRequest` answered `N`
(encryption is never negotiated; a client that requires it fails by
itself, fail-closed); `CancelRequest` forwarded and the connection closed;
simple query `Q`; extended protocol `P`, `B`, `E`, `D`, `C`, `S`, `H`;
authentication data `p`; `X`. Backend: `R`, `K`, `S`, `Z`, `C`, `E`, `N`,
`1`, `2`, `T`, `D`; the rest forwarded unparsed. COPY, function call
(`F`), frames over 8 MiB and invalid framing mark the connection
unsupported: the proxy keeps forwarding, injects nothing there, and the
run is INCONCLUSIVE — never a fabricated injection.

A statement is classified from its SQL text: plain `COMMIT`/`END`
(optionally `TRANSACTION`/`WORK`, `AND [NO] CHAIN`) is a commit (`COMMIT
PREPARED` is not); `BEGIN`/`START` and `ROLLBACK`/`ABORT` are recognized;
everything else is a statement. Prepared statement names and portals are
tracked, so an extended-protocol COMMIT is found at its
`Parse`/`Bind`/`Execute`. Statement faults target only data-modifying
statements (postgres-js reads `pg_type` on every new connection).

## Faults

Armed for the next matching operation of a connection (by the backend pid
the driver reports, else by `application_name`), one-shot:

| fault | on the wire | what it establishes |
| --- | --- | --- |
| drop response | the operation is forwarded; the server's answer and everything after it are withheld from the client | the server completed it (its answer was **observed**); the client did not hear |
| hold request | the operation's frames, through the next `Sync`, are not forwarded; the server connection stays open | the client sent it, the server did not receive it (`idle in transaction`, observed in `pg_stat_activity`) |
| deliver | the held frames (+ `Terminate`) are written to the kept server connection | the COMMIT arrives late, after the client died or retried |
| discard | the held frames are dropped and the server connection closed | the COMMIT never arrives; PostgreSQL rolls back |
| sever client | the client's socket is closed without writing anything to it; no signal is sent to the process | a live process sees a broken connection |

The same faults apply to the next autocommit write. A fault is verified,
never assumed: after releasing the operation the coordinator waits for the
proxy's own event (`OperationHeld` / `OperationCompleted`); if it does not
come, the run is INCONCLUSIVE ("no delay is taken as proof").

## From schedule to plan

| model transition | plan step |
| --- | --- |
| statement, BEGIN, COMMIT, effect | `Release` |
| `CrashInFlight` | one `InFlight` per actor of the process whose operation is in flight (fate from the model), `Crash` steps for the others (announced-and-held, waiting for a lock, or waiting for a pool connection) |
| `ServerComplete` right after a crash | folded into it: fate `answered-and-withheld` |
| `ServerComplete` later | the proxy delivers the held operation |
| `LostCommitted` / `LostNotCommitted` (live process) | `ConnectionLoss`: fate not-reached, answered-and-withheld (when the server completes it next), or held for later delivery |
| termination counterexample | the schedule, then `Stuck` |
| a `Retry` of an actor in a shared process | refused: UNSUPPORTED |

`model-check --fault-mode in-flight` restricts crashes to moments with a
COMMIT or autocommit write in flight (`bounds.crashPolicy:
"in-flight-only"`, needs `maxInFlight ≥ 1` and `maxCrashes ≥ 1`); the
default keeps every crash point and adds the in-flight ones.

**Process killed with an operation in flight.** The coordinator waits for
the operation to be announced and held, matches it with the model's,
arms the proxy, releases the operation onto the wire, waits for the
proxy's event, samples `pg_stat_activity`, and once every crash step of the
process is established SIGKILLs the process group **once**. Then per fate:
not-reached → discard, wait for the session to end; answered-and-withheld
→ wait for the session to end; held → check the backend is still `idle in
transaction` and keep the connection for the later `ServerComplete`. The
modeled tables are read after the kill and again after a late delivery.

**Connection broken under a live process.** Only a COMMIT or an
autocommit write qualifies, and the actor must have one running task. The
proxy applies the fate, then severs the client socket. The driver's error
reaches application code (`try`/`catch`), which may open a new connection
and retry. The process lifecycle and the connection lifecycle are
established separately: proxy events for the wire, the operating system
(`waitid(..., WNOWAIT)`) for the process, `pg_stat_activity` and durable
rows for PostgreSQL, the worker's report for the driver's error. Drizzle's
ROLLBACK after the failure goes to the broken client; node-postgres
rejects it without sending it, and the worker reports such statements as
`dead-connection` (recognized from node-postgres's own client state) so
they are never matched against the plan. A process that dies when the
model predicts it lives is a **divergence**, with the worker's stderr
quoted.

## Observed driver behavior

Observations of these versions in these runs, not documented guarantees:

| | node-postgres 8.23.1 + Drizzle 0.45.4 | postgres-js 3.4.5 + Drizzle 0.45.4 |
| --- | --- | --- |
| protocol | simple query for `BEGIN`/`COMMIT`, extended protocol for parameterized statements | extended protocol for everything, including `begin`/`commit` |
| error after the connection broke | `Connection terminated unexpectedly`, no SQLSTATE | `write CONNECTION_CLOSED <socket>`, no SQLSTATE |
| checked-out client's `error` event | emitted; **unhandled, it ends the Node process** — harnesses must attach a client `error` listener | not applicable |
| ROLLBACK after the failure | sent to the broken client, rejected without reaching the wire | none observed |
| connection for the retry | the pool replaces the broken client (new backend pid, also with a pool of one) | a new connection (backend pids not visible to the worker; faults armed by `application_name`) |
| shutdown | normal | `sql.end()` without a timeout did not return through the proxy, fault or not (cause not established); harnesses use `end({ timeout: 5 })` |

## Evidence recorded

Per run `sourceVerification.wireLog` (redacted) and per fault
`recovery.crashes[].inFlight` / `recovery.losses[]` /
`recovery.completions[]`: model fate vs. observed fate, mechanism,
forwarded/withheld, the server's answer as seen on the wire,
`clientSevered`, worker pid and `workerAliveAfter`, the driver's error
(run directory redacted), backend state before the fault, whether the
session ended or stayed open, statements on the broken client, the
connections used afterwards, the application's knowledge ("ambiguous")
and the physical outcome kept apart, and the durable rows compared with
the model's.

The wire log keeps event names (connection opened, backend pid, fault
armed, forwarded/held, server answer, withheld, delivered/discarded,
closed); for transaction control the statement keyword, for any other SQL
only `statement`. It **never** keeps bind parameters, result rows,
authentication messages, the cancel key or the text of an ordinary
statement; at most 8192 events (later ones counted).

| fact | established by |
| --- | --- |
| the application sent the COMMIT | proxy (frames) |
| PostgreSQL answered it | proxy (the answer frame, before withholding) |
| PostgreSQL committed it | durable rows read after the fault, independent of the proxy |
| the server had not received it | proxy (not forwarded) and `idle in transaction` |
| the backend ended | `pg_stat_activity`, with the measured delay |

## Failure behavior

| situation | result |
| --- | --- |
| fault not observed on the wire; unsupported frames on the connection | INCONCLUSIVE, reason given |
| worker not killed by SIGKILL; held backend no longer `idle in transaction` | INCONCLUSIVE |
| PostgreSQL keeps a session the fate should end | INCONCLUSIVE ("the transaction's fate is not established") |
| model fate ≠ established fate; a process dies that should live | DIVERGED |
| proxy cannot start | blocked (no verdict) |

## Not covered

* Connection errors other than a closed socket: timeouts, `57P01` admin
  shutdown, TLS errors, partial frames, a server that closes first.
* A connection broken in the middle of a transaction (only a COMMIT or an
  autocommit write in flight).
* Several tasks of one process sharing the broken pool; retry of an actor
  in a shared process.
* postgres-js with a bounded pool, under REPEATABLE READ/SERIALIZABLE, or
  in a shared process; a lock wait or deadlock together with a connection
  loss.

Tests: `crates/midwrite-replay/tests/proxy_postgres.rs` (real PostgreSQL,
`psql` as client so the test controls the wire exactly),
`crates/midwrite-cli/tests/{in_flight,connection_loss}.rs`; fixtures
[`fixtures/apps/ledger-app`](../../fixtures/apps/ledger-app),
[`fixtures/apps/ledger-live`](../../fixtures/apps/ledger-live) (with
`verify/harness-unhandled.ts`, which omits the client `error` listener and
is expected to diverge),
[`fixtures/apps/ledger-pgjs`](../../fixtures/apps/ledger-pgjs), scenarios
[`fixtures/scenarios/in-flight`](../../fixtures/scenarios/in-flight) and
[`fixtures/scenarios/connection-loss`](../../fixtures/scenarios/connection-loss).

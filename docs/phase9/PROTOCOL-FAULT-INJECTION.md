# Protocol-level fault injection

`crates/commitrift-replay/src/proxy.rs`, driven by `source.rs`
(`in_flight_prepare`, `crash_group`, `server_complete_step`).

## Why a proxy

The Phase 8 driver-level checkpoint holds a statement *before* the driver
sends it. That proves nothing about an operation that was sent: a held
JavaScript Promise says nothing about what PostgreSQL did. To exercise "the
COMMIT is on the wire and the application will never hear" the coordinator
needs control over **bytes between the driver and the server**. Options
considered (`IMPLEMENTATION-PLAN.md` has the full comparison):

| approach | verdict |
| --- | --- |
| driver-level checkpoints (Phase 8) | cannot place a fault after submission; kept for every other checkpoint |
| server-side synchronization (a hook, `pg_sleep`, advisory locks) | cannot withhold a *response* or delay a COMMIT *request*; changes the statement |
| a test-only protocol proxy | **chosen**: frame-aware, forwards unchanged, withholds, never edits |
| a hybrid (proxy + PostgreSQL's own observations) | what the evidence is: proxy events say what crossed the wire, `pg_stat_activity` and the durable rows say what PostgreSQL did |

## What it is, and is not

A Unix-socket proxy inside the run's private directory, between one worker
and the run's disposable cluster:

```
worker (node-postgres) ──► <run>/px/.s.PGSQL.5432 ──proxy──► <run>/pg/.s.PGSQL.5432 ──► PostgreSQL
```

* It starts only when the plan contains an in-flight step; every other run
  connects straight to the cluster as in Phase 8.
* It refuses to start unless both sockets are inside the run directory
  (`start`), and the coordinator checks that the socket handed to the worker
  is inside it too (`safety::check_database_target`). It never opens a TCP
  socket and never connects anywhere else.
* It is not a PostgreSQL implementation. It parses message *frames*
  (tag, length, body) and recognizes only what a fault needs.

## Protocol subset

Frontend: `StartupMessage`; `SSLRequest`/`GSSENCRequest` are answered with `N`
(encryption is never negotiated: a client that requires it fails by itself,
which is the correct fail-closed outcome); `CancelRequest` is forwarded and
the connection closed; `Q` simple query; extended protocol `P` Parse, `B`
Bind, `E` Execute, `D`, `C`, `S` Sync, `H`; `p` authentication data;
`X` Terminate. Backend: `R`, `K`, `S`, `Z`, `C`, `E`, `N`, `1`, `2`, `T`, `D`,
and the rest forwarded unparsed.

Not supported and **marked unsupported** (the proxy keeps forwarding; no
fault is injected on that connection; the coordinator reports INCONCLUSIVE,
never a fabricated injection): COPY (`d`, `c`, `f`, `G`, `H`, `W`), function
call (`F`), frames over 8 MiB, invalid framing.

A statement is classified from its SQL text (`classify_sql`): plain `COMMIT` /
`END` (optionally `TRANSACTION`/`WORK`, `AND [NO] CHAIN`) is a commit;
`COMMIT PREPARED` is not; `BEGIN`/`START`, `ROLLBACK`/`ABORT` are recognized;
everything else is a statement. Prepared statement names and portals are
tracked so that an extended-protocol COMMIT (postgres-js style) is found at
its `Parse`/`Bind`/`Execute`.

## Faults

Armed for the next matching operation of a connection (by the backend pid the
driver reports, else by `application_name`), one-shot:

| fault | on the wire | what it establishes |
| --- | --- | --- |
| `DropResponse` | the operation is forwarded; the server's answer (`CommandComplete`/`ErrorResponse`, then everything after) is withheld from the client | the server completed it (its own answer was **observed**); the client did not hear |
| `HoldRequest` | the operation's frames, through the next `Sync`, are not forwarded | the client sent it; the server did not receive it. The server connection stays open: `idle in transaction` (**observed in `pg_stat_activity`**) |
| `deliver(conn)` | the held frames (+ `Terminate`) are written to the kept server connection | the COMMIT arrives late: the server completes it *after* the client died and after whatever the restarted client did |
| `discard(conn)` | the held frames are dropped, the server connection closed | the COMMIT never arrives; PostgreSQL rolls the transaction back |

`Target::Statement` applies the same faults to the next non-control statement
(an autocommit write): `DropResponse` withholds the answer to the `INSERT`,
`HoldRequest` holds `Parse`…`Sync` of it.

A fault is verified, never assumed: after releasing the operation the
coordinator waits for the proxy's own event (`OperationHeld` or
`OperationCompleted`); if it does not come (the driver used a form the proxy
cannot see, or never sent the operation) the run is **inconclusive**, with the
message "no delay is taken as proof".

## Evidence recorded, and what is not

Per run `sourceVerification.wireLog` (redacted), per crash
`recovery.crashes[].inFlight`:

* event names and details: connection opened (`application_name`), backend pid
  (the cancel key is **not** kept), fault armed, operation forwarded/held,
  server answered (`CommandComplete COMMIT`, `ErrorResponse 40001`), answer
  withheld, delivered/discarded, client/server closed;
* for transaction control the statement (`COMMIT`, `BEGIN`, `ROLLBACK`); for
  any other SQL only `statement`; **never** bind parameters, result rows,
  authentication messages or SQL text of an ordinary statement;
* at most 8192 events; later ones are counted (`proxyLogTruncated`).

Observed by the proxy vs confirmed by PostgreSQL:

| fact | source |
| --- | --- |
| the application sent the COMMIT | proxy (frames) |
| PostgreSQL answered it | proxy (the answer frame, before withholding) |
| PostgreSQL committed it | the durable rows read *after* the worker's death — independent of the proxy |
| the server had not received it | proxy (not forwarded) and `pg_stat_activity.state = 'idle in transaction'` |
| the backend ended after the kill | `pg_stat_activity`, with the measured delay |

## Driver support

node-postgres: simple query for `BEGIN`/`COMMIT`, extended protocol for
parameterized statements — both exercised (`BEGIN`/`COMMIT` held and dropped,
parameterized `INSERT` dropped and held). postgres-js 3.4.5 sends its
transaction control and statements through the extended protocol and its
connections are not individually visible to the worker; the proxy recognizes
its frames, but no postgres-js in-flight scenario is verified in Phase 9 and
the version restriction (`POSTGRES_JS_VERSIONS`) is unchanged. The runtime
does not assume that two drivers behave alike.

## Failure behavior

| situation | result |
| --- | --- |
| fault not observed on the wire | INCONCLUSIVE |
| connection used COPY / unsupported frames | INCONCLUSIVE, reason given |
| worker not killed by SIGKILL | INCONCLUSIVE |
| PostgreSQL keeps the session after the kill (for a fate that should end it) | INCONCLUSIVE ("the transaction's fate is not established") |
| held backend no longer `idle in transaction` | INCONCLUSIVE |
| model fate ≠ established fate | DIVERGED |
| proxy cannot start | blocked (no verdict) |

Tests: `crates/commitrift-replay/tests/proxy_postgres.rs` (8, real
PostgreSQL, `psql` as client so the test controls the wire exactly),
`crates/commitrift-cli/tests/phase9.rs`.

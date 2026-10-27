# Phase 10 security and reliability

Phase 10 adds one new fault action and two test-only seams. Everything else of
`docs/phase9/SECURITY.md` and `docs/phase8/` is unchanged and still applies:
explicit `--allow-execution`, a disposable PostgreSQL cluster in a private run
directory, a Unix-socket proxy confined to that directory, network-denied
workers (bubblewrap on Linux), restricted environment, process-group cleanup,
fail-closed protocol handling.

## The new fault: closing a client connection

* `FaultProxy::sever_client(conn)` shuts down the proxy's end of one client
  socket. It acts only on a connection the proxy accepted on its own socket
  inside the run directory; there is no address argument and no way to reach
  another socket. A second call, or a call for an unknown connection, is an
  error (`a_connection_can_only_be_severed_once_and_only_if_it_exists`).
* It writes nothing to the client and signals no process. Whether the worker
  survived is then checked with the operating system (`waitid(WNOWAIT)`); a
  worker that died is reported (DIVERGED, with its stderr), never re-labeled as
  a connection loss.
* What the server side does follows from the fault armed before: the proxy
  never invents a server answer, and the coordinator never assumes one —
  every fate is established by the proxy's own event and by PostgreSQL's
  `pg_stat_activity` and rows.

## Statements on a broken client

The worker reports a statement on a client node-postgres has marked broken
(`deadConnection: true`); the coordinator releases it at once, records it as
`dead-connection` and never matches it against the plan. Such a statement
cannot reach the server (the driver rejects it locally), so releasing it
cannot change PostgreSQL's state. On a driver without those markers nothing
is reported and the statement is held as usual: the run then fails to match
its plan instead of being accepted silently.

## Redaction

* The driver's error is quoted as reported with the private run directory
  replaced (postgres-js names the proxy socket path in its message).
* A dead worker's stderr is quoted with the run directory and the harness
  directory replaced (`<harness>`), so no home-directory path reaches a report
  (`without_a_client_error_handler_the_process_dies_and_the_run_diverges`).
* The wire log is unchanged from Phase 9: event names, transaction-control
  statements, never parameters, rows, authentication or ordinary SQL text; the
  new `ClientSevered` event carries a fixed text.

## Test-only seams

* `COMMITRIFT_KEEP_ARTIFACTS=1` keeps the private run directory (worker logs)
  for debugging. Off by default; the directory is created with private
  permissions as before. Nothing reads it automatically.
* The `mutation-testing` feature of `commitrift-model` (deleting a kind of
  access from footprints) is enabled only for that crate's own tests and
  examples (a self dev-dependency). The release CLI does not include it
  (`cargo tree -p commitrift-cli -e features -i commitrift-model` lists no such
  feature); when the whole workspace is built for tests the hook is compiled
  in but off unless a test switches it on for its own thread.
* `COMMITRIFT_DPOR_PROFILE=1` makes `--dpor` print a profile to stderr. It
  changes no result.

## Timeouts are not deadlocks

Unchanged: a quiet period is classified (`PoolStarvation`,
`ApplicationWaitCycle`) only with the pool's or PostgreSQL's own counters; a
worker that does not finish before the deadline makes a run INCONCLUSIVE
("workers did not finish before the deadline"), never a verified deadlock. The
postgres-js shutdown that did not return (`POSTGRES-JS-IN-FLIGHT.md`) was
first seen exactly that way and was investigated, not reclassified.

## Isolation

Not weakened: the Phase 10 runs on macOS use the same coordinator isolation as
Phase 9; the Linux run uses bubblewrap as before (`FINAL-REPORT.md` §Linux).
No test disables isolation to pass.

# Processes, tasks and failure domains

## In the model

`process` on an actor groups actors into one process
(`Model::process_of`); actors without it get a process of their own. A
process has

* in-process variables (`State::apps[p]`) shared by its actors and lost in a
  crash;
* a connection pool, unbounded unless `pools` bounds it (`CONNECTION-POOL.md`);
* a lifecycle: running, or crashed — `CrashInFlight`/`Crash` end every running
  task of every actor of the process at once.

An actor's *tasks* are its threads: `Promise.all` children, `Promise.allSettled`
children (their failures do not fail the parent), detached (fire-and-forget)
tasks, whose unhandled rejection terminates the process
(`fail_thread`, Node's default). Awaited operations are the visible
transitions; pending Promises are tasks at a join or waiting on a statement.
Deferred callbacks outside what the extractor lowers are `Unsupported`.

A process crash:

| thing | effect |
| --- | --- |
| tasks of every actor of the process | end (`Crashed`) |
| open transactions | aborted (locks released at once in the model) |
| an operation in flight | resolved per the crash's mask: orphan (the server holds it) or lost |
| pool | emptied, waiting requests die |
| in-process variables | reset |
| committed rows | unchanged |
| performed external effects | unchanged — killing the process does not cancel a charge or a publication |

Failure domains the model keeps apart: *application task termination* (the
tasks end), *operating-system process termination* (SIGKILL), *client
connection closure* (the kernel closes the sockets), *PostgreSQL backend
recognition* (the server notices later; the model releases locks at once, the
runtime measures the delay), *transaction rollback* and *in-flight effect
completion* (an orphan completes after the process is gone).

## At runtime

`ActorLaunch.process` puts several actors into **one worker**: one Node.js
process runs their entries as concurrent tasks (`COMMITRIFT_SOURCE_TASKS`;
protocol 4 of `runtime/node/source.js`).

* **Task attribution.** Every message of a task carries `task: <actor name>`
  taken from a harness-provided context (`AsyncLocalStorage`,
  `scope.run({ task }, …)`): never a function name or a stack frame. The
  context is merged through Drizzle's `then` and `transaction` wrappers and
  restored around `pool.connect`'s callback — `pg-pool` serves a queued
  `pool.query` from the callback of whoever *returned* the connection, so
  without this the waiter's statement would be attributed to the releaser
  (found while building the scenario: the first run diverged exactly there).
* **Replies by `seq`.** The coordinator's `go` carries the `seq` it answers;
  concurrent operations are released independently.
* **One kill.** Consecutive crash and in-flight plan steps of actors of one
  worker are one operation (`crash_group`): every checkpoint and fault is
  established first, then the process group is killed once (one pid for all
  tasks), then PostgreSQL's sessions and the durable rows are observed. The
  report lists, per task, what it was doing (`boundary`), the operations in
  flight, the backends and the pid.
* **What cannot be controlled.** Tasks are scheduled only at *operations the
  runtime can observe* (an announced statement, effect or connection
  request). Pausing a task at an arbitrary JavaScript instruction is not
  attempted: if the model needs such a point the plan is refused or the run
  is inconclusive.
* **Restarts.** A restart starts a *new process*; restarting one task of a
  shared process is not realizable. A schedule that retries an actor of a
  shared process is refused (`UNSUPPORTED`).

## Evidence (macOS arm64, PostgreSQL 17.11, node-postgres 8.23.1)

`fixtures/phase9/scenarios/ledger-shared-process-crash.json`: three requests
as concurrent tasks of one process with a pool of one connection. The model
schedule: A commits and releases the connection to the waiting C, C's
autocommit `INSERT` is in flight, B and A's own next operation (publish a
confirmation) are announced. The run: all three tasks killed with one pid;
`A`/`B` at their checkpoints, `C`'s `INSERT` completed by the server with its
answer withheld; both payments durable, no confirmation published;
`ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED`.

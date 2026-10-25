# Running original code under a schedule

`crates/commitrift-replay/src/source.rs` (coordinator),
`runtime/node/source.js` (worker client, `@commitrift/replay/source`).

## Harness

An *authorized* harness directory contains a manifest
`commitrift-source.json`:

```json
{ "schemaVersion": 1, "kind": "commitrift.source-harness", "harness": "team-app",
  "command": ["node", "--enable-source-maps", "dist/verify/harness.js"],
  "schema": ["drizzle/0000_init.sql"], "network": "deny", "timeoutMs": 20000,
  "entries": { "inviteTeamMember": { "selector": "src/app/actions.ts#inviteTeamMember/arg[1]" } } }
```

The harness (`fixtures/phase7/team-app/verify/harness.ts`) imports the
application's **unchanged** exported action and calls it the way the
framework would: inside a request session, with a `FormData` body. It
provides only what a framework provides: the database pool (through the
application's own pool seam) and the session. Each entry names the selector
of the function the model checks, so the model and the run concern the same
function.

## Worker instrumentation (no application change)

* `coordinatePool(pool)` wraps `query` of every client the pool creates
  (`pool.on("connect")`), promise and callback style: the statement is
  announced, held until released, then sent unchanged on the same
  connection; its result (command tag, row count, rows with PostgreSQL's
  column provenance `tableID`) or error (SQLSTATE) is reported. Await
  semantics, connection ownership and transaction boundaries are those of
  the application and Drizzle.
* `instrumentDrizzle(drizzle, pgCore)` records the innermost application
  stack frame when application code calls a builder method (`db.select()`,
  `.where()`, `.limit()`, `db.transaction()`), on every prototype of each
  builder's chain, and runs `then`/`transaction` inside an
  `AsyncLocalStorage` scope carrying it. Needed because the application
  awaits a thenable: the driver call has no application frame. Drizzle
  copies `then` into some builders with a mixin; each copy is patched.
* Each worker connects with `application_name=commitrift-actor-<name>`.

## Coordinator

1. Refuses unless `--allow-execution`; loads and checks the manifest
   (`network: deny` only, files inside the harness directory, no
   `COMMITRIFT-TODO` marker, compiled entry present).
2. Creates a private run directory, OS isolation for every worker
   (macOS `sandbox-exec`, Linux bubblewrap — the replay backends), a
   private PostgreSQL cluster on a Unix socket inside it, the run database,
   the application's schema, and the seed derived from the scenario's
   initial state (application column names via `sqlName`, sequences set to
   the model's start).
3. Starts one worker per actor with an environment built from scratch.
4. Follows the plan. For `Release(expected)`: wait for the actor's next
   statement *inside the modeled function*; statements outside it (the
   wrapper's `getUser()` lookup) are released immediately and recorded
   (reads only; a write there makes the run inconclusive). The statement
   must match the expected kind, table and source region, otherwise the run
   **diverges** (recorded, never skipped). After release the coordinator
   waits for the result, or for PostgreSQL to report the backend waiting on
   a lock (`pg_stat_activity.wait_event_type = 'Lock'`, checked after a
   150 ms settle time); the model's prediction (completes / waits) must
   agree. For `Complete(actor)`: the waiting statement must now finish.
5. Afterwards every worker finishes (remaining statements are released and
   recorded as `beyond-plan`, which the comparison treats as unexpected),
   final rows of the modeled columns are read, processes are killed at the
   deadline, the cluster is stopped and the directory removed.

A query that is submitted is not assumed finished: completion is the
driver's result; waiting is the server's report.

## Connections and pools

Actors are separate processes, each with its own `pg.Pool` (`max: 10`).
For the tested actions this is equivalent to concurrent requests sharing
one pool with at least as many free connections as concurrent transactions;
pool exhaustion and queueing for connections are **not** modeled or
exercised.

## Not supported against original code

Crashes (killing a worker at a model transition), retries,
deadlock-victim steering, cursors/streams (reported as unsupported),
non-`pg` drivers (saas-starter itself uses `postgres-js`: its original
application would need a `postgres-js` adapter, not written), Bun.

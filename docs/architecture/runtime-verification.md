# Runtime verification

Crates and packages: `midwrite-replay` (Rust), `runtime/node`
(`@midwrite/replay`, JavaScript, on npm), `midwrite-model`
(`oracle.rs`, `source_verify.rs`, `probe.rs`). Everything here is opt-in;
nothing runs without `--allow-execution` and an authorized manifest,
except the PostgreSQL oracle, which runs only the model's SQL. User guide:
[runtime verification](../user-guide/source-verification.md).

## Components

| component | file | role |
| --- | --- | --- |
| disposable PostgreSQL | `replay/src/postgres.rs` | `initdb` + `pg_ctl` into a private temporary directory, Unix socket only, deleted afterwards; server binaries from `MIDWRITE_PG_BIN` or `PATH` |
| OS isolation | `replay/src/isolation.rs` | macOS `sandbox-exec` (IP networking denied, writes confined), Linux bubblewrap (new user, network (loopback only), PID, IPC, UTS and cgroup namespaces, minimal read-only filesystem view) |
| manifest validation | `replay/src/manifest.rs`, `safety.rs` | `midwrite-replay.json` / `midwrite-source.json`: files inside the harness, `network: deny`, no external database URLs or credential-like variables, no `MIDWRITE-TODO` marker |
| single-request replay | `replay/src/supervisor.rs`, `observe.rs` | checkpoint protocol for `replay`: run a control scenario and each case from a fresh database, inject a kill or exception at a named checkpoint, observe surviving state, check declared invariants ([replay protocol](replay-protocol.md)) |
| source coordinator | `replay/src/source.rs` | one worker per actor (or a shared process), plan-ordered release of held statements, lock-wait observation, crash/restart, effect doubles, response checkpoint |
| protocol fault proxy | `replay/src/proxy.rs` | between a worker and PostgreSQL: in-flight fates and severed client sockets ([fault injection](fault-injection.md)) |
| catalog reads | `replay/src/catalog.rs` | schema drift comparison ([schema fidelity](schema-extraction.md#schema-fidelity-source-vs-database-catalog)) |
| worker client | `runtime/node/source.js`, `pg.js`, `index.js`, `event-log.js` | wraps the application's pool and Drizzle builders without changing application code; announces each statement, holds it, reports results and SQLSTATEs; attributes statements to source frames |
| differential comparison | `model/src/source_verify.rs` | turns a counterexample into a plan, compares statements, cells, rows, outcomes, effects, crashes and return values with the model, assigns the evidence level |
| recovery probes | `model/src/probe.rs` | the shortest model schedule containing required events, for scenarios without a counterexample |
| lockstep oracle | `model/src/oracle.rs` | replays a model schedule as SQL, one session per actor ([PostgreSQL model](postgres-model.md)) |

Three comparisons are kept apart, in separate report sections; passing one
does not count for the others:

```text
abstract model vs PostgreSQL     --validate-postgres   report.runtime             (model SQL; never runs application code)
Drizzle SQL vs abstract model    --verify-source       sourceVerification.statements
original application vs model    --verify-source       sourceVerification.runs (rows, outcomes, invariant)
```

The model crate does not parse TypeScript and the replay crate does not
know the model: the coordinator is driven by a **plan** (per step: kind,
table, source region, expected lock wait, crash checkpoint, fault) that
the model crate derives.

## The source harness

An authorized harness directory contains `midwrite-source.json`:

```json
{ "schemaVersion": 1, "kind": "midwrite.source-harness", "harness": "team-app",
  "command": ["node", "--enable-source-maps", "dist/verify/harness.js"],
  "schema": ["drizzle/0000_init.sql"], "network": "deny", "timeoutMs": 20000,
  "entries": { "inviteTeamMember": { "selector": "src/app/actions.ts#inviteTeamMember/arg[1]" } } }
```

The harness ([example](../../fixtures/apps/team-app/verify/harness.ts))
imports the application's **unchanged** code and calls it the way the
framework would (inside a request session, with the request body),
providing only what a framework provides: the database pool through the
application's own seam, the session, and implementations of provider seams
for external effects. Each entry names the selector of the function the
model checks ([source selectors](source-extraction.md#function-identity-and-source-selectors)),
so the model and the run concern the same function.

## Worker instrumentation (no application change)

* **node-postgres:** `coordinatePool(pool)` wraps `query` of every client
  the pool creates, promise and callback style. Each statement is
  announced, held until released, then sent unchanged on the same
  connection; its result (command tag, row count, rows with PostgreSQL's
  column provenance) or error (SQLSTATE) is reported. Await semantics,
  connection ownership and transaction boundaries stay the application's
  and Drizzle's. The caller's async context is captured at `pool.connect`
  and restored in the callback.
* **postgres-js** (3.4.5 only): postgres-js has no public hook that can
  delay a statement, and wrapping `sql.unsafe` would miss COMMIT/ROLLBACK
  sent through tagged templates. Every query passes through
  `Query.prototype.handle()` before it reaches a connection; the adapter
  wraps it (announce, wait for release, call the original). Completion is
  the query promise's own settlement; a tagged template that is never
  awaited is never executed or announced. This is an internal: other
  versions are refused before the application loads, and the prototype is
  checked before it is patched. Its connection-opening `pg_type` read is
  not held (reported `driverInternal`); `SET TRANSACTION` after `BEGIN` is
  released with the BEGIN (`transactionConfig`); lock waits and session
  ends are found by the execution's `application_name`; cursors,
  `forEach` and streams are reported unsupported. `sourcePostgresUrl()`
  gives the application a host-less URL and sets `PGHOST`/`PGPORT`/`PGUSER`
  so its `postgres(process.env.POSTGRES_URL)` stays unchanged.
* **Drizzle attribution:** `instrumentDrizzle` records the innermost
  application stack frame when application code calls a builder method
  (`db.select()`, `.where()`, `db.transaction()`), on every prototype of
  each builder chain, and runs `then`/`transaction` inside an
  `AsyncLocalStorage` scope carrying it — the driver call itself has no
  application frame because the application awaits a thenable. Across an
  un-awaited `return helper()`, where V8 loses the caller's frame, the
  chain is restored from an earlier stack of the same execution when
  unambiguous (reported as such); otherwise the statement may be
  classified outside the model and the run is inconclusive.
* Each worker connects with `application_name`
  `midwrite-actor-<name>#<attempt>`.

## Original-code run, step by step

1. Refuse unless `--allow-execution`; load and check the manifest
   (`network: deny`, files inside the harness, no `MIDWRITE-TODO`,
   compiled entry present) and the scenario's `sourceVerification`
   mapping. Actors declared instead of extracted, actors without a harness
   entry, and entries whose selector differs from the actor's are refused.
2. Create a private run directory and a disposable cluster; apply the
   harness's schema SQL; compare the catalog with source schema facts when
   the scenario reads them; seed the initial rows (application column
   names via `sqlName`) and set sequences to the model's start.
3. Start one isolated worker per actor with an environment built from
   scratch; each imports the unchanged application through the harness.
4. Follow the plan. For a `Release`: wait for the actor's next statement
   *inside the modeled function*; statements outside it (a wrapper's
   `getUser()`) are released and recorded (reads only — a write there makes
   the run inconclusive). The statement must match the expected kind,
   table and source region, otherwise the run **diverges** (recorded, never
   skipped). Then wait for the result, or for PostgreSQL to report the
   backend waiting on a lock (`pg_stat_activity.wait_event_type = 'Lock'`,
   after a 150 ms settle time); the model's prediction (completes / waits)
   must agree. A submitted query is never assumed finished: completion is
   the driver's result, waiting is the server's report.
5. Faults: crashes at verified checkpoints (below); in-flight and
   connection-loss fates through the proxy ([fault injection](fault-injection.md)).
6. Read the committed rows (after each crash and at the end); match
   generated values chronologically (crash checkpoints before final rows,
   since rows differing only in a generated key are interchangeable);
   evaluate the invariant **on the observed rows**, effect log and return
   values — never inferred from the model.
7. Afterwards every worker finishes (remaining statements are released and
   recorded as beyond the plan, which counts as unexpected), processes are
   killed at the deadline, the cluster is stopped and the directory
   removed. Repeat on a fresh database (`--source-repeat`) and classify
   ([evidence levels](../concepts/evidence-levels.md)).

The witness shows, per statement, the model operation and its source span,
the SQL the application sent and its innermost application frame, the
predicted and observed outcome, and every difference; statements outside
the model and unexpected statements are listed separately. Only modeled
columns are compared; other columns (timestamps) are shown.

## Crashes

A crash is a **SIGKILL of the worker's process group**, sent by the
coordinator — never an exception thrown into the application. Local state
ends, `catch`/`finally` do not run, the kernel closes the sockets,
PostgreSQL discards uncommitted transactions when it notices the lost
client (every session of the execution gone from `pg_stat_activity`,
within 10 s, else inconclusive), committed changes stay, and effects
already performed stay (the test double outlives the worker). Detached
descendants are found by a per-run token in their environment and killed;
the coordinator kills every worker on every exit path.

The model crashes a process between two transitions; at runtime that
boundary must be an observable checkpoint, verified before the kill:

| model state at the crash | runtime checkpoint | verified by |
| --- | --- | --- |
| next operation is a statement, BEGIN, COMMIT, ROLLBACK, an external effect or the response | that operation is **announced and held** (never sent) | it must match the model's next operation (kind, table/channel, source region, idempotency key), else **diverged** |
| a statement waits for a lock whose holder is active | in flight on the server and waiting | `wait_event_type = 'Lock'` re-checked right before the kill, else inconclusive |
| a statement waited, its blocker finished, the model has not resumed it | possibly executed by PostgreSQL | allowed only inside a transaction; for an **autocommit** statement the plan is refused |
| several tasks of the actor run (`Promise.all`) | not one observable checkpoint | refused |

"After a statement completes", "after the COMMIT acknowledgment", "before
or after an external publication" and "before the response" are all the
first row. The coordinator never kills while a released statement runs
and is not waiting — a COMMIT lost in flight is an
[in-flight fault](fault-injection.md) through the proxy instead.

With PostgreSQL's default `client_connection_check_interval = 0`, a backend
**blocked on a lock** does not notice its killed client (measured: more
than 10 s), so crash plans set it to 50 ms (PostgreSQL 14+) and the report
lists it (`recovery.serverSettings`). The model's "a crash releases its
locks at once" holds on servers with that setting.

## Retries, effects, probes

**Retries are declared, never invented.** A retry is a new execution: a
new worker process (new pid and connections, attempt + 1), started only
where the model has a `Retry` transition, which exists only when the
scenario declares a mechanism: a client resubmitting after no response
(`onCrash`), queue redelivery (`atLeastOnce`, also after success), a
supervisor restart (`onCrash`, `guaranteed`). The coordinator plays that
client, queue or supervisor — a declared test contract, reported in
`recovery.contracts`, not evidence about a real queue. An application's
own transaction-retry helper (bound by `extraction.transactionRetries`)
or retry loop **runs** in the same process. Without a declared mechanism
there is no retry, and the report says "no recovery mechanism is
declared".

**External effects** (payments, events, e-mails) reach a **test double
hosted by the coordinator** through the application's own provider seam:
the harness implements the seam with `sourceEffect(channel, {key,
payload})`, held and released like a statement. The double applies the
channel's **declared** deduplication (`none` or `byKey`); exactly-once is
never inferred from a key. Database-enforced idempotency (a claim row with
a unique index) is observed as the database's semantics, including its
other side: a crash between claim and effect loses the effect.

**Quiescence.** The report says what the final state means
(`recovery.quiescence`): whether every execution ended, whether a further
optional retry is still possible within the bounds, whether the retry
budget is exhausted. No eventual-recovery or eventual-delivery claim is
made from a bounded run; a guaranteed retry is a declared fairness
assumption.

**Recovery probes.** When a scenario has no counterexample there is no
schedule to run. A probe names events (`external:card-charge`,
`crash:Client`, `retry:Client`, `blocked:Accept2`, `error:40001`, …) and the
model checker selects the **shortest complete schedule** containing them
in order (breadth-first over the same transition relation). The schedule
is the model's, never hand-written; the original code must reproduce it
with the invariant holding. One schedule is bounded evidence, not a proof.

## Responses and return values

With `MIDWRITE_SOURCE_RESPONSE=hold` the worker, after the harness entry
returned, announces `{"type":"respond","value":…}` and holds; the
coordinator releases it at the model's `Respond` step or SIGKILLs the
worker there ("before RESPONSE (announced, never sent)"). Return values the
original functions produced are reported and fed to invariants that read
them. The coordinator plays the HTTP client: "delivered" means the
coordinator received the response.

## Worker protocol

Statements are announced, held and released; results, errors and lock
waits are observed; Drizzle frames are attributed. Plan steps cover
releases, completions of waiting statements, crashes and restarts (one
slot per attempt), effects through coordinator-hosted doubles,
proxy-mediated connections with in-flight fates and severed clients,
shared processes and pools (the scenario's pool capacity must equal the
harness pool's `max`), and the held response. Messages over a Unix
socket (JSON lines), worker → coordinator: `hello` (attempt, pid), `query`
(flagged `deadConnection` on a broken client), `effect`,
`result`/`error`, `driver-internal`, `pool`, `unsupported`, `respond`,
`done`; coordinator → worker: `go` (with an effect receipt for effects),
`stop` (the held operation fails).

## Reliability rules

Observations are waited for with a deadline, never assumed after a sleep:
a proxy event, a `pg_stat_activity` state, a worker checkpoint or a row.
The deadline only turns a hang into INCONCLUSIVE. Each run owns its
cluster and directory. Timeouts, proxy errors and worker deaths are
inconclusive, never agreement. Test rules: [testing](../development/testing.md).

## Guarantees and non-guarantees

* The harness is trusted and cooperative. Isolation protects against
  accidents in a trusted harness; it is **not** a sandbox for malicious
  code ([security](../reference/security.md)).
* Neither platform limits CPU or memory beyond per-worker rlimits.
* A result is evidence about one schedule of one first-party application
  with pinned driver versions on one PostgreSQL version.

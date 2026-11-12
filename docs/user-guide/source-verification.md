# Runtime verification: PostgreSQL replay, original code, replay harnesses

Static analysis and model checking never run application code. Three
opt-in features go further; each says exactly what it ran.

| feature | command | runs application code? | covers |
| --- | --- | --- | --- |
| PostgreSQL replay of the model's SQL | `model-check --validate-postgres` | no — only the statements the model predicts, on a disposable cluster | the model's database semantics for one schedule |
| original-code verification | `model-check --verify-source --allow-execution` | **yes**, through an authorized source harness | concurrent schedules, crashes and retries, lost acknowledgments, live connection loss, the response step |
| single-request replay harness | `replay --allow-execution` (skeleton from `init-replay`) | **yes**, the harness you name | one request with a failure injected at a named checkpoint |
| schema drift check | `schema --migrations … --allow-execution` | executes the migration SQL on a disposable cluster | Drizzle schema facts vs. the database catalog ([schema fidelity](../architecture/schema-extraction.md#schema-fidelity-source-vs-database-catalog)) |

Without `--allow-execution`, `replay` prints the plan and `--verify-source`
is refused.

> **Before `--allow-execution`:** read the harness and its SQL files first;
> they run as you. Midwrite only ever uses a disposable PostgreSQL cluster
> it creates itself, never your database, and its isolation guards against
> accidents in a harness you trust, not against hostile code.

Isolation and authorization rules:
[security reference](../reference/security.md). Architecture:
[runtime verification](../architecture/runtime-verification.md).

## Original-code verification (`--verify-source`)

```bash
midwrite model-check SCENARIO.json --verify-source --allow-execution --source-repeat 3
midwrite model-check SCENARIO.json --verify-source --harness path/to/midwrite-source.json --allow-execution
```

What it needs:

1. A **source-harness manifest** `midwrite-source.json` in the
   application (`"kind": "midwrite.source-harness"`): the command that
   starts a worker (for example `node --enable-source-maps
   dist/verify/harness.js`), the runtime, the schema SQL to apply, `"network":
   "deny"`, a timeout, and the entries the harness can call, each with the
   source selector of the function the model checks. Example:
   [`fixtures/apps/request-app/midwrite-source.json`](../../fixtures/apps/request-app/midwrite-source.json);
   format: [the source harness](../architecture/runtime-verification.md#the-source-harness).
2. A **harness** that imports the application's unchanged code and calls it
   the way the framework would, providing only what the framework provides
   (the database pool through the application's own seam, a session). The
   application is not instrumented; the worker wraps the pool and Drizzle's
   builders (`@midwrite/replay`, `runtime/node`).
3. A scenario with a `sourceVerification` section mapping each actor to a
   harness entry and its input. `midwrite discover` writes it when the
   contracts declare a `harness` ([contracts](contracts.md)).
4. PostgreSQL server binaries, Node.js ≥ 22, an isolation backend, and the
   compiled harness (`npm ci && npm run build` in the fixture or
   application).

What it does: one isolated worker per actor (or a shared process when the
scenario declares one); every SQL statement Drizzle sends is announced,
held, and released in the counterexample's order; PostgreSQL's answers and
lock waits (`pg_stat_activity`) are observed; crashes are real SIGKILLs at
verified checkpoints, a retry is a new process; external effects go to
coordinator-hosted test doubles; in-flight faults go through a protocol
proxy; the invariant is evaluated on the rows (and the return values)
the run produced. Every run uses a fresh disposable database. How each
step works: [runtime verification](../architecture/runtime-verification.md),
[fault injection](../architecture/fault-injection.md).

```bash
midwrite model-check SCENARIO.json --fault-mode in-flight                 # model only: crashes only with a COMMIT or write in flight
midwrite model-check SCENARIO.json --verify-source --allow-execution --fault-mode in-flight --source-repeat 5
```

A harness for node-postgres must attach an `error` listener to every
client: node-postgres emits `error` on a checked-out client whose
connection ends, and without a listener the Node process dies (the run is
then `diverged`, with the worker's stderr quoted).

### Results

| `sourceVerification` / evidence | meaning |
| --- | --- |
| `original-code-reproduced` | every run followed the plan, every statement and outcome agreed with the model, and the invariant is violated on the observed state |
| `original-code-crash-reproduced` | the same for a counterexample with a real crash and/or retry |
| `original-code-in-flight-failure-reproduced` | the same with a COMMIT or autocommit write in flight at the kill |
| `original-code-connection-loss-reproduced` | a connection broke under a live worker |
| `original-code-recovery-reproduced`, `original-code-invariant-held` | no counterexample within bounds; a recovery or probe schedule ran on the original code and the invariant held — bounded evidence about one schedule |
| `diverged`, `not-reproduced`, `inconclusive`, `unstable` | the run did not reproduce; differences are reported statement by statement (`MW7103`); the model counterexample keeps its own level |
| `unsupported`, `blocked` | not run, with the reason |

Definitions: [evidence levels](../concepts/evidence-levels.md).

### Supported runtime

| | status |
| --- | --- |
| node-postgres (`pg`) 8.23.1 + drizzle-orm 0.45.4 | coordinated: schedules, lock waits, crashes, retries, in-flight faults, connection loss, shared processes and pools |
| postgres-js 3.4.5 + drizzle-orm 0.45.4 | coordinated: schedules, lock waits, crashes, retries, in-flight faults, connection loss; other versions refused (the adapter relies on an internal); harnesses end the client with `sql.end({ timeout: 5 })` |
| Bun, other drivers (Neon, Bun SQL, …), other ORMs | not coordinated |
| read-replica schedules | refused ("replication is not steered against original code") |
| retries of an actor that shares a process with others | refused |

Every result is about **this** application code, these inputs and declared
preconditions, this schedule, this PostgreSQL version and a disposable
database — not about production traffic. All original-code evidence so far
is from first-party fixtures written for the purpose; no external
application has been executed ([evidence index](../research/evidence-index.md#5-original-code-reproduction)).

## Single-request replay harnesses

This workflow turns one static finding (`check`) into observed
evidence: inject a failure at a named checkpoint of one request, against a
disposable PostgreSQL, and check invariants the harness declares.

```bash
midwrite trace src --format json > trace.json
midwrite init-replay --trace trace.json --finding <FINGERPRINT> --output my-harness
#   fill in the MIDWRITE-TODO markers by hand; replay refuses to run until none is left
midwrite replay my-harness                                 # prints the plan; nothing runs
midwrite replay my-harness --allow-execution --case kill-after-db-commit --repeat 5
midwrite replay my-harness --allow-execution --format json --output v.json
midwrite report v.json -o v.html
midwrite replay my-harness --allow-execution --baseline old.json --regression-output r.json
```

Results: `RUNTIME_REPRODUCED`, `RUNTIME_NOT_REPRODUCED`, `INCONCLUSIVE`.
With `--baseline`, exit 1 when a case changed between reproduced and not
reproduced, 3 when a case is inconclusive or was not executed. A runtime
result never changes a finding's static confidence. Complete example:
[`scripts/demo-init-replay.sh`](../../scripts/demo-init-replay.sh) on
[`fixtures/replay/drizzle-node`](../../fixtures/replay/drizzle-node).
Protocol, checkpoints and harness generation:
[single-request replay](../architecture/replay-protocol.md); limitations
of replay harnesses:
[limitations](../reference/limitations.md#unsupported-runtime-drivers-and-environments).

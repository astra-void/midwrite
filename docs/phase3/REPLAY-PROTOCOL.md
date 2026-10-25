# Replay protocol (version 1)

`commitrift replay` turns a selected failure window into observed runtime
evidence, for one explicitly authorized **harness** at a time. Security
properties and their limits are in [`SECURITY.md`](SECURITY.md).

```bash
commitrift replay fixtures/replay/order-vulnerable                      # prints the plan, runs nothing (exit 2)
commitrift replay fixtures/replay/order-vulnerable --allow-execution    # control run + every case
commitrift replay fixtures/replay/order-vulnerable --allow-execution --case kill-after-db-commit --repeat 5
commitrift replay fixtures/replay/order-vulnerable --allow-execution --format json --output report.json
commitrift report report.json -o report.html
```

`--case` takes a case id, or a diagnostic code only when exactly one case of
the harness targets it ("a diagnostic code does not identify a unique
trace"). `--finding <fingerprint>` selects the cases linked to one finding.
Nothing outside the named directory is discovered or run.

Requirements: PostgreSQL server binaries (`initdb`, `pg_ctl`, `postgres`,
`psql` from one installation: `$COMMITRIFT_PG_BIN`, else next to `postgres`
on `PATH`, else `pg_config --bindir`), the harness runtime (the fixtures use
Bun ≥ 1.2 for its built-in PostgreSQL client), and for network isolation
macOS `sandbox-exec`. Docker is not used.

Exit status: `0` every case has a runtime verdict and matches its declared
expectation; `1` a case's verdict differs from its `expect`; `2` usage,
invalid or unsafe manifest, or not authorized; `3` replay blocked (missing
PostgreSQL, no network isolation) or an `INCONCLUSIVE` case without an
expectation.

## Harness layout

```text
fixtures/replay/order-vulnerable/
  commitrift-replay.json   the manifest (required; nothing runs without it)
  commitrift.json          static-analysis configuration for the application
  schema.sql               initial state, applied to a fresh database before every run
  harness.ts               entry point: runScenarios({ ... })
  src/orders.ts            the application code that is analyzed and executed
fixtures/replay/shared/     Drizzle-shaped adapter over Bun's PostgreSQL client and a fake queue;
                           both emit the checkpoints, so application code has no instrumentation
runtime/typescript/commitrift-replay.ts   protocol client (no dependencies)
```

## Manifest (`commitrift-replay.json`, schema version 1)

Unknown fields are rejected; validation runs before anything executes and
never echoes rejected values.

| Field | Meaning |
| --- | --- |
| `schemaVersion` | `1`. |
| `harness` | Id, `[a-z0-9-]+`. |
| `description` | Free text. |
| `command` | Program and arguments, run from the harness directory, **without a shell**. |
| `timeoutSeconds` | Deadline of one harness process, 1–300 (default 30). |
| `network` | `deny` (default) or `allow`. `allow` takes effect only with `--allow-network` too. |
| `database` | `{"provider": "ephemeral-postgres", "setup": ["schema.sql", ...]}`. Setup files must lie inside the harness directory. `url`, `connectionString`, `host`, `port` or `password` are refused: replay never connects to an existing database. |
| `analysis` | `{"paths": ["src"], "config": "commitrift.json"}`: what to analyze statically (relative to the harness) to link cases to findings. |
| `env` | Extra environment variables. Names that look like database settings, loader settings, replay internals or credentials (`PG*`, `DATABASE*`, `*SECRET*`, `*TOKEN*`, `*KEY*`, `*PASSWORD*`, `*URL*`, ...) and values containing `://` are refused. |
| `observations` | `[{"name", "sql"}]` (a read-only query returning at most one row with one column; integers become numbers, other values text, no row `null`) or `[{"name", "events": {"topic", "measure"}}]` with `measure` `count` (default), `distinctIds`, `duplicates` (`count − distinctIds`) or `withoutId`. |
| `invariants` | The application contract: `[{"id", "description", "when": [conditions], "require": [conditions], "requireAny": [conditions]}]`. An invariant holds when some `when` condition is false, or all `require` and at least one `requireAny` hold. Conditions: `{"observation", "equals" \| "notEquals" \| "atLeast" \| "atMost": value}`. At least one invariant is required. |
| `control` | `{"scenario", "input"}`: run once without injection; must complete and satisfy every invariant, otherwise every case is `INCONCLUSIVE`. |
| `cases` | See below. |

A case:

| Field | Meaning |
| --- | --- |
| `id` | `[a-z0-9-]+`. |
| `finding` | `{"id"?, "code"?, "function"?, "file"?}` selecting one static finding of the harness application (`file` relative to the harness). Matched → the report embeds its trace. |
| `scenario`, `input` | What the harness runs (`input` is passed as JSON). |
| `inject` | `{"checkpoint", "occurrence" (default 1), "label"?, "mode": "processKill" \| "exception"}`. Omitted: no injection. |
| `verifyBeforeInjection` | Conditions checked from a separate database session while the harness waits at the checkpoint. A false one cancels the injection (`INCONCLUSIVE`): e.g. that a write is visible, i.e. committed, before killing "after the commit". |
| `predict` | `{observation: value}`: the state the trace predicts after the failure; compared and reported, never used for the verdict. |
| `recovery` | Scenarios run afterwards without injection (a relay, a client retry); observed as `afterRecovery`. |
| `expect` | The runtime level the author expects, for regression checks. |

## Wire protocol

JSON lines over a Unix socket at `$COMMITRIFT_REPLAY_SOCKET` (inside the
replay directory). One connection per harness process.

```text
harness    → supervisor  {"type":"hello","protocol":1,"scenario":"completeOrder","pid":123}
harness    → supervisor  {"type":"checkpoint","seq":1,"name":"before-db-write","label":"orders"}
supervisor → harness     {"type":"continue"}
harness    → supervisor  {"type":"checkpoint","seq":2,"name":"after-db-commit","label":"orders"}
                         processKill: no answer; SIGKILL to the harness' process group
                         exception:   {"type":"throw","message":"CommitRift injected fault at checkpoint `after-db-commit`"}
harness    → supervisor  {"type":"done","outcome":"returned"|"threw","error":"...","injected":true|false}
```

A harness sends a checkpoint only after the operation it names has
completed (for `after-db-commit`: the server acknowledged the commit), then
waits for the answer. The supervisor selects the `occurrence`-th checkpoint
with the given name (and label, if set). A malformed or unknown message, a
protocol version mismatch, or a harness that never connects ends the run as
`INCONCLUSIVE`.

### Checkpoint names used by the first-party fixtures

| Name | Emitted |
| --- | --- |
| `before-db-write` (label: table) | before an INSERT/UPDATE is sent |
| `after-db-commit` (label: table) | after an auto-commit write was acknowledged |
| `after-tx-write` (label: table) | after a write inside a transaction (not committed) |
| `before-tx-commit` (label: `transaction`) | after the transaction callback returned, before COMMIT |
| `after-db-commit` (label: `transaction`) | after COMMIT was acknowledged |
| `before-event-publish` / `after-event-publish` (label: topic) | around the fake queue's durable append (written and fsync'ed before `after`) |

These match the `checkpoints` hints in failure traces.

## Failure modes

| Mode | What happens | What it does not represent |
| --- | --- | --- |
| `processKill` | SIGKILL while the harness waits at the checkpoint. No application code runs afterwards: no `catch`, no `finally`, no rollback in the application. PostgreSQL sees the connection drop and rolls back any open transaction. | A database or machine crash (PostgreSQL itself keeps running; committed data is not put at risk). |
| `exception` | `checkpoint()` throws `InjectedFault`. The application's `catch`/`finally` blocks and transaction rollbacks run. | A crash: compensation in a `catch` block can make an exception harmless where a crash is not (the `order-vulnerable` fixture shows exactly this). |
| none | The scenario runs normally (control and recovery runs, cases without `inject`). | |

## Environment of a harness process

Cleared, then: `PATH` (inherited), `HOME` and `TMPDIR` (inside the replay
directory), `LANG=C`, `NO_COLOR=1`, `COMMITRIFT_REPLAY=1`,
`COMMITRIFT_REPLAY_PROTOCOL=1`, `COMMITRIFT_REPLAY_ROOT`,
`COMMITRIFT_REPLAY_SOCKET`, `COMMITRIFT_REPLAY_SCENARIO`,
`COMMITRIFT_REPLAY_INPUT`, `COMMITRIFT_REPLAY_PG_SOCKET`,
`COMMITRIFT_REPLAY_PG_DATABASE`, `COMMITRIFT_REPLAY_PG_USER`,
`COMMITRIFT_REPLAY_EVENT_LOG`, plus the manifest's validated `env`. The
client's `replayDatabase()` refuses a socket outside `COMMITRIFT_REPLAY_ROOT`.

## Verification report (`kind: "commitrift.verification"`, schema version 1)

| Field | Content |
| --- | --- |
| `harness` | id, description, manifest path, manifest hash (FNV-1a 64 of its bytes), command. |
| `source` | Git revision of the harness directory and whether it is dirty (`null` outside a repository with commits). |
| `environment` | OS, architecture, PostgreSQL version, runtime version, network isolation, database description, **names** of the environment variables passed (never values). |
| `control` | The control run (a run record). |
| `cases[]` | `id`, `description`, `staticLink` (`matched`/`notFound`/`ambiguous`/`none` + detail), `findingId`, `traceId`, `code`, the embedded `trace`, `failureMode`, `checkpoint`, `boundary` (`onTraceBoundary` when the checkpoint is one of the trace's boundary checkpoints, `outsideTraceBoundary`, `notApplicable`), `initialState`, `runs[]`, `verification`, `consistent`, `expected`, `matchesExpectation`, `limitations`. |
| run record | `run`, `scenario`, `failureMode`, `injected`, `checkpoints[]` (`seq`, `name`, `label`, `action`: `continue`/`throw`/`kill`/`abort`), `preInjectionChecks[]`, `exit` (`exited`/`signaled`/`timedOut`/`notStarted`, code, signal), `outcome` (`returned`, `threw`, `killed`, `timedOut`, `crashed`, `notStarted`), `error`, `observations` by stage (`before`, `after`, `afterRecovery`), `invariants[]` (id, stage, holds, detail), `prediction[]`, `recovery[]`, `durationMs`, `verification`, `reason`, `logs` (stdout/stderr, 16 KiB each, redacted). |
| `summary` | Counts by level and against expectations. |
| `limitations` | Always includes that results hold for this harness and environment only, and that replay is not a sandbox for malicious code. |

Determinism: with the same harness and environment, two reports differ only
in `durationMs` (checked by `reports_are_deterministic_apart_from_durations`).
Temporary paths are replaced by `<replay-root>` and `scheme://user:password@`
credentials by `user:***` before anything enters a report. Classification
rules are in [`ARCHITECTURE.md`](ARCHITECTURE.md#3-verification-evidence-model-commitrift-coreverification).

## Writing a harness for your own application

1. Put the code under test behind the same adapters it uses in production,
   or thin test adapters, that call `checkpoint(name, label)` after each
   effect completes (and before it starts, if you want to inject there).
   Keep checkpoints out of the application code you want analyzed: an
   unknown `await checkpoint()` call next to a commit becomes a CP9001.
2. Connect only through `replayDatabase()`; write external effects to
   `eventLogPath()` or another file inside `COMMITRIFT_REPLAY_ROOT`.
3. Declare observations and the invariants your application must keep, then
   a control scenario that satisfies them.
4. Add cases at the boundary the trace names, and negative cases outside it.
5. Review the harness; run `commitrift replay <dir>` to see the plan; then
   `--allow-execution`.

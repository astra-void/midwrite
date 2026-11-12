# Single-request replay: protocol, harnesses and isolation

`midwrite replay` turns one failure window of a static finding into
observed runtime evidence, for one explicitly authorized **harness** at a
time: it injects a failure at a named checkpoint of one request against a
disposable PostgreSQL database and checks invariants the harness declares.
It is separate from original-code verification of concurrency
counterexamples ([runtime verification](runtime-verification.md)), which
reuses the same disposable database and isolation. Usage:
[source verification § single-request replay](../user-guide/source-verification.md#single-request-replay-harnesses).

```text
static finding ──► failure trace ──► controlled injection ──► observed state ──► verification report ──► HTML
 (check)            (trace)            (replay, opt-in)          (replay)            (replay --output)       (report)
```

Every arrow is a separate command; only `replay` executes code.
`midwrite-core` and `midwrite-analyzer` do not depend on `midwrite-replay`,
and the CLI builds without it (`--no-default-features`). Code:
`midwrite-replay` (`supervisor.rs`, `manifest.rs`, `safety.rs`,
`postgres.rs`, `observe.rs`, `isolation.rs`), `midwrite-core::verification`,
`runtime/node`, `midwrite-cli/src/init_replay.rs`. Tests:
`crates/midwrite-replay/tests/`, `crates/midwrite-analyzer/tests/traces.rs`,
`runtime/node/test/`.

## Harness and manifest

A harness is a directory with `midwrite-replay.json` (nothing runs without
it), the application code, SQL files for its initial state and an entry
point. Example: [`fixtures/replay/order-vulnerable`](../../fixtures/replay/order-vulnerable)
(Bun), [`fixtures/replay/drizzle-node`](../../fixtures/replay/drizzle-node)
(Node.js, real Drizzle ORM and `pg`). Nothing outside the named directory is
discovered or run.

The manifest (schema version 1) is validated before anything executes;
unknown fields are rejected and rejected values are never echoed.

| field | meaning |
| --- | --- |
| `harness`, `description` | id `[a-z0-9-]+`, free text |
| `command` | program and arguments, run from the harness directory **without a shell** |
| `runtime` | optional `node` \| `bun`: `command[0]` must start it; Node.js ≥ 22 / Bun ≥ 1.2, else blocked; the entry point must exist inside the harness (replay never compiles or installs) |
| `timeoutSeconds` | deadline per harness process, 1–300 (default 30) |
| `network` | `deny` (default) or `allow` (effective only with `--allow-network`) |
| `database` | `{"provider": "ephemeral-postgres", "setup": [...]}`; setup files inside the harness; `url`, `connectionString`, `host`, `port`, `password` are refused |
| `analysis` | what to analyze statically to link cases to findings |
| `env` | extra variables; names starting `PG`, `DATABASE`, `DB_`, `MIDWRITE`, `LD_`, `DYLD_`, `NODE_`, `BUN_`, `PATH`, `HOME`, or containing secret-like words (`SECRET`, `TOKEN`, `PASSWORD`, `KEY`, `AUTH`, …), and values containing `://`, are refused |
| `observations` | `[{"name", "sql"}]` (read-only query, at most one row and one column) or `[{"name", "events": {"topic", "measure"}}]` with `measure` `count`, `distinctIds`, `duplicates`, `withoutId` |
| `invariants` | the application contract: `[{"id", "when", "require", "requireAny"}]`; holds when some `when` condition is false, or all `require` and at least one `requireAny` hold; conditions `{"observation", "equals" \| "notEquals" \| "atLeast" \| "atMost": value}`; at least one is required |
| `control` | `{"scenario", "input"}` run once without injection; it must complete and satisfy every invariant, else every case is `INCONCLUSIVE` |
| `cases[]` | `id`; `finding` (`{id?, code?, function?, file?}`, linked to exactly one static finding whose trace is embedded); `scenario`, `input`; `inject` (`{checkpoint, occurrence, label?, mode: processKill \| exception}`); `verifyBeforeInjection` (conditions checked from a separate session while the harness waits, e.g. that a write is visible, i.e. committed; a false one cancels the injection); `predict` (reported, never used for the verdict); `recovery` (scenarios run afterwards without injection); `expect` |

A manifest, setup SQL file or compiled entry point that still contains the
`MIDWRITE-TODO` marker is refused (the error lists each JSON pointer).

## Supervisor

For one invocation the supervisor:

1. creates a private 0700 directory, short enough for Unix socket paths;
2. creates a PostgreSQL cluster in it (`initdb`, trust authentication for a
   local user) with `listen_addresses=''`: Unix socket only, in that
   directory. Server binaries come from `MIDWRITE_PG_BIN`, else `PATH`,
   else `pg_config --bindir`;
3. chooses an isolation backend (below) or refuses;
4. runs the control scenario from a fresh database;
5. for each selected case and repetition: fresh database, setup files,
   observe (`before`), start the harness with a cleared environment, answer
   its checkpoints; at the selected checkpoint run `verifyBeforeInjection`,
   then inject; wait for the harness's sessions to end, observe (`after`),
   run `recovery` scenarios, observe (`afterRecovery`), evaluate the
   invariants, drop the database;
6. stops the cluster and deletes the directory on every exit path (`Drop`),
   unless `--keep-artifacts`.

Because the harness blocks at a checkpoint until the supervisor answers,
the kill can land only at that checkpoint: no race between notification and
termination, no reliance on timing. Deadlines bound connection, every
message and the process lifetime.

## Wire protocol

JSON lines over a Unix socket at `$MIDWRITE_REPLAY_SOCKET` inside the
replay directory; one connection per harness process; protocol version 1.

```text
harness    → supervisor  {"type":"hello","protocol":1,"scenario":"completeOrder","pid":123}
harness    → supervisor  {"type":"checkpoint","seq":1,"name":"before-db-write","label":"orders"}
supervisor → harness     {"type":"continue"}
harness    → supervisor  {"type":"checkpoint","seq":2,"name":"after-db-commit","label":"orders"}
                         processKill: no answer; SIGKILL to the harness's process group
                         exception:   {"type":"throw","message":"… injected fault at checkpoint `after-db-commit`"}
harness    → supervisor  {"type":"done","outcome":"returned"|"threw","error":"...","injected":true|false}
```

A harness sends a checkpoint only after the operation it names has
completed, then waits. The supervisor selects the `occurrence`-th
checkpoint with the given name (and label). A malformed or unknown message,
a version mismatch, or a harness that never connects ends the run
`INCONCLUSIVE`. `done.injected` says whether the error a scenario ended
with is, or wraps through `error.cause`, the injected fault (Drizzle wraps
driver errors in `DrizzleQueryError`); reports record it as
`injectedFaultSurfaced`.

Harness environment: cleared, then `PATH`, a private `HOME`/`TMPDIR`,
`LANG=C`, `NO_COLOR=1`, `MIDWRITE_REPLAY=1`, `MIDWRITE_REPLAY_PROTOCOL`,
`MIDWRITE_REPLAY_ROOT`, `MIDWRITE_REPLAY_SOCKET`, `MIDWRITE_REPLAY_SCENARIO`,
`MIDWRITE_REPLAY_INPUT`, `MIDWRITE_REPLAY_PG_SOCKET`,
`MIDWRITE_REPLAY_PG_DATABASE`, `MIDWRITE_REPLAY_PG_USER`,
`MIDWRITE_REPLAY_EVENT_LOG`, plus the validated manifest `env`. The
client's `replayDatabase()` / `replayPoolConfig()` refuse a socket outside
`MIDWRITE_REPLAY_ROOT`.

## Checkpoints

Checkpoints are emitted by adapters, never by the analyzed application code
(an awaited `checkpoint()` next to a commit would itself become a MW9001).
For Node.js and Bun, `runtime/node` (`@midwrite/replay` on npm,
one dependency-free ES module for both runtimes) provides
`instrumentPgPool(pool)` for node-postgres, which wraps `pool.query`,
`pool.connect()` and the clients it returns and classifies each statement
the ORM sends by its leading keyword, and `publishToEventLog` (an fsync'ed
append used as a fake external transport).

| checkpoint (label) | emitted |
| --- | --- |
| `before-db-write` (table) | before an INSERT/UPDATE/DELETE is sent |
| `after-db-commit` (table) | after an auto-commit write was acknowledged |
| `after-tx-write` (table) | after a write inside an open transaction |
| `before-tx-commit` (`transaction`) | before COMMIT is sent |
| `after-db-commit` (`transaction`) | after PostgreSQL acknowledged COMMIT **as a commit** |
| `after-tx-rollback` (`transaction`) | after ROLLBACK, or a COMMIT that PostgreSQL answered with `ROLLBACK` |
| `before-event-publish` / `after-event-publish` (topic) | around the event-log append |

What counts as committed comes from PostgreSQL's reply: a COMMIT of an
aborted transaction returns the tag `ROLLBACK` and never yields
`after-db-commit`; a COMMIT that fails with an error yields no checkpoint.
Not understood (passed through without a checkpoint): writes inside CTEs or
database functions, savepoint statements, callback-style and cursor
queries. These names match the `checkpoints` hints of
[failure traces](../reference/trace-format.md#injectionpoint).

## Failure modes

| mode | what happens | what it does not represent |
| --- | --- | --- |
| `processKill` | SIGKILL while the harness waits at the checkpoint; no `catch`, `finally` or application rollback runs; PostgreSQL sees the connection drop and rolls back any open transaction | a database or machine crash (PostgreSQL keeps running) |
| `exception` | `checkpoint()` throws; the application's `catch`/`finally` and transaction rollbacks run | a crash: compensation in a `catch` can make an exception harmless where a crash is not |

## Verification levels

| level | produced by | meaning |
| --- | --- | --- |
| `STATIC_CANDIDATE` | `trace` | a possible failure path from source analysis |
| `SIMULATED` | `trace` | the abstract transaction model applied to the trace |
| `RUNTIME_REPRODUCED` | `replay` | the failure was injected at the selected checkpoint and a declared invariant was observed to fail |
| `RUNTIME_NOT_REPRODUCED` | `replay` | the run completed as specified and every declared invariant held; not a proof of safety |
| `INCONCLUSIVE` | `replay` | no outcome |

A run is classified in this order: a protocol, setup or pre-injection
problem → `INCONCLUSIVE`; timeout → `INCONCLUSIVE`; injection requested but
the checkpoint never reached → `INCONCLUSIVE`; a kill requested but the
process did not die by SIGKILL → `INCONCLUSIVE`; a run without injection,
or a recovery scenario, that did not complete → `INCONCLUSIVE`; any
invariant violated at any stage → `RUNTIME_REPRODUCED`; otherwise
`RUNTIME_NOT_REPRODUCED`. A case's level is its runs' common level; runs
that disagree make it `INCONCLUSIVE`.

These levels are a separate axis from `confidence`: a runtime result never
changes a static finding (`"reproduced": false` stays), and runtime results
live only in verification reports tied to a harness, its manifest hash and
its environment. Invariants come only from the harness author: a missing
event is a bug only if the declared contract says so.

**Recovery observation.** For a case with `recovery` scenarios,
`recoveryObservation` counts the runs in which every invariant held after
recovery and in which one had been violated before. It never changes the
case's level: a window that was reproduced stays `RUNTIME_REPRODUCED` even
when recovery repaired the state.

## Reports

Verification report (`kind` `midwrite.verification`, schema
[v1](../reference/schemas/verification-report.v1.schema.json),
[v2](../reference/schemas/verification-report.v2.schema.json)): `harness`
(id, manifest path and FNV-1a hash, command), `source` (Git revision of the
harness, dirty flag), `environment` (OS, architecture, PostgreSQL and
runtime versions, `isolation` backend, **names** of passed environment
variables, never values), `control`, `cases[]` (static link, embedded
trace, failure mode, checkpoint, `boundary`: `onTraceBoundary` /
`outsideTraceBoundary` / `notApplicable`, runs with checkpoint logs,
observations per stage, invariant results, predictions, recovery runs, exit
and outcome, redacted logs ≤ 16 KiB per stream), `summary`, `limitations`.
Version 2 adds the isolation backend, `recoveryObservation` and
`injectedFaultSurfaced`. With the same harness and environment two reports
differ only in `durationMs`. Temporary paths become `<replay-root>` and
`scheme://user:password@` becomes `user:***` before anything enters a
report.

Regression report (`replay --baseline OLD --regression-output FILE`,
`kind` `midwrite.regression`,
[v1](../reference/schemas/regression-report.v1.schema.json)): compares
cases by stable key; a case that changed between reproduced and not
reproduced needs review (exit 1); an inconclusive or unexecuted case is
never a pass (exit 3).

## Harness generation (`init-replay`)

`midwrite init-replay --trace trace.json --finding <ID> --output DIR`
writes an editable skeleton for one finding: the manifest (runtime,
command, disposable database, `analysis` pointing back at the project so
fingerprints match, placeholder observations and invariant, control run,
`kill-`/`exception-` cases at the trace's boundary checkpoint, a case at
the window's other end and two negative cases, each linked by exact
fingerprint, **no `expect`**), `harness.ts` (instrumented pool and event-log
wiring, a scenario stub, the source spans of the trace's operations),
`schema.sql` (placeholder plus the tables the trace touches),
`package.json`/`tsconfig.json`, a `README.md`, `finding.json` and a vendored
copy of `runtime/node`.

Not generated, on purpose: SQL queries, topics, inputs, business
invariants, expected outcomes. A guessed query that happens to run would
look like evidence; every unknown is a `MIDWRITE-TODO` that blocks replay.
Before writing, the generator re-runs the static analysis as the manifest
will and warns if that does not reproduce the finding.

Safety: it executes, installs and contacts nothing; writes only into
`--output`, which must lie inside `--project` and is not the project root;
refuses existing files without `--force` (`create_new`, so links are never
followed) and resolves every target directory first (a pre-existing
symlinked subdirectory is refused); replaces control characters in text
interpolated into comments, so a tampered trace cannot inject TypeScript or
SQL. A finding without a trace, or an `incomplete` trace without
`--allow-incomplete`, is refused.

Still manual: a seam to hand the application a pool and a transport,
observations and invariants, compiling the sources (the generated
`tsconfig.json` suits plain `NodeNext` projects), adapters for drivers other
than node-postgres, and refreshing the vendored client after an upgrade.

## Isolation

| | macOS `sandbox-exec` | Linux bubblewrap |
| --- | --- | --- |
| network | IP traffic denied; Unix-socket connections only inside the replay root (blocks e.g. `/tmp/.s.PGSQL.5432`) | network namespace with loopback only |
| filesystem | writes denied outside the replay root; reads allowed | read-only view of system directories, the harness project (its Git work tree) and the runtime's installation; the replay root is the only writable host directory; fresh `/proc`, minimal `/dev`, private `/tmp`; `/run`, `/var`, `/home` not mounted |
| processes | one process group per run, SIGKILL on kill and timeout; processes that leave the group (`setsid`) can outlive the run | new user, PID, IPC, UTS and cgroup namespaces, `--die-with-parent`; detached grandchildren end with the PID namespace |

Before the first run on Linux the exact bubblewrap argument list is probed
with `true`; if bubblewrap is missing or cannot create namespaces (kernel
settings, Ubuntu's AppArmor restriction, container seccomp or masked
`/proc`), replay is **blocked** (exit 3). Bind-mounting the host `/proc`
as a fallback was rejected: it would expose other processes' environments.
Running without isolation requires both `"network": "allow"` and
`--allow-network` and is recorded as `environment.isolation: "none"`.
Bubblewrap was chosen over hand-written `unshare(2)` setup (riskier after
`fork` in a multithreaded process) and rootless containers (a second
container runtime per run).

The harness is **trusted code**: isolation protects a cooperative harness
against accidents (a real database, a real payment service, inherited
credentials, leftovers). It is not a sandbox for malicious code
([security](../reference/security.md)). Not protected: reads of files you
can read (on Linux, a `.env` inside the project is readable), a live Unix
socket inside the visible paths on Linux, CPU/memory/disk exhaustion within
the deadline, another process of the same user connecting to the private
PostgreSQL socket during the run, and kernel or sandbox bugs.
`sandbox-exec` is deprecated by Apple; its effectiveness is checked by the
test suite on every run.

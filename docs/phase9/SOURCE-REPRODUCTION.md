# Reproducing in-flight failures against the original code

How a model counterexample (or recovery probe) with a lost acknowledgment, a
shared process or a pool becomes a run of the application's own TypeScript.
`docs/phase8/CRASH-RETRY-SEMANTICS.md` still describes the crash/retry
machinery; this note covers what Phase 9 adds.

## Commands

```bash
commitrift model-check scenario.json                                  # model only; never executes code
commitrift model-check scenario.json --fault-mode in-flight           # crashes only with an operation in flight
commitrift model-check scenario.json --verify-source --allow-execution
commitrift model-check scenario.json --verify-source --allow-execution --fault-mode in-flight --source-repeat 5
```

`--fault-mode in-flight` is `bounds.crashPolicy: "in-flight-only"`: a process
crashes only with a COMMIT or an autocommit write in flight (it needs
`bounds.maxInFlight ≥ 1` and `maxCrashes ≥ 1`, else the command refuses). The
default `any` keeps every Phase 8 crash point and adds the in-flight ones.
`--max-in-flight N` overrides the budget. Source execution still requires
`--allow-execution`; nothing else enables it.

Prerequisites for the first-party fixture: PostgreSQL 14+ binaries, Node ≥ 22,
`npm ci` in `fixtures/phase9/ledger-app` (it uses the lockfile's pinned
`drizzle-orm 0.45.4`, `pg 8.23.1`; `@commitrift/replay` is the local
`runtime/node`).

## From schedule to plan

`source_verify.rs::plan` walks the schedule on the model and emits plan steps:

| model transition | plan step(s) |
| --- | --- |
| statement/BEGIN/COMMIT/effect | `Release` (as in Phase 7/8) |
| `CrashInFlight(mask)` | one `InFlight` per actor whose operation is in flight (fate from its bit), `Crash` steps for the other actors of the process (checkpoint: announced and held, waiting for a lock, or waiting for a pool connection) |
| `ServerComplete` right after a crash | folded into that crash: fate `answered-and-withheld` |
| `ServerComplete` later | `ServerComplete`: the proxy delivers the held operation |
| termination counterexample | the schedule, then `Stuck` |
| `LostCommitted`/`LostNotCommitted` (a live process with a broken connection) | refused: **UNSUPPORTED** (no realization yet; never approximated by a crash) |
| a `Retry` of an actor in a shared process | refused: **UNSUPPORTED** |

The model's durable state right after a kill includes the completions the
server performs at once; the report compares it with PostgreSQL's rows.

## What the coordinator does at an in-flight step

1. waits for the operation to be *announced and held* (driver checkpoint) and
   matches it with the model's (kind, table, source region);
2. arms the proxy for that connection (backend pid, else `application_name`);
3. releases the operation: it really goes onto the wire;
4. waits for the proxy's **own** event — `OperationHeld` or
   `OperationCompleted` — else INCONCLUSIVE;
5. samples `pg_stat_activity` for the backend (`idle in transaction` for a held
   COMMIT);
6. with every other crash/in-flight step of the same process established,
   `SIGKILL`s the worker's process group **once**;
7. per fate: `not-reached` → discard + wait for PostgreSQL to end the session;
   `answered-and-withheld` → wait for the session to end; `held-for-later-delivery`
   → check the backend is still `idle in transaction`, remember the
   connection for the later `ServerComplete`;
8. reads the modeled tables (the durable state), records the evidence;
9. later, `ServerComplete`: delivers the held frames (+ `Terminate`), waits for
   the server's answer (`CommandComplete COMMIT`), waits for the backend to end,
   reads the tables again.

## Report additions (schema 4, `docs/phase9/schemas/`)

| field | content |
| --- | --- |
| `epistemic.twins[]` | per lost acknowledgment: actual/other world, knowledge, twin status, both worlds' invariants, shared observations, first difference, reading |
| `sourceVerification.recovery.crashes[].inFlight` | operation, model fate vs observed fate, mechanism, forwarded/withheld, **server answer observed**, client acknowledged (false), backend state before the kill, session open after the kill, wire events, application knowledge ("ambiguous") **and** physical outcome, kept apart |
| `sourceVerification.recovery.completions[]` | the held operation delivered: answer, session ended, model vs runtime rows |
| `sourceVerification.wireLog` | redacted proxy log (first run) |
| `sourceVerification.stuck` | model kind, observed kind, quiet time, pool counters, lock waits, unfinished tasks, confirmed |
| `sourceVerification.sharedProcesses` | processes that ran several tasks, with the pool capacity verified |
| `exploration.stuck` | classification of stuck states |
| `bounds.reduction` | `dpor` when it ran |

## Evidence levels

| level (`evidence.level`) | `sourceVerification.evidence` | meaning |
| --- | --- | --- |
| `model-counterexample` | — | the model found it; nothing ran (MODEL_COUNTEREXAMPLE) |
| `postgres-confirmed-schedule` | — | the model's SQL replayed on PostgreSQL, including held COMMITs (POSTGRESQL_MODEL_REPLAY_CONFIRMED) |
| `original-code-reproduced` | — | the application ran under the schedule; for a stuck state: observed (ORIGINAL_CODE_RUNTIME_REPRODUCED) |
| `original-code-in-flight-failure-reproduced` | `ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED` | an operation was in flight when the worker was killed, the wire and PostgreSQL established what the server saw, every comparison agreed, the invariant is violated on the observed state |
| `original-code-recovery-reproduced` | `ORIGINAL_CODE_RECOVERY_REPRODUCED` | no counterexample; a probe with a real fault and retry matched and the invariant holds (one schedule) |
| `original-code-crash-reproduced` | `ORIGINAL_CODE_CRASH_REPRODUCED` | Phase 8 (crash at a checkpoint, no operation in flight) |
| `runtime-inconclusive` / `INCONCLUSIVE`, `UNSUPPORTED`, `DIVERGED`, `NOT_REPRODUCED` | | never upgraded |

Phase 8 labels are not redefined; Phase 9 adds one level and the fields above.

## Reproducing the first-party results

```bash
(cd fixtures/phase9/ledger-app && npm ci --ignore-scripts && npx tsc -p tsconfig.json)
cargo build --release
scripts/phase9-evidence.py reports/phase9/evidence 5      # every scenario, 5 runs each
cargo test -p commitrift-cli --test phase9                # 20 end-to-end tests
```

Each run uses its own disposable cluster and private directory; nothing is
shared between runs or tests (`TEST-RELIABILITY.md`).

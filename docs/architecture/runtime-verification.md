# Runtime verification

Crates and packages: `midwrite-replay` (Rust), `runtime/node`
(`@midwrite/replay`, JavaScript, unpublished), `midwrite-model`
(`oracle.rs`, `source_verify.rs`). Everything here is opt-in; nothing runs
without `--allow-execution` and an authorized manifest, except the
PostgreSQL oracle, which runs only the model's SQL.

## Components

| component | file | role |
| --- | --- | --- |
| disposable PostgreSQL | `replay/src/postgres.rs` | `initdb` + `pg_ctl` into a private temporary directory, Unix socket only, deleted afterwards; server binaries from `MIDWRITE_PG_BIN` or `PATH` |
| OS isolation | `replay/src/isolation.rs` | macOS `sandbox-exec` (IP networking denied, writes confined), Linux bubblewrap (new user, network (loopback only), PID, IPC, UTS and cgroup namespaces, minimal read-only filesystem view) |
| manifest validation | `replay/src/manifest.rs`, `safety.rs` | `midwrite-replay.json` / `midwrite-source.json`: files inside the harness, `network: deny`, no external database URLs or credential-like variables, no `MIDWRITE-TODO` marker |
| single-request replay | `replay/src/supervisor.rs`, `observe.rs` | Phase 3/4 checkpoint protocol: run a control scenario and each case from a fresh database, inject a kill or exception at a named checkpoint, observe surviving state, check declared invariants |
| source coordinator | `replay/src/source.rs` | Phase 7+: one worker per actor (or a shared process), plan-ordered release of held statements, lock-wait observation, crash/restart, effect doubles, response checkpoint |
| protocol fault proxy | `replay/src/proxy.rs` | Phase 9/10: sits between a worker and PostgreSQL; applies the in-flight fates (not-reached, answered-and-withheld, held-for-later-delivery) and severs client sockets for live connection loss |
| catalog reads | `replay/src/catalog.rs` | schema drift comparison (`midwrite schema --migrations`) |
| worker client | `runtime/node/source.js`, `pg.js`, `index.js`, `event-log.js` | wraps the application's pool and Drizzle builders without changing application code; announces each statement, holds it, reports results and SQLSTATEs; attributes statements to source frames |
| differential comparison | `model/src/source_verify.rs` | turns a counterexample into a plan, compares statements, cells, rows, outcomes, effects, crashes and return values with the model, assigns the evidence level |
| lockstep oracle | `model/src/oracle.rs` | replays a model schedule as SQL, one session per actor |

## Original-code run, step by step

1. Refuse unless `--allow-execution`; load and check the manifest and the
   scenario's `sourceVerification` mapping.
2. Create a private run directory and a disposable cluster; apply the
   harness's schema SQL; seed the scenario's initial rows and sequence
   starts.
3. Start one isolated worker per actor (`application_name`
   `midwrite-actor-<name>`); each imports the unchanged application
   through the harness.
4. Release statements in the counterexample's order; observe each result
   or SQLSTATE and each lock wait (`pg_stat_activity`); a statement the
   model does not expect at that point diverges the run.
5. Faults: SIGKILL of the worker's process group at a verified checkpoint
   (an announced-and-held operation, a confirmed lock wait, or — Phase 13 —
   the held response); PostgreSQL session termination observed;
   `client_connection_check_interval` set for crash plans; a retry runs as
   a new process. In-flight and connection-loss fates go through the proxy.
6. Read the committed rows (after a crash and at the end); match generated
   values chronologically (crash checkpoints before final rows, a Phase 13
   harness fix); evaluate the invariant on the observed rows, effect log
   and return values.
7. Repeat on a fresh database (`--source-repeat`); classify
   (`original-code-reproduced`, … `diverged`, `unstable`).

Protocol and design: [Phase 7 runtime scheduler](../phase7/RUNTIME-SCHEDULER.md),
[Phase 7 differential verification](../phase7/DIFFERENTIAL-VERIFICATION.md),
[Phase 8 crash/retry semantics](../phase8/CRASH-RETRY-SEMANTICS.md),
[Phase 8 postgres-js adapter](../phase8/POSTGRES-JS-ADAPTER.md),
[Phase 9 protocol fault injection](../phase9/PROTOCOL-FAULT-INJECTION.md),
[Phase 10 runtime verification](../phase10/RUNTIME-VERIFICATION.md),
[Phase 13 return values and responses § 5](../phase13/RETURN-VALUES-AND-RESPONSES.md#5-original-code-verification-b8-vertical-slices-2-and-3).

## Worker protocol additions

| phase | addition |
| --- | --- |
| 7 | announce/hold/release of statements; Drizzle frame attribution |
| 8 | crash and restart plan steps; external effects to coordinator-hosted doubles; postgres-js adapter (version-pinned) |
| 9 | proxy-mediated connections; shared processes; pools |
| 10 | live connection loss; postgres-js in-flight faults |
| 13 | `respond` message (active only with `MIDWRITE_SOURCE_RESPONSE=hold`, after the harness entry returned); return values reported |

## Guarantees and non-guarantees

* The harness is trusted and cooperative. Isolation protects against
  accidents in a trusted harness; it is **not** a sandbox for malicious
  code ([security](../reference/security.md)).
* Neither platform limits CPU or memory beyond per-worker rlimits.
* A result is evidence about one schedule of one first-party application
  with pinned driver versions on one PostgreSQL version.

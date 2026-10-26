# Crash and retry semantics against original code

`crates/commitrift-replay/src/source.rs` (coordinator),
`crates/commitrift-model/src/source_verify.rs` (plan, prediction,
comparison), `runtime/node/source.js` (worker client, protocol 3).

## What a crash is

A crash is a **SIGKILL of the worker's process group**, sent by the Rust
coordinator — never an exception thrown into the application. Consequences
are therefore the operating system's and PostgreSQL's, not the model's:

| | how it happens | how it is observed |
| --- | --- | --- |
| application-local state ends | the process is gone | `waitpid` status: terminated by signal 9 (`killedBySignal`) |
| `catch`/`finally` do not run | no JavaScript runs after SIGKILL | (by construction) |
| client connections close | the kernel closes the sockets | — |
| uncommitted transactions are discarded | PostgreSQL ends the backend when it notices the lost client | every session of the execution's `application_name` gone from `pg_stat_activity` (`sessionsEnded`, `sessionsEndedMs`), with a 10 s timeout → otherwise **inconclusive** |
| committed changes stay | PostgreSQL | the modeled tables read right after the sessions ended, compared with the model's committed rows after the crash transition (`modelTables` vs `runtimeTables`) |
| performed external effects stay | the channel test double lives in the coordinator | the effect log is unchanged by the kill |

A worker that leaves its process group (`setsid`, Node's
`detached: true`) is found after the run by a per-run random token in its
environment and killed (macOS: `ps -E`; Linux: `/proc/*/environ`, and
bubblewrap's PID namespace already kills it). A worker whose coordinator
socket closes exits by itself. The coordinator kills every worker on every
exit path, including a panic (`Drop`).

## Crash boundaries (checkpoints)

The model crashes a process between two transitions. At runtime that
boundary must be an observable checkpoint; the plan says which one, and the
coordinator verifies it before killing:

| model state of the actor at the crash | runtime checkpoint | how it is verified |
| --- | --- | --- |
| next operation is a statement, BEGIN, COMMIT, ROLLBACK or an external effect | the worker has **announced** that operation and it is **held** (never sent) | the held operation must match the model's next operation (kind, table/channel, source region, idempotency key), otherwise the run **diverges** |
| a statement waits for a lock and its blocker is still active | the statement is **in flight on the server and waiting** | `pg_stat_activity.wait_event_type = 'Lock'` re-checked right before the kill; if PostgreSQL no longer reports the wait, **inconclusive** |
| a statement waited, the blocker has finished, the model has not resumed it yet | in flight: PostgreSQL may already have executed it | allowed only inside a transaction (which never commits: the effect is the same either way); for an **autocommit** statement the plan is **refused** — PostgreSQL may have committed it, which the model rolls back |
| several tasks of the actor run (`Promise.all`) | not observable as one checkpoint | refused |

So "after a statement completes", "after COMMIT acknowledgment", "before an
external publication", "after an external publication" and "before the
application response" are all the first row: the previous operation's
result (or effect receipt) reached the worker, and the next operation was
announced. "During an in-flight operation" is the second and third rows.
The coordinator never kills while a released statement is running and not
waiting: the model has no transition there, and the outcome of such a kill
(did COMMIT reach the server?) would be ambiguous.

### Why `client_connection_check_interval`

Measured (`reports/phase8/connection-check-experiment.txt`): with
PostgreSQL's default (`0`), a backend **blocked on a lock** does not notice
that its client was killed; after 10 s its session was still there, and the
run was reported **inconclusive**. With `client_connection_check_interval =
50ms` (set only for plans that contain a crash, PostgreSQL 14+) the session
ended 47 ms after SIGKILL. Consequence for users: the model's "a crash
releases its locks at once" holds on servers with that setting; on a
default production server a crashed transaction can keep its locks until the
backend next touches the socket. The report lists the setting
(`recovery.serverSettings`).

## Retries are declared, never invented

A retry is a **new execution**: a new worker process (new pid, new
connections, attempt number + 1, `application_name` `commitrift-actor-<name>#<attempt>`),
started by the coordinator only where the model has a `Retry` transition,
which exists only when the scenario declares a mechanism:

| mechanism | scenario | runtime |
| --- | --- | --- |
| client resubmits after no response | `retry: {trigger: "onCrash"}` | coordinator restarts the harness entry with the original input |
| queue redelivery (at-least-once, lost acknowledgement) | `trigger: "atLeastOnce"` | same; also after a successful execution |
| supervisor restarts a dead worker | `trigger: "onCrash", guaranteed: true` | same |
| transaction retry after 40001/40P01 | application code (`withTransactionRetry`) bound with `extraction.transactionRetries` | **the application's own loop runs** in the same process; the model's `Begin { retry }` predicts BEGIN/40001/ROLLBACK/BEGIN |
| retry of a single SQL statement | not expressible | — |

Without a declared mechanism there is no retry: the dual-write scenario
crashes and stops, and its report says "no recovery mechanism is declared"
(test `a_retry_mechanism_is_never_invented`). For the first three
mechanisms, the coordinator plays the client, queue or supervisor: that is a
**declared test contract**, reported in `recovery.contracts`, not evidence
about a real queue.

## External effects and exactly-once

Effects (payments, events, e-mails, notifications) reach a **test double
hosted by the coordinator** through the application's own provider seam:
the harness implements the seam with `sourceEffect(channel, {key,
payload})`, which is held and released like a statement. The double records
the effect when released and applies the channel's **declared**
deduplication (`none` or `byKey`). Consequences:

* an effect performed before a crash stays performed (the double outlives
  the worker);
* "exactly once" is never inferred from a key: with `dedup: none` repeated
  keys are repeated effects; with `byKey` the double suppresses them — an
  assumption about the provider, listed as such;
* database-enforced idempotency (a claim row with a unique index) is the
  database's semantics and is observed as such (`job_effects`), including
  its other side: a crash between claim and effect loses the effect
  (at-most-once).

## Quiescence and liveness

The report says what the final state means (`recovery.quiescence`):
every execution ended; whether a further optional retry is still possible
within the bounds; whether the retry budget is exhausted ("further retries
are outside the bounds, not excluded"). No eventual-recovery or
eventual-delivery claim is made from a bounded run; a guaranteed retry is a
declared fairness assumption.

## Evidence classification (report schema 3)

`sourceVerification.evidence`, only for schedules with a crash or retry or
for recovery probes (Phase 7 runs keep their meaning):

| value | meaning |
| --- | --- |
| `ORIGINAL_CODE_CRASH_REPRODUCED` | a counterexample with a crash and/or retry: every crash checkpoint, termination, restart, statement, effect and outcome matched; the invariant is violated on the observed state |
| `ORIGINAL_CODE_RECOVERY_REPRODUCED` | no counterexample within bounds; the model's recovery schedule (`recoveryProbe`) with a real crash/retry ran, matched, and the invariant **holds** |
| `ORIGINAL_CODE_INVARIANT_HELD` | the same for a probe without crash or retry |
| `DIVERGED`, `INCONCLUSIVE`, `UNSUPPORTED`, `NOT_REPRODUCED` | as named; never upgraded |

`evidence.level` gains `original-code-crash-reproduced`,
`original-code-recovery-reproduced`, `original-code-invariant-held`.
`MODEL_COUNTEREXAMPLE` and `POSTGRESQL_MODEL_REPLAY_CONFIRMED` are the
existing levels `model-counterexample` and `postgres-confirmed-schedule`.

## Recovery probes

When a variant has **no** counterexample, there is no schedule to run. A
probe names events (`external:card-charge`, `crash:Client`,
`retry:Client`, `blocked:Accept2`, `error:40001`, ...); the model checker
selects the **shortest complete schedule** that contains them in order
(breadth-first over the same transition relation as every counterexample;
`model/src/probe.rs`). The schedule is the model's, never hand-written; its
final state is quiescent and satisfies the invariant in the model, and the
original code must reproduce it with the invariant holding. One schedule is
bounded evidence, not a proof.

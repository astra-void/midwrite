# Phase 9 limitations

Everything in `docs/phase8/LIMITATIONS.md` still holds unless it is listed as
resolved here.

## Resolved or narrowed by Phase 9

| Phase 8 limitation | now |
| --- | --- |
| a kill while a released COMMIT runs is not planned; "lost acknowledgment" is a crash between operations | modeled (in-flight operations, orphans, `ServerComplete`) and reproduced at runtime through the proxy: held COMMIT, answered-and-withheld COMMIT, not-reached COMMIT, for `pg` + drizzle |
| the client's ignorance of the outcome is not represented | worlds, `OutcomeKnowledge`, twin analysis in the report |
| a reconciliation read can not race with a COMMIT the server has not completed | it can (`held-for-later-delivery`); `ledger-reconciled-commit-in-flight` |
| actors in one process are refused for source verification | allowed for crashes and tasks; one SIGKILL for the whole group |
| no connection pool in the model | bounded FIFO pools, starvation and wait cycles classified and confirmed at runtime |
| the model's visited table grows with the size of a state | fragment interning + varint key table: 33× less visited memory on 6 actors, exact identity (`STATE-SPACE-STORAGE.md`) |

## In-flight semantics

* **`LostCommitted` / `LostNotCommitted` have no runtime realization.** They are
  a live process whose connection broke after submitting an operation. The model
  explores them; a counterexample using one is refused with UNSUPPORTED and is
  never approximated by a crash. Model-only evidence (MODEL_COUNTEREXAMPLE) and
  PostgreSQL replay of the SQL are the strongest evidence for those schedules.
* **Effect response loss is not realized.** An external effect whose response is
  lost (the provider acted, the client did not hear) is modeled for effects the
  Phase 8 doubles already cover (crash after the effect); the runtime does not
  withhold effect responses.
* **At most `maxInFlight` ≤ 8 operations per fault event**; the budget is per
  fault event, not a global concurrency bound.
* **One in-flight operation per connection.** Pipelined statements are not
  modeled.
* **Orphans complete atomically.** A held COMMIT either commits (with its
  effects) or the server drops it; partial completion and server-side timeouts
  (`idle_in_transaction_session_timeout`, `statement_timeout`) are not modeled
  beyond "the server may complete at any later time".
* **Network behavior is limited to the three fates.** No delay distributions,
  reordering, duplication or partial writes.

## Runtime verification

* **Drivers.** `pg` (node-postgres 8.23.1) with drizzle-orm 0.45.4 is covered.
  postgres-js has no proxy support and its in-flight scenarios are not
  reproduced (UNSUPPORTED).
* **Retries of an actor in a shared process are refused** (UNSUPPORTED): a retry
  needs a new process while its siblings keep running, which the group model
  does not express.
* **A proxy needs a Unix socket** and the harness's own pool configuration; an
  application that opens its own connection string is not covered.
* **One schedule per run.** Recovery probes (no counterexample) check the
  shortest schedule that realizes the required faults; they are bounded evidence,
  not proofs. Repeat counts are reported, not extended into a guarantee.
* **Process-level isolation only.** The worker sandbox (sandbox-exec,
  bubblewrap) is for authorized, cooperative harnesses; a hostile harness is out
  of scope (`SECURITY.md`).
* **No cap on temporary disk and no memory cap on the worker beyond rlimits.**

## State-space exploration

* **DPOR (`--dpor`) is evidence-checked, not proved.** Terminal-state set equality
  with the unreduced search holds on 14 targeted workloads and 150 random
  scenarios per test run (plus 700 seeds once, release mode, 364 s); a missed
  interleaving elsewhere cannot be excluded.
* **DPOR preserves safety and stuck-state reachability only.** It is refused with
  an `always` invariant and with `maxContextSwitches`; it is not a liveness check.
* **DPOR executes more transitions than sleep sets plus the state cache** on
  every workload measured; its value is memory, not time (`PERFORMANCE.md`).
* **Reduction is weak with faults on.** Crashes, retries and in-flight faults are
  conservative (`Access::Global`), so neither reduction helps much there.
* **The state space is still exponential in actors.** 8 actors on the benchmark
  workload finish (4.5 M states); 9 were not run. Bounds stay explicit and
  truncation is reported, never silent.

## Pools

* A pool is a count with FIFO leases; real pools' idle timeouts, connection
  recycling, `maxUses`, health checks and priority are not modeled.
* The scenario's pool capacity must equal the harness pool's `max`; a mismatch
  is refused rather than reconciled.
* Stuck confirmation at runtime relies on quiescence plus pool counters and
  `pg_blocking_pids`; a task blocked by something neither shows is reported as
  unexplained.

## What is not claimed

* No guarantee that an application is correct when no counterexample is found:
  the result is "none within bounds", for the modeled faults, drivers and
  schedules.
* No claim about production behavior: every run uses a disposable local
  PostgreSQL.
* No independent human review of the semantics, the protocol implementation or
  the scenarios was performed.

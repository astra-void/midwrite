# Phase 10 limitations

Everything in `docs/phase9/LIMITATIONS.md` still applies unless listed as
lifted below.

## Lifted in Phase 10

* `LostCommitted` / `LostNotCommitted` with a live process: realized against
  original code for a COMMIT or an autocommit write (`LIVE-CONNECTION-
  FAILURES.md`).
* postgres-js in-flight faults: verified for postgres-js 3.4.5 (both fault
  families; `POSTGRES-JS-IN-FLIGHT.md`).
* DPOR performance: the classic search's redundancy is explained and a
  source-set DPOR removes most of it (`STATE-SPACE-PERFORMANCE.md`).
* The Phase 9 DPOR soundness defect on conditionally enabled transitions
  (last race only) is fixed in the classic search.

## Reductions

* Source-set DPOR is **evidence-checked, not proved** (`POR-CORRECTNESS.md`
  §6). The state-cache summary argument and the combination of source sets,
  sleep sets and state-dependent enabling are tested against the unreduced
  search on generated and targeted models, not established in general.
* Three defects were found in DPOR variants during Phase 10; each is a
  regression fixture. The generators may not express the next one: several
  kinds of footprint access (`RowRead`, `LockCheck`, `Wait`, `Seq`, `Budget`)
  are deleted without any workload noticing (believed redundant; analyzed,
  not proved).
* DPORs are refused with `always` invariants and `maxContextSwitches`; no
  argument for lifting this was made.
* No liveness or fairness property is checked by any search.
* The DPOR comparison oracle identifies terminal states that differ only in
  the interleaving of external effects of different channels. A property that
  depended on that interleaving could not be expressed by the invariant
  language today, and is not preserved by any reduction.
* Sleep sets remain the default.

## Live connection failures

* Only a closed socket is realized: no timeouts, no `57P01`/`57P02`, no server
  that closes first, no partial frames, no TLS.
* Only a COMMIT or an autocommit write loses its connection; a loss in the
  middle of a transaction, or during a read, is refused (UNSUPPORTED).
* At the loss the actor must have one running task.
* The node-postgres harnesses attach a client `error` listener; without one
  the process dies (recorded as DIVERGED). Applications that rely on a
  different setup were not studied.
* Statements on a broken node-postgres client are recognized through driver
  internals verified on pg 8.23.1; other versions fail closed (unmatched
  statements), they are not silently accepted.

## postgres-js

* 3.4.5 only. Backend pids of postgres-js connections are not visible to the
  worker; faults are armed by `application_name`.
* Through the fault proxy `sql.end()` without a timeout did not return, with
  or without a broken connection; the harness ends with `end({ timeout: 5 })`.
  The cause of this interaction was not established.
* Not run on postgres-js: bounded pools, REPEATABLE READ/SERIALIZABLE, shared
  processes.

## Evidence scope

* All original-code evidence is from first-party fixtures written for it.
  No external application was executed in Phase 10, and no finding was
  reviewed by a person outside this work.
* A reproduction is one schedule of one application with pinned driver
  versions, repeated; it does not generalize to other schedules, versions or
  applications.
* PostgreSQL: 17.11 on macOS (as in Phase 9), 15 in the Linux container.
  Behavior is compared across these two versions only.

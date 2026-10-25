# Semantics of the Phase 6 model

This is the reference semantics every Phase 6 claim is relative to. Model
version `1.0.0` (`report.modelVersion`); a change that can alter a verdict
changes the version.

## Global state

`State` (`crates/commitrift-model/src/state.rs`) consists of:

* **store** — for each row id, a chain of versions `(values | deleted,
  creator transaction, commit sequence | uncommitted)`, and the row-lock
  table `row → transaction`;
* **commit sequence** — a counter incremented by every commit that wrote;
* **transactions** — active ones (and, when the scenario uses
  SERIALIZABLE, committed ones): isolation, snapshot (taken at the first
  statement for REPEATABLE READ and SERIALIZABLE), written rows, implicit
  (autocommit) flag, SERIALIZABLE predicate reads;
* **dependency edges** between SERIALIZABLE transactions (wr, ww, rw);
* **actors** — locals, tasks (code, program counter, status, parent,
  detached flag, pending statement), open transaction, the state saved at
  `BEGIN` for transaction retries, outcome, successful completions, attempt,
  per-actor transaction and insert counters;
* **in-process variables** per process; the **external effect log**;
* **crashes** and **retries** used.

Row ids are `(table, inserting actor, per-actor counter)` and transaction ids
`(actor, per-actor counter)`, never global counters, so independent inserts
and transactions commute.

## Transitions

A transition is `(kind, actor, task)`; `enabled(state)` lists them in a fixed
order (kind, actor, task) — exploration is deterministic.

| Kind | Enabled when | Effect |
| --- | --- | --- |
| `Step` | the task is running, not waiting, and its next operation is visible (a statement, `BEGIN`, `COMMIT`, `ROLLBACK`, an external effect, an in-process read/write, `Unknown`); statements on the actor's transaction connection are serialized (one at a time per actor, as on one connection) | executes that operation, then *settles* the actor |
| `Resume` | the task's pending statement waits for a transaction that has ended | continues the statement with its original snapshot and remaining rows |
| `Deadlock` | the task's pending statement is part of a wait-for cycle | aborts that statement's transaction with SQLSTATE 40P01. Any member of the cycle can be the victim: PostgreSQL's choice depends on which backend's `deadlock_timeout` fires after the cycle formed |
| `Crash` | crashes < `maxCrashes`, and some actor of the process is running and has done something | every running actor of the process stops (outcome *crashed*); their open transactions are rolled back; in-process variables reset; committed data and performed external effects remain |
| `Retry` | retries < `maxRetries`, the actor declares an operation retry whose trigger matches its outcome, and all its tasks have finished | a new attempt from the start with the original arguments; earlier effects and committed data remain |

**Settling** executes local operations (assignments, branches, jumps,
spawning tasks, joins whose children have finished, `return`, `throw`) until
every task stands at a visible operation, waits at a join, or has finished.
Local operations only touch the actor's own locals: they commute with every
other actor's transitions, so folding them is exact for every property this
checker evaluates (they are recorded in the witness as `CHECK` / assignment
lines).

**Statement granularity.** A statement executes atomically unless it must
wait: it takes its snapshot, finds its candidate rows and processes them in
key order; if a row is locked by another active transaction, or a unique key
is being written by one, the statement records its progress and becomes
*pending*. A waiting statement keeps its snapshot. Interleavings *inside* a
non-waiting statement are not explored; for single-row statements the
outcome of such an interleaving equals one of the explored orders (the
re-check after a concurrent commit gives the same result as running before
or after it); for multi-row statements this is an approximation (see
[`LIMITATIONS.md`](LIMITATIONS.md)). Autocommit statements (`Conn::Pool`)
commit within the same step.

**Errors.** A statement error (constraint violation, serialization failure,
lock not available) aborts the transaction and raises an application
exception in the task. A `throw`, an evaluation error (`TypeError` on
`null.stock`, overflow) or a failed child task (at its `join`) fails the
task; a failing main task fails the actor and rolls back its open
transaction. An unawaited task that fails crashes its process (Node.js's
default for unhandled rejections). A transaction declared with `retry n`
re-executes from its `BEGIN`, with the locals saved there, after 40001/40P01,
up to `n` times and within `maxRetries`.

## Quiescence and invariants

A state is **quiescent** when no task can progress by itself and no
*guaranteed* retry is pending — only optional environment transitions (a
crash, an optional retry or redelivery) may remain. `final` invariants are
evaluated in every quiescent state in which all tasks finished; `always`
invariants in every reachable state. A quiescent state with unfinished tasks
is *stuck* and makes the result `INCONCLUSIVE` (none has been observed).
Retries and crashes are optional unless a retry is declared `guaranteed`:
both "the client retries" and "the client gives up" are explored.

## State equality

Two states are merged when their **canonical encodings** are equal
(`State::canonical`). The encoding contains everything that can influence
future behavior: versions with their images and creators, locks,
transactions, dependency edges, every task's program counter, locals,
pending statement, outcomes and counters, in-process variables, the full
external log, crashes and retries. It replaces each commit sequence number by
its *visibility* to each live snapshot (the active transactions' snapshots
and the snapshots held by waiting statements): a new snapshot sees every
committed version, so the absolute numbers carry no further information. It
omits the step count and, unless a context-switch bound is active, the
previous task. States with the same database contents but different program
counters, snapshots, locks or logs are never merged (tested).

## Bounded search

Schedules are explored up to the bounds of
[`SCHEDULE-EXPLORATION.md`](SCHEDULE-EXPLORATION.md). Semantic bounds
(`maxCrashes`, `maxRetries`, `maxContextSwitches`, `maxActors`) define the
system that is verified; resource bounds (`maxStates`, `maxTransitions`,
`maxSteps`, `maxWallMs`, `maxMemoryBytes`) only cut the search, and hitting
one makes the result `INCONCLUSIVE`.

## Isolation semantics

Summarized here, specified in [`POSTGRES-ISOLATION.md`](POSTGRES-ISOLATION.md)
and [`DATABASE-MODEL.md`](DATABASE-MODEL.md):

* **READ COMMITTED**: a snapshot per statement; a row updated or deleted by a
  transaction that committed after the statement's snapshot is re-checked
  against the predicate on its newest version (EvalPlanQual) and the write
  applies to that version or the row is skipped.
* **REPEATABLE READ**: one snapshot per transaction, taken at its first
  statement; writing or locking a row changed by a transaction that committed
  after that snapshot raises 40001.
* **SERIALIZABLE**: REPEATABLE READ plus a serialization-graph check after
  every statement and at commit: 40001 when the transaction would close a
  cycle with committed SERIALIZABLE transactions. Read sets are the exact
  logical ones; PostgreSQL's predicate locks can be coarser, so PostgreSQL
  may abort *more* (measured).

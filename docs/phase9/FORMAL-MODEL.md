# Phase 9 formal model

The Phase 6–8 transition system (`docs/phase6/SEMANTICS.md`) is extended, not
replaced. Everything below is *additive*: with `maxInFlight = 0`, no `pools`,
no `termination` option and no `process` sharing the transition system, the
canonical state encoding and every verdict are those of Phase 8 (tested:
`phase9_inflight::without_the_bound_there_are_no_in_flight_transitions`,
`phase9_process::an_unbounded_pool_changes_nothing`, the unchanged Phase 6–8
suites).

## 1. Three kinds of state

| kind | what it is | where it lives |
| --- | --- | --- |
| **physical** | what PostgreSQL (and an external system) actually did: row versions, locks, transactions, performed effects | `State::store`, `txns`, `externals` |
| **execution** | which processes, tasks, operations, retries, pool requests and server-held operations are pending | `ActorState`, `Thread`, `State::orphans`, `State::pools` |
| **knowledge** | what an application can conclude about operations whose answer it never saw | `ActorState::ambiguous` (`epistemic::Ambiguity`) |

A committed transaction and an application that knows it committed are
different states. They are different in the encoding too: the canonical
encoding contains the physical world of every unresolved outcome and the set
of worlds the application cannot exclude (`State::canonical`, trailer `0xA9`).

## 2. Operation lifecycle

An operation of a task is a COMMIT, or a write outside a transaction (an
*autocommit* write; it commits by itself). Reads and statements inside a
transaction have no durable outcome of their own: when their acknowledgment
is lost the transaction ends with the connection, which the model already
represents by a crash or a failure.

```
Prepared ──► Submitted ──► ServerReceived ──► Executing ──► ServerCompleted ──► ResponsePending ──► ClientAcknowledged
   │            │                │                 │                │                    │
   └────────────┴────────────────┴─────────────────┴────────────────┴────────────────────┴──► OutcomeUnknown
```

Not every stage can be observed, and the model does not pretend otherwise.
Each stage has an evidence class:

| stage | in the model | observed at runtime by |
| --- | --- | --- |
| Prepared | the task's next visible operation | the worker *announces* the operation (driver checkpoint) |
| Submitted | the operation was handed to the connection | the proxy saw the frames (`Message` events) — **observed** |
| ServerReceived | the operation is an `Orphan` (server holds it) or was executed | the proxy forwarded the frames — **observed**; for held operations `idle in transaction` in `pg_stat_activity` — **observed by PostgreSQL** |
| Executing / ServerCompleted | `ServerComplete` transition | `CommandComplete`/`ErrorResponse` frame — **observed on the wire**; durable rows — **observed by PostgreSQL** |
| ResponsePending | the answer exists and is not delivered | `ResponseWithheld` — **observed** (the proxy holds it) |
| ClientAcknowledged | the task continues | worker `result` message |
| OutcomeUnknown | `Ambiguity` in `ActorState::ambiguous` | **inferred**: the application sent the operation and received no answer |

*Inferred* is never reported as *observed*: in reports the `InFlightCheck`
names which facts the proxy saw and which PostgreSQL itself confirmed.

## 3. Transitions added

All are enabled only when `bounds.maxInFlight > 0` and `state.in_flight <
maxInFlight` (one budget unit per *fault event*; a crash with several
operations in flight is one event).

* **`LostCommitted(a, th)`** — the next operation of task `th` is in flight,
  the connection breaks, the process lives. Effect: the operation becomes an
  `Orphan` (the server received it and *will* complete it; see
  `ServerComplete`); the task fails with `connection terminated unexpectedly:
  the outcome of COMMIT is unknown`; the pool drops the broken connection.
* **`LostNotCommitted(a, th)`** — the same observation for the application,
  but the server never gets the operation (or rolls it back): the transaction
  aborts, nothing is durable.
* **`CrashInFlight(p, mask)`** — process `p` crashes while it has operations in
  flight (`engine::in_flight_ops`: at most eight, ordered by actor and task).
  Bit *i* of `mask` says that the *i*-th operation reaches the server (an
  `Orphan`); the others do not. The plain crash is the all-zero mask. When the
  process has operations in flight the plain `Crash` transition is not
  offered, since it would duplicate mask 0 (their physical outcome is equal and
  the restarted client's knowledge is identical).
* **`ServerComplete(a, seq)`** — the server completes an orphan *at any later
  point*: a COMMIT commits (or is rejected, e.g. SQLSTATE 40001 under
  SERIALIZABLE: the recorded world then becomes `NotCommitted`); an autocommit
  write executes (it is enabled only when it would not wait for a lock).
  `ServerComplete` counts as progress: final invariants are evaluated after the
  server finished what it received (assumption: the server completes a COMMIT
  it holds).

The reason for the pending state is the **reconciliation hazard**. A client
that, after a crash, reads the table and inserts only when it finds nothing
can still be raced by the COMMIT the server has not completed yet. If
"reached the server" meant "committed at once", that interleaving — which real
PostgreSQL permits — would be missing and the reconciliation design would look
safe (it is not; `AMBIGUOUS-COMMIT.md`). "Committed before the restarted client
looks" is the schedule in which `ServerComplete` immediately follows the fault;
it is explored like every other interleaving.

## 4. Process, task and failure domain

* A **process** is a failure domain (`Model::process_of`): its actors share
  `State::apps` (in-process variables) and crash together; `Crash` aborts
  every open transaction of every actor of the process, kills every task,
  empties the process's pool and loses its in-process state.
* A **task** is a thread of an actor (`Promise.all` children, detached tasks).
  Several tasks of an actor and several actors of a process run concurrently.
* A crash propagates to every task of the process; it does not cancel what the
  database or an external system already received: orphans survive, performed
  effects stay.
* After a crash the *server* sees the lost connections eventually (the model
  releases the crashed transactions' locks at once; the runtime sets
  `client_connection_check_interval`, see `SOURCE-REPRODUCTION.md`).

## 5. Connection pool (documented subset)

`pools: {"<process>": n}` bounds a process's connections to `n`.

* A task that reaches a BEGIN or an autocommit statement *requests* a
  connection: it takes an idle one or queues (first come first served,
  `PoolState::queue`); the request is made when the task *arrives* at the
  operation (the end of its previous transition), not when it is scheduled,
  because that is when `pool.connect()` is called.
* A transaction holds its connection until it commits, rolls back, is aborted,
  or its connection breaks (`LostCommitted`); an autocommit statement holds it
  for its duration (including a lock wait).
* A task that ends or is abandoned while holding an unused connection returns
  it; a crash empties the pool (`Lease`, `Txn::leased`).
* Not modeled: acquisition timeouts, `idleTimeoutMillis`, connection errors
  other than the lost-acknowledgment break, pipelining, `sql.reserve()`,
  pool shutdown. The model cannot tell which driver's *ordering* policy runs;
  FIFO is node-postgres's (`pg-pool`). postgres-js is not modeled for pools
  (`CONNECTION-POOL.md`).

A state in which every remaining task waits for a connection, or for a lock
that only the application's own waiting could release, is *stuck*:
`StuckKind::PoolStarvation`, `StuckKind::ApplicationWaitCycle`. With a bounded
pool, or `options.termination: true`, it is reported as a violation of the
implicit invariant `termination`. A PostgreSQL lock deadlock is a different
thing: it has a `Deadlock` transition (the server aborts a victim) and is
never stuck.

## 6. Knowledge and the possible-worlds reading

For a lost acknowledgment, `Ambiguity { op, world, possible, via, attempt }`
records the physical outcome of this execution (`world`) and every outcome the
semantics allowed at that point (`possible`, here always both: whether the
server received the operation is exactly what the application cannot know).
`OutcomeKnowledge::Ambiguous` is the application's view; there is no
transition that *removes* a world, because the explorer visits every world as
its own transition: the invariants are quantified over all of them. What can
remove a world is an *observation*: the application's later reads or errors
differ between worlds, which the *twin analysis* (`twin.rs`) computes for a
counterexample:

1. take the in-flight transition of the schedule, flip the server's behavior;
2. replay the resulting schedule strictly on the same model;
3. compare the invariants and the actor's observations (reads, errors,
   effects, failures) up to the first difference.

If there is none, the application can never tell the worlds apart. This is
bounded and exact for the modeled alternatives: no world is invented, and a
twin is only reported if the model's own replay accepts it
(`replayed`/`diverges`).

## 7. Reduction

Sleep sets (Phase 6) and DPOR (`DYNAMIC-POR.md`) treat the new transitions
conservatively: lost-acknowledgment transitions share the fault budget
(`Access::Budget`), crashes are global, `ServerComplete` conflicts through the
rows it commits and the submitting actor, pool operations of one process
conflict through `Access::Pool`.

## 8. Supported abstraction boundaries

Not claimed: soundness for arbitrary Node.js or PostgreSQL behavior; real
networks (partitions, reordering, duplication); TCP buffering of a COMMIT
that is partly written; PostgreSQL `synchronous_commit`/replication; whether a given backend
completes a COMMIT it has received from a client that has since died (the
model keeps both outcomes and assumes nothing; the runtime evidence covers the
cases the proxy can establish, `PROTOCOL-FAULT-INJECTION.md`); two-phase
commit; savepoints; unbounded executions. Every verdict is "within the
declared bounds and under the stated assumptions".

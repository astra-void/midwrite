# Model semantics

The reference semantics every `model-check` and `discover --model-check`
result is relative to (`crates/midwrite-model`: `ir.rs`, `state.rs`,
`engine.rs`, `exec.rs`). The overview is in the
[concurrency model](concurrency-model.md); faults in the
[failure model](failure-model.md); PostgreSQL behavior in the
[PostgreSQL model](../architecture/postgres-model.md); values in
[value semantics](value-semantics.md); reductions in
[partial-order reduction](../architecture/partial-order-reduction.md).
`report.modelVersion` names the semantics; a change that can alter a
verdict changes the version.

## "Sound" — relative to what

The word is used only with its reference:

* **Exploration** is exhaustive for the transition system below, within the
  declared semantic bounds, when no resource bound was hit.
* **Reductions** preserve what [partial-order
  reduction](../architecture/partial-order-reduction.md#what-is-preserved)
  lists, under stated assumptions that are checked, not proved.
* **Relative to PostgreSQL**, the transition system is an approximation
  whose agreement was measured for the supported subset; for SERIALIZABLE
  it intentionally over-approximates what PostgreSQL allows.
* **Relative to the TypeScript program**, extraction is exact only for the
  supported subset; everything else becomes an unknown operation, which
  makes a reachable path `UNSUPPORTED`.

Values are concrete and bounded: a scenario gives the arguments and initial
rows. The checker explores schedules, failures and retries, not symbolic
inputs (input variation is discovery's world search,
[scenario synthesis](../architecture/scenario-synthesis.md)).

## Three kinds of state

| kind | what it is | where |
| --- | --- | --- |
| **physical** | what PostgreSQL and external systems actually did: row versions, locks, transactions, performed effects, replica progress | `State::store`, `txns`, `externals`, `replica_seq` |
| **execution** | which processes, tasks, operations, retries, pool requests and server-held operations are pending | `ActorState`, `Thread`, `State::orphans`, `State::pools` |
| **knowledge** | what an application can conclude about operations whose answer it never saw | `ActorState::ambiguous` |

A committed transaction and an application that knows it committed are
different states, also in the encoding.

## Global state

* **Store** — per row id, a chain of versions `(image | deleted, creator,
  commit sequence | uncommitted)`, and the row locks with their modes and
  holders.
* **Commit sequence** — incremented by every commit that wrote.
* **Transactions** — isolation, snapshot (taken at the first statement for
  REPEATABLE READ and SERIALIZABLE), written rows, autocommit flag, leased
  connection, SERIALIZABLE read sets; dependency edges between
  SERIALIZABLE transactions.
* **Actors** — locals, tasks (code, program counter, status, parent,
  detached flag, pending statement and pending foreign-key checks), open
  transaction, state saved at `BEGIN` for transaction retries, outcome,
  completions, return values (only when an invariant reads them), attempt,
  generation counter, per-actor transaction and insert counters,
  ambiguities.
* **Processes** — in-process variables, pool state; the external-effect
  log; in-flight operations held by the server (orphans); crash, retry and
  fault budgets used.

Row ids are `(table, inserting actor, per-actor counter)` and transaction
ids `(actor, per-actor counter)`, never global counters, so independent
inserts and transactions commute.

## Transitions

A transition is `(kind, actor, task)`; `enabled(state)` lists them in a
fixed order, so exploration is deterministic.

| kind | enabled when | effect |
| --- | --- | --- |
| `Step` | the task runs, does not wait, and its next operation is visible (statement, `BEGIN`/`COMMIT`/`ROLLBACK`, external effect, in-process read/write, response, unknown); statements on one transaction connection are serialized | executes it, then *settles* the actor |
| `Resume` | the task's pending statement waits for a transaction that has ended | continues with the original snapshot and remaining rows |
| `Deadlock` | the pending statement is part of a wait-for cycle | aborts that transaction with 40P01; any member of the cycle can be the victim |
| `Crash` | crashes < `maxCrashes`, and a task of the process has started | every task of the process stops; open transactions roll back; in-process state and the pool are lost; committed data and performed effects stay |
| `Retry` | retries < `maxRetries`, a declared retry trigger matches, all tasks of the attempt ended | a new attempt with the original arguments |
| `LostCommitted`, `LostNotCommitted` | `maxInFlight` > 0 and the task's next operation is a COMMIT or autocommit write | the connection breaks under a live process; the operation is left on the server (an orphan) or never arrives |
| `CrashInFlight(mask)` | a process crashes with operations in flight | bit *i* says whether the *i*-th in-flight operation reached the server; replaces the plain crash in that state |
| `ServerComplete` | an orphan exists and would not wait | the server completes it at any later point (a COMMIT may also be rejected, e.g. 40001) |
| `Replicate` | feature `replicas`, replica behind the primary | the replica replays the next commit |

**Settling** runs local operations (assignments, branches, jumps, spawns,
joins whose children finished, `return`, `throw`) until every task stands
at a visible operation, waits, or has finished. Local operations touch only
the actor's own locals, so folding them is exact for every property the
checker evaluates; they appear in the witness as `CHECK`/assignment lines.

**Statement granularity.** A statement runs atomically unless it must wait:
it takes its snapshot, finds candidate rows in key order, and on a row
locked by another transaction (or a unique key being written by one)
records its progress and becomes pending, keeping its snapshot and the
locks it already took. Interleavings inside a non-waiting statement are not
explored: exact for single-row statements (a re-check after a concurrent
commit equals running before or after it), an approximation for multi-row
statements. Autocommit statements commit within their step.

**Errors.** A statement error (constraint, serialization failure, lock not
available) aborts the transaction and raises an exception in the task. A
`throw`, an evaluation error (`TypeError` on `null.x`, overflow) or a failed
child task at its join fails the task; a failing main task fails the actor
and rolls back its open transaction. An unawaited task that fails crashes
its process (Node.js's default for unhandled rejections). An evaluation the
model cannot decide (`Unknown`) ends the path as unsupported — never as an
exception the application could catch.

## Processes, tasks, connections

* A **process** is a failure domain: its actors share in-process variables
  and a pool, and crash together. Actors without `process` get their own.
* A **task** is a thread of an actor: the main code, `Promise.all` /
  `Promise.allSettled` children, detached (unawaited) calls. Locals are
  actor-wide.
* **Connection pools** (`pools: {"<process>": n}`) are bounded FIFO
  leases, as node-postgres's `pg-pool`: a task requests a connection when it
  *arrives* at a `BEGIN` or autocommit statement; a transaction holds it to
  its end (or until its connection breaks); a crash empties the pool. Tasks
  queued in the initial state queue in declaration order. Not modeled:
  acquisition and idle timeouts, pool shutdown, connection errors other
  than the lost-acknowledgment break, pipelining, postgres-js pools.
* The failure domains are kept apart: task termination, OS process
  termination, client connection closure, the server noticing the lost
  connection (the model releases locks at once), transaction rollback, and
  completion of an in-flight operation after the process is gone.

## Quiescence, termination, stuck states

A state is **quiescent** when no task can progress by itself and no
*guaranteed* retry is pending — only optional environment transitions may
remain (crashes, optional retries, `Replicate`). `ServerComplete` counts as
progress: final invariants are evaluated after the server completed what it
received (an assumption). `final` invariants are evaluated in quiescent
states where every task finished; `always` invariants in every reachable
state. Retries and crashes are optional unless a retry is declared
`guaranteed`: both "the client retries" and "the client gives up" are
explored.

A quiescent state with unfinished tasks is **stuck** and classified:

| situation | classified as |
| --- | --- |
| a lock cycle between transactions | not stuck: a `Deadlock` transition (40P01) |
| every remaining task waits for a pool connection | `PoolStarvation` |
| a statement waits for a lock whose holder waits (in the application) for that statement | `ApplicationWaitCycle` |
| a wait without a known cause | `Unexplained` |

A stuck state makes the result `INCONCLUSIVE`; with a bounded pool or
`options.termination: true` it is a counterexample of the implicit
invariant `termination`.

## State identity

Two states are merged only when their **canonical encodings** are equal.
The encoding contains everything that can influence future behavior:
versions with images and creators, locks, transactions and dependency
edges, every task's program counter, locals and pending statement,
outcomes and counters, in-process variables, the full external log,
budgets used, and — when present — orphans and ambiguities (possible
worlds), pool leases, lock modes, return values, generation counters and
replica progress. Commit sequence numbers are replaced by their visibility
to each live snapshot. The step count is omitted, and the previous task
unless a context-switch bound is active.

Semantics enter the encoding only when used — lock modes only when a shared
mode can be requested, return values only when an invariant reads them,
replica progress only with the `replicas` feature, new value forms only
when they occur — so scenarios that do not use a feature keep identical
states, transitions and report bytes.

Storage is exact: the encoder cuts the encoding into fragments (tables,
threads, transactions, locks, orphans, pools); each distinct fragment is
interned once with an id; a state's id sequence is stored as LEB128 varints
in one arena behind an open-addressing table. Lookups compare fragments and
keys **byte for byte**; a hash only selects a bucket, so a collision can
cost a probe but never merge two states (tested with forced equal 64-bit
hashes). There is no lossy mode (bitstate hashing) and no spilling to
disk. `maxMemoryBytes` is checked against `visitedBytes` (the table,
interner, edges and nodes), not against RSS.

## Bounds

| bound | kind | default | when reached |
| --- | --- | --- | --- |
| `maxActors` | scenario | 4 | the scenario is rejected |
| `maxCrashes`, `maxRetries` | semantic | 0 | no further crash / retry transition |
| `maxInFlight` | semantic | 0 | no further lost acknowledgment (one unit per fault event; ≤ 8 operations per event) |
| `maxContextSwitches` | semantic | none | longer schedules are not explored (reported); disables reduction |
| `maxSteps` | resource | 64 | path cut; `INCONCLUSIVE` |
| `maxStates`, `maxTransitions`, `maxWallMs`, `maxMemoryBytes` | resource | 200 000 / none / none / none | search stops; `INCONCLUSIVE` |

Semantic bounds define the system that is verified; resource bounds only
cut the search. `NO_COUNTEREXAMPLE_WITHIN_BOUNDS` requires that no
resource bound was reached, no unsupported operation was reachable and no
state was stuck.

## Exploration

Per state the explorer evaluates `always` invariants the first time the
state is reached; in a quiescent state it evaluates `final` invariants (or
records a stuck state); it stops at `maxSteps`; it applies every enabled
transition once to obtain successors and footprints (an unsupported
transition is recorded and has no successor). A state already expanded at
a smaller or equal depth is not expanded again for transitions it already
explored; a shallower revisit re-expands it, so the step bound cannot hide
a state. The depth-first search is iterative (no recursion limit).

## Possible worlds and knowledge

A lost acknowledgment is a branch, not a probability: each outcome
(`LostCommitted`/`LostNotCommitted`, each `CrashInFlight` mask, each point
at which `ServerComplete` may run) is its own transition, and invariants
are checked in every terminal state of every branch — a bounded
quantification over the declared fault model. Worlds are explicit
trajectories that share their prefix; the application's view is
`Ambiguity { op, world, possible, via, attempt }`. No transition removes a
world. Knowledge is kept per attempt and never cleared by a retry, and
states that differ only in unresolved outcomes are never merged.

What *can* distinguish worlds is an observation that differs between them.
For a counterexample, the **twin analysis** (`twin.rs`) flips the server's
behavior at each lost acknowledgment, replays the resulting schedule
strictly on the same model (`replayed` or `diverges`), and compares the
invariants and the actor's observations (reads, errors, effects) up to the
first difference. When none exists the report says the application can
never tell the worlds apart — and when the invariant outcome differs
between them, the same decision is safe in one world and unsafe in the
other, so it is unsafe for the application. A read returning nothing does
not eliminate the world in which the server still completes a held COMMIT.
The knowledge of a *different* actor about the lost operation is not
modeled (other clients see only the database).

## Not claimed

Soundness for arbitrary Node.js or PostgreSQL behavior; real networks
(partitions, reordering, duplication, partial writes); TCP buffering of a
partly written COMMIT; `synchronous_commit` and replication settings;
whether a backend completes a COMMIT of a client that has died (both
outcomes are kept); two-phase commit; savepoints; unbounded executions;
liveness. Every verdict is "within the declared bounds and under the
stated assumptions".

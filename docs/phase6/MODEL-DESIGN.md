# Phase 6 model design

CommitRift Phase 6 adds a **bounded explicit-state model checker** for
concurrent executions of application operations against a modeled
PostgreSQL. It is a separate, opt-in pipeline (`commitrift model-check
scenario.json`); `commitrift check` is unchanged and never runs it.

```
TypeScript (Oxc, analyzer module resolution, config)
        │  commitrift-analyzer::concurrency (TsActorProvider)
        ▼
Concurrency IR (actors, tasks, statements, ops)        ── crates/commitrift-model/src/ir.rs
        │  scenario: schema, constraints, initial rows,     scenario.rs (validation)
        │  isolation, actors, processes, channels,
        │  retries, crash bounds, invariants
        ▼
Model = transition system                               engine.rs, exec.rs, state.rs, db.rs
        │  bounded DFS/BFS, canonical-state dedup,         explore.rs
        │  sleep-set partial-order reduction               footprint.rs
        ▼
Invariant check  →  counterexample schedule            invariant.rs
        │  replay, completion, minimization                witness.rs
        ▼
Optional lockstep replay on a real PostgreSQL           oracle.rs
        ▼
Versioned JSON / text / script-free HTML report         report.rs, html.rs
```

## The bounded model

A **scenario** fixes the finite system that is explored. Its parts, and how
each maps to the global state ([`SEMANTICS.md`](SEMANTICS.md)):

| Concept | Representation |
| --- | --- |
| **Actors** | One per independently executing operation (HTTP request, job, consumer, retry attempt). Each runs one function with concrete arguments. Actors never share a transaction, connection or event loop unless the scenario puts them in the same *process*, which only shares in-process variables and crash fate. |
| **Tasks** | An actor has a main task and the tasks it spawns (`Promise.all` elements, fire-and-forget calls). Locals are actor-wide (closures share them). |
| **Program counters** | `(code vector, index)` per task; local operations are folded (not scheduling points). |
| **Abstract database state** | Rows with identity, a chain of row versions (committed with a commit sequence, or the single uncommitted version of its writer), row locks. |
| **Transaction-local state** | Per transaction: isolation level, snapshot (once taken), written rows, and for SERIALIZABLE its predicate reads; plus the global dependency graph. |
| **Externally visible effects** | An ordered log of performed effects with channel, idempotency key, payload, attempt, whether the actor's transaction was open, and whether the channel deduplicated it. |
| **Pending operations** | A statement that waits keeps its snapshot, remaining candidate rows and partial results, and the transaction it waits for. |
| **Locks / blocking** | Row locks from `UPDATE`, `DELETE`, `SELECT ... FOR UPDATE`; waits on in-progress writers of a unique key; the wait-for graph and deadlock resolution. |
| **Failure state** | Per actor: running, ok (with a count of successful completions), failed (with reason), crashed. |
| **Recovery state** | Per actor: attempt number, transaction-retry attempts; global: crashes and retries used. |
| **Declared invariants** | Data-only assertions over committed rows, actor outcomes, external effects and in-process variables ([`INVARIANTS.md`](INVARIANTS.md)). |
| **Exploration bounds** | max actors, steps per schedule, distinct states, crashes, retries, context switches, transitions, wall time, memory ([`SCHEDULE-EXPLORATION.md`](SCHEDULE-EXPLORATION.md)). |

Values are **concrete and bounded** (small integers, strings, booleans,
`null`, rows): a scenario gives the arguments and the initial rows. The model
checker explores *schedules*, *failures* and *retries*, not symbolic inputs;
path constraints are therefore always concrete and need not be stored.

## Results

| Verdict | Meaning |
| --- | --- |
| `COUNTEREXAMPLE_FOUND` | A schedule of exact transitions violates an invariant. The witness is replayed through the model before it is reported. |
| `NO_COUNTEREXAMPLE_WITHIN_BOUNDS` | Every behavior within the declared bounds was explored and none violates an invariant. **Not** a proof that the application is race-free: only the modeled actors, data, operations and bounds are covered. |
| `INCONCLUSIVE` | A resource limit (states, transitions, steps, wall time, memory) cut the search, or a stuck state was reached; nothing is claimed about the unexplored part. |
| `UNSUPPORTED` | An operation the model does not understand is reachable (raw SQL, an unsupported construct). Unknown behavior is never a no-op; exploration of that path stops. |

Evidence levels of a counterexample are separate from the verdict:
`model-counterexample` → `postgres-confirmed-schedule` (the schedule was
replayed on PostgreSQL and every outcome matched) or
`postgres-did-not-reproduce`; `runtime-inconclusive` when validation could
not run. A runtime reproduction of the *original application* or a
maintainer-confirmed bug are levels Phase 6 does not produce
([`COUNTEREXAMPLES.md`](COUNTEREXAMPLES.md)).

## "Sound" — relative to what

The word is used only with its reference semantics:

* **Exploration** is exhaustive *for the transition system defined in
  [`SEMANTICS.md`](SEMANTICS.md)*, within the declared semantic bounds
  (crashes, retries, context switches) and when no resource limit was hit.
* **Partial-order reduction** preserves the set of reachable canonical
  states of that transition system (hence every state invariant), under the
  conditions in [`PARTIAL-ORDER-REDUCTION.md`](PARTIAL-ORDER-REDUCTION.md);
  this is checked by differential tests, not proven mechanically.
* **Relative to PostgreSQL**, the transition system is an approximation
  whose agreement was measured on 17.11 and 15.19 for the supported subset
  ([`POSTGRES-VALIDATION.md`](POSTGRES-VALIDATION.md)). For `SERIALIZABLE`
  it intentionally over-approximates the histories PostgreSQL allows.
* **Relative to the TypeScript program**, the concurrency IR is an exact
  lowering only for the subset in [`CONCURRENT-IR.md`](CONCURRENT-IR.md);
  everything else becomes `Unknown`.

## Code

| Crate / file | Role |
| --- | --- |
| `crates/commitrift-model` | the model checker (no TypeScript dependency) |
| `ir.rs`, `dsl.rs`, `provider.rs` | concurrency IR; declared-program language; frontend boundary |
| `value.rs`, `db.rs`, `state.rs` | values and expressions; schema, constraints, versions; global state and canonical encoding |
| `exec.rs`, `engine.rs` | statement semantics (isolation, locks, EPQ, unique keys, SSI graph); transitions |
| `explore.rs`, `footprint.rs` | bounded search, deduplication, sleep sets, independence audit |
| `invariant.rs`, `witness.rs` | assertion language; replay, completion, minimization, rendering |
| `oracle.rs` | lockstep differential replay on PostgreSQL (feature `postgres`) |
| `report.rs`, `html.rs`, `scenario.rs` | report schema, script-free HTML, scenario format |
| `crates/commitrift-analyzer/src/concurrency.rs` | TypeScript → concurrency IR (`TsActorProvider`) |
| `crates/commitrift-cli/src/model_check.rs` | `commitrift model-check`; links operations to Effect IR effects and findings |

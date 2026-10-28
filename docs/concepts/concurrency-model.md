# Concurrency model

The model checker (`crates/commitrift-model`) is an explicit-state,
bounded model checker. A **scenario** fixes everything it explores: the
actors and their arguments, the schema and initial rows, the isolation
level, the faults, the bounds and the invariants. A result holds for that
scenario and the model's semantics only.

## What is explored

| element | model |
| --- | --- |
| actor | one request, job or process entry; its code is extracted from TypeScript, given as a declared program (scenario DSL), or given as pre-lowered steps |
| task | a thread of an actor: the main code, `Promise.all` elements, detached (unawaited) calls |
| process | actors are separate processes and connections unless declared to share a process; a crash kills every task of the process |
| connection pool | optional bounded FIFO pools (Phase 9) |
| transition | one statement, external effect, commit/rollback, crash, retry, server completion of an in-flight operation, replica replay (Phase 13), response (Phase 13) or deadlock resolution; local computation between them is not a scheduling point |
| state | committed and uncommitted row versions, snapshots, row locks with modes and holders, pending statements and foreign-key checks, actor program counters and locals, task outcomes and return values (when an invariant reads them), the external-effect log, in-process variables, in-flight operations and possible worlds, replica progress |
| schedule | a sequence of transitions; a statement runs atomically unless it waits |

Every schedule the transitions allow is explored within the bounds (maximum
steps, states, crashes, retries, in-flight operations, context switches,
wall time, memory). Every bound that was reached is reported; a search that
ended on a bound is `INCONCLUSIVE`, never "no counterexample".

Formal description: [Phase 6 semantics](../phase6/SEMANTICS.md),
[Phase 6 concurrency IR](../phase6/CONCURRENT-IR.md),
[Phase 9 formal model](../phase9/FORMAL-MODEL.md),
[Phase 9 process and task model](../phase9/PROCESS-AND-TASK-MODEL.md),
[Phase 10 formal assumptions](../phase10/FORMAL-ASSUMPTIONS.md).

## Isolation levels

READ COMMITTED and REPEATABLE READ follow PostgreSQL's snapshot and
row-lock behavior (including EvalPlanQual re-evaluation and unique-key
waits). SERIALIZABLE is a conservative serialization-graph abstraction,
not PostgreSQL's SSI implementation. Details and measured differences:
[PostgreSQL model](../architecture/postgres-model.md).

## Reductions

The default search uses sleep-set partial-order reduction over
footprint-based independence, with full deduplication of canonical states.
Classic DPOR (`--dpor`, Phase 9) and source-set DPOR (`--source-dpor`,
Phase 10) are opt-in. All reductions are checked against the unreduced
search on targeted, random and adversarial models and by a state-by-state
independence audit; none is proved correct in general
([model checker § reductions](../architecture/model-checker.md#reductions)).

## Results are bounded

`NO_COUNTEREXAMPLE_WITHIN_BOUNDS` says nothing about more actors, other
inputs, longer schedules, other operations or other deployments. The state
space grows exponentially with the number of actors; measured sizes are in
the [benchmarks](../development/benchmarks.md).

## Concurrency is an assumption

Whether two invocations really overlap in production is not known from
source. `discover` records it as declared (`concurrent`, `delivery`),
`framework` (Server Actions and route handlers: double submit, two tabs,
client retry) or `discovery-default` (any other exported function, "NOT
established from source") — [Phase 11 candidate discovery](../phase11/CANDIDATE-DISCOVERY.md#concurrency-is-an-assumption).

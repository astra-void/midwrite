# Model checker

Crate: `commitrift-model`. Commands: `model-check`, `discover
--model-check`. An explicit-state, bounded model checker over a model IR
of concurrent actors and a PostgreSQL database model.

## Components

| component | files | design |
| --- | --- | --- |
| model IR and provider interface | `ir.rs`, `provider.rs`, `model.rs` | [Phase 6 concurrent IR](../phase6/CONCURRENT-IR.md) |
| scenario loader and validation | `scenario.rs`, `dsl.rs` (declared programs) | [Phase 6 invariants](../phase6/INVARIANTS.md) |
| database model | `db.rs`, `exec.rs`, `value.rs`, `schema_facts.rs` | [PostgreSQL model](postgres-model.md) |
| engine (transitions, faults, tasks, pools, in-flight operations, replicas, response) | `engine.rs`, `state.rs`, `epistemic.rs`, `twin.rs` | [Phase 6 model design](../phase6/MODEL-DESIGN.md), [Phase 9 formal model](../phase9/FORMAL-MODEL.md) |
| exploration | `explore.rs`, `explore/sdpor.rs`, `footprint.rs`, `keytable.rs`, `audit.rs` | [Phase 6 schedule exploration](../phase6/SCHEDULE-EXPLORATION.md), [Phase 10 POR architecture](../phase10/POR-ARCHITECTURE.md) |
| invariants | `invariant.rs` | [invariants](../concepts/invariants.md) |
| witnesses (replay, completion, greedy minimization) | `witness.rs` | [Phase 6 counterexamples](../phase6/COUNTEREXAMPLES.md) |
| reports | `report.rs`, `html.rs` | [output formats](../reference/output-formats.md) |
| PostgreSQL lockstep oracle | `oracle.rs`, `oracle_types.rs` | [Phase 6 PostgreSQL validation](../phase6/POSTGRES-VALIDATION.md) |
| original-code differential | `source_verify.rs`, `source_types.rs` | [runtime verification](runtime-verification.md) |
| schema fidelity | `schema_fidelity.rs` | [Phase 8 schema fidelity](../phase8/SCHEMA-FIDELITY.md) |

## State identity

States are deduplicated by an exact canonical encoding. Since Phase 9 the
visited table stores interned fragments with a varint key table and exact
byte comparison ([Phase 9 state-space storage](../phase9/STATE-SPACE-STORAGE.md)).
New semantics enter the encoding only when used: lock modes only when the
model can request a shared mode, return values only when an invariant
reads them, replica progress only with the `replicas` feature — so earlier
scenarios keep identical states and transitions
([Phase 13 architecture § 2](../phase13/ARCHITECTURE.md#2-new-model-state-checkpoint-1-representations)).

## Reductions

| search | status | properties preserved | refused with |
| --- | --- | --- | --- |
| sleep sets over footprint-based conditional independence, with the state cache | **default** | reachable states (without a context-switch bound) | — (disabled by `maxContextSwitches`) |
| classic stateless DPOR (`--dpor`, Phase 9; every race since Phase 10) | opt-in | terminal states, final invariants, counterexamples, deadlocks, stuck states | `always` invariants, `maxContextSwitches` |
| source-set DPOR with sleep sets and/or a summarizing state cache (`--source-dpor`, Phase 10) | opt-in, experimental | as classic DPOR | as classic DPOR |
| none (`--no-por`) | opt-in | everything | — |

Every reduction is **evidence-checked, not proved**: compared with the
unreduced search on targeted workloads, random and adversarial generated
models, metamorphic transformations and regression fixtures, plus a
state-by-state independence audit and mutation testing of the dependence
relation. Remaining proof obligations are listed in
[Phase 10 POR correctness § 6](../phase10/POR-CORRECTNESS.md). Each new
Phase 13 operation (foreign-key checks, lock modes, upserts, replica reads
and replication, the response step) records a footprint and was checked
the same way ([Phase 13 model correctness § 2](../phase13/MODEL-CORRECTNESS.md#2-partial-order-reduction-track-g3)).
Measured numbers: [evidence index](../research/evidence-index.md#3-model-checking-correctness).

## Counterexamples

A counterexample is found by the search, completed, minimized greedily
within a budget, and replayed in the model; a minimized witness is valid
and violating, not necessarily the shortest. Reports list the schedule
with source locations, the final state, the evaluated invariant, the
evidence level and the extraction's represented/assumed/unsupported
operations.

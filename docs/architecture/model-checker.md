# Model checker

Crate: `midwrite-model` (no TypeScript dependency; TypeScript is lowered to
its IR by `midwrite-analyzer`). Commands: `model-check`, `discover
--model-check`. An explicit-state, bounded model checker over a model IR
of concurrent actors and a PostgreSQL database model. `midwrite check`
never runs it.

```text
TypeScript ─► midwrite-analyzer (source extraction) ─► model IR: actors, tasks, statements, ops
             scenario: schema, constraints, initial rows, isolation, actors, processes,
             pools, channels, retries, fault bounds, features, invariants
        ─► transition system (engine.rs, exec.rs, state.rs, db.rs)
        ─► bounded search, exact state deduplication, reductions (explore.rs, footprint.rs)
        ─► invariant check ─► counterexample: replay, completion, minimization (witness.rs)
        ─► optional lockstep replay on PostgreSQL (oracle.rs) / original code (source_verify.rs)
        ─► versioned JSON / text / script-free HTML report (report.rs, html.rs)
```

## Components

| component | files | design |
| --- | --- | --- |
| model IR and provider interface | `ir.rs`, `provider.rs`, `model.rs` | [model semantics](../concepts/model-semantics.md), [source extraction](source-extraction.md) |
| scenario loader and validation; declared programs | `scenario.rs`, `dsl.rs` | [`model-check` § scenario files](../user-guide/model-check.md#scenario-files) |
| database model | `db.rs`, `exec.rs`, `value.rs`, `jsfn.rs`, `sqlfn.rs`, `schema_facts.rs` | [PostgreSQL model](postgres-model.md), [value semantics](../concepts/value-semantics.md) |
| engine (transitions, faults, tasks, pools, in-flight operations, replicas, response) | `engine.rs`, `state.rs`, `epistemic.rs`, `twin.rs` | [model semantics](../concepts/model-semantics.md), [failure model](../concepts/failure-model.md) |
| exploration and reductions | `explore.rs`, `explore/sdpor.rs`, `footprint.rs`, `keytable.rs`, `audit.rs` | [partial-order reduction](partial-order-reduction.md) |
| invariants | `invariant.rs` | [invariants](../concepts/invariants.md) |
| witnesses (replay, completion, greedy minimization) | `witness.rs` | [below](#counterexamples) |
| reports | `report.rs`, `html.rs` | [output formats](../reference/output-formats.md) |
| PostgreSQL lockstep oracle | `oracle.rs`, `oracle_types.rs` | [PostgreSQL model § validation](postgres-model.md#validation-against-postgresql) |
| original-code differential | `source_verify.rs`, `source_types.rs` | [runtime verification](runtime-verification.md) |
| schema fidelity | `schema_fidelity.rs` | [schema extraction](schema-extraction.md) |

## Verdicts

| verdict | meaning |
| --- | --- |
| `COUNTEREXAMPLE_FOUND` | a schedule of exact transitions violates an invariant; the witness is replayed through the model before it is reported |
| `NO_COUNTEREXAMPLE_WITHIN_BOUNDS` | every behavior within the declared bounds was explored and none violates an invariant — only for the modeled actors, data, operations and bounds; **not** a proof that the application is race-free |
| `INCONCLUSIVE` | a resource bound cut the search, or a stuck state was reached; nothing is claimed about the unexplored part |
| `UNSUPPORTED` | an operation the model does not understand is reachable; unknown behavior is never a no-op, and exploration of that path stops |

Evidence levels of a counterexample are separate from the verdict
([evidence levels](../concepts/evidence-levels.md)).

## State identity

States are deduplicated by an exact canonical encoding stored as interned
fragments behind a varint key table, compared byte for byte; a hash only
selects a bucket ([model semantics § state
identity](../concepts/model-semantics.md#state-identity)). New semantics
enter the encoding only when used: lock modes only when the model can
request a shared mode, return values only when an invariant reads them,
replica progress only with the `replicas` feature, JavaScript values only
when they occur — so earlier scenarios keep identical states, transitions
and report bytes (checked against golden reports and state digests,
[testing](../development/testing.md)).

## Reductions

| search | status | properties preserved | refused with |
| --- | --- | --- | --- |
| sleep sets over footprint-based conditional independence, with the state cache | **default** | reachable states (without a context-switch bound) | — (disabled by `maxContextSwitches`) |
| classic stateless DPOR (`--dpor`; handles every race) | opt-in | terminal states, final invariants, counterexamples, deadlocks, stuck states | `always` invariants, `maxContextSwitches` |
| source-set DPOR with sleep sets and/or a summarizing state cache (`--source-dpor`) | opt-in, experimental | as classic DPOR | as classic DPOR |
| none (`--no-por`) | opt-in | everything | — |

Every reduction is **evidence-checked, not proved**: compared with the
unreduced search on targeted workloads, random and adversarial generated
models, metamorphic transformations and regression fixtures, plus a
state-by-state independence audit and mutation testing of the dependence
relation. Every state-changing operation (foreign-key checks, lock modes,
upserts, multi-row locks, `NULLS NOT DISTINCT` keys, replica reads and
replication, the response step) records a footprint and is checked the
same way. Algorithms, assumptions, preserved properties and the remaining
proof obligations: [partial-order reduction](partial-order-reduction.md).
Measured numbers: [evidence index](../research/evidence-index.md).

## Counterexamples

The explorer produces only a **schedule**: the transitions `(kind, actor,
task)` that lead to the violating state. The witness is regenerated by
**replaying** that schedule through the model with recording on — every
line of a witness is an event of a transition the model executed, nothing
is templated. Replay is strict: a transition that is not enabled rejects
the schedule. For a `final` invariant the schedule is completed to
quiescence by running the lowest enabled task step (never an optional
crash or retry); for an `always` invariant it is cut at the first violating
state.

Minimization is greedy and budgeted, and every candidate is validated by
replay: delete single transitions (accepted if the same invariant is still
violated and the schedule got shorter, or equally long with fewer context
switches), then swap adjacent transitions of different tasks (accepted if
valid, still violating, and with fewer context switches). A transition
disappears only if a schedule without it still reaches a violation. A
minimized witness is valid and violating, not necessarily the shortest.

Reports list the schedule with source locations (file, line, column, the
inlining call chain, an excerpt, the Effect IR effect id at the same
location and linked findings — a sequential finding is never promoted to a
concurrency finding), the initial and final state, the evaluated invariant
with its operands (and an identity diagnosis for generated-value
formulas), the evidence level, per lost acknowledgment the twin-world
analysis, and the extraction's represented/assumed/unsupported operations.
Serialization is deterministic (no timestamps, ordered maps, timing in a
separate object omitted by `--no-timing`). The HTML view is script-free
(a CSP forbids scripts and remote resources; every string is escaped) and
draws one column per request plus an environment column, with transaction
boundaries, blocking, commits, rollbacks, crashes, retries, deadlock
victims and — after `--validate-postgres` — PostgreSQL's observation next
to each transition. Schemas: [`docs/reference/schemas/`](../reference/schemas/).

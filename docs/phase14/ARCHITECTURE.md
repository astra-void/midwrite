# Phase 14 architecture and work organization

Phase 14 extends the source-to-model pipeline; no component was replaced.
Current, phase-independent architecture: `docs/architecture/overview.md`.

## 1. How the work was organized

| work | owner | where |
| --- | --- | --- |
| Phase 13 closeout (gates, evidence, reports) | lead | frozen copy of the tree in a private scratch directory |
| documentation consolidation, link checker | agent (docs only, no code) | `docs/**` (except `phase13/`, `phase14/`), `README.md`, `CONTRIBUTING.md`, `scripts/check-doc-links.py` |
| Phase 14 evaluation set | agent without access to `crates/` | `fixtures/phase14/eval-app`, labels, `FROZEN.json`, `scripts/phase14-eval.py` |
| Track B — JavaScript values | agent, isolated copy | `model/src/{jsfn,value,dsl}.rs`, `analyzer/src/concurrency/js_values.rs`, `discover/src/synth.rs` typing |
| Track D — Drizzle/PostgreSQL semantics | agent, isolated copy | `model/src/{ir,exec,engine,db,state,oracle,sqlfn,scenario,source_verify}.rs`, `analyzer/src/schema.rs`, predicates/inserts in `concurrency.rs` |
| Tracks A, C, E, F, integration | lead | `discover/src/{synth,detect,effects,pipeline,report,contracts}.rs`, `analyzer/src/{concurrency,zod,packages}.rs`, `replay/src/source.rs`, `model/src/{engine,invariant,source_verify}.rs` |

**Shared interface first.** Before the agents started, the lead added the
value-domain interface to `model/src/value.rs` (`Value::{List, Undefined,
Map, Set}`, `Expr::{Array, Call, Each}`, `Builtin`, `EachOp`, the dispatch to
`jsfn`/`sqlfn` stubs) and the encoding tags in `state.rs`; both agents
started from a copy that contained it (`TypeOf` was added the same way
later, identically on both sides). The copies were merged with per-file
three-way merges (`git merge-file`, base = the copy they started from);
conflicts were limited to both sides adding scenario features and a test
exclusion. Nothing was committed.

## 2. Versioned semantics

| switch | effect | who turns it on |
| --- | --- | --- |
| scenario feature `js-values` (`ExtractRequest.js_values`) | JavaScript value semantics in extraction | discovery extracts with it; a synthesized scenario gets it when an actor's code uses a JavaScript value form |
| `timestamps` | time columns modeled | discovery's schema probe; a scenario whose tables have such columns |
| `json` | JSON columns modeled | as above |
| `foreign-keys`, `replicas` (Phase 13) | unchanged | unchanged |

Without the features, extraction and model are the Phase 13 ones (state
digests of 83 earlier scenarios identical). Discovery output changes are
intended and listed in the final report.

## 3. New IR and model state

| | representation |
| --- | --- |
| JavaScript values | `Value::{List, Undefined, Map, Set}`; built-ins `Expr::Call { func: Builtin, args }`; callbacks `Expr::Each { op, list, var, body }`; canonical tags 6–9 |
| multi-row insert | `StmtKind::Insert.more_rows` (omitted when empty); `ClosedStmt::InsertMany` with a row cursor for mid-statement waits |
| clocks | `ColumnDefault::{Now, Clock}`, `ColumnSchema.on_update`; readings are `Value::Gen` with generator `CLOCK_DB` (transaction start) or `CLOCK_APP` |
| package generators | `Op::Generate { local, source }` (`nanoid()`, `randomUUID()`), generator `CALL_GENERATOR` |
| unknown evaluation | a local step whose evaluation is undetermined ends the path as unsupported (`engine.rs`), never a catchable exception |

## 4. Where each feature lives

```text
analyzer   concurrency.rs ....... closures (Binding::Closure, call_closure), zod lowering,
                                  fresh-value package calls, module array constants,
                                  multi-row values, predicates, clocks (Track D)
           concurrency/js_values.rs  JavaScript value lowering (Track B)
           zod.rs ............... zod schema interpreter
           packages.rs .......... nanoid / randomUUID contracts
model      jsfn.rs / sqlfn.rs ... JavaScript and SQL built-ins
           engine.rs ............ Op::Generate, unsupported local evaluation, InsertMany
           invariant.rs ......... export names in `actors` terms
           source_verify.rs ..... generated keys at loss/completion checkpoints
discover   synth.rs ............. round trip, feature opt-in, typed fills, NoOrphan /
                                  ReturnsArgument invariants, serial-outcome fix
           detect.rs ............ parent-child, return-value (declared), ambiguous commit
                                  of transactions, redelivered inserts
           effects.rs ........... handler region by reachability
           pipeline.rs / report.rs  next step, partial-unique-index rule
           contracts.rs ......... return-value templates
replay     source.rs ............ resume of a waiting statement already answered
```

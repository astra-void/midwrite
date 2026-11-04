# Phase 16 architecture: tracks, ownership and shared interfaces

Recorded at Checkpoint 1, before any Phase 16 code was written. The
implementation tracks ran in isolated git worktrees created from
`1efa383` (the Phase 15 tree plus the post-freeze clippy fix, digest
`98567599…`); each track's diff was applied to the main checkout at
integration (Checkpoint 3). Nothing was committed by any track.

## Ownership

| track | owns (may edit) | must not edit |
| --- | --- | --- |
| A zod | `analyzer/src/zod.rs`; `concurrency.rs` `zod_*` functions and their call sites; `model/src/value.rs` new `Builtin` variants for object keys and string formats, `model/src/jsfn.rs` their evaluation; `discover/src/domains.rs`; synthesized-value helpers in `discover/src/synth.rs` (`fill`, `uuid_literal`, distinct values) | statement lowering, select/predicate lowering, `exec.rs`, `invariant.rs` |
| B loops/retry/request | `concurrency.rs` `statement`, `try_statement`, loop lowering (new `for`/`while` functions), request/response lowering; caught-error shapes (`Op::Try` binding) in `model/src/{ir,exec,engine}.rs` limited to the `Try`/`Rethrow` paths; retry-loop detection in `discover/src/detect.rs` | zod, select/predicates, unique checks, invariants |
| C predicates/locks | `concurrency.rs` `select`, `pred`, `pred_in`, `where_clause`, `find_first`, `hoist_operands`; multi-row `SELECT` and lock acquisition in `model/src/{ir,exec,engine,db,footprint,oracle}.rs`; replay SQL for it | unique checks, `Try`, zod, invariants |
| D NULLS NOT DISTINCT | `model/src/db.rs` `TableSchema` unique metadata, `exec.rs` `unique_check` and the `ON CONFLICT` arbiter, `footprint.rs` key sets, `scenario.rs` unique loading, oracle DDL; `discover/src/pipeline.rs` the Phase 15 unenforced-unique INCONCLUSIVE rule | lock acquisition, select lowering |
| E generated ids | `model/src/invariant.rs` (+ its DSL in `dsl.rs`), `discover/src/synth.rs` `serial_values`/`value_term`/property formulas, contract templates | everything else |
| F evaluation | `fixtures/phase16/eval-app`, labels, freeze manifest (independent author, no access to `crates/`) | all code |
| G gates | discovery-report JSON Schema, schema tests, `scripts/validate-schemas.py`, CLI messages | model semantics |
| H integration | merges, evidence scripts, corpus runs, documentation | — |

## Shared-interface agreements

1. **Objects.** A JavaScript object is `Value::Row` (a `BTreeMap`). A
   missing key is an absent map entry; an explicit `undefined` is a
   present `Value::Undefined`. No new `Value` variant. Key *order* is not
   modeled (validation results never depend on it in the supported
   subset).
2. **Builtins.** New `Builtin` variants are appended at the end of the
   JavaScript section (before `// SQL.`), each serialized by name; no
   existing variant is renamed or renumbered.
3. **Compatibility.** Every new IR or scenario field is
   `#[serde(default, skip_serializing_if = …)]`, so every historical
   scenario, model report and static output serializes byte-identically
   (gates: `scripts/phase10-model-identity.py` 56/56,
   `scripts/phase8-identity.sh` 144/144).
4. **Loops** lower to the existing `Let`/`JumpIf`/`Jump` ops by bounded
   unrolling; no new loop op. A loop whose bound is not established is
   either unrolled up to an explicit exploration limit and reported
   `TRUNCATED`, or refused as `Op::Unknown`.
5. **Caught errors** are values only when their origin is a modeled
   statement or `throw`: a row `{ code, name, message }` with the
   SQLSTATE of a modeled database error; anything else stays opaque.
6. **Multi-row SELECT** is one `StmtKind::Select` with an optional
   multi-row mode (serde default: single row, `LIMIT 1`); its row locks are
   acquired one row at a time in the model's scan order, so a statement can
   hold a prefix of its locks while it waits.
7. **Unique sets** keep `TableSchema.unique: Vec<Vec<String>>`; a parallel,
   defaulted list names the sets that are `NULLS NOT DISTINCT`.
8. **Generated identity** stays `Value::Gen(GenId)`; invariants gain terms
   and formulas over generated values without inventing concrete values.
9. **Tests** go in new files `crates/<crate>/tests/phase16_<track>.rs` and
   fixtures under `fixtures/phase16/<track>/` (the analyzer fixture harness
   excludes `fixtures/phase16`).

# Phase 15 architecture: from one synthesized state to a bounded world search

Phase 11–14 built one initial state and one argument vector per scenario
variant ("the base world") and model-checked it. Phase 15 keeps that world
and adds a bounded, deterministic search over further worlds, each built by
the same synthesizer and checked by the same model checker. No new model
checker, no SMT solver.

```text
candidate ─► variants (actual / control / hypothetical)
              │
              ├─ base world: synth::build (Phase 11–14 synthesis, now with
              │   guard satisfaction and literal-union arguments) ─► invariants ─► model check
              │        │ solo runs record branch outcomes (CHECK events)
              │        ▼
              ├─ pending worlds (actual variants only, model checking on):
              │     domain worlds        domains.rs: typed path domains → value classes
              │     actor worlds         worlds.rs: non-key argument differs between actors
              │     relationship worlds  worlds.rs: stored value = / ≠ written value
              │     boundary worlds      worlds.rs: stored value at a guard's integer operand
              │     ordered by priority; feedback raises worlds over branches taken one way
              │
              └─ loop (≤ --max-worlds attempts per variant):
                   synth::build_world(spec) ─► fingerprint (dedup) ─► invariants ─► model check
                   stop at the first validated counterexample (unless --all-worlds)
              ▼
coverage (four dimensions) ─► status (unchanged codes) ─► finding text ─► reproduction artifact
```

## Shared representations (defined first, used by every track)

| type | file | what it is |
| --- | --- | --- |
| `PathDomain`, `DomainKind`, `Permission`, `ValueClass` | `crates/commitrift-discover/src/domains.rs` | the typed domain of one parameter path: kind, null/undefined permission with provenance, the value classes the code makes relevant |
| `WorldSpec`, `ArgOverride`, `CellOverride`, `WorldOrigin` | `crates/commitrift-discover/src/worlds.rs` | one world: argument overrides (applied before rows are synthesized) and cell overrides (applied to the rows the actors need), with its relationships in words |
| `Observations` | `worlds.rs` | branch outcomes and locations reached by each actor's solo run on a world's final rows |
| `WorldInfo`, `WorldOutcome`, `Coverage` (+ `DimensionCoverage`, `ScheduleCoverage`, `SourceCoverage`, `BranchCoverage`) | `crates/commitrift-discover/src/report.rs` | report sections: the world of each scenario, what happened to every world, and the candidate's coverage |
| `synth::build_world` | `synth.rs` | `build` for one world; `build` is `build_world(base)` |
| `engine::initial_state_recorded` | `crates/commitrift-model/src/engine.rs` | `initial_state` with a recorder, so that branches evaluated before the first visible step are observed (same state; `initial_state` calls it with the recorder off) |

## Where each track lives

| track | code |
| --- | --- |
| A typed domains | `domains.rs`; literal unions resolved by `TsActorProvider::param_signature` (`crates/commitrift-analyzer/src/concurrency/js_values.rs`, follows relative imports and barrel re-exports) |
| B constraint-aware states | `synth.rs`: `build_world`, `satisfy_guard` (stored row), `argument_guard` (argument), `stopped_at`; `worlds.rs`: `distinct_value`, `apply_cells`, `identity_column` |
| C feedback search | `pipeline.rs`: `run_world`, the world loop in `discover`, `unflipped`; `worlds.rs`: `domain_worlds`, `actor_worlds`, `relationship_worlds`, `boundary_worlds`, `prioritize`, `fingerprint` |
| D verdicts and coverage | `pipeline.rs`: `coverage_of`, `deciding_scenario`, `status_of` (+ `unenforced_unique`); `report.rs`: rendering |
| E source support | `detect.rs`: `guarded_update` family, declared key from the INSERT; analyzer: literal-union aliases |
| G CLI | `crates/commitrift-cli/src/discover.rs`: `--max-worlds`, `--all-worlds`, `--explain-coverage`; reproduction artifacts in `pipeline.rs` |

## Compatibility

* Status codes are unchanged (`MODEL_COUNTEREXAMPLE`,
  `NO_VIOLATION_WITHIN_BOUNDS`, `INCONCLUSIVE`, `UNSUPPORTED`, `BLOCKED`,
  `STATIC_CANDIDATE`); coverage is a new optional section, and finding
  texts gain a sentence. The discovery report keeps `schemaVersion` 1; new
  fields are additive (`coverage`, `world`, five `budgets` counters).
* `scenarios[i]` is still one base world per variant, in variant order;
  further worlds are appended after them.
* `--max-worlds 1` turns the search off. The base world itself changed
  where Phase 15 made synthesis more faithful (argument values from
  string-literal unions; guards satisfied with stated preconditions);
  those changes are listed in `FINAL-REPORT.md` §2.
* The model crate changed only by `initial_state_recorded` (no behavior
  change); model-check reports of the historical scenarios are compared in
  `FINAL-REPORT.md` §6.

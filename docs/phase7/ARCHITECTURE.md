# Phase 7 architecture

Phase 7 connects the three Phase 1–6 engines on *original* application
code. Nothing of the existing pipeline was replaced; every addition is
opt-in, versioned, or proven output-identical.

```
TypeScript source ──► Oxc ──► static analyzer (Effect IR, paths, CP1001–CP9001)
      │                              │   ▲ compact call-scope summaries (exact, default)
      │                              │   │ legacy walk (COMMITRIFT_PATHS=legacy)
      │                              ▼
      │            function identity = (module, start line:column)  ◄── shared
      │                              ▲
      ▼                              │
 source selector ──► concurrency extractor ──► concurrency IR ──► model checker
 (file#binding/arg[1])   wrapper facts,           joins, projections,     │
                         declared contracts,      RETURNING, sequences,   │ counterexample
                         try/catch/finally,       defaults, try handlers  ▼
                         switch, for..of                          plan (statements per transition)
                                                                          │
 authorized harness (original code) ◄── coordinator (replay crate) ◄──────┘
   Node workers, one per actor            holds every SQL statement,
   pg.Pool coordinated,                   releases in plan order,
   Drizzle call sites attributed          observes pg_stat_activity
                                                   │
                                                   ▼
                          differential comparison: statements, results,
                          final rows, outcomes, invariant on observed rows
                                                   │
                                                   ▼
                     model-check report v2: extraction + sourceVerification
```

## Components

| Component | Location | Document |
| --- | --- | --- |
| Function identity and selectors | `crates/commitrift-analyzer/src/identity.rs` | [`SOURCE-IDENTITY.md`](SOURCE-IDENTITY.md) |
| Wrapper invocation analysis, declared contracts | `analyzer/src/wrapper.rs`, `concurrency.rs` (`entry_info`) | [`CALLBACK-RESOLUTION.md`](CALLBACK-RESOLUTION.md) |
| Compact call-scope summaries | `crates/commitrift-core/src/analysis/paths.rs` (`call_compact`) | [`COMPACT-SUMMARIES.md`](COMPACT-SUMMARIES.md) |
| Exceptions, `switch`, bounded loops | `concurrency.rs` (`try_statement`, `switch_statement`, `for_of`), model `engine.rs` (`Op::Try`, handlers in `fail_thread`) | [`CONTROL-FLOW.md`](CONTROL-FLOW.md) |
| Joins, projections, row arrays, `RETURNING`, sequences, defaults | model `ir.rs`, `exec.rs`, `oracle.rs`; extractor `select`/`mutation` | [`DRIZZLE-EXTRACTION.md`](DRIZZLE-EXTRACTION.md) |
| Extraction report, assumptions vs facts, sealing unknowns | `model/src/source_types.rs`, `report.rs`, `scenario.rs` (`seal_unknowns`) | [`SOURCE-MODEL-FIDELITY.md`](SOURCE-MODEL-FIDELITY.md) |
| Original-code coordinator | `crates/commitrift-replay/src/source.rs`, `runtime/node/source.js` | [`RUNTIME-SCHEDULER.md`](RUNTIME-SCHEDULER.md) |
| Plan, comparison, evidence | `crates/commitrift-model/src/source_verify.rs` | [`DIFFERENTIAL-VERIFICATION.md`](DIFFERENTIAL-VERIFICATION.md) |
| CLI | `commitrift model-check … --verify-source [--harness M] --allow-execution [--source-repeat N]` | — |

## Boundaries kept

* `commitrift check`/`trace` never call the extractor or execute anything;
  their output is byte-identical to the Phase 6 binary on the whole corpus
  (both path modes, depths 3 and 6).
* `--validate-postgres` still replays *model* SQL only. Running application
  code needs `--verify-source` **and** `--allow-execution`; the report keeps
  the two comparisons in separate sections (`runtime` vs
  `sourceVerification`).
* The model crate does not parse TypeScript; the replay crate does not know
  the model. The coordinator is driven by a *plan* (kind, table, source
  region, expected lock wait per statement) that the model crate derives.
* Report schema: version 1 unless a Phase 7 section is present (then 2,
  `docs/phase7/schemas/model-check-report.v2.schema.json`, a superset).

## Not done (see [`IMPLEMENTATION-PLAN.md`](IMPLEMENTATION-PLAN.md))

Milestone 2's *shared semantic lowering* was not implemented: the static
analyzer and the concurrency extractor still lower TypeScript separately.
They now share the function identity (by source position) and nothing
else. The compact summaries are summaries of *inlined call scopes in the
path walker*, not parameterized summaries reused across call sites (the
IR-level inliner already memoizes expansions per callee and context).

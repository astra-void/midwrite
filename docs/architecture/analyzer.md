# Analyzer and static failure-path analysis

Crates: `commitrift-analyzer` (frontend) and `commitrift-core` (IR and
analysis). Used by `check`, `trace`, `ir`, `init-replay`. The detailed,
still-accurate design is [`docs/ARCHITECTURE.md`](../ARCHITECTURE.md);
this page is the map.

## Stages

```text
per file (parallel):  Oxc parser ─► Oxc semantic ─► provenance ─► recognition ─► lowering + value analysis
project:              module + export resolution ─► class table ─► one function table
                      ─► summary fixpoint (inertness, parameter use) ─► call summaries (inlined, memoized)
per function:         path enumeration (feasibility, merging, compact summaries)
                      ─► replay of points (commits, visible effects) ─► hazards ─► diagnostics / traces
```

| stage | where | what it decides | design |
| --- | --- | --- | --- |
| parsing | `oxc_parser` | a file with any parse error is a `FileFailure` and is not analyzed | [ARCHITECTURE § frontend](../ARCHITECTURE.md#frontend-commitrift-analyzer) |
| provenance | `bindings.rs` | what an identifier is: import, `drizzle(...)` instance, transaction handle, pending builder, call result, contrary value | same |
| recognition | `recognize.rs` | database read/write/transaction, configured external effect, lookalike (unknown with the path to configure) | same |
| lowering | `lower.rs`, `functions.rs` | structured Effect IR per function: effects, transactions, calls, branches with facts, switches, loops, `try`, exits | [ARCHITECTURE § Effect IR](../ARCHITECTURE.md#effect-ir-commitrift-coreir) |
| values | `values.rs`, `core::absint` | bounded abstract values, heap, provenance, return facts (Phase 5) | [Phase 5 analysis design](../phase5/ANALYSIS-DESIGN.md) |
| resolution | `resolve.rs`, `exports.rs`, `project.rs`, `link.rs`, `classes.rs` | relative imports, `tsconfig` `paths`/`baseUrl`, workspace packages, re-exports, classes across modules; ambiguity is never resolved by choice | [Phase 2 cross-module analysis](../phase2/CROSS-MODULE-ANALYSIS.md), [Phase 5 class resolution](../phase5/CLASS-RESOLUTION.md) |
| summaries | `core::analysis::summary` | callee bodies inlined at call sites up to `--max-call-depth`, handles bound to parameters, memoized; inert callees resolved at any depth | [ARCHITECTURE § call summaries](../ARCHITECTURE.md#call-summaries-commitrift-coreanalysissummary), [Phase 5 interprocedural summaries](../phase5/INTERPROCEDURAL-SUMMARIES.md) |
| paths | `core::analysis::paths` | traces per function (cap `--max-paths`), contradictory branch facts pruned, compact summaries spliced per call (Phase 7), shared path summaries (Phase 8) | [Phase 7 compact summaries](../phase7/COMPACT-SUMMARIES.md), [Phase 8 parameterized summaries](../phase8/PARAMETERIZED-SUMMARIES.md) |
| detection | `core::analysis::detect` | points (commit, staged, visible), adjacent-pair hazards, CP9001 placement | [ARCHITECTURE § failure-path analysis](../ARCHITECTURE.md#failure-path-analysis-commitrift-coreanalysis) |
| rendering | `core::analysis::render`, CLI | message, failure sequence, evidence, confidence, fingerprint, policy severity | [`DIAGNOSTICS.md`](../DIAGNOSTICS.md) |
| traces | `core::trace` | failure trace IR and crash simulation (Phase 3) | [Phase 3 trace format](../phase3/TRACE-FORMAT.md) |

## Boundaries

* `commitrift-core` never sees an AST; a new frontend only has to produce
  `FunctionIr` plus call sites.
* The static analyzer does not use the concurrency extractor: Phase 13's
  source-resolution work changed discovery, and the static `check` output
  stayed byte-identical (144/144, `reports/phase13/analyzer/check-identity.txt`).
* Results are deterministic and independent of thread scheduling (up to 8
  threads, `COMMITRIFT_THREADS` overrides).

## Environment variables

| variable | effect |
| --- | --- |
| `COMMITRIFT_THREADS` (legacy `CRASHPROOF_THREADS`) | worker threads for per-file work |
| `COMMITRIFT_PATHS=legacy\|compact\|shared` | path mode: `shared` (default since Phase 8), `compact` (Phase 7), `legacy` (walk every trace through inlined bodies); any other value is an error |

The three path modes are compared in every phase's corpus identity check
(they must produce identical output).

# Analyzer and static failure-path analysis

Crates: `midwrite-analyzer` (frontend) and `midwrite-core` (IR and
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
| parsing | `oxc_parser` | a file with any parse error is a `FileFailure` and is not analyzed | [ARCHITECTURE § frontend](../ARCHITECTURE.md#frontend-midwrite-analyzer) |
| provenance | `bindings.rs` | what an identifier is: import, `drizzle(...)` instance, transaction handle, pending builder, call result, contrary value | same |
| recognition | `recognize.rs` | database read/write/transaction, configured external effect, lookalike (unknown with the path to configure) | same |
| lowering | `lower.rs`, `functions.rs` | structured Effect IR per function: effects, transactions, calls, branches with facts, switches, loops, `try`, exits | [ARCHITECTURE § Effect IR](../ARCHITECTURE.md#effect-ir-midwrite-coreir) |
| values | `values.rs`, `core::absint` | bounded abstract values, heap, provenance, return facts | [value analysis](value-analysis.md) |
| resolution | `resolve.rs`, `exports.rs`, `project.rs`, `link.rs`, `classes.rs` | relative imports, `tsconfig` `paths`/`baseUrl`, workspace packages, re-exports, classes across modules; ambiguity is never resolved by choice | [ARCHITECTURE § project analysis](../ARCHITECTURE.md#project-analysis-midwrite-analyzerproject-resolve), [class resolution](value-analysis.md#class-and-method-resolution) |
| summaries | `core::analysis::summary` | callee bodies inlined at call sites up to `--max-call-depth`, handles bound to parameters, memoized; inert callees resolved at any depth | [ARCHITECTURE § call summaries](../ARCHITECTURE.md#call-summaries-midwrite-coreanalysissummary), [interprocedural summaries](value-analysis.md#interprocedural-summaries) |
| paths | `core::analysis::paths` | traces per function (cap `--max-paths`), contradictory branch facts pruned, compact and shared path summaries spliced per call | [ARCHITECTURE § failure-path analysis](../ARCHITECTURE.md#failure-path-analysis-midwrite-coreanalysis) |
| detection | `core::analysis::detect` | points (commit, staged, visible), adjacent-pair hazards, MW9001 placement | [ARCHITECTURE § failure-path analysis](../ARCHITECTURE.md#failure-path-analysis-midwrite-coreanalysis) |
| rendering | `core::analysis::render`, CLI | message, failure sequence, evidence, confidence, fingerprint, policy severity | [`DIAGNOSTICS.md`](../DIAGNOSTICS.md) |
| traces | `core::trace` | failure trace IR and crash simulation | [trace format](../reference/trace-format.md) |

## Boundaries

* `midwrite-core` never sees an AST; a new frontend only has to produce
  `FunctionIr` plus call sites.
* The static analyzer does not use the concurrency extractor: changes to
  source resolution for discovery leave the static `check` output
  byte-identical, which the corpus identity check compares.
* `trace` output can be turned into runtime evidence by an authorized
  harness: [single-request replay](replay-protocol.md).
* Results are deterministic and independent of thread scheduling (up to 8
  threads, `MIDWRITE_THREADS` overrides).

## Environment variables

| variable | effect |
| --- | --- |
| `MIDWRITE_THREADS` | worker threads for per-file work |
| `MIDWRITE_PATHS=legacy\|compact\|shared` | path mode: `shared` (default), `compact`, `legacy` (walk every trace through inlined bodies); any other value is an error |

The three path modes must produce identical output; the corpus identity
check and `MIDWRITE_PATHS_DIFFERENTIAL=1` compare them.

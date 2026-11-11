# Architecture overview

This page describes the system as it is and links to the pages that
detail each part; it does not repeat them.

## Pipeline

```text
TypeScript sources (.ts/.tsx/.mts/.cts)
   │  Oxc parser + Oxc semantic analysis (scopes, symbols)
   ▼
midwrite-analyzer ── provenance, recognition, lowering ──► Effect IR ──► midwrite-core
   │                    module/export resolution                          failure-path analysis
   │                    interprocedural summaries                         → diagnostics, traces (check/trace/ir)
   │
   │  concurrency extractor (TsActorProvider), entries, schema facts,
   │  handle provenance, package contracts, shared ProjectContext
   ▼
midwrite-discover ── entry and effect index ──► concurrency candidate discovery
   │                   property candidates (origin stated) ──► bounded scenario synthesis
   ▼
midwrite-model ── scenario loader ──► explicit-state model checker (reductions, faults)
   │                 ──► counterexample (witness, minimization, replay in the model)
   │                 ──► optional: lockstep PostgreSQL oracle (--validate-postgres)
   │                 ──► optional: original-code differential (--verify-source)
   ▼                                              │
midwrite-replay ◄───────────────────────────────┘ disposable PostgreSQL, OS isolation,
   │                                                source coordinator, protocol proxy
   ▼
runtime/node (worker client, pg / postgres-js / Drizzle instrumentation)
   ▼
evidence report: text, JSON (versioned schemas), script-free HTML  (midwrite-cli)
```

Two analyses share the frontend but stay separate: the **static
failure-path analysis** (`check`, Effect IR, `midwrite-core`) and the
**concurrency pipeline** (`discover`/`model-check`, model IR,
`midwrite-model`). They share function identity by source position
and semantic facts for chain decomposition; they lower
TypeScript separately.

## Crates and boundaries

| crate | responsibility | depends on (workspace) | must not |
| --- | --- | --- | --- |
| `midwrite-core` | Effect IR, failure-path analysis (paths, detection, rendering, summaries), bounded abstract interpretation (`absint`), diagnostics and code catalog, configuration, recovery contracts, trace and verification report types | — | depend on a parser or on TypeScript syntax |
| `midwrite-analyzer` | TypeScript frontend: file discovery, Oxc parsing and semantics, provenance, effect recognition, lowering, module/export resolution, classes, values; the concurrency extractor (`concurrency.rs`), entries, source identity, wrapper analysis, Drizzle schema facts (`schema.rs`), database-handle provenance (`handles.rs`), package contracts (`packages.rs`), the shared `ProjectContext` (`context.rs`) | core, model (IR and provider types), Oxc | execute or import project code |
| `midwrite-discover` | entry and effect index, resource identity, detectors, contracts file, property synthesis, state synthesis, scenario construction, ranking/deduplication, discovery report, evaluation scoring | core, analyzer, model | run application code |
| `midwrite-model` | model IR and scenario loader, declared-program DSL, PostgreSQL database model, engine and exploration (sleep sets, DPOR, source-DPOR), invariants, witnesses, reports and HTML, PostgreSQL lockstep oracle, original-code differential comparison, schema facts and fidelity, epistemic analysis | replay (optional, feature `postgres`) | — |
| `midwrite-replay` | disposable PostgreSQL clusters, OS isolation (macOS `sandbox-exec`, Linux bubblewrap), manifest validation, single-request replay supervisor and observations, original-code source coordinator, protocol fault proxy, catalog reads | core | run anything without `--allow-execution` and an authorized manifest |
| `midwrite-cli` | the `midwrite` binary: argument parsing, configuration discovery, text/JSON/SARIF/HTML rendering, baselines, exit codes; commands `check`, `explain`, `ir`, `trace`, `report`, `model-check`, `discover`, `schema`, `replay`, `init-replay` | core, analyzer, model, discover, replay (default feature `replay`) | — |
| `midwrite-corpus` | developer tool, not published: corpus runs, run comparison, reviewed precision, memory profile, trace coverage, review packets | core, analyzer | be used by `check` |
| `runtime/node` (`@midwrite/replay`, unpublished) | dependency-free JavaScript client: checkpoint protocol, `pg` adapter, event log, source worker (`source.js`) for Node.js and Bun | — | be required by analyzed applications outside a harness |

## Detailed pages

| topic | page | more detail |
| --- | --- | --- |
| TypeScript frontend and static analysis | [analyzer](analyzer.md) | [`docs/ARCHITECTURE.md`](../ARCHITECTURE.md), [value analysis](value-analysis.md), [trace format](../reference/trace-format.md) |
| concurrency extraction from source | [source extraction](source-extraction.md) | [schema extraction](schema-extraction.md), [loops and caught errors](loops-and-errors.md), [zod](../reference/zod.md) |
| discovery and synthesis | [scenario synthesis](scenario-synthesis.md) | [world search](world-search.md), [contracts](../user-guide/contracts.md) |
| model checker | [model checker](model-checker.md) | [model semantics](../concepts/model-semantics.md), [partial-order reduction](partial-order-reduction.md), [value semantics](../concepts/value-semantics.md) |
| PostgreSQL semantics | [PostgreSQL model](postgres-model.md) | [validation against PostgreSQL](postgres-model.md#validation-against-postgresql) |
| runtime verification | [runtime verification](runtime-verification.md) | [replay protocol](replay-protocol.md), [fault injection](fault-injection.md) |

## Cross-cutting rules

* **Determinism.** Sorted file walks, results collected by index, no
  hash-map iteration in anything that reaches a report; `--no-timing`
  makes `model-check` and `discover` output byte-reproducible.
* **Compatibility checks.** A change re-runs the previous binary and the
  new one on the same inputs: static corpus outputs (144 combinations)
  and the hand-written model-check scenarios
  ([evidence index](../research/evidence-index.md#7-compatibility-and-regression-identity)).
* **Opt-in semantics.** New model behavior is enabled per scenario so that
  old scenarios keep their state spaces.
* **Execution boundary.** Only `replay`, `model-check --verify-source` and
  `schema --migrations` execute harness, application or migration code,
  and only with `--allow-execution`. `model-check --validate-postgres`
  starts a private PostgreSQL and sends it only the model's SQL
  ([security](../reference/security.md)).

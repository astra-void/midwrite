# Architecture overview

Current state: Phase 13 working tree (base commit `32efbca`, uncommitted
Phase 13 changes). This page describes the system as it is and links to
the phase documents that justify each part; it does not repeat them.

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
(Phase 7) and semantic facts for chain decomposition (Phase 8); they lower
TypeScript separately.

## Crates and boundaries

| crate | responsibility | depends on (workspace) | must not |
| --- | --- | --- | --- |
| `midwrite-core` | Effect IR, failure-path analysis (paths, detection, rendering, summaries), bounded abstract interpretation (`absint`), diagnostics and code catalog, configuration, recovery contracts, trace and verification report types | — | depend on a parser or on TypeScript syntax |
| `midwrite-analyzer` | TypeScript frontend: file discovery, Oxc parsing and semantics, provenance, effect recognition, lowering, module/export resolution, classes, values; the concurrency extractor (`concurrency.rs`), entries, source identity, wrapper analysis, Drizzle schema facts (`schema.rs`), database-handle provenance (`handles.rs`), package contracts (`packages.rs`), the shared `ProjectContext` (`context.rs`) | core, model (IR and provider types), Oxc | execute or import project code |
| `midwrite-discover` | entry and effect index, resource identity, detectors, contracts file, property synthesis, state synthesis, scenario construction, ranking/deduplication, discovery report, evaluation scoring | core, analyzer, model | run application code |
| `midwrite-model` | model IR and scenario loader, declared-program DSL, PostgreSQL database model, engine and exploration (sleep sets, DPOR, source-DPOR), invariants, witnesses, reports and HTML, PostgreSQL lockstep oracle, original-code differential comparison, schema facts and fidelity, epistemic analysis | replay (optional, feature `postgres`) | — |
| `midwrite-replay` | disposable PostgreSQL clusters, OS isolation (macOS `sandbox-exec`, Linux bubblewrap), manifest validation, Phase 3/4 replay supervisor and observations, Phase 7+ source coordinator, protocol fault proxy, catalog reads | core | run anything without `--allow-execution` and an authorized manifest |
| `midwrite-cli` | the `midwrite` (and legacy `crashproof`) binary: argument parsing, configuration discovery, text/JSON/SARIF/HTML rendering, baselines, exit codes; commands `check`, `explain`, `ir`, `trace`, `report`, `model-check`, `discover`, `schema`, `replay`, `init-replay` | core, analyzer, model, discover, replay (default feature `replay`) | — |
| `midwrite-corpus` | developer tool, not published: corpus runs, run comparison, reviewed precision, memory profile, trace coverage, review packets | core, analyzer | be used by `check` |
| `runtime/node` (`@midwrite/replay`, unpublished) | dependency-free JavaScript client: checkpoint protocol, `pg` adapter, event log, source worker (`source.js`) for Node.js and Bun | — | be required by analyzed applications outside a harness |

## Detailed pages

| topic | page | historical design |
| --- | --- | --- |
| TypeScript frontend and static analysis | [analyzer](analyzer.md) | [`docs/ARCHITECTURE.md`](../ARCHITECTURE.md), [Phase 2](../phase2/CROSS-MODULE-ANALYSIS.md), [Phase 5](../phase5/ANALYSIS-DESIGN.md) |
| concurrency extraction from source | [source extraction](source-extraction.md) | [Phase 6](../phase6/CONCURRENT-IR.md), [Phase 7](../phase7/ARCHITECTURE.md), [Phase 12](../phase12/ARCHITECTURE.md), [Phase 13](../phase13/ARCHITECTURE.md) |
| discovery and synthesis | [scenario synthesis](scenario-synthesis.md) | [Phase 11](../phase11/ARCHITECTURE.md) |
| model checker | [model checker](model-checker.md) | [Phase 6](../phase6/MODEL-DESIGN.md), [Phase 9](../phase9/FORMAL-MODEL.md), [Phase 10](../phase10/POR-ARCHITECTURE.md) |
| PostgreSQL semantics | [PostgreSQL model](postgres-model.md) | [Phase 6](../phase6/DATABASE-MODEL.md), [Phase 13](../phase13/POSTGRESQL-FK-LOCKS.md) |
| runtime verification | [runtime verification](runtime-verification.md) | [Phase 3](../phase3/ARCHITECTURE.md), [Phase 7](../phase7/RUNTIME-SCHEDULER.md), [Phase 8](../phase8/ARCHITECTURE.md) |

## Cross-cutting rules

* **Determinism.** Sorted file walks, results collected by index, no
  hash-map iteration in anything that reaches a report; `--no-timing`
  makes `model-check` and `discover` output byte-reproducible.
* **Compatibility checks per phase.** Each phase re-runs the previous
  binary and the new one on the same inputs: static corpus outputs
  (144 combinations) and the 56 Phase 6–9 model-check scenarios
  ([evidence index](../research/evidence-index.md#7-compatibility-and-regression-identity)).
* **Opt-in semantics.** New model behavior is enabled per scenario so that
  old scenarios keep their state spaces.
* **Execution boundary.** Only `replay`, `model-check --verify-source` and
  `schema --migrations` execute harness, application or migration code,
  and only with `--allow-execution`. `model-check --validate-postgres`
  starts a private PostgreSQL and sends it only the model's SQL
  ([security](../reference/security.md)).

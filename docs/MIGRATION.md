# Documentation migration

The documentation is organized by topic. Until November 2026 it also
carried a phase-by-phase engineering record (one documentation directory
per phase, with a matching directory of raw evidence). That record was removed from the
working tree; it is preserved unchanged in the git tag
`history/phase-records`, and [development history](HISTORY.md) explains
how to read it. Everything in it that still describes the system was
folded into the topic documents below. Index: [`docs/README.md`](README.md).

## Where phase topics went

If you have an old link to a phase document, the current home of its
subject is:

| subject of the phase documents | current document |
| --- | --- |
| cross-module analysis, call summaries, compact and parameterized path summaries | [ARCHITECTURE.md](ARCHITECTURE.md) |
| abstract domain, heap and aliases, interprocedural summaries, async semantics, class resolution, effect relationships | [architecture/value-analysis.md](architecture/value-analysis.md) |
| precision changes, policies | [DIAGNOSTICS.md](DIAGNOSTICS.md), [user-guide/check.md](user-guide/check.md) |
| trace format | [reference/trace-format.md](reference/trace-format.md) |
| replay protocol, Node.js + Drizzle replay, harness generation, Linux isolation | [architecture/replay-protocol.md](architecture/replay-protocol.md) |
| recovery semantics and contracts | [reference/configuration.md](reference/configuration.md#recovery-declarations) |
| model design, semantics, formal model, epistemic semantics, process and task model | [concepts/model-semantics.md](concepts/model-semantics.md) |
| database model, PostgreSQL isolation, FK locks, multi-row locks, NULLS NOT DISTINCT, replica consistency, PostgreSQL validation and differential matrices | [architecture/postgres-model.md](architecture/postgres-model.md) |
| partial-order reduction, DPOR, source-set DPOR, dependence and enabling relations, formal assumptions, POR correctness, adversarial generation | [architecture/partial-order-reduction.md](architecture/partial-order-reduction.md) |
| state-space storage, counterexamples, verdict semantics | [architecture/model-checker.md](architecture/model-checker.md) |
| failure and retry, crash-retry semantics, ambiguous commit, operation lifecycle | [concepts/failure-model.md](concepts/failure-model.md) |
| invariants, generated-ID invariants | [concepts/invariants.md](concepts/invariants.md) |
| generated-value semantics, JavaScript value semantics | [concepts/value-semantics.md](concepts/value-semantics.md) |
| source identity, callback resolution, control flow, Drizzle extraction, predicates, request bodies, return values and responses, source resolution, package contracts, source-model fidelity | [architecture/source-extraction.md](architecture/source-extraction.md) |
| schema extraction and schema fidelity | [architecture/schema-extraction.md](architecture/schema-extraction.md) |
| loop semantics, caught errors | [architecture/loops-and-errors.md](architecture/loops-and-errors.md) |
| zod validation | [reference/zod.md](reference/zod.md) |
| runtime scheduler, original-code recovery, postgres-js adapter, runtime verification, test reliability | [architecture/runtime-verification.md](architecture/runtime-verification.md), [development/testing.md](development/testing.md) |
| protocol fault injection, connection pool runs, live connection failures, postgres-js in flight | [architecture/fault-injection.md](architecture/fault-injection.md) |
| candidate discovery, effect index, invariant synthesis, state synthesis, contracts | [architecture/scenario-synthesis.md](architecture/scenario-synthesis.md), [user-guide/contracts.md](user-guide/contracts.md) |
| input domains, constraint synthesis, feedback-guided search, coverage | [architecture/world-search.md](architecture/world-search.md) |
| every phase's `SECURITY.md` | [reference/security.md](reference/security.md) |
| every phase's `LIMITATIONS.md` | [reference/limitations.md](reference/limitations.md) (static analyzer detail: [LIMITATIONS.md](LIMITATIONS.md)) |
| evaluation datasets, precision and recall, independent evaluation | [research/evaluation-datasets.md](research/evaluation-datasets.md), [research/evaluation-methodology.md](research/evaluation-methodology.md) |
| corpus and real-world evaluation | [development/corpus-evaluation.md](development/corpus-evaluation.md) |
| quality gates, alpha readiness | [development/testing.md](development/testing.md), [alpha/README.md](alpha/README.md) |
| report schemas (per-phase `schemas/` directories) | [reference/schemas/](reference/schemas) |
| baselines, implementation plans, performance measurements, final reports | archived only; headline numbers in [research/evidence-index.md](research/evidence-index.md) |

## Moved files

Test and fixture names no longer carry phase numbers either: for example
the first-party applications are under `fixtures/apps/`, model-check
scenarios under `fixtures/scenarios/` and `fixtures/model/`, the frozen
evaluation sets under `fixtures/eval/p11` … `p16`, and report files that
tests read under `fixtures/reports/`. Integration tests are named by the
behavior they cover ([testing](development/testing.md)).

Identifiers that are part of a wire format or of a frozen evaluation set
were not renamed by this reorganization.

## Feature-based code

Extractor state is grouped by behavior: loop unrolling (`LoopState`),
caught errors (`CaughtErrorState`) and request-body consumption
(`RequestState`). Zod schemas expose `supports_explicit_lowering` to select
explicit validation steps or the checked parser. These names describe the
supported operations rather than the development phase that added them.

`SourceVerification::required_report_schema_version` returns the oldest
model-check report schema that can represent the evidence present. Report
generation combines that requirement with the model's own requirement by
taking the higher version. Existing JSON field names and schema versions
are preserved.

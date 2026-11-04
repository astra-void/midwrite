# Midwrite documentation

Midwrite statically analyzes TypeScript applications that use Drizzle ORM
on PostgreSQL for crash and concurrency hazards, synthesizes bounded
scenarios, model-checks them, and can optionally run a counterexample
against the original code on a disposable PostgreSQL database.

```text
TypeScript ─► Oxc semantic analysis ─► interprocedural effect extraction ─► concurrency candidate discovery
           ─► bounded scenario synthesis ─► explicit-state model checker ─► counterexample
           ─► optional PostgreSQL replay / original-code verification ─► evidence report
```

**Status.** Experimental. These pages describe the current system: the
Phase 16 working tree (base commit `1efa383`; Phase 16 changes
uncommitted). Phase 16 added zod object validation (zod 3 and 4,
differentially checked against the pinned runtimes), bounded loops and
retry loops, caught database errors, `request.json()`, held Drizzle
predicates, multi-row row locks, `NULLS NOT DISTINCT`, properties over
generated identifiers, a discovery-report JSON Schema and a Closed Alpha
package ([Phase 16 final report](phase16/FINAL-REPORT.md)). Phase 14 added JavaScript values, closures, zod validation,
multi-row inserts, predicates, timestamps/JSON subsets and new candidate
families ([Phase 14 final report](phase14/FINAL-REPORT.md)); Phase 15 added
typed input domains, a feedback-guided search over initial worlds,
guard-satisfying synthesis and coverage accounting in four dimensions
([Phase 15 final report](phase15/FINAL-REPORT.md)). Example outputs name
the binary that produced them. No independent human review of any result
has been performed, and no external application has been executed.

The `docs/phaseN/` directories are the historical engineering record and
are not rewritten; this layer links to them for detail.
[What changed in the documentation](MIGRATION.md).

## Start here

| | |
| --- | --- |
| [Installation](getting-started/installation.md) | build from source; prerequisites for the optional runtime steps |
| [Quickstart](getting-started/quickstart.md) | `midwrite check` on one file, with real output |
| [First verification](getting-started/first-verification.md) | model checking, discovery, PostgreSQL replay, original code |
| [Closed Alpha guide](alpha/README.md) | for invited developers: build, first run, coverage, reproduction, feedback (not distributed) |

## User guide

| | |
| --- | --- |
| [`check`](user-guide/check.md) | static failure-path analysis, policies, baselines, CI |
| [`discover`](user-guide/discover.md) | candidates, properties and scenarios from source |
| [`model-check`](user-guide/model-check.md) | scenarios, verdicts, search options |
| [Runtime verification](user-guide/source-verification.md) | `--validate-postgres`, `--verify-source`, replay harnesses |
| [Contracts](user-guide/contracts.md) | what source cannot say: `midwrite-contracts.json` |
| [Interpreting results](user-guide/interpreting-results.md) | the verification workflow and what each result does not say |

## Reference

| | |
| --- | --- |
| [CLI reference](reference/cli.md) | every command and flag of the current binary, exit statuses, environment |
| [Configuration](reference/configuration.md) | `midwrite.json`, recovery declarations, trust model |
| [Output formats](reference/output-formats.md) | text, JSON kinds and schema versions, SARIF, HTML |
| [Diagnostics](DIAGNOSTICS.md) | `MW1001`–`MW9001`, `MW7001`–`MW7103`, JSON fields |
| [Supported patterns](reference/supported-patterns.md) | what the analyzer and the extractor recognize |
| [**Limitations**](reference/limitations.md) | the single current limitations document |
| [Security](reference/security.md) | what is read, executed and written; isolation; privacy |

## Concepts

| | |
| --- | --- |
| [Supported semantics and technologies](concepts/supported-semantics.md) | TypeScript, Drizzle, PostgreSQL, drivers: what is in scope |
| [Failure model](concepts/failure-model.md) | commits, visible effects, crashes, retries, lost acknowledgments |
| [Concurrency model](concepts/concurrency-model.md) | actors, schedules, bounds, reductions |
| [Invariants](concepts/invariants.md) | assertion language, property origins, templates |
| [Evidence levels](concepts/evidence-levels.md) | from static candidate to original-code reproduction |

## Architecture

| | |
| --- | --- |
| [Overview](architecture/overview.md) | pipeline, crates and their boundaries |
| [Analyzer](architecture/analyzer.md) | Oxc frontend, Effect IR, failure-path analysis |
| [Source extraction](architecture/source-extraction.md) | TypeScript → model IR |
| [Scenario synthesis](architecture/scenario-synthesis.md) | discovery pipeline |
| [Model checker](architecture/model-checker.md) | engine, state identity, reductions |
| [PostgreSQL model](architecture/postgres-model.md) | modeled and refused PostgreSQL behavior |
| [Runtime verification](architecture/runtime-verification.md) | coordinator, worker protocol, proxy, isolation |
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | detailed static-analyzer design (Phases 1–7), still accurate for `check` |

## Development and research

| | |
| --- | --- |
| [Contributing](../CONTRIBUTING.md) | ground rules, fixtures, gates |
| [Testing](development/testing.md) | test suite, runtime prerequisites, Linux container |
| [Benchmarks](development/benchmarks.md) | benchmark methodology and scripts |
| [Corpus evaluation](development/corpus-evaluation.md) | the real-world corpus and its rules |
| [Evaluation harness](development/evaluation-harness.md) | precision and recall on dev and held-out projects (`scripts/eval.py`) |
| [Roadmap](development/roadmap.md) | open problems named by the phase reports |
| [Evidence index](research/evidence-index.md) | every major measured result with its revision, environment and raw evidence |
| [Evaluation methodology](research/evaluation-methodology.md) | how results are measured; independence |

## Historical phase records

Each phase directory holds a baseline, design notes, evaluations and a
final report; raw evidence is under `reports/phaseN/`. Phase 1 has no
separate directory. Numbers in these records belong to the binary of that
phase.

| phase | topic | final report |
| --- | --- | --- |
| 2 | real-world evaluation, cross-module analysis, policies, baselines | [phase2](phase2/FINAL-REPORT.md) |
| 3 | rename to Midwrite; failure traces; opt-in runtime replay | [phase3](phase3/FINAL-REPORT.md) |
| 4 | Node.js + Drizzle replay, Linux isolation, harness generation, recovery contracts | [phase4](phase4/FINAL-REPORT.md) |
| 5 | bounded interprocedural abstract interpretation: values, classes, callbacks, async | [phase5](phase5/FINAL-REPORT.md) |
| 6 | explicit-state model checker, PostgreSQL isolation model, lockstep validation | [phase6](phase6/FINAL-REPORT.md) |
| 7 | source selectors, wrapped callbacks, Drizzle fidelity, original-code verification, compact summaries | [phase7](phase7/FINAL-REPORT.md) |
| 8 | crash and retry against original code, postgres-js, schema extraction, shared summaries | [phase8](phase8/FINAL-REPORT.md) |
| 9 | lost acknowledgments (in-flight operations), client knowledge, pools, DPOR, compact state storage | [phase9](phase9/FINAL-REPORT.md) |
| 10 | source-set DPOR, adversarial testing of reductions, live connection loss, postgres-js in-flight | [phase10](phase10/FINAL-REPORT.md) |
| 11 | automatic scenario synthesis (`discover`), property provenance, contracts | [phase11](phase11/FINAL-REPORT.md) |
| 12 | schema fidelity, generated values, cross-module resolution, shared analysis context | [phase12](phase12/FINAL-REPORT.md) |
| 13 | foreign-key and row-lock semantics, return values and the response step, read replicas, upserts, source resolution | [phase13](phase13/FINAL-REPORT.md) |
| 14 | JavaScript values, closures, zod, multi-row inserts, predicates, timestamps/JSON, replica round trip, parent-child and return-value families, next steps | [phase14](phase14/FINAL-REPORT.md) |
| 15 | typed input domains, feedback-guided search over initial worlds, guard-satisfying synthesis, coverage in four dimensions, reproduction artifacts, a fresh frozen evaluation set | [phase15](phase15/FINAL-REPORT.md) |
| 16 | zod validation, bounded and retry loops, caught errors, request bodies, held predicates, multi-row locks, NULLS NOT DISTINCT, generated-identity properties, discovery-report schema, Closed Alpha package | [phase16](phase16/FINAL-REPORT.md) |

Top-level records kept as they were: [`LIMITATIONS.md`](LIMITATIONS.md)
(static analyzer, Phases 1–7; superseded as the *current* list by
[reference/limitations.md](reference/limitations.md)),
[`ARCHITECTURE.md`](ARCHITECTURE.md), [`DIAGNOSTICS.md`](DIAGNOSTICS.md).

## Checking these documents

```bash
python3 scripts/check-doc-links.py --historical report
```

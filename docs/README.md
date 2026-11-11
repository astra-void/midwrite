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

**Status.** Experimental. These pages describe the current system by
topic. Example outputs name the binary that produced them. No independent
human review of any result has been performed, and no external
application has been executed. How the system was built, phase by phase,
and where the archived phase records are: [development history](HISTORY.md).
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
| [Report schemas](reference/schemas) | JSON Schemas of every report version |
| [Trace format](reference/trace-format.md) | the failure-trace report |
| [zod support](reference/zod.md) | supported zod subset, versions, semantics |
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
| [Model semantics](concepts/model-semantics.md) | state, transitions, processes, bounds, possible worlds: what "sound" means |
| [Value semantics](concepts/value-semantics.md) | SQL vs JavaScript values, generated values, clock, JSON |

## Architecture

| | |
| --- | --- |
| [Overview](architecture/overview.md) | pipeline, crates and their boundaries |
| [Analyzer](architecture/analyzer.md) | Oxc frontend, Effect IR, failure-path analysis |
| [Value analysis](architecture/value-analysis.md) | abstract values, heap, summaries, classes and callbacks for `check` |
| [Source extraction](architecture/source-extraction.md) | TypeScript → model IR |
| [Schema extraction](architecture/schema-extraction.md) | Drizzle schemas, defaults and generated values, catalog comparison |
| [Loops and caught errors](architecture/loops-and-errors.md) | loop unrolling, retry loops, caught database errors |
| [Scenario synthesis](architecture/scenario-synthesis.md) | discovery pipeline |
| [World search](architecture/world-search.md) | typed input domains, guard satisfaction, initial worlds, coverage |
| [Model checker](architecture/model-checker.md) | engine, state identity, reductions |
| [Partial-order reduction](architecture/partial-order-reduction.md) | sleep sets, DPOR, source-set DPOR, formal assumptions, how they are checked |
| [PostgreSQL model](architecture/postgres-model.md) | modeled and refused PostgreSQL behavior |
| [Runtime verification](architecture/runtime-verification.md) | coordinator, worker protocol, original-code runs |
| [Fault injection](architecture/fault-injection.md) | protocol proxy, lost acknowledgments, live connection loss |
| [Replay protocol](architecture/replay-protocol.md) | single-request replay, checkpoints, `init-replay`, isolation |
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | detailed static-analyzer design, current for `check` |

## Development and research

| | |
| --- | --- |
| [Contributing](../CONTRIBUTING.md) | ground rules, fixtures, gates |
| [Testing](development/testing.md) | test suite, runtime prerequisites, Linux container |
| [Benchmarks](development/benchmarks.md) | benchmark methodology and scripts |
| [Corpus evaluation](development/corpus-evaluation.md) | the real-world corpus and its rules |
| [Evaluation harness](development/evaluation-harness.md) | precision and recall on dev and held-out projects (`scripts/eval.py`) |
| [Roadmap](development/roadmap.md) | open problems |
| [Evidence index](research/evidence-index.md) | headline measured results, the tests and fixtures behind them, archived raw evidence |
| [Evaluation methodology](research/evaluation-methodology.md) | how results are measured; independence |
| [Evaluation datasets](research/evaluation-datasets.md) | the frozen evaluation sets in `fixtures/eval/` |
| [Development history](HISTORY.md) | what each development phase added; the `history/phase-records` tag |

## Earlier records

[`LIMITATIONS.md`](LIMITATIONS.md) is the detailed list for the static
analyzer (`check`, `trace`, replay); [reference/limitations.md](reference/limitations.md)
is the current list for everything. The per-phase design notes, reports
and raw evidence are archived in the git tag `history/phase-records`
([development history](HISTORY.md)).

## Checking these documents

```bash
python3 scripts/check-doc-links.py
```

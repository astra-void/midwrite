# Documentation migration (2026-10-28)

A current documentation layer was added on top of the historical phase
records. Nothing was deleted or moved; no `docs/phaseN/` file and nothing
under `reports/` was changed. Index: [`docs/README.md`](README.md).

## Created

| area | files |
| --- | --- |
| index | `docs/README.md`, `docs/MIGRATION.md` (this file) |
| getting started | `getting-started/installation.md`, `quickstart.md`, `first-verification.md` |
| user guide | `user-guide/check.md`, `discover.md`, `model-check.md`, `source-verification.md`, `interpreting-results.md`, `contracts.md` |
| concepts | `concepts/failure-model.md`, `concurrency-model.md`, `invariants.md`, `evidence-levels.md`, `supported-semantics.md` |
| architecture | `architecture/overview.md`, `analyzer.md`, `source-extraction.md`, `scenario-synthesis.md`, `model-checker.md`, `postgres-model.md`, `runtime-verification.md` |
| reference | `reference/cli.md`, `configuration.md`, `output-formats.md`, `supported-patterns.md`, `limitations.md`, `security.md` |
| development | `development/testing.md`, `benchmarks.md`, `corpus-evaluation.md`, `roadmap.md` (contributing stays in [`CONTRIBUTING.md`](../CONTRIBUTING.md)) |
| research | `research/evidence-index.md`, `research/evaluation-methodology.md` |
| tooling | `scripts/check-doc-links.py` (Python 3 standard library) |

Configuration lives in `reference/configuration.md` (no separate
getting-started page). The planned `reference/report-formats.md` is named
`reference/output-formats.md` (the authoring tool refused file names
containing "report"); rename it if the original name is preferred and
update the links to it in `docs/README.md`,
`architecture/model-checker.md` and `reference/cli.md`.

## Consolidated (sources kept unchanged)

| new document | built from |
| --- | --- |
| `reference/limitations.md` — the single current limitations document, with a "Phase 14 changes" placeholder | `docs/LIMITATIONS.md` and `docs/phase6` … `phase13/LIMITATIONS.md`; items lifted by later phases moved to a "lifted" table (FK locks, return values, response step, replicas, upserts and lock strengths in Phase 13; generated keys and multi-module schemas in 12; live connection loss and postgres-js in-flight in 10; in-flight operations, pools and shared processes in 9; crash/retry and postgres-js in 8; extraction coverage in 7) |
| `research/evidence-index.md` | every phase's `FINAL-REPORT.md`, `BASELINE.md`, evaluation and performance documents; raw paths checked to exist under `reports/` |
| `reference/cli.md` | `--help` of every subcommand of the release binary built 2026-10-27 (pre-Phase 14); exit statuses read from `crates/commitrift-cli/src` |
| `reference/supported-patterns.md` | README "Supported patterns", `docs/LIMITATIONS.md` support levels, Phase 6/7/12/13 extraction documents |
| `user-guide/contracts.md` | `docs/phase11/CONTRACTS.md`, Phase 13 `replicaFactories`, `crates/commitrift-discover/src/contracts.rs` field list |
| `concepts/evidence-levels.md` | Phase 3, 6, 7, 8, 13 evidence definitions and `EvidenceLevel` in `crates/commitrift-model/src/report.rs` |
| `reference/security.md` | README "Security considerations" and the per-phase `SECURITY.md` files |

Example outputs in the getting-started and reference pages were produced
by the pre-Phase-14 binary with read-only commands (`check`, `trace`,
`explain`, `model-check` without validation flags, `discover`, `schema`
without `--migrations`). Nothing was run with `--allow-execution`.

## Modified

| file | change |
| --- | --- |
| `README.md` | documentation callout near the top; status block updated from "Phase 5" to the Phase 13 state (Phases 2–5 history kept below it); a Phase 13 paragraph after the Phase 12 one; `model-check`, `discover`, `schema` rows and a CLI-reference link in the usage table; one false-negative bullet corrected (methods of known classes are followed since Phase 5); project layout lists `commitrift-discover`, `commitrift-model`, the new docs and `reports/`; link to the testing page. No section was removed and no heading changed, so existing anchors (`#configuration`, `#supported-patterns`, `#policies`, …) still resolve. |
| `CONTRIBUTING.md` | limitations go into `docs/reference/limitations.md`; run the link checker |
| `docs/LIMITATIONS.md`, `docs/ARCHITECTURE.md` | one-paragraph pointer to the current documents at the top; content unchanged |

## Moved

Nothing.

## Intentionally retained

* `docs/phase2/` … `docs/phase13/`: the historical engineering record,
  unchanged. `docs/phase13/` is being completed by the lead engineer.
* `reports/phase2/` … `reports/phase13/`: raw evidence, unchanged.
* `docs/LIMITATIONS.md` (static analyzer detail, Phases 1–7),
  `docs/ARCHITECTURE.md` (static analyzer design), `docs/DIAGNOSTICS.md`
  (the diagnostics catalog, still current).
* README's deep sections (configuration, policies, transaction semantics,
  false positives/negatives, security): duplicated in the new reference
  pages but kept, because they are linked into and remain accurate.

## Link check

`python3 scripts/check-doc-links.py --historical report` at the end of
this migration: README/CONTRIBUTING 0 broken, current docs 0 broken,
historical `docs/phaseN/` 0 broken relative links (187 relative Markdown
links in phase documents). No historical file needed a link fix.

Not checked by the tool (plain-text path references, not links), noted for
the phase owners:

* `docs/phase13/POSTGRESQL-FK-LOCKS.md` cited `reports/phase13/evidence/`,
  which did not exist then; resolved at the Phase 13 closeout (the evidence
  script was fixed and run; the directory now exists).
* `docs/phase13/FINAL-REPORT.md` listed `REAL-WORLD-EVALUATION.md` and
  `PERFORMANCE.md`; resolved at the closeout (both are sections of the
  final report; the list says so).

## Phase 14 additions (lead)

* `docs/phase14/` (baseline, architecture, value semantics, replica
  integration, source coverage, Drizzle differential, evaluation,
  real-world results, performance, security, limitations, final report).
* Updated current pages: `reference/limitations.md` (Phase 14 changes),
  `user-guide/discover.md` (new families, next step), `user-guide/contracts.md`
  (return-value templates), `getting-started/quickstart.md` (a discovery
  walk-through with Phase 14 output), this index.

## Phase 15 additions

* `docs/phase15/` (baseline, architecture, input domains, constraint
  synthesis, feedback-guided search, verdict semantics, source coverage,
  real-world evaluation, PostgreSQL differential, precision/recall,
  performance, alpha readiness, security, limitations, final report). The
  brief's STATE-SPACE-SAMPLING, INPUT-COVERAGE, MIDDAY-EVALUATION and
  SCHOOL-LEDGER-EVALUATION are consolidated into FEEDBACK-GUIDED-SEARCH,
  VERDICT-SEMANTICS and REAL-WORLD-EVALUATION.
* Updated current pages: `reference/cli.md` (`--max-worlds`,
  `--all-worlds`, `--explain-coverage`, reproduction artifacts),
  `reference/output-formats.md` (`coverage`, `commitrift.counterexample`),
  `reference/limitations.md` (Phase 15 changes), `user-guide/discover.md`
  (input exploration), `user-guide/interpreting-results.md` (verdicts are
  relative to explored worlds), `getting-started/quickstart.md` (an
  input-world walk-through validated with the Phase 15 binary),
  `research/evidence-index.md` (Phase 15 rows), this index.
* No historical `docs/phaseN/` file was changed.

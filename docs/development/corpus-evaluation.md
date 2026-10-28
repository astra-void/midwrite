# Corpus evaluation

The corpus is a set of real TypeScript/Drizzle/PostgreSQL applications that
CommitRift analyzes **statically only**. It measures recognition, coverage,
regressions and performance on code nobody wrote for CommitRift. It is not
a source of confirmed defects.

## Projects

| project | visibility | revision | notes |
| --- | --- | --- | --- |
| saas-starter | public | `6e33e58b1e553a41fe22e6b941a7229a002de361` | Next.js, Drizzle, postgres-js |
| midday | public | `51587319f26a0ffaa9dfccab1920373cb65689b7` | 2 022 modules; its own `withReplicas` |
| school-ledger | **private** | local copy | aggregate counts only |
| unnamed-ai-project | **private** | local copy | aggregate counts only |

Manifests and per-project configurations: `corpora/manifest*.json`,
`corpora/configs/`, review labels `corpora/labels/`, discovery contracts
`corpora/contracts/` (Phase 13: midday's replica factory). The public
projects are cloned at pinned revisions by `corpora/fetch.sh` into
`.corpora/` (ignored by git).

## Running it

```bash
corpora/fetch.sh
cargo run --release -p commitrift-corpus -- run --manifest corpora/manifest.json --output reports/after.json
cargo run --release -p commitrift-corpus -- compare reports/phase3/corpus-phase3.json reports/after.json
cargo run --release -p commitrift-corpus -- precision --report reports/after.json --labels corpora/labels
commitrift discover .corpora/midday --model-check --contracts corpora/contracts/midday.json --format json -o midday.json
python3 scripts/phase13-analyzer-corpus.py      # the Phase 13 measurement (see the script for arguments)
```

The identity checks of each phase (`scripts/phase7-identity.sh`,
`scripts/phase8-identity.sh`, and the phase evidence scripts) compare the
previous and new binaries on all four projects × depths 3, 6, 12 × three
path modes × json, sarif, text, trace (144 outputs).

## Rules

* Never execute corpus code, install packages into it, or modify its
  working tree. Private projects are analyzed from read-only copies
  without `node_modules`.
* Never write file names, paths, selectors, identifiers or SQL parameters
  of the private projects into reports, documentation, tests or Rust code;
  only aggregate counts and generic causes. No project-specific name
  appears in Rust code.
* No threshold may be tuned for the corpus. A corpus result is never
  claimed as a defect of the project without independent review (none has
  been performed); maintainers have not been contacted.

## What the corpus has shown

Summarized with numbers in the [evidence index § 1](../research/evidence-index.md#1-static-analysis-accuracy)
and [§ 8](../research/evidence-index.md#8-real-world-corpus-coverage-static-discovery):

* Static analysis: Phase 2 review (implementer) and Phase 3 blind
  automated re-review; most structurally correct findings were deliberate
  (best-effort notifications, job retries).
* Discovery: one heuristic model counterexample (saas-starter, Phase 11,
  never executed). Phase 12 made most tables modelable (generated keys);
  Phase 13 bound midday's replicated database through a declared factory
  and resolved school-ledger's alias blockers, which then reached
  inconclusive or unsupported synthesis. No new corpus counterexample was
  found in Phases 12 and 13.

Per-phase write-ups: [2](../phase2/CORPUS-EVALUATION.md),
[5](../phase5/CORPUS-EVALUATION.md), [6](../phase6/CORPUS-EVALUATION.md),
[7](../phase7/CORPUS-EVALUATION.md), [8](../phase8/CORPUS-EVALUATION.md),
[9](../phase9/CORPUS-EVALUATION.md), [10](../phase10/CORPUS-EVALUATION.md),
[11](../phase11/REAL-WORLD-EVALUATION.md), [12](../phase12/REAL-WORLD-COVERAGE.md),
[13 midday](../phase13/MIDDAY-COVERAGE.md),
[13 school-ledger](../phase13/SCHOOL-LEDGER-COVERAGE.md).

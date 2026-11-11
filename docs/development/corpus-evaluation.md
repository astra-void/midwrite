# Corpus evaluation

The corpus is a set of real TypeScript/Drizzle/PostgreSQL applications that
Midwrite analyzes **statically only**. It measures recognition, coverage,
regressions and performance on code nobody wrote for Midwrite. It is not
a source of confirmed defects. A later, separate
[upstream source-review campaign](upstream-review.md) reports four potential
bugs with source-level failure sequences; those reports are not runtime
confirmation or independent human review and do not revise these scores.

Precision and recall with a dev / held-out split are measured by
[`scripts/eval.py`](evaluation-harness.md).

## Projects

| project | visibility | revision | notes |
| --- | --- | --- | --- |
| saas-starter | public | `6e33e58b1e553a41fe22e6b941a7229a002de361` | Next.js, Drizzle, postgres-js |
| midday | public | `51587319f26a0ffaa9dfccab1920373cb65689b7` | 2 022 modules; its own `withReplicas` |
| school-ledger | **private** | local copy | aggregate counts only |
| unnamed-ai-project | **private** | local copy | aggregate counts only |

Manifests and per-project configurations: `corpora/manifest*.json`,
`corpora/configs/`, review labels `corpora/labels/`, discovery contracts
`corpora/contracts/` (midday's declared replica factory). Each project's
configuration was written from its database and client modules before any
result was looked at, and is not changed afterwards; no `exclude` patterns
were added, so test and seed files are analyzed. The private projects are
analyzed from `git archive`s of pinned revisions in a scratch directory. The public
projects are cloned at pinned revisions by `corpora/fetch.sh` into
`.corpora/` (ignored by git).

## Running it

```bash
corpora/fetch.sh
cargo run --release -p midwrite-corpus -- run --manifest corpora/manifest.json --output reports/after.json
cargo run --release -p midwrite-corpus -- compare reports/before.json reports/after.json
cargo run --release -p midwrite-corpus -- precision --report reports/after.json --labels corpora/labels
midwrite discover .corpora/midday --model-check --contracts corpora/contracts/midday.json --format json -o midday.json
python3 scripts/corpus.py               # discovery on the corpus, stage by stage, two binaries
```

The identity checks (`scripts/check-identity.sh`,
`scripts/check-identity.sh`, and the evidence scripts) compare the
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
  been performed). No maintainers were contacted for those corpus scores;
  later [upstream reports](upstream-review.md) are recorded separately.

## What is measured

* **Static analysis** (`check`): per project, functions with recognized
  effects, resolved and unresolved cross-file calls, calls replaced by a
  summary, truncated functions, contradictory paths pruned, diagnostics by
  code, and wall time and peak RSS. Two runs of the same command produce
  byte-identical reports (also with 1 and 8 threads). Findings are reviewed
  as described in [evaluation methodology § static-finding review](../research/evaluation-methodology.md#static-finding-review).
* **Discovery** (`discover --model-check`): per project, modules, entry
  candidates, entries not reaching a database, extracted and effectful
  entries, unknown operations by category, tables modeled and refused,
  candidates, model-checkable candidates, statuses, counterexamples, wall
  time (median of 3) and peak RSS (maximum), with the previous and the new
  binary on the same machine and inputs. Each candidate's **first blocker**
  (the stage where it stopped) is recorded; for private projects only its
  generic category. No contracts are written for corpus projects except
  where a project's own source states the fact (midday's replica factory):
  a contract would be an assumption about their behavior.

## What the corpus has shown

Numbers are in the [evidence index § 1](../research/evidence-index.md#1-static-analysis-accuracy)
and [§ 8](../research/evidence-index.md#8-real-world-corpus-coverage-static-discovery).

* Static analysis: the first review (implementer) and a blind automated
  re-review found most structurally correct findings deliberate
  (best-effort notifications, job retries); all true positives were found
  through cross-module summaries.
* Discovery: one heuristic model counterexample (saas-starter's
  invitation check-then-act, never executed). Generated keys made most
  tables modelable; a declared replica factory bound midday's replicated
  database; authorization guards in school-ledger are now satisfied with
  stated preconditions. No other corpus counterexample has been found.

### Current blockers

| project | candidates | first blocker | what would unblock it faithfully |
| --- | ---: | --- | --- |
| school-ledger | 7 | zod `.superRefine()` on an array | a declared or modeled refinement, verified against zod |
| school-ledger | 2 | method call on a value of unresolved type | type resolution of that value |
| midday | 1 | `?.` on a call result; `Promise.all` whose result is used | optional chains on non-identifier bases; `Promise.all` with a destructured result |
| midday | 2 | no declared invariant (write skew) | a team declaring its rule ([templates](../user-guide/contracts.md#templates-for-multi-row-invariants)) |
| midday | 1 | none: model-checkable, `NO_VIOLATION_WITHIN_BOUNDS` against a heuristic property over generated identity | — |
| saas-starter | 1 | model-checkable (the heuristic counterexample above); the other candidate is unsupported | — |
| unnamed-ai-project | 0 | no candidate | — |

The per-phase corpus write-ups are archived with the rest of the
development record ([history](../HISTORY.md)).

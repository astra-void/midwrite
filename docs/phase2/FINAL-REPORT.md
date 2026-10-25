# Phase 2 final report

Question: *can CrashProof produce useful, sufficiently precise findings on
real TypeScript applications?* Measured answer, on four projects (two public,
two private): the structural claim of a finding (a partial-completion window
exists between two independently committed effects) held in 92 of 97 reviewed
findings with a decided label, but only 27 of them were judged likely bugs;
most others are windows the code accepts on purpose. Cross-module analysis
was the change that made findings appear at all in the largest project (0
cross-resource findings in Phase 1, 49 in Phase 2, all reached through
imported functions). Details and caveats below; every number comes from
files under `reports/phase2/` or `corpora/labels/`.

> **Follow-up.** The next step recommended in section 7 has since been
> implemented: handles returned by configured calls (`getDb()`), object-literal
> services, best-effort external effects with first-argument rules, and
> transaction-result facts (prompted by two false positives the `getDb()`
> evaluation exposed). With the original corpus configurations no finding
> changes. With configurations written after the review, midday warnings
> drop from 49 to 36 without demoting any reviewed true positive. See
> [`FOLLOW-UP.md`](FOLLOW-UP.md). The tables below describe the Phase 2
> state; test counts are updated to the follow-up.

## 1. Implementation summary

| Change | Why | Phase 1 weakness addressed |
| --- | --- | --- |
| Corpus runner (`crates/crashproof-corpus`), pinned corpus manifests, label files, `scripts/bench.py`, `scripts/compare-phase1.py` | Reproducible measurement, run comparison, precision only over reviewed findings | No real-world evaluation |
| Branch facts and path feasibility (`ir::Fact`, `paths.rs`) | Discard paths whose simple conditions contradict each other | Correlated conditions reported (`if (a) … if (!a) …`) |
| `pathCertainty` + `evidenceStrength`; member-path matches capped at `medium`; unawaited transactions capped at `medium` | Separate path certainty from recognition evidence | A member-path guess could be `high` |
| Policies `recommended` (default) / `strict` / `audit` | CP1004/CP1005 volume; keep cross-resource hazards and transaction bypasses as warnings | 96 of 123 baseline warnings were CP1004 |
| Parameter handles and argument binding (`DbHandle::Parameter`, `CallArgument`) | A helper receiving `tx` must join the caller's transaction | High-confidence false CP1004 "bypass" (confirmed in the corpus) |
| Project analysis: module resolution (relative, `tsconfig` paths, workspace packages), export resolution, one function table, cross-file summaries with file-aware locations | Real applications split actions, services and queries across modules | Only same-file helpers were followed |
| Summary memoization; reads excluded from traces; dominated traces merged after calls; parallel per-file work | Keep the larger analysis usable | — (new cost of cross-module analysis) |
| Merged alternative partners; grouped unknown calls | Fewer duplicate diagnostics, same information | Up to four diagnostics for one write |
| Fingerprints, `--baseline`, `--write-baseline`, SARIF `partialFingerprints`, `exclude` patterns, `summary.analysis` counts | CI adoption | No way to adopt incrementally |

## 2. Architecture changes

* **Project graph** (`crashproof-analyzer/src/project.rs`, `exports.rs`,
  `resolve.rs`): per-file lowering records call targets, argument handles and
  exports; a project step resolves modules and exports, places all functions
  in one table indexed by position (never by AST symbol id), and links calls
  only through resolved imports and exports.
* **Cross-module resolution**: relative specifiers, nearest `tsconfig.json`
  `paths`/`baseUrl` with relative `extends`, workspace packages via
  `package.json` `name`/`exports`/`main`; ambiguous candidates and exports are
  never chosen; type-only imports never link.
* **Summary caching**: callee expansions and their every-path certainty are
  memoized per (callee, remaining depth) unless a recursive cut made them
  stack-dependent; arguments are substituted at copy time, so the cache key
  needs no bindings. midday: 749 expansions, 1181 reuses.
* **Path feasibility**: arm facts on never-reassigned variables, renamed per
  inlined call; contradictory traces discarded; reads excluded; dominated
  traces merged when leaving a call.
* `crashproof-core` stays parser-independent: it gained `Location::file`,
  `FileIr::sources`, `Arm::facts`, `DbHandle::Parameter`,
  `TransactionInfo::parent_parameter`, `inline_calls` with `ProjectFiles`,
  `analyze_function_in`, policies and fingerprints.

## 3. Correctness report

| Suite | Phase 1 (baseline) | Phase 2 |
| --- | --- | --- |
| `cargo test --workspace` | 102 passed | 156 passed, 0 failed |
| core unit tests | 48 | 69 |
| analyzer unit tests | 3 | 8 |
| `tests/analysis.rs` | 32 | 32 (unchanged) |
| `tests/fixtures.rs` (53 single-file fixtures) | 3 | 3 |
| `tests/projects.rs` (21 multi-file fixture projects, 48 TypeScript files) | — | 1 |
| `tests/phase2.rs` | — | 22 |
| `tests/cli.rs` | 15 | 19 |
| corpus runner unit tests | — | 1 |
| doc-tests | 1 | 1 |

`cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --
-D warnings` are clean. SARIF output for all fixtures (70 results) and for
midday with a baseline (269 results, with `baselineState`) validates against
the SARIF 2.1.0 JSON schema from json.schemastore.org (checked with Python
`jsonschema`); JSON report fields are checked by `tests/cli.rs`.

**Regressions in existing fixtures:** none in `(code, line)` terms: the 50
single-file fixtures produce the same 51 findings as Phase 1. Intentional
changes: five member-path findings went from `high` to `medium` confidence;
under the new default policy a non-bypass CP1004 or a CP1005 is `info`
(one CLI test updated, with `--policy strict` covering the old exit status).
Two fixtures gained two explanatory comment lines.

**False positives fixed:**

* helper parameters bound to the caller's handle: removes false CP1004
  "bypass" findings; confirmed in the corpus at midday
  `packages/db/src/queries/teams.ts:280` (2 Phase 1 diagnostics);
* contradictory conditions (fixtures and tests; no Phase 1 corpus finding
  depended on them);
* variant duplicates: 17 Phase 1 CP1004 merged into their primary finding.

**Known remaining limitations:** `docs/LIMITATIONS.md`. Most important:
methods and callbacks are not followed; database handles from calls
(`getDb()`) are not recognized; adjacency pairs are reported even when the
second operation is not owed because of the first (the main false-positive
class); job retries, webhook redelivery and best-effort intent are invisible
(the main "intended risk" class); 22 corpus functions are truncated at 512
paths.

## 4. Real-world evaluation

| Project | Revision | Files | Diagnostics (default policy) | Warnings |
| --- | --- | --- | --- | --- |
| saas-starter | `6e33e58b1e553a41fe22e6b941a7229a002de361` | 41 | CP1001 1, CP1004 11, CP1005 3, CP9001 6 | 1 |
| midday | `51587319f26a0ffaa9dfccab1920373cb65689b7` | 2022 | CP1001 40, CP1002 9, CP1004 116, CP1005 15, CP9001 89 | 49 |
| school-ledger (private) | `4b97f342` + 2 local changes | 462 | CP1004 123, CP9001 194 | 0 |
| unnamed-ai-project (private) | `39fcfc7c` + 2 local changes | 104 | CP1004 8, CP9001 9 | 0 |

Reviewed findings: 103 (every CP1001/CP1002; all CP1004/CP1005 of the two
small projects; deterministic samples of the others). Results with 95%
Wilson intervals (`reports/phase2/precision.txt`):

| Code | Decided | Real window ("hazard precision") | Likely bug ("bug precision") |
| --- | --- | --- | --- |
| CP1001 | 38 | 0.97 (0.87–1.00) | 0.34 (0.21–0.50) |
| CP1002 | 8 | 0.75 (0.41–0.93) | 0.25 (0.07–0.59) |
| CP1004 | 43 | 0.98 (0.88–1.00) | 0.28 (0.17–0.43) |
| CP1005 | 8 | 0.88 (0.53–0.98) | 0.00 (0.00–0.32) |

The reviewer was the implementer of Phase 2, reading source code; no
maintainer confirmed any finding and nothing was reproduced at runtime. No
recall is claimed. Examples of likely bugs (midday): invoices marked sent
before their generation job is enqueued; a scheduled-invoice job whose own
status guard makes a crash retry skip the invoice permanently; a bank
connection deleted before the job that revokes it is triggered.

Unsupported patterns seen: `getDb()` handles (school-ledger throughout),
methods and callbacks, non-declaration exports, third-party packages, and
truncation in large test functions and worker processors
(`CORPUS-EVALUATION.md`).

## 5. Performance report

Median of 10 runs, wall seconds (`PERFORMANCE.md`):

| Project | Phase 1 | Phase 2 (8 threads) | Phase 2 (1 thread) | Peak RSS Phase 1 → Phase 2 |
| --- | --- | --- | --- | --- |
| saas-starter | 0.0077 | 0.0076 | 0.0088 | 4.9 → 6.8 MiB |
| midday | 0.2325 | 0.1563 | 0.3513 | 20.5 → 73.5 MiB |
| school-ledger | 0.0879 | 0.1177 | 0.2347 | 9.5 → 76.3 MiB |
| unnamed-ai-project | 0.0111 | 0.0099 | 0.0137 | 5.5 → 8.1 MiB |

Regressions, measured and explained: single-threaded CPU work grew 14–167%;
peak memory grew 1.4–8×, because the whole project's IR is kept for
cross-module summaries; school-ledger is 34% slower even with threads
(larger inlined test functions). The parallel default makes the large
monorepo 33% faster than Phase 1.

## 6. Milestones

| Milestone | Status |
| --- | --- |
| 0 Baseline and audit | Done (`BASELINE.md`) |
| 1 Validation infrastructure | Done: corpus runner (run, compare, precision), manifests, labels, benchmark script |
| 2 Correctness and false-positive reduction | Done: feasibility, confidence semantics, CP1004/CP1005 policy, transaction audit |
| 3 Cross-module analysis | Done for the documented subset (relative, `tsconfig` paths, workspace packages; plain function calls) |
| 4 Scalability | Done: memoization, path pruning/merging, parallelism, measurements; memory regression remains |
| 5 CI readiness | Done: fingerprints, baselines (with moved-code matching), policies, SARIF fingerprints, `exclude` |
| 6 Independent validation | Done with caveats: two public and two private projects, 103 findings reviewed by the implementer, no maintainer confirmation, no runtime reproduction |
| 7 Documentation | Done (this directory, README, ARCHITECTURE, DIAGNOSTICS, LIMITATIONS) |

Not done in Phase 2 itself: method calls and callbacks, `getDb()`-style
handles, an incremental mode, reducing memory, labeling every corpus
finding. Call-returned handles and object-literal services were added in the
follow-up; class methods, callbacks, incremental mode and memory remain.

## 7. Next recommended priority

**Reduce "intended risk" noise without guessing intent**, because it now
dominates the non-bug findings (65 of 97 decided). Two concrete steps,
both measurable with the existing labels:

1. Recognize *call-returned* database handles (`getDb()`, `ctx.db` from
   destructured parameters) and **method calls on module-level service
   objects**, the two largest remaining false-negative classes seen in the
   corpus.
2. Let teams state, in configuration, that an external effect is
   best-effort or is driven from durable state (an outbox relay, a retried
   job queue), and report pairs ending in such effects as `info`. This keeps
   the decision with the team instead of inferring it from `try`/`catch`
   shapes, which the review showed are present around real bugs too.

Then re-run the corpus and the precision report, and extend the labels to a
second reviewer.

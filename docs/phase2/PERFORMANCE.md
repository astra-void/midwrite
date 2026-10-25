# Performance (Milestone 4)

All numbers were measured on 2026-10-24 on an Apple M1 Pro (8 performance +
2 efficiency cores), 16 GiB, macOS, release builds (`cargo build --release`,
rustc 1.98.1), on the fixed corpus of `CORPUS-EVALUATION.md`.

## Method

* End to end: `scripts/bench.py --runs 10`: one untimed warm-up run, then 10
  runs of `crashproof check . --config <cfg> --format json --fail-on never`
  from the project root, each a fresh process; wall time with
  `time.perf_counter`, peak RSS from `getrusage(RUSAGE_CHILDREN)` of a forked
  helper per run. Median, minimum and maximum are reported. Raw rows:
  `reports/phase2/bench-phase2.tsv` (default threads),
  `reports/phase2/bench-phase2-1thread.tsv` (`CRASHPROOF_THREADS=1`),
  `reports/phase2/baseline/bench-phase1.tsv` and
  `reports/phase2/baseline/bench-phase1-rerun.tsv` (Phase 1 binary, measured
  at the start of the work and again in the same session as the Phase 2
  runs).
* Phases: `crashproof-corpus run --timings`, in process, median of 5 runs
  (`reports/phase2/phase-timings.txt`).
* Phase 1 is the saved binary `.corpora/bin/crashproof-phase1`. Its run is
  single-threaded; Phase 2 parallelizes per-file work on up to 8 threads.

## End to end

Median wall time in seconds (min–max over 10 runs) and median peak RSS:

| Project | Phase 1 | Phase 2, 8 threads | Phase 2, 1 thread | RSS Phase 1 | RSS Phase 2 (8 / 1 threads) |
| --- | --- | --- | --- | --- | --- |
| saas-starter (41 files) | 0.0077 (0.0075–0.0083) | 0.0076 (0.0075–0.0081) | 0.0088 (0.0084–0.0137) | 4.9 MiB | 6.8 / 6.0 MiB |
| midday (2022 files) | 0.2325 (0.2302–0.2361) | 0.1563 (0.1550–0.1635) | 0.3513 (0.3482–0.3695) | 20.5 MiB | 73.5 / 61.0 MiB |
| school-ledger (462 files) | 0.0879 (0.0877–0.0887) | 0.1177 (0.1138–0.1219) | 0.2347 (0.2304–0.2406) | 9.5 MiB | 76.3 / 62.1 MiB |
| unnamed-ai-project (104 files) | 0.0111 (0.0110–0.0114) | 0.0099 (0.0094–0.0106) | 0.0137 (0.0135–0.0143) | 5.5 MiB | 8.1 / 7.0 MiB |

(Phase 1 column: the in-session re-run; the first Phase 1 measurement, in
`BASELINE.md`, agrees within 5%.)

Relative to Phase 1, measured:

* single-threaded, Phase 2 is slower on every project: +14% (saas-starter),
  +51% (midday), +167% (school-ledger), +23% (unnamed-ai-project);
* with the default threads, wall time is −1%, −33%, +34% and −11%;
* peak RSS is 1.4–8× higher (midday 20.5 → 73.5 MiB, school-ledger
  9.5 → 76.3 MiB).

These regressions buy the cross-module analysis: midday now replaces 1150
calls with summaries and school-ledger 1310 (most of them cross-file), the
IR of every function of the project is held in memory at once (Phase 1
analyzed and dropped one file at a time), and the larger inlined functions
have more paths to explore.

## Phases (in process, median of 5, milliseconds)

| Project | Threads | Total | Lower | Resolve + call graph | Summaries | Paths + detection | Files/s | Functions/s |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| saas-starter | 8 | 3.7 | 1.9 | 0.3 | 0.3 | 0.5 | 11221 | 42695 |
| midday | 8 | 144.2 | 68.2 | 29.1 | 15.9 | 16.4 | 14019 | 102801 |
| school-ledger | 8 | 96.5 | 19.0 | 6.3 | 21.0 | 39.7 | 4789 | 66242 |
| unnamed-ai-project | 8 | 4.8 | 2.9 | 0.3 | 0.3 | 0.3 | 21757 | 135565 |
| saas-starter | 1 | 5.0 | 3.4 | 0.2 | 0.3 | 0.4 | 8213 | 31248 |
| midday | 1 | 342.5 | 246.1 | 27.9 | 14.7 | 40.5 | 5903 | 43288 |
| school-ledger | 1 | 218.5 | 81.8 | 5.9 | 19.4 | 100.6 | 2115 | 29255 |
| unnamed-ai-project | 1 | 9.7 | 7.9 | 0.2 | 0.2 | 0.4 | 10748 | 66967 |

"Resolve + call graph" includes reading `tsconfig.json`/`package.json` files
and building the function table; it is single-threaded. Summary
memoization: midday 749 expansions / 1181 reuses, school-ledger 1001 / 4918.
No warm (cached) mode exists, so no warm time is reported.

## Path explosion: what changed and what it did

Cross-module summaries make functions much larger (an integration test that
calls ten services now contains ten inlined service bodies). Measured on
school-ledger with the default `--max-paths 512`, in the order the changes
were made:

| State | Truncated functions | CP9001 |
| --- | --- | --- |
| cross-module summaries only (first working version) | 129 | 770 |
| + unknown calls grouped per inlined call | 129 | 756 |
| + reads do not fork paths or appear in traces | 33 | 741 |
| + context-only unknown calls grouped per function (default policy) | 33 | 212 |
| + dominated traces merged when leaving an inlined call | 12 | 194 |

(Single development runs; the wall-time figures above are the benchmark.)
An intermediate version that walked each incoming trace through a callee
without bounding the result took 4.5 s and 1.27 GB on school-ledger; the
walk is now bounded by the caller's path budget.

midday: 15 truncated functions before trace merging, 10 after. Phase 1 had
1 truncated function in each of midday and school-ledger.

The three path changes are sound within the analysis model:

* **Contradictory paths** are not real paths (`PRECISION-CHANGES.md`).
* **Reads** never become points (commits or visible effects), so a branch
  whose arms differ only in reads cannot change a finding.
* **Merging** keeps, of the traces that leave one inlined call from the same
  incoming trace, those not dominated by another: same events after the call
  started, and no fewer branch decisions between any two consecutive events.
  A dominated trace yields the same findings with at most the same
  confidence; the facts that distinguish the traces concern the callee's
  variables only, which nothing after the call can refer to. A callee with
  `k` early returns therefore contributes one continuation per distinct
  effect sequence instead of `k + 1` (`callee_early_returns_do_not_multiply_caller_paths`).

Truncation still happens (22 functions in the corpus) and is reported as
CP9001 and in `summary.analysis.truncatedFunctions`; the results of a
truncated function are partial.

## Determinism

Corpus reports are byte-identical across runs and between 1 and 8 threads
(checked with `cmp`); `crates/crashproof-analyzer/tests/phase2.rs`
(`project_analysis_is_deterministic`) and the CLI determinism test cover the
fixtures.

## Possible improvements (not done)

* Drop each module's AST-derived data earlier and avoid the second copy of
  the function table that the inliner keeps (`functions.to_vec()`), which
  accounts for part of the RSS growth.
* Parallelize resolution and the summary pass.
* Analyze only functions reachable from changed files for CI (incremental
  mode).

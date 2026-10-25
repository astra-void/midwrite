# Performance and memory

All numbers were measured on 2026-10-25 on one Apple Silicon Mac (macOS),
release builds, files in the OS cache (one warm-up run). Phase 2 numbers are
from the final Phase 2 binary (`.corpora/bin/crashproof-phase2`, built from
the unchanged working tree before Phase 3) run **in the same session**; the
values recorded in `docs/phase2/` were measured on another day and are not
used as the reference (the same Phase 2 binary measured 6–17% slower today
than recorded, see `BASELINE.md`). Raw data: `reports/phase3/bench-*.tsv`,
`phase-timings.txt`, `memory-phases-*.txt`, `bench-trace-and-replay.txt`.

## Static analysis (`check`), wall time and peak RSS

`scripts/bench.py`, 10 runs, median; peak RSS is the child's `ru_maxrss`.

| Project | Phase 2, 8 threads | Phase 3, 8 threads | Phase 2, 1 thread | Phase 3, 1 thread |
| --- | --- | --- | --- | --- |
| saas-starter | 0.0087 s, 6.8 MiB | 0.0092 s, 7.0 MiB | 0.0102 s, 6.0 MiB | 0.0106 s, 6.3 MiB |
| midday | 0.1676 s, 75.5 MiB | 0.1694 s, **67.4 MiB** | 0.3790 s, 61.1 MiB | 0.3888 s, **54.0 MiB** |
| school-ledger | 0.1225 s, 76.4 MiB | 0.1227 s, **72.8 MiB** | 0.2524 s, 64.6 MiB | 0.2530 s, **60.7 MiB** |
| unnamed-ai-project | 0.0106 s, 8.3 MiB | 0.0110 s, 8.3 MiB | 0.0155 s, 7.2 MiB | 0.0159 s, 7.2 MiB |

* **Peak RSS decreased** on the large projects: midday −8.1 MiB (−11%) with
  8 threads and −7.1 MiB (−12%) with one; school-ledger −3.6 MiB (−5%) and
  −3.9 MiB (−6%).
* **Wall time**: +0.1% to +2.6% on the large projects, +0.4–0.5 ms (+4–6%)
  on the two small ones. The in-process phase timings below show no
  difference in the analysis itself beyond noise; the small fixed cost on
  tiny projects is consistent with the larger binary (3.8 → 5.2 MB:
  replay, trace, HTML code) being loaded. Not investigated further.
* Small projects use +0.2–0.3 MiB more RSS (binary size).

## Phase timings (in process, corpus runner `--timings`, median of 5)

| Project | Phase 2: lower / resolve / summaries / analysis (ms) | Phase 3 |
| --- | --- | --- |
| midday, 8 threads | 72.8 / 30.4 / 16.1 / 17.8 (total 153.1) | 70.7 / 30.6 / 15.8 / 17.5 (151.1) |
| school-ledger, 8 threads | 20.7 / 6.4 / 22.0 / 42.4 (102.5) | 19.5 / 6.5 / 21.7 / 41.8 (102.3) |
| midday, 1 thread | 262.4 / 30.2 / 15.9 / 43.1 (366.7) | 265.0 / 30.6 / 16.0 / 43.6 (372.7) |
| school-ledger, 1 thread | 88.0 / 6.1 / 21.4 / 110.0 (237.3) | 88.8 / 6.4 / 20.5 / 111.1 (238.0) |

A first measurement showed Phase 3 three times slower with 8 threads. The
cause was measurement, not the analyzer: the corpus runner's new counting
allocator updated shared atomic counters on every allocation from all
threads. It now counts only during `commitrift-corpus memory`; the numbers
above are after that fix. The `commitrift` binary never had it.

## Memory investigation (Milestone 7.2)

Method: `commitrift-corpus memory` installs a counting global allocator and
reads live heap bytes at the analyzer's phase boundaries (an optional probe,
`commitrift_analyzer::MEMORY_PROBE`, unset in normal runs), single-threaded,
`--max-call-depth 3`. Values are live bytes above the start of the analysis.

| Project | Code | after lowering | after resolution | after summaries | after analysis | peak | IR nodes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| midday | before the fix | 20.2 MiB | 22.7 | 24.5 | 27.6 | 34.8 | 27,278 |
| midday | Phase 3 | 11.8 MiB | 14.2 | 17.6 | 20.7 | **27.4** | 27,278 |
| school-ledger | before the fix | 10.2 MiB | 12.1 | 23.2 | 26.0 | 31.1 | 40,358 |
| school-ledger | Phase 3 | 6.9 MiB | 8.7 | 20.1 | 22.8 | **26.9** | 40,358 |

Findings, by the profile items the plan asked about:

* **Project-level IR retention** is the largest part on midday. A scratch
  probe at `--max-call-depth 0` showed the returned analysis holding
  14.2 MiB, of which 11.7 MiB were function bodies for only 19,082 IR nodes:
  ~610 bytes per node of 232 bytes. Cause: bodies are built by `push`, a
  `Vec<Node>` starts at capacity 4, and most bodies hold one or two nodes.
  **Fix:** `shrink_to_fit` on every lowered body (11.7 → 6.1 MiB).
* **Node size.** Every `Node` is as large as its largest variant, `Effect`.
  Phase 3's new `references` field had grown it from 208 to 232 bytes (the
  measured +4–5 MiB RSS regression before the fix). **Fix:** box
  `Effect::recognition` (absent on most nodes) and store `references` as a
  boxed slice: `Node` is now 176 bytes, smaller than in Phase 2.
* **Summary expansion copies** dominate school-ledger: +11.4 MiB for
  27,800 inlined nodes (most in test files). Copies are collected with exact
  capacity. Sharing callee bodies instead of copying them would need a
  different IR representation; not done (not a targeted change).
* **Duplicate function tables**: none found; the project table is moved,
  not copied, into the per-file results.
* **Path allocations**: peak minus "after analysis" is 4–7 MiB on the large
  projects with one thread (paths of the largest functions, freed per
  function); with 8 threads the peak adds 4–5 MiB for concurrent work.
* **Cached traces**: traces are built only by `commitrift trace`; `check`
  allocates none. `trace` costs +2.4 MiB live (midday, `--traces` in the
  probe) and +11 MiB RSS end to end, mostly its 2.8 MB JSON output.
* **File-level semantic data lifetime**: the Oxc AST arena, semantic tables
  and source text are dropped at the end of each file's lowering; they do not
  appear in the retained heap.

With 8 threads the peak live heap is 32.2 MiB on midday and 32.5 MiB on
school-ledger (before the fix: 38.8 and 36.0;
`memory-phases-phase3-8threads.txt`). Peak RSS is higher than peak live heap
(67 vs 32 MiB on midday) because of allocator retention, thread stacks and
the binary; those were not changed.

## `trace` vs `check`

| Project | `check` | `trace --format json` |
| --- | --- | --- |
| midday | 0.165 s, 66.3 MiB, 1.3 MB output | 0.173 s, 77.2 MiB, 2.8 MB output |
| school-ledger | 0.121 s, 74.3 MiB, 2.2 MB output | 0.131 s, 80.1 MiB, 4.9 MB output |

(median of 7, 8 threads; `reports/phase3/bench-trace-and-replay.txt`)

## Runtime replay (separate; not comparable with static analysis)

Each invocation creates and starts a PostgreSQL cluster (`initdb` ~0.8 s),
then runs the control and every case from a fresh database, each run
starting a Bun process.

| Harness | Harness runs | Wall time (median of 3 invocations) | Per-run duration (median) |
| --- | --- | --- | --- |
| order-vulnerable, `--repeat 1` | 7 | 2.45 s | 189 ms |
| order-vulnerable, `--repeat 5` | 31 | 7.71 s | 186 ms |
| order-outbox, `--repeat 1` | 6 | 3.16 s | 388 ms (cases with a recovery run start a second process) |
| order-outbox, `--repeat 5` | 26 | 11.13 s | 386 ms |

The replay tests (`cargo test -p commitrift-cli --test replay`, 14 tests)
take about 7 s on this machine.

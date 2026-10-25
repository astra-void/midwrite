# Phase 6 performance

Measured on 2026-10-25 on one Apple Silicon Mac (macOS), release builds.
Raw data: `reports/phase6/perf/`.

## 1. Ordinary static analysis is unchanged

`commitrift check` never runs the model checker; Phase 6 added no code to
its path (the analyzer gained a separate module used only by
`model-check`). Phase 5 binary (preserved before any change) vs Phase 6
binary, same corpus checkouts, `scripts/bench.py --runs 5`, two rounds each
(median wall time, peak RSS of the median run):

| Project | Phase 5 r1 | Phase 5 r2 | Phase 6 r1 | Phase 6 r2 |
| --- | --- | --- | --- | --- |
| saas-starter | 0.0092 s, 8.0 MiB | 0.0094 s, 8.0 MiB | 0.0104 s, 8.2 MiB | 0.0094 s, 8.3 MiB |
| midday | 0.2087 s, 93.1 MiB | 0.2075 s, 95.8 MiB | 0.2064 s, 96.5 MiB | 0.2061 s, 98.8 MiB |
| school-ledger | 0.1494 s, 91.7 MiB | 0.1486 s, 91.4 MiB | 0.1479 s, 91.3 MiB | 0.1478 s, 92.3 MiB |
| unnamed-ai-project | 0.0118 s, 9.7 MiB | 0.0118 s, 9.6 MiB | 0.0118 s, 9.9 MiB | 0.0117 s, 9.9 MiB |

Wall time: no measurable change (differences are within the spread between
two rounds of the same binary). Peak RSS: +0–3 MiB, at the edge of
run-to-run spread; the release binary grew from 6.2 MB to 8.3 MB (model
checker, oracle, HTML) and its mapped pages count toward RSS. Output:
byte-identical `check` and `trace` reports on all four projects
([`CORPUS-EVALUATION.md`](CORPUS-EVALUATION.md)).

## 2. Phase 5 performance debt (investigated, not changed)

Per-phase timings (`commitrift-corpus run --timings`, 8 threads, depth 3):

| | lower | resolve | summaries | path analysis | total | summary expansions / reuses |
| --- | --- | --- | --- | --- | --- | --- |
| midday | 80.1 ms | 42.5 ms | 35.6 ms | 18.2 ms | 199 ms | 2 816 / 1 489 |
| school-ledger | 23.3 ms | 9.5 ms | 36.0 ms | 44.4 ms | 127 ms | 2 496 / 5 099 |

Effect of the call-depth bound (`--max-call-depth`), same machine:

| Depth | midday findings | midday time | midday RSS | school-ledger findings | school-ledger time (summaries / analysis) | school-ledger RSS |
| --- | --- | --- | --- | --- | --- | --- |
| 3 (default) | 282 | 187 ms | 96 MiB | 332 | 123 ms (36 / 44) | 91 MiB |
| 4 | 285 | 206 ms | — | 544 | 149 ms (43 / 64) | — |
| 5 | 285 | 213 ms | — | 594 | 175 ms (50 / 82) | — |
| 6 | 285 | 212 ms | 113 MiB | 593 | 191 ms (53 / 94) | 126 MiB |

Findings:

* In midday, cost is dominated by lowering and resolution (61 % of the
  total); summaries and path analysis are 27 %. Deeper summaries change
  little (+3 findings).
* In school-ledger, deeper call chains add 261 findings, and most of the extra
  time is **path analysis** (+114 % from depth 3 to 6), not summary
  construction (+47 %): summaries are expanded into the caller's IR at every
  call site, and the caller's path enumeration then walks the inlined
  bodies. A compact summary — the callee's ordered effect classes and its
  internal failure windows, without its full body — would let the caller's
  analysis skip re-walking inlined paths; its cost would scale with call
  sites rather than with inlined path counts.
* Not implemented in Phase 6: it changes the findings of the default
  analysis (depth) and needs the reviewed policy decision Phase 5 already
  asked for. The default depth stays 3. This is the recommended next step
  for the static analyzer.

## 3. Model checker

`cargo run --release -p commitrift-model --example bench`
(`reports/phase6/perf/model-check-bench.tsv`). "Schedules" counts maximal
paths in the explored state graph; reduction prunes transitions and
schedules, not states (by design: every reachable state is still
visited).

| Scenario | Actors | Unreduced: states / transitions / schedules | Reduced (sleep sets): transitions / schedules | Wall (unreduced → reduced) | First counterexample (reduced) |
| --- | --- | --- | --- | --- | --- |
| purchase in a transaction, RC (violation) | 2 | 40 / 60 / 92 | 47 / 15 | 1.2 → 0.7 ms | 13 transitions, 0.2 ms |
| | 3 | 313 / 694 / 92 448 | 500 / 940 | 7.8 → 5.8 ms | 21 transitions, 0.2 ms |
| | 4 | 3 002 / 9 745 / 984 462 720 | 6 973 / 267 063 | 68 → 59 ms | 41 transitions, 0.4 ms |
| atomic conditional update (no violation) | 2 | 24 / 34 / 24 | 25 / 6 | 0.41 → 0.35 ms | — |
| | 3 | 110 / 232 / 1 524 | 142 / 47 | 1.7 → 1.4 ms | — |
| | 4 | 488 / 1 458 / 206 208 | 794 / 526 | 9.0 → 7.3 ms | — |
| `SELECT FOR UPDATE` (no violation) | 4 | 596 / 1 782 / 575 280 | 1 120 / 2 638 | 10.4 → 9.4 ms | — |
| write skew, SERIALIZABLE | 2 | 30 / 40 / 70 | 33 / 21 | 0.26 → 0.23 ms | — |
| deadlock | 2 | 36 / 48 / 42 | 40 / 9 | 0.20 → 0.18 ms | — |
| 300 random scenarios (aggregate) | 2–3 | 21 157 states / 42 019 transitions | 32 409 transitions | 217 → 203 ms | — |

Visited-state memory: ≤ 3.2 MiB in every row (canonical encodings of
40–150 bytes plus 96 bytes of table overhead per state).

Effect of crashes and retries (two workers, at-least-once delivery):

| Crashes / retries | States | Transitions (unreduced → reduced) | Schedules (unreduced → reduced) |
| --- | --- | --- | --- |
| 0 / 0 | 43 | 52 → 46 | 46 → 19 |
| 1 / 0 | 85 | 120 → 114 | 194 → 81 |
| 0 / 1 | 115 | 158 → 148 | 208 → 83 |
| 1 / 1 | 349 | 508 → 486 | 1 110 → 503 |
| 2 / 2 | 1 290 | 2 040 → 1 989 | 9 798 → 5 070 |

Observations, stated as measured:

* **State merging** is what keeps these scenarios small: 4 purchasers have
  ~10⁹ schedules but 3 002 distinct states.
* **Sleep sets** remove 28–46 % of transitions on 3–4 actors and shrink the
  explored schedule graph by up to three orders of magnitude, but save only
  10–20 % of wall time: every enabled transition is still *applied* once
  per expansion to obtain its footprint. Crash and retry transitions are
  global (dependent on everything), so they reduce little.
* **Depth-first search with reduction reaches the first counterexample
  fastest** (it dives into one complete schedule); breadth-first finds the
  shortest one (36 ms vs 0.4 ms for 4 actors), and the minimizer shortens
  depth-first witnesses afterwards.
* The 16 fixture scenarios (extracted from TypeScript) explore in under
  1 ms each (below the report's millisecond resolution); the whole
  `model-check` command takes about 0.03 s, dominated by extraction and
  Effect IR linking.

PostgreSQL validation cost: the full differential matrix (60
configurations, 1 695 schedules, every one in a fresh database state with
2–4 `psql` sessions) took 255 s; `model-check --validate-postgres` on a
fixture counterexample takes about 0.95 s end to end (measured on all 8),
mostly cluster creation and start-up.

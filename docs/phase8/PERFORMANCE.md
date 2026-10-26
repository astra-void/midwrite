# Phase 8 performance

Apple Silicon (arm64), macOS (Darwin 27.0.0), release builds, Phase 7
binaries rebuilt from the committed Phase 7 tree (`git archive` of
`6ada341`) and run interleaved with the Phase 8 binaries in the same
session. Raw data: `reports/phase8/perf/`. These are measurements, not
targets.

## 1. Static analysis by call depth (Phase 7 vs Phase 8)

`scripts/phase8-depth-bench.py` — `commitrift-corpus run --timings` (8
threads), median of 5 interleaved runs per cell; the findings of both
binaries are asserted identical (ids) at every depth.

| project | depth | findings | total ms | summary construction ms | path analysis ms | expansions | reuses (of which depth-canonical) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| school-ledger | 3 | 332 | 121.8 → 114.7 (−6 %) | 40.2 → 35.0 (−13 %) | 24.3 → 22.7 (−7 %) | 2 496 → 1 989 | 5 099 → 5 528 (446) |
| school-ledger | 4 | 544 | 146.4 → 135.9 (−7 %) | 47.6 → 41.9 (−12 %) | 41.9 → 36.2 (−14 %) | 2 617 → 1 987 | 5 165 → 5 633 (507) |
| school-ledger | 6 | 593 | 171.9 → 148.5 (−14 %) | 52.6 → 44.0 (−16 %) | 64.8 → 52.5 (−19 %) | 2 726 → 1 904 | 5 202 → 5 599 (561) |
| school-ledger | 8 | 593 | 164.4 → 140.3 (−15 %) | 53.1 → 40.9 (−23 %) | 57.9 → 50.5 (−13 %) | 2 753 → 1 777 | 5 210 → 5 412 (549) |
| school-ledger | 12 | 593 | 165.1 → 143.1 (−13 %) | 53.8 → 40.4 (−25 %) | 59.8 → 51.2 (−14 %) | 2 754 → 1 769 | 5 210 → 5 400 (544) |
| midday | 3 | 282 | 227.2 → 223.5 (−2 %) | 38.3 → 35.3 (−8 %) | 15.9 → 15.1 (−5 %) | 2 816 → 2 209 | 1 489 → 1 987 (522) |
| midday | 4 | 285 | 240.5 → 230.9 (−4 %) | 42.5 → 39.5 (−7 %) | 19.6 → 18.9 (−4 %) | 2 940 → 2 185 | 1 523 → 2 044 (589) |
| midday | 6 | 285 | 220.4 → 214.2 (−3 %) | 41.2 → 36.1 (−12 %) | 20.8 → 18.9 (−9 %) | 3 047 → 2 108 | 1 541 → 2 023 (598) |
| midday | 8 | 285 | 221.9 → 211.2 (−5 %) | 42.1 → 35.4 (−16 %) | 17.3 → 18.8 (+9 %) | 3 082 → 2 090 | 1 546 → 2 007 (591) |
| midday | 12 | 285 | 217.8 → 214.5 (−2 %) | 42.5 → 35.6 (−16 %) | 21.0 → 16.8 (−20 %) | 3 095 → 2 054 | 1 546 → 1 979 (576) |
| saas-starter | 3–12 | 23 | 4.5–5.2 (±4 %) | 0.5–0.7 | 0.5–0.6 | 52 → 37 | 43 → 57 (14) |
| unnamed-ai-project | 3–12 | 17 | 6.7–7.7 (±4 %) | 0.5–0.6 | 0.3 | 61 → 53 | 23 → 30 (7) |

Peak RSS of the whole four-project run (median): depth 3 104.6 → 105.6
MiB; depth 4 124.2 → 116.5; depth 6 149.5 → 141.4; depth 8 150.5 →
146.8; depth 12 150.7 → 140.5.

Reading:

* Expansions drop 20–36 % (depth-canonical memo). Summary construction
  drops less than that (−7 % to −25 %) because expansions were not the
  dominant cost; instantiation copies were, which the shared strings and
  cached per-summary facts reduced.
* Path analysis is 4–20 % faster on the projects where calls are deep;
  shared path summaries instantiated 860 (depth 3) to 2 409 (depth ≥ 8)
  call summaries instead of walking them. The midday depth-8 cell (+9 %)
  is within the run-to-run variation seen at depth 12 (−20 %) and
  depth 6 (−9 %).
* Lowering and resolution (≈ 60 % of midday) are unchanged; they do not
  depend on depth.
* Summary statistics as requested by the task — summaries created
  (expansions), reused (memo hits, of which depth-canonical), cache hit rate
  (reuses / (reuses + expansions): school-ledger depth 8 65 % → 75 %,
  midday depth 8 33 % → 49 %), specializations (unchanged, bounded by 8 per
  callee), inlined IR avoided (path summaries instantiated: 2 409 at depth
  8), truncation (unchanged).

## 2. Model checking

`scripts/phase8-model-bench.py`, `reports/phase8/perf/model-check.md`.

Phase 6 fixture scenarios: identical verdicts, states and transitions;
all below the reports' 1 ms resolution on both binaries.

Exhaustive exploration of the extracted team-app action with a
hypothetical `UNIQUE(teamId, email)` (identical states and transitions on
both binaries, asserted):

| actors | POR | states | transitions | Phase 7 ms | Phase 8 ms | peak RSS MiB (7 / 8) |
| --- | --- | --- | --- | --- | --- | --- |
| 3 | on | 280 | 294 | 9 | 5 | 9.6 / 9.6 |
| 4 | on | 1 984 | 2 291 | 97 | 48 (−51 %) | 17.6 / 17.1 |
| 4 | off | 1 984 | 5 344 | 124 | 65 | 17.2 / 16.9 |
| 5 | on | 13 984 | 18 042 | 936 | 427 (−54 %) | 92.3 / 91.2 |
| 5 | off | 13 984 | 46 160 | 1 200 | 604 | 91.8 / 91.0 |
| 6 | on | 97 408 | 140 975 | 8 968 | 3 756 (−58 %) | 740.9 / 744.4 |

States grow ≈ 7× per actor (unchanged: the reduction and the state space
are the same); sleep sets remove 57 % (4 actors) to 61 % (5 actors) of the
transitions and 26–29 % of the time. Memory is dominated by the visited
set and did not change.

Crash/retry dimensions on the extracted idempotent checkout: with one
client, 0/1/2 crashes and retries give 6/30/96 states (exhaustive, below
1 ms); with 2–3 clients a counterexample (two requests charged) is found
within 15–32 states.

## 3. Runtime verification cost

Process wall time (median of 3):

| scenario | model only | `--validate-postgres` | `--verify-source` (1 run) |
| --- | --- | --- | --- |
| team-app invite (node-postgres, Phase 7) | 15 ms | 1 127 ms | 2 001 ms |
| same on postgres-js | 16 ms | 1 120 ms | 2 015 ms |
| checkout crash + retry (node-postgres) | 12 ms | 1 126 ms | 2 139 ms |
| checkout crash + retry (postgres-js) | 12 ms | 1 141 ms | 2 085 ms |
| crash while waiting for a lock (probe) | 14 ms | — | 2 358 ms |

A crash adds ≈ 0.1–0.2 s to a ≈ 2 s run (the kill, the wait for
PostgreSQL to end the sessions — 15–50 ms — the table snapshot, and a new
worker process for the retry, which reloads Node.js, Drizzle and the
application). The postgres-js adapter costs the same as node-postgres. A
full 5-repetition evidence run of one scenario takes 8–12 s
(`reports/phase8/model-check/SUMMARY.md`).

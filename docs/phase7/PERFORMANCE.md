# Phase 7 performance

Same machine as the baseline (Apple Silicon, macOS 27.0.1), release
builds, machine otherwise idle. Phase 6 numbers come from the preserved
Phase 6 binary on this machine, measured in the same session. Raw data:
`reports/phase7/perf/`. These are measurements, not targets.

## 1. Static analysis by call depth: legacy vs compact path walk

`scripts/phase7-depth-bench.py` — `commitrift-corpus run --timings`
(8 threads), median of 5 runs per cell; findings identical in both modes at
every depth (asserted by the script).

| project | depth | findings | legacy total / path-analysis ms | compact total / path-analysis ms | path analysis |
| --- | --- | --- | --- | --- | --- |
| midday | 3 | 282 | 234.0 / 20.9 | 218.3 / 14.3 | −32 % |
| midday | 4 | 285 | 250.6 / 33.7 | 228.7 / 18.9 | −44 % |
| midday | 5 | 285 | 257.8 / 36.0 | 238.9 / 18.4 | −49 % |
| midday | 6 | 285 | 257.2 / 35.6 | 230.0 / 20.5 | −43 % |
| midday | 8 | 285 | 255.8 / 35.5 | 238.2 / 21.6 | −39 % |
| school-ledger | 3 | 332 | 148.9 / 51.4 | 117.5 / 23.3 | −55 % |
| school-ledger | 4 | 544 | 177.4 / 73.5 | 142.8 / 39.2 | −47 % |
| school-ledger | 5 | 594 | 202.9 / 90.7 | 162.2 / 52.2 | −42 % |
| school-ledger | 6 | 593 | 230.3 / 112.5 | 174.3 / 60.8 | −46 % |
| school-ledger | 8 | 593 | 227.5 / 109.0 | 177.5 / 61.7 | −43 % |
| saas-starter | 3–8 | 23 | 4.9–5.0 / 0.6–0.7 | 4.7–5.0 / 0.6 | −5 to −11 % |
| unnamed-ai-project | 3–8 | 17 | 7.1–7.5 / 0.3 | 7.3–7.6 / 0.3 | noise (±0.1 ms) |

Peak RSS of the whole four-project run (median):

| depth | legacy MiB | compact MiB |
| --- | --- | --- |
| 3 | 110.6 | 106.9 |
| 4 | 124.5 | 121.1 |
| 5 | 138.0 | 133.8 |
| 6 | 155.8 | 150.8 |
| 8 | 153.1 | 152.8 |

Reading:

* Compact summaries remove 32–55 % of path-analysis time where call
  chains matter. With them, school-ledger at depth 4 (142.8 ms) is cheaper
  than the legacy walk at depth 3 (148.9 ms); depth 6 costs +48 % over
  compact depth 3 instead of +55 % (legacy) over legacy depth 3.
* Summary construction (the IR-level inliner) is unchanged and is now the
  larger share at depth ≥ 4: school-ledger 40 → 59 ms from depth 3 to 6;
  lowering and resolution still dominate midday.
* Memory improves by 0.3–5 MiB; it is dominated by lowered IR, not traces.
* The depth plateau at 6–8 is the Phase 5/6 observation (the projects have
  no deeper chains that change paths).

The historical Phase 5 midday figures (`0.162 s → 0.209 s`, `66 → 96 MiB`)
were measured on an earlier toolchain; section 2 re-measures `check`
directly.

## 2. `commitrift check` on midday

| binary | depth | median wall s (7 runs, interleaved) | median peak RSS MiB |
| --- | --- | --- | --- |
| Phase 6 | 3 | 0.230 | 97.7 |
| Phase 7 | 3 | 0.230 | 91.1 |
| Phase 6 | 6 | 0.260 | 112.8 |
| Phase 7 | 6 | 0.230 | 99.2 |

`/usr/bin/time -l` (10 ms wall-time resolution), the two binaries
interleaved, idle machine. At the default depth the wall time is unchanged
and peak RSS drops by 6.6 MiB; at depth 6 the Phase 7 binary is as fast as
at depth 3 and uses 13.6 MiB less. (An earlier run of this measurement
while the test suite was compiling in the background was discarded.)

## 3. Model checker

`scripts/phase7-model-bench.py`, median of 5 runs
(`reports/phase7/perf/model-check.md`).

* Phase 6 fixture scenarios (16): identical verdicts, states and
  transitions with the Phase 7 binary; exploration remains below the
  reports' 1 ms resolution for all of them on both binaries.
* The **extracted** team-app invite action (joins, generated ids, five
  statements per actor), stopping at the first counterexample: 2 actors 19
  states / 0 ms, 3 actors 46 / 1 ms, 4 actors 145 / 6 ms.
* Exhaustive exploration (hypothetical UNIQUE(team_id, email), no
  counterexample): 2 actors 40 states (reduction 39 vs 60 transitions),
  3 actors 280 (294 vs 588 transitions, 7 vs 10 ms), 4 actors 1 984 states
  (2 291 vs 5 344 transitions, 83 vs 112 ms). States grow ~7× per actor;
  sleep sets remove 35–57 % of transitions and 26–30 % of time (3–4
  actors).
* The new semantics (sequences, joins, handlers) add no measurable cost to
  Phase 6 workloads; the thread state is 24 bytes larger (handler stack).

## 4. Runtime verification

Process wall time per `model-check` invocation, team-app scenarios:

| scenario | model only | `--validate-postgres` | `--verify-source` (1 run) | `--verify-source --source-repeat 5` |
| --- | --- | --- | --- | --- |
| invite (2 actors, 10 statements + 2 outside the model) | 14 ms | 1 081 ms | 1 914 ms | 9 395 ms |
| accept (2 actors, transactions) | 11 ms | 1 050 ms | 1 893 ms | 9 411 ms |

A source run costs ≈ 1.9 s: a fresh cluster and database (≈ 0.9 s, as for
`--validate-postgres`), two Node.js workers started under `sandbox-exec`
(loading Drizzle and the application), and the coordinated statements. A
released statement that completes is acknowledged at once; only a
statement still running after 150 ms is checked against
`pg_stat_activity` for a lock wait. Repetitions are linear; each starts
from a fresh database.

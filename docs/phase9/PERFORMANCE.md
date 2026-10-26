# Phase 9 performance

Apple Silicon (arm64), macOS (Darwin 27.0.0), release builds, one session,
no other heavy job running. "Phase 8" is the saved release binary of
`4dc01af` (`reports/phase9/baseline/commitrift-phase8`); "Phase 9" is the
final build (sha256 `c2f4d6d4…`, source digest `df9f2a6d…`). Raw data:
`reports/phase9/final/model-bench.md` (`scripts/phase9-model-bench.py`) and
`reports/phase9/final/profile-bench.md` (`scripts/phase9-profile-bench.py`).
These are measurements, not targets. Model checking and original-code
verification are reported separately (§5): PostgreSQL and Node start-up
dominate the latter.

## 1. Identity first

Before any timing: the 40 model scenarios of Phases 6–8 give the same
verdicts, states, transitions, schedules and texts with both binaries
(`reports/phase9/final/model-identity.txt`, 40/40); the benchmark asserts
equal verdicts and counts per scenario as well. Lower memory below is
**storage efficiency for the same states**, not fewer explored states.

## 2. Actor scaling (model only)

Workload: the extracted team-app `inviteTeamMember` action with a
hypothetical `UNIQUE(teamId, email)`, N copies of the actor, sleep sets on
(default). Median of 3 runs for 2–6 actors, 1 run for 7 and 8. Wall time is
the exploration time the report states; peak RSS from `/usr/bin/time -l`.

| actors | states | transitions | Phase 8 wall ms | Phase 9 wall ms | Phase 8 peak RSS | Phase 9 peak RSS | Phase 8 bytes/state | Phase 9 bytes/state |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 3 | 280 | 294 | 4 | 4 | 9.5 MiB | 8.7 MiB | 2 639 | 181 |
| 4 | 1 984 | 2 291 | 40 | 40 | 17.0 MiB | 9.4 MiB | 3 362 | 137 |
| 5 | 13 984 | 18 042 | 360 | 342 | 90.9 MiB | 12.1 MiB | 4 066 | 120 |
| 6 | 97 408 | 140 975 | 3 222 | 2 997 | 742.9 MiB | 26.7 MiB | 4 758 | 144 |
| 7 | 669 568 | 1 084 010 | 28 979 | 25 240 | 2 934.5 MiB | 118.5 MiB | 5 442 | 169 |
| 8 | 4 544 512 | 8 183 475 | not run | 207 387 | not run | 532.4 MiB | — | 194 |

* Peak RSS: 1.8× lower at 4 actors, 7.5× at 5, 28× at 6, 25× at 7. The
  ratio stops growing at 7 because Phase 9's own per-state cost (table
  slots, node records, edges) dominates once the keys are small.
* "bytes/state" uses each binary's own `visitedBytes` definition (Phase 8:
  key + 96 bytes; Phase 9: key arena, table, interner, edges and nodes), so
  the comparable number is peak RSS.
* Phase 8 was not run at 8 actors: its keys alone would need ≈ 23.6 GiB
  (5 442 bytes × 4.5 M states; extrapolated, not measured).
* Wall time: equal at 3–4 actors, 5–13 % lower at 5–7. This is a single
  machine and (at 7) a single run; the difference is small next to the
  memory change and is not claimed beyond this workload.

Re-run of the same benchmark before the finalization fix (earlier the same
day, `reports/phase9/model-bench.md`): identical states, transitions and
bytes per state; wall times 0–24 % higher than in the final run at every
size (the cause was not investigated; the comparison above uses one session).

## 3. Where the time goes (profile)

`sample` (1 ms) on the exploring thread for 20 s, 7 actors. Inclusive
sample shares of the named functions; profiler estimates, not
instrumentation.

| | Phase 8 | Phase 9 |
| --- | ---: | ---: |
| canonical encoding (`State::canonical` / `State::encode`) | 17.8 % | 17.8 % |
| fragment interning (`Interner::id`) | — | 6.3 % |
| whole canonical key | 17.8 % | 24.5 % |
| visited-table lookup / insert (std `HashMap` of flat keys / `KeyTable::intern`) | 8.7 % | 1.5 % |
| transition execution (`engine::*`) | 54.8 % | 60.1 % |

The trade: Phase 9 spends more CPU building a key (interning the fragments
adds 6.3 points) and much less comparing and hashing it (43-byte keys
instead of ~5 KB). Net, canonicalization plus lookup is 26.5 % of samples
in Phase 8 and 26.0 % in Phase 9. The profile therefore does not explain the
lower wall time; less memory traffic is a plausible cause that was not
measured. The Phase 8
lookup bucket counts hash-table frames called from the explorer outside the
engine and the encoder, so it may include a small amount of non-visited
hash-map work.

## 4. Counterexample discovery

Same workload without the unique constraint (a duplicate invitation is
reachable), `--no-minimize`, median of 3:

| actors | states stored at the first violation | Phase 8 ms | Phase 9 ms | Phase 8 peak RSS | Phase 9 peak RSS |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 4 | 145 | 3 | 2 | 9.4 MiB | 9.1 MiB |
| 5 | 532 | 13 | 12 | 11.9 MiB | 9.5 MiB |
| 6 | 2 071 | 63 | 57 | 24.1 MiB | 10.2 MiB |

The search order is unchanged, so the first counterexample is found after
the same number of states by both binaries.

## 5. Reduction modes: full search, sleep sets, DPOR (Phase 9 build)

| workload | full: transitions / ms / RSS | sleep sets: transitions / ms / RSS | DPOR: transitions / ms / RSS |
| --- | --- | --- | --- |
| team-app, 3 actors | 588 / 5 / 8.7 MiB | 294 / 4 / 8.6 MiB | 6 625 / 55 / 8.5 MiB |
| team-app, 4 actors | 5 344 / 50 / 9.5 MiB | 2 291 / 35 / 9.4 MiB | 3 476 614 / 30 362 / 8.9 MiB |
| independent actors, 4 | 768 / 3 / 5.0 MiB | 255 / 2 / 5.1 MiB | 12 / 0 / 4.8 MiB |
| 2 clients, crash + retry + in-flight COMMIT | 1 552 / 4 / 5.2 MiB | 1 336 / 3 / 5.2 MiB | 21 897 / 62 / 5.1 MiB |
| 2 tasks of one process, pool of 1 | 6 / 0 / 4.5 MiB | 6 / 0 / 4.5 MiB | 6 / 0 / 4.4 MiB |

DPOR is a **memory-for-time trade**, and on these workloads the memory side
barely pays: with the compact table the stateful searches already need
under 10 MiB, so DPOR saves 0.1–0.5 MiB while executing up to ~1 500× more
transitions (team-app, 4 actors: 30.4 s against 35 ms). It wins only where
actors are fully independent (12 transitions against 255). The DPOR
differential suite (`DYNAMIC-POR.md`) shows the same pattern: on 682 random
scenarios DPOR executed 17.6 M transitions against 137 k for sleep sets.
DPOR therefore stays opt-in; sleep sets stay the default.

## 6. In-flight faults: cost of the extra dimension (model only)

`--max-in-flight 1` on the Phase 8 delivery scenarios: states grow 1.0–4.3×
(e.g. `job-claimed-effect-redelivery` 21 → 90, `shipping-outbox-no-loss`
29 → 72); verdicts unchanged (`reports/phase9/final/model-bench.md` §4).

## 7. Original-code verification cost (separate from model checking)

Process wall time, median of 3, one source run:

| scenario | model only | + `--validate-postgres` | + `--verify-source` (1 run) |
| --- | ---: | ---: | ---: |
| checkout-crash-after-charge (Phase 8) | 9 ms | 947 ms | 1 787 ms |
| ledger-blind-commit-in-flight | 10 ms | 902 ms | 1 683 ms |
| ledger-reconciled-commit-in-flight | 10 ms | 966 ms | 1 877 ms |
| ledger-shared-process-crash | 11 ms | 948 ms | 1 410 ms |
| ledger-pool-starvation | 9 ms | 922 ms | 2 173 ms |

The model is ~1 % of a verification run; `initdb` + server start dominate
`--validate-postgres`, and Node start-up, the proxy and the deliberate quiet
period (≈ 800 ms for starvation confirmation) add the rest. A full evidence
run (10 scenarios × 5 runs) took about 90 s.

## 8. Regressions and what was not measured

* The Phase 9 key costs more CPU to build (§3). On this workload it is
  offset by cheaper lookups; a model with many distinct fragments (≥128
  needs 2-byte references) was not benchmarked.
* Sleep-set exploration of pooled workloads executes slightly more
  transitions after the finalization fix (DYNAMIC-POR.md refinement 5;
  e.g. 114 → 130 on a 3-task pool-of-2 workload with a crash); states are
  unchanged. Phase 6–8 scenarios have no pools and are unaffected (40/40
  identical).
* Static analysis was not re-measured: its code is unchanged and its output
  is byte-identical (`CORPUS-EVALUATION.md`).
* Linux timings are not compared: the container is a VM with a different
  CPU budget.

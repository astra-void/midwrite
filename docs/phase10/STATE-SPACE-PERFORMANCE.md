# State-space performance (Phase 10)

Release builds, macOS arm64, one machine, no other heavy job during the
benchmark. Raw output: `reports/phase10/final/por-bench.md`
(`scripts/phase10-por-bench.py`). Wall time is the exploration time the report
states; peak RSS is the whole process (`/usr/bin/time`). Runs under 10 s are
the median of three. A run that hits the 120 s wall limit is shown as
truncated, never as a result.

## 1. The primary regression workload (team-app, 4 actors)

| search | states | transitions | wall ms | peak RSS MiB |
| --- | ---: | ---: | ---: | ---: |
| unreduced | 1 984 | 5 344 | 64 | 9.3 |
| sleep sets (default) | 1 984 | 2 291 | 44 | 9.4 |
| classic DPOR (Phase 9 baseline binary) | 3 476 615 nodes | 3 476 614 | 30 362 (Phase 9 report) | 8.9 |
| classic DPOR (Phase 10, every race) | 3 476 615 nodes | 3 476 614 | 37 973 | 8.7 |
| source-DPOR stateful | 930 | 1 665 | 35 | 9.0 |
| source-DPOR stateless+sleep | 697 nodes | 696 | 10 | 9.0 |
| **source-DPOR stateful+sleep** | **454** | **574** | **12** | 9.2 |

Against the classic search the stateful+sleep configuration executes
6 057× fewer transitions (3 476 614 → 574) and is 3 160× faster (37 973 →
12 ms). Against sleep sets it executes 4.0× fewer transitions and is 3.7×
faster. The terminal states are the same in every row (32).

## 2. Workload matrix

Transitions executed (wall ms). "TRUNCATED" hit the 120 s limit and is not a
result. Full tables, states, RSS and reduction counters:
`reports/phase10/final/por-bench.md`.

| workload | unreduced | sleep sets | classic DPOR | sdpor stateful | sdpor stateless+sleep | sdpor stateful+sleep |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 independent actors (5) | 12 500 (58) | 3 124 (31) | 20 (0) | 20 (0) | 20 (0) | 20 (0) |
| 2 mostly independent (4, one shared counter) | 4 320 (20) | 1 416 (13) | 14 394 505 (94 646) | 1 039 (14) | 160 (1) | **106 (1)** |
| 3 conflicting, lost update (first counterexample) | 22 (0) | 21 (0) | 25 (0) | 22 (0) | 21 (0) | 21 (0) |
| 3b conflicting, safe (4 atomic increments) | 6 232 (31) | **4 638 (26)** | 17 667 614 (81 117) | 5 220 (61) | 535 327 (2 790) | 4 843 (55) |
| 4 pool contention (4 tasks, pool of 2) | 226 (0) | **204 (1)** | 3 944 (41) | 222 (3) | 1 216 (13) | 210 (3) |
| 5 write skew (3 doctors, SERIALIZABLE) | 522 (3) | 423 (3) | 25 586 (157) | 388 (4) | 3 707 (24) | **357 (4)** |
| 6 in-flight COMMIT + reconciliation (first counterexample) | 10 (0) | 10 (0) | 10 (0) | 10 (0) | 10 (0) | 10 (0) |
| 6b lookup + unique key, concurrent delivery | 612 (2) | 551 (2) | 17 008 (62) | 520 (3) | 3 214 (13) | **505 (4)** |
| 7 crash/retry (4 actors) | 32 600 (142) | 24 024 (120) | TRUNCATED | 11 960 (126) | 405 624 (2 693) | **10 584 (107)** |
| 8 tasks of one process (3 + 1 actor) | 36 (0) | 28 (0) | 51 (0) | 29 (0) | 37 (0) | **25 (0)** |
| 9 queue redelivery (first counterexample) | 10 (0) | 10 (0) | 10 (0) | 10 (0) | 10 (0) | 10 (0) |
| 10a team-app, 4 actors | 5 344 (64) | 2 291 (44) | 3 476 614 (37 973) | 1 665 (35) | 696 (10) | **574 (12)** |
| 10b team-app, 5 actors | 46 160 (599) | 18 042 (398) | TRUNCATED | 13 076 (322) | 6 039 (92) | **3 849 (93)** |

Time to the first counterexample (workloads 3, 6, 9) is 0 ms in every mode
at this size: those counterexamples are found on the first path.

**Where the new search loses.**

* *Highly conflicting and safe* (3b): stateful+sleep executes 4 % more
  transitions than sleep sets and takes 2.1× the wall time (55 vs 26 ms): the
  summaries cost race checks (21 915 summary checks) and save nothing when
  every pair conflicts.
* *Pool contention* (4): 3 % more transitions, 3 ms vs 1 ms.
* *Without the state cache* (stateless+sleep) the search explores one run per
  trace: on 3b, 7 and team-app ≥ 6 it executes 10–100× more transitions than
  sleep sets. It uses the least memory.
* The classic DPOR (with the Phase 10 every-race fix) is never competitive on
  a dependent workload; at 5 team-app actors and on crash/retry it does not
  finish in 120 s.

## 3. Actor scaling and memory (team-app)

| actors | search | states | transitions | wall ms | peak RSS MiB | state storage bytes | summaries bytes (estimate) |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 5 | sleep sets | 13 984 | 18 042 | 411 | 11.8 | 1 680 001 | — |
| 5 | sdpor stateful+sleep | 2 562 | 3 849 | 92 | 10.4 | 377 473 | 127 312 |
| 5 | sdpor stateless+sleep | 6 040 nodes | 6 039 | 91 | 9.5 | 0 | 0 |
| 6 | sleep sets | 97 408 | 140 975 | 3 613 | 28.3 | 14 034 161 | — |
| 6 | sdpor stateful+sleep | 15 358 | 27 345 | 755 | 13.4 | 1 762 545 | 516 344 |
| 6 | sdpor stateless+sleep | 64 513 nodes | 64 512 | 1 021 | 10.1 | 0 | 0 |
| 7 | sleep sets | 669 568 | 1 084 010 | 30 811 | 117.9 | 113 256 237 | — |
| 7 | sdpor stateful+sleep | 91 407 | 190 830 | 6 046 | 32.7 | 14 165 805 | 2 177 648 |
| 7 | sdpor stateless+sleep | 807 496 nodes | 807 495 | 13 482 | 10.6 | 0 | 0 |

(3 and 4 actors: `por-bench.md`.) At 7 actors stateful+sleep stores 7.3× fewer
states, executes 5.7× fewer transitions, is 5.1× faster and peaks at 3.6×
less memory than sleep sets. Its reduction metadata (summaries and
descriptors, estimated) is 15 % of its state storage. The sleep-set numbers at
5–7 actors equal Phase 9's (`docs/phase9/PERFORMANCE.md` §2: 13 984 / 97 408 /
669 568 states, 18 042 / 140 975 / 1 084 010 transitions; 12.1 / 26.7 /
118.5 MiB then, 11.8 / 28.3 / 117.9 MiB now): the default search and its
storage did not change.


## Exact state identity (Milestone 10)

Unchanged from Phase 9 and still tested: the canonical encoding, interned
fragments compared byte for byte, the varint key table compared exactly,
forced 64-bit hash collisions (`keytable.rs` tests,
`colliding_hashes_do_not_merge_states`, `slot_collisions_do_not_merge_states`),
and the compact table against the flat encoding (`phase9_storage`).

The new search adds metadata **beside** the key, never in it: per node the
interned summary id, the step count it was explored at and the sleep set it
was explored under; a pool of interned summaries; a table of
`(transition, footprint)` descriptors. Two prefixes reaching the same state
reach the same node, with different obligations decided by the reuse
conditions (`SOURCE-DPOR.md`). `reductionMetrics.summaryBytes` is an estimate
of that metadata; peak RSS is the measured total.

## Decision

Sleep sets stay the default. Against the criteria of the brief:

| criterion | status |
| --- | --- |
| broad differential correctness evidence | yes: targeted, Phase 9, 149 random, 3 000 adversarial seeds, metamorphic tests, regression fixtures, all agreeing (`POR-CORRECTNESS.md`) |
| no known missed counterexample | none known after the fixes |
| competitive on dependent workloads | better on 8 of 13 rows, equal on 3 (first counterexample found on the first path), behind on 2 (3b, 4) |
| predictable memory | yes: on small workloads within +0.6 MiB of sleep sets (3b: 6.2 vs 5.6 MiB); on team-app 5–7 lower (10.4 / 13.4 / 32.7 vs 11.8 / 28.3 / 117.9 MiB) |
| documented property preservation | yes; but `always` invariants and context-switch bounds are **refused**, which sleep sets support |
| explicit support limits | yes |

What tips it: the evidence base is one phase old and found three defects on
its way, all in DPOR variants (each hit the new algorithm during development;
one was also present in the Phase 9 classic search); and a default must
serve every scenario, while source-DPOR has to fall back to sleep sets for
`always` invariants. `--source-dpor` (default configuration
`stateful-sleep`) is recommended for large final-invariant models; the report
says which search ran (`bounds.reduction`, schema 5).


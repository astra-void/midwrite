# Phase 16 performance

(Track H's H6.) macOS arm64 (10 cores, 16 GB), release build, rustc
1.98.1. Medians of 3 runs (wall clock), peak RSS the maximum. Raw data:
* `reports/phase16/performance/` (`scripts/phase16-perf.py`);
* `reports/phase16/performance/midday-cold-warm.txt`;
* `reports/phase16/corpus/` (`scripts/phase16-corpus.py`).

The Phase 15 numbers are the Phase 15 release binary re-measured on the
same machine and inputs.

## Real projects

| project | Phase 15 binary | Phase 16 cold | Phase 16 warm (`--cache`) |
| --- | --- | --- | --- |
| midday, declared factory (1 168 entries) | 1.23 s, 149 MiB | 1.74–1.77 s, 230 MiB | 1.33 s, 272 MiB |
| school-ledger (546 entries) | 0.72 s, 121 MiB | 0.81 s, 131 MiB | — |
| saas-starter (37 entries) | 0.03 s, 13 MiB | 0.03 s, 13 MiB | — |
| unnamed-ai-project (33 entries) | 0.03 s, 14 MiB | 0.03 s, 15 MiB | — |

The brief's Phase 15 reference was ≈ 1.32 s / 148 MiB (midday) and ≈ 0.76 s /
121 MiB (school-ledger). Today's re-measurement of the same binary agrees
within run-to-run noise.

**Where midday's time goes** (Phase 16, from the report's own timings,
cold): index 150–290 ms, **extraction 1 420 ms**, detection 32 ms,
synthesis 20 ms, model checking 1 ms. Extraction cost grew because more
code is now reached (loop bodies, catch handlers, request bodies, held
predicates). The workload is larger:

| midday (declared) | Phase 15 | Phase 16 |
| --- | ---: | ---: |
| effectful entries | 81 | 86 |
| read-only entries | 63 | 84 |
| replica `SELECT`s extracted | 99 | 225 |
| transactions extracted | 19 | 67 |
| unknown operations recorded | 18 861 | 35 302 |

Normalized cost: about 10.4 ms per effectful-or-read-only entry in Phase 16
(1.77 s / 170 entries) vs 8.5 ms in Phase 15 (1.23 s / 144 entries). Most
of the rise is the larger extraction. No newly
supported operation is skipped to save time.

**Warm runs.** The extraction cache (`--cache FILE`, keyed by file
contents) cuts midday's extraction from 1.42 s to 1.0 s. Peak memory rises
to about 272 MiB because the 31 MiB cache file is loaded. On the small
first-party applications, warm and cold runs are within noise: they are
dominated by indexing and process start.

## First-party and evaluation applications

| application | binary | candidates | model-checkable | counterexamples | worlds built | scenarios checked | states | total ms | wall s | peak RSS MiB |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Phase 13 set | Phase 15 | 20 | 18 | 10 | 32 | 32 | 1 056 | 58 | 0.066 | 12.2 |
| Phase 13 set | Phase 16 | 20 | 18 | 10 | 32 | 32 | 1 056 | 50 | 0.059 | 12.7 |
| Phase 14 set | Phase 15 | 20 | 18 | 8 | 30 | 30 | 558 | 45 | 0.053 | 12.3 |
| Phase 14 set | Phase 16 | 24 | 22 | 10 | 37 | 37 | 621 | 56 | 0.065 | 13.4 |
| Phase 15 set | Phase 15 | 19 | 13 | 8 | 28 | 28 | 803 | 53 | 0.063 | 12.1 |
| Phase 15 set | Phase 16 | 23 | 19 | 10 | 39 | 39 | 954 | 67 | 0.076 | 13.1 |
| Phase 15 worlds-app | Phase 15 | 3 | 3 | 2 | 8 | 8 | 105 | 9 | 0.016 | 9.8 |
| Phase 15 worlds-app | Phase 16 | 3 | 3 | 2 | 9 | 9 | 122 | 9 | 0.016 | 10.1 |
| Phase 11 commerce-app | Phase 15 | 22 | 20 | 12 | 32 | 32 | 836 | 52 | 0.062 | 12.1 |
| Phase 11 commerce-app | Phase 16 | 23 | 21 | 13 | 34 | 34 | 889 | 57 | 0.066 | 12.5 |
| **Phase 16 set** | Phase 15 | 4 | 2 | 1 | 3 | 3 | 59 | 16 | 0.024 | 12.6 |
| **Phase 16 set** | Phase 16 | 26 | 12 | 6 | 24 | 24 | 286 | 92 | 0.101 | 17.2 |
| phase16/retry | Phase 16 | 6 | 6 | 2 | 11 | 11 | 298 | 14 | 0.022 | 11.0 |
| phase16/locks | Phase 16 | 6 | 6 | 3 | 12 | 12 | 295 | 15 | 0.022 | 10.4 |
| phase16/nulls | Phase 16 | 4 | 4 | 2 | 10 | 10 | 106 | 8 | 0.014 | 9.9 |
| phase16/genid | Phase 16 | 14 | 12 | 6 | 16 | 16 | 202 | 13 | 0.021 | 10.8 |
| phase16/zod/app | Phase 16 | 2 | 2 | 1 | 7 | 7 | 75 | 10 | 0.017 | 10.3 |

The Phase 15 binary finds 0 candidates on `retry` and `zod/app`, 3
unmodelable ones on `locks`, and cannot load `genid`'s contracts (they use
Phase 16 formulas). Those rows are in the raw summary.

* **Worlds per candidate:**
  * Phase 16 set: 24 worlds for 12 model-checkable candidates (2.0);
  * Phase 15 set: 39 for 19 (2.1);
  * Phase 13 set: 32 for 18 (1.8).
* **State space:** worlds stay small (4–40 states each). The largest growth
  is on the Phase 15 set (+19 % states), from candidates that were
  unsupported before.
* **Time to first counterexample:** bounded by the world order. Over the
  ten applications, 53 candidates have a counterexample: 38 in the base
  world, 13 in the first derived world, 2 in the second. Model checking
  itself takes at most 11 ms per application.
* **POR efficiency:** see
  [MODEL-CORRECTNESS.md](MODEL-CORRECTNESS.md#1-partial-order-reduction-on-the-new-semantics)
  (source-set DPOR 0.88 of the unreduced states; sleep sets equal; classic
  DPOR revisits).

## Bounds that keep this tractable

* Loops unroll at most to their established bound, or 8 iterations when
  the bound is unknown (then TRUNCATED).
* Multi-row reads and locks are limited by the synthesized world's row
  count.
* Worlds change one relationship at a time, at most `--max-worlds` per
  variant.
* `--max-total-states` caps all model checking.

Where completeness would need more, the coverage section lists the gap
instead of paying for it.

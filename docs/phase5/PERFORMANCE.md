# Performance and memory

Same Apple Silicon Mac, same session, Phase 4 binary
(`.corpora/bin/commitrift-phase4`) vs the final Phase 5 release binary.
Raw data: `reports/phase5/perf/`. `scripts/bench.py`: 7 timed runs after a
warm-up, median wall time, peak RSS of the child process.

## Wall time and peak RSS

8 threads (default), round 2 (round 1 within 1 %):

| Project | Wall P4 → P5 | Δ | Peak RSS P4 → P5 | Δ |
| --- | --- | --- | --- | --- |
| saas-starter | 0.0086 → 0.0097 s | +13 % | 7.1 → 8.0 MiB | +13 % |
| midday | 0.1616 → 0.2085 s | **+29 %** | 66.0 → 96.0 MiB | **+45 %** |
| school-ledger | 0.1183 → 0.1526 s | **+29 %** | 72.8 → 89.1 MiB | +22 % |
| unnamed-ai-project | 0.0106 → 0.0120 s | +13 % | 8.3 → 9.7 MiB | +17 % |

1 thread (`COMMITRIFT_THREADS=1`):

| Project | Wall P4 → P5 | Peak RSS P4 → P5 |
| --- | --- | --- |
| saas-starter | 0.0101 → 0.0113 s | 6.4 → 7.2 MiB |
| midday | 0.3718 → 0.4794 s (+29 %) | 54.9 → 77.8 MiB (+42 %) |
| school-ledger | 0.2486 → 0.3254 s (+31 %) | 60.3 → 77.4 MiB (+28 %) |
| unnamed-ai-project | 0.0155 → 0.0190 s | 7.4 → 8.4 MiB |

Reports are byte-identical with 1 and 8 threads.

## Phases (1 thread, `commitrift-corpus run --timings`, 3 alternating runs)

| ms | midday P4 | midday P5 | school-ledger P4 | school-ledger P5 |
| --- | --- | --- | --- | --- |
| lower (parse, semantics, lowering, **value analysis**) | 258 | 318 | 88 | 117 |
| resolve (linking, classes, **fixpoint**) | 30 | 41 (solve 2.5) | 6 | 9 (solve 1.2) |
| summaries (inlining) | 15 | 33 | 21 | 36 |
| analysis (paths, detection, rendering) | 42 | 59 | 110 | 136 |
| total | 362 | 468 | 236 | 309 |

Summary expansions 774 → 2 816 (midday) and 1 001 → 2 496 (school-ledger);
calls summarized roughly doubled (1 150 → 2 382; 1 310 → 2 554), which is
also why analysis time grew: more inlined IR (retained IR nodes 27 278 →
33 254 midday).

## Live heap by phase (1 thread, counting allocator)

| MiB | lowered | resolved | summarized | analyzed | peak |
| --- | --- | --- | --- | --- | --- |
| midday P4 | 11.8 | 14.2 | 17.6 | 20.7 | 27.4 |
| midday P5 | 19.4 | 19.8 | 23.3 | 26.6 | 39.6 |
| school-ledger P4 | 6.9 | 8.7 | 20.1 | 22.8 | 26.9 |
| school-ledger P5 | 11.1 | 11.0 | 25.6 | 28.6 | 36.4 |

With traces: midday peak 41.8 MiB, school-ledger 36.6 MiB.
`size_of::<Node>()` 176 → 184 bytes (the optional boxed provenance).

## Investigation and optimizations

The first complete Phase 5 build was 1.82× slower single-threaded on midday
(lowering 485 ms) and used 51.9 MiB peak heap. Profiling (`sample` on a
debug-info release build) and targeted measurements led to:

| Change | Effect (midday, 1 thread) |
| --- | --- |
| `compact()` labels in one pass, labels only when needed, callee-only labels for summary facts | lowering −8 % |
| copy-on-write value state (`Rc` environment and heap; free joins of unchanged states) | lowering −11 % |
| lookalike receiver paths computed once per file (they were rebuilt for every classified call — a Phase 4 inefficiency amplified by loop passes) | lowering −9 % |
| module-resolution cache for summary facts | resolve 61 → 50 ms |
| inline `AbsValue` representation (no allocation for ⊥, ⊤, one atom) | lowering −6 %, summaries −10 % |
| receiver evaluated once per method call (as in JavaScript) | small |
| optimistic first loop pass + loop-head order ignoring body-local bindings (dry runs 1 650 → 563) | lowering −5 % |
| interned summary facts, refuted bodies keep only parameter-forwarding calls, shrink in-place-collected vectors, release lowering data after linking | peak heap 51.9 → 39.6 MiB |
| callback specializations: fall back to the context-free expansion past the limit | precision fix (was a regression), neutral |

None of these weakens the analysis.

## Remaining regression

+29 % wall time and +22–45 % peak RSS on the large projects. Where it goes:
value analysis during lowering (~+60 ms midday: environment/heap
maintenance, provenance, call facts), twice as many summarized calls
(+18 ms inlining, +17 ms paths), fact linking (+11 ms). The memory growth is
the larger IR (more inlined calls, provenance) and, transiently, the
per-module facts and call sites until linking. Not done (each would trade
precision or needs a larger design change): dropping reads from inert
callees (changes trace ids), skipping provenance for effects that can never
be in a sequence, a persistent map for environments.

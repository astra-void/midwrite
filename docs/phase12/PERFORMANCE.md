# Phase 12 performance

Same project revisions, same configuration (no `commitrift.json`, no
contracts), discovery with bounded model checking, macOS arm64 (10 cores),
release builds; median of 3 runs, peak RSS the maximum
(`scripts/phase12-corpus.py`, raw: `reports/phase12/corpus/`). The Phase 11
column is the Phase 11 final binary (sha256 `0643249f…`) run now on the same
machine.

## Totals and cost per extracted entry

| project | Phase 11 wall | Phase 12 wall | ms / extracted entry (11 → 12) | peak RSS (11 → 12) |
| --- | ---: | ---: | --- | --- |
| midday (2 022 modules, 1 168 extracted) | 28.07 s | **1.13 s** | 24.03 → **0.97** | 73 → 112 MiB |
| school-ledger (461 modules, 546 extracted) | 3.36 s | **0.53 s** | 6.15 → **0.97** | 50 → 81 MiB |
| saas-starter (41 modules, 37 extracted) | 0.06 s | 0.03 s | 1.62 → 0.81 | 11 → 11 MiB |
| unnamed-ai-project (104 modules, 33 extracted) | 0.07 s | 0.03 s | 2.12 → 0.91 | 11 → 12 MiB |

Phase 12 extracts the same entries and does **more** work per entry
(re-exports followed, methods inlined, factories resolved; midday now models
50 tables instead of 8, school-ledger has 21 effectful entries instead of 0
and model-checks/synthesizes 9 candidates). Nothing was skipped to gain time:
`entriesExtracted` is identical, and the static `check` outputs are
byte-identical (144 / 144).

## Where the time went (discovery's own stage timings, midday)

| stage | Phase 11 | Phase 12 |
| --- | ---: | ---: |
| index (module discovery, entry candidates, schema extraction, table probes) | 348 ms | 311 ms |
| extraction (1 168 entries) | **27 323 ms** | **873 ms** |
| effect summaries + detection | 1 ms | 2 ms |
| synthesis / model checking | 0 / 0 ms (no candidates) | 0 / 0 ms |
| total (report generation included) | 27 675 ms | 1 193 ms |

School-ledger: extraction 3 241 → 390 ms, index 89 → 59 ms, synthesis 0 →
28 ms (9 candidates now built). The extraction stage was dominated by
project-level work repeated per entry — a full module-discovery walk and a
new resolver (workspace `package.json` scan, `tsconfig` reads) for each of
the 2 864 entry candidates. The shared `ProjectContext` does that once;
per-entry work is now parsing and lowering the modules the entry actually
inlines. ASTs are still parsed per use (sources are cached, parse trees are
not); at 0.97 ms per extracted entry the remaining cost did not justify
self-referential arena caching.

## Extraction cache (cold, then warm), midday

| | cold | warm | warm hits | same candidates |
| --- | ---: | ---: | ---: | --- |
| Phase 11 | 34.39 s / 100 MiB | 7.21 s / 92 MiB | 1 168 | yes |
| Phase 12 | **1.54 s** / 161 MiB | **1.06 s** / 124 MiB | 1 168 | yes |

A first Phase 12 measurement showed the cache *costing* time (cold 7.54 s,
warm 6.96 s against 1.10 s without a cache): every entry re-read and
re-hashed all files of its import closure. Keys now use a per-run memo of
file digests and closures (`pipeline::KeyMemo`); the warm run is then
cheaper than extraction itself. With declaration-identity binding, a schema
change invalidates only the entries whose import closure reaches the changed
module (`ARCHITECTURE.md` §4; tested).

## Memory

Peak RSS rose on the large projects (midday +39 MiB, school-ledger +31 MiB):
the shared context keeps every module's source text and summary for the
whole run, and Phase 12 builds more extracted operations and
(school-ledger) candidate models. Per-structure sizes (index, summaries,
cache, schema graph) are **not** instrumented; only process peak RSS was
measured. The cache runs' higher RSS includes the cache file held in memory
for serialization.

## Model checking and runtime

Model checking is unchanged for earlier scenarios (identical state and
transition counts on all 56 Phase 6–9 scenarios; `visitedBytes` grows by the
4-byte generation counter per actor state). The workspace application's
discovery with model checking of 17 candidates (both variants) takes well
under a second; each original-code run takes 1.5–2 s (disposable cluster,
one Node process per actor), and runs only when explicitly authorized.

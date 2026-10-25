# Performance and memory (Phase 4)

Phase 4 milestone 8. Same machine as the baseline (Apple Silicon, 10 cores,
16 GiB, macOS 27.0.1), release builds, same session. The Phase 3 binary was
rebuilt from the verified Phase 3 digest (no Phase 3 binary had been kept).
Raw data: `reports/phase4/perf/`. Static analysis and runtime replay are
measured separately and must not be compared with each other.

## Static analysis (`commitrift check`)

`scripts/bench.py`: one untimed warm-up, then 7 runs, fresh process each,
median wall time and median peak RSS (getrusage of the child).

| Project | Wall, 8 threads, Phase 3 → 4 | Peak RSS, 8 threads | Wall, 1 thread | Peak RSS, 1 thread |
| --- | --- | --- | --- | --- |
| midday (2022 files, 14 827 functions) | 0.1654 / 0.1654 → 0.1641 / 0.1633 s | 65.5–68.6 → 66.2–67.6 MiB | 0.371–0.373 → 0.373–0.374 s (alternating, 3 rounds each) | 53.5–54.6 → 54.1–54.7 MiB |
| school-ledger (462, 6 391) | 0.1210 / 0.1222 → 0.1212 / 0.1218 s | 73.6–74.2 → 73.1–73.5 MiB | 0.251–0.254 → 0.249–0.252 s | 61.0–61.5 → 60.0–61.2 MiB |
| saas-starter (41, 156) | 0.0090 → 0.0088–0.0092 s | 7.1 → 7.1 MiB | 0.0101 → 0.0105–0.0107 s | 6.3 → 6.4 MiB |
| unnamed-ai-project (104, 648) | 0.0107–0.0113 → 0.0104–0.0105 s | 8.4 → 8.3 MiB | 0.0157–0.0161 → 0.0157–0.0161 s | 7.2–7.3 → 7.3–7.4 MiB |

Within run-to-run noise everywhere (8-thread peak RSS varies by ±1.5 MiB
between rounds of the *same* binary). The JSON output of `check` is
**byte-identical** between the two binaries on all four projects, and so are
the corpus runner's reports (`REAL-WORLD-EVALUATION.md`).

**A regression found and fixed.** The first Phase 4 build was
reproducibly ~3.5 % slower single-threaded on school-ledger (0.261 vs
0.252 s, four rounds; `perf/before-unzip-fix/`). The cause was the recovery
contract hook in `project.rs`, which unzipped each file's
(diagnostic, trace) pairs, applied contracts, and re-zipped them — for every
file, even without contracts. The hook now runs only when contracts are
configured and works in place; after the change the difference disappeared
(0.252 / 0.250 s vs 0.251 / 0.254 s). An apparent +4 % on midday in one
later round did not reproduce in three alternating rounds (0.373 vs 0.373 s).

`commitrift check` pays for none of the replay infrastructure: the replay
crate, the generator and the Node runtime are not on the static path, the
contract step is skipped without contracts, and no new data is retained.

## Memory by phase (counting allocator, 1 thread)

`commitrift-corpus memory`: identical to Phase 3 for every project and phase
(midday: lowered 11.8, resolved 14.2, summary 17.6, analyzed 20.7, peak
27.4 MiB; 27 278 IR nodes; `size_of::<Node>()` still 176 bytes). With traces
(`--traces`): midday peak 31.5 MiB, school-ledger 29.8 MiB
(`memory-phases-phase4-traces.txt`; trace overhead ≈ +4 MiB, as in Phase 3).
The new `Config::recovery_contracts` (one empty `Vec`) and the larger
`RecoveryNote` (only on diagnostics that have one) do not change these
numbers. No duplicate Effect IR, trace structures or logs are retained by
Phase 4 code; verification logs remain capped at 16 KiB per stream.

## Trace generation overhead

Unchanged from Phase 3: `commitrift trace` costs ~5–10 % more wall time than
`check` and produces ~2× the output (see `../phase3/PERFORMANCE.md`; no
change to trace generation in Phase 4).

## Runtime replay

Includes `initdb`, cluster start, per-run database creation, runtime
start-up, observations and cleanup; median of 3 invocations
(`replay-cost*.txt`).

| Harness | Runs | Before observation batching | After |
| --- | --- | --- | --- |
| `order-vulnerable` (Bun, 2 observations) | 7 | 2.42 s, 187 ms/run | 2.37 s, 184 ms/run |
| `drizzle-node` (Node + Drizzle, 13 observations) | 17 | 15.58 s, 746 ms/run | **10.25 s, 446 ms/run** |
| `drizzle-node` `a-kill-after-db-commit --repeat 5` | 6 | 5.42 s, 732 ms/run | 3.70 s, 434 ms/run |
| generated harness (Node + Drizzle, 2 observations) | 6 | 3.69 s, 428 ms/run | 3.68 s, 432 ms/run |

Profiling by elimination: the generated harness (same runtime and ORM,
2 observations) was ~300 ms per run faster than the fixture (13
observations), so the cost was observations, not Node.js: each SQL
observation spawned its own `psql`, 2–3 stages per run. Phase 4 batches all
SQL observations of a stage into one read-only `psql` invocation (with a
per-query fallback that names a failing observation). The remaining
~430 ms per Node run is mostly Node.js + Drizzle start-up and per-run
database creation; not optimized further.

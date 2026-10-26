# Phase 9 storage profile

macOS-27.0.1-arm64-arm-64bit-Mach-O; Phase 8 binary = commitrift-phase8 (saved release build of 4dc01af); this build = target/release/commitrift.

## 1. Where exploration time goes (7 actors, sleep sets, `sample` 1 ms for 20 s)

| binary | samples on the exploring thread | canonical encoding | fragment interning | whole canonical key | visited-table lookup / insert | transition execution (engine) |
| --- | --- | --- | --- | --- | --- | --- |
| Phase 8 | 13616 | 2429 (17.8 %) | n/a | 2429 (17.8 %) | 1190 (8.7 %) | 7457 (54.8 %) |
| Phase 9 | 14197 | 2532 (17.8 %) | 893 (6.3 %) | 3480 (24.5 %) | 216 (1.5 %) | 8534 (60.1 %) |

## 2. Counterexample discovery (no unique constraint, `--no-minimize`, median of 3)

| actors | binary | verdict | time to first violation ms | exploration wall ms | states stored | peak RSS MiB |
| --- | --- | --- | --- | --- | --- | --- |
| 3 | Phase 8 | COUNTEREXAMPLE_FOUND | 0 | 0 | 46 | 8.6 |
| 3 | Phase 9 | COUNTEREXAMPLE_FOUND | 0 | 0 | 46 | 8.8 |
| 4 | Phase 8 | COUNTEREXAMPLE_FOUND | 3 | 3 | 145 | 9.4 |
| 4 | Phase 9 | COUNTEREXAMPLE_FOUND | 2 | 2 | 145 | 9.1 |
| 5 | Phase 8 | COUNTEREXAMPLE_FOUND | 13 | 13 | 532 | 11.9 |
| 5 | Phase 9 | COUNTEREXAMPLE_FOUND | 12 | 12 | 532 | 9.5 |
| 6 | Phase 8 | COUNTEREXAMPLE_FOUND | 63 | 63 | 2071 | 24.1 |
| 6 | Phase 9 | COUNTEREXAMPLE_FOUND | 57 | 57 | 2071 | 10.2 |

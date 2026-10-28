# Benchmarks

How performance is measured, and where the scripts and results are. The
measured numbers, with the binaries they belong to, are collected in the
[evidence index § 6](../research/evidence-index.md#6-performance); this
page does not restate them.

## Methodology

* **Same machine, same inputs, two binaries.** Every comparison runs the
  previous phase's preserved release binary and the new one in the same
  session (for example `reports/phase13/baseline/commitrift-phase12`).
  Figures from different sessions are not compared directly.
* **Medians and maxima.** Wall time is the median of repeated runs (3 in
  Phases 11–13), peak RSS the maximum, from `/usr/bin/time -l` on macOS.
* **Identical results first.** A speed-up counts only when outputs are
  identical (static analysis byte-identical; model checking identical
  states, transitions and verdicts).
* **Machine.** One Apple Silicon Mac (arm64). The Linux container is a VM
  on the same machine; its timings are not compared across platforms.
* **Reproducible output.** `--no-timing` removes wall-clock fields so that
  outputs can be compared byte for byte.

## Scripts

| script | measures | phase |
| --- | --- | --- |
| `scripts/bench.py`, `scripts/compare-phase1.py` | static analysis on the corpus; run comparison | 2 |
| `scripts/phase5-attribute.py` | cost attribution per analysis phase | 5 |
| `scripts/phase7-depth-bench.py`, `scripts/phase8-depth-bench.py` | static analysis at increasing call depths | 7, 8 |
| `scripts/phase7-model-bench.py`, `scripts/phase8-model-bench.py`, `scripts/phase9-model-bench.py` | model checking by number of actors | 7–9 |
| `scripts/phase9-profile-bench.py` | profile of the visited-state structures | 9 |
| `scripts/phase10-por-bench.py`, `scripts/phase10-workloads.py` | reductions on 13 workloads × 6 modes | 10 |
| `scripts/phase12-corpus.py`, `scripts/phase13-analyzer-corpus.py` | discovery on the corpus (wall, RSS, counts) | 12, 13 |
| `commitrift model-check --profile-storage` | memory of visited states for one scenario | 9+ |
| `commitrift-corpus` (`cargo run --release -p commitrift-corpus -- …`) | corpus runs, comparisons, phase memory profile | 2+ |

## Workloads

* **Static analysis:** the four corpus projects at depths 3, 6, 12 (and up
  to 8 in depth benchmarks) and the three path modes.
* **Model checking:** the team-app invitation action at 4–8 actors (Phases
  7–10), the Phase 10 workload matrix (independent, mostly independent,
  conflicting-safe, pool contention, write skew, crash/retry, team-app),
  fixture scenarios.
* **Discovery:** the commerce, workspace and evaluation applications and
  the corpus projects.

## Where results are

`reports/phase{5,6,7,8}/perf/`, `reports/phase9/model-bench.md`,
`reports/phase10/final/por-bench.md`, `reports/phase11/evidence/SUMMARY.md`,
`reports/phase12/corpus/`, `reports/phase13/analyzer/corpus/`,
`reports/phase13/compat/state-space-overhead.md`. Per-phase write-ups:
[6](../phase6/PERFORMANCE.md), [7](../phase7/PERFORMANCE.md),
[8](../phase8/PERFORMANCE.md), [9](../phase9/PERFORMANCE.md),
[10](../phase10/STATE-SPACE-PERFORMANCE.md), [11](../phase11/PERFORMANCE.md),
[12](../phase12/PERFORMANCE.md). Phase 13's performance write-up is not yet
in `docs/phase13/`.

## Known scale limits

The model-checking state space is exponential in the number of actors
(≈ 7× per actor on the Phase 8 workload); 8 actors finish on the Phase 9
benchmark workload, 9 were not run. Classic DPOR saves memory but executes
far more transitions than sleep sets; source-DPOR is opt-in and slower than
sleep sets on highly conflicting safe workloads.

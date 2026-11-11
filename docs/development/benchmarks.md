# Benchmarks

How performance is measured, and where the scripts and results are. The
measured numbers, with the binaries they belong to, are collected in the
[evidence index § 6](../research/evidence-index.md#6-performance); this
page does not restate them.

## Methodology

* **Same machine, same inputs, two binaries.** Every comparison runs the
  previous release binary (preserved outside version control) and the new
  one in the same session.
  Figures from different sessions are not compared directly.
* **Medians and maxima.** Wall time is the median of repeated runs (3 for
  discovery and corpus runs), peak RSS the maximum, from `/usr/bin/time -l` on macOS.
* **Identical results first.** A speed-up counts only when outputs are
  identical (static analysis byte-identical; model checking identical
  states, transitions and verdicts).
* **Machine.** One Apple Silicon Mac (arm64). The Linux container is a VM
  on the same machine; its timings are not compared across platforms.
* **Reproducible output.** `--no-timing` removes wall-clock fields so that
  outputs can be compared byte for byte.

## Scripts

| script | measures |
| --- | --- |
| `scripts/bench.py` | static analysis on the corpus |
| `scripts/depth-bench.py` | static analysis at increasing call depths (`--old BIN` compares with an older binary) |
| `scripts/model-bench.py` | model checking by fixture, number of actors, crash/retry dimensions, reduction and in-flight sections |
| `scripts/por-bench.py` | the partial-order reduction matrix |
| `scripts/perf.py`, `scripts/corpus.py` | discovery cold/warm cache and corpus runs, optionally against an older binary |
| `scripts/profile-bench.py` | profile of the visited-state structures |
| `scripts/por-bench.py`, `scripts/por-bench.py` | reductions on 13 workloads × 6 modes |
| `scripts/corpus.py`, `scripts/corpus.py` | discovery on the corpus (wall, RSS, counts) |
| `midwrite model-check --profile-storage` | memory of visited states for one scenario |
| `midwrite-corpus` (`cargo run --release -p midwrite-corpus -- …`) | corpus runs, comparisons, memory profile per analysis stage |

## Workloads

* **Static analysis:** the four corpus projects at depths 3, 6, 12 (and up
  to 8 in depth benchmarks) and the three path modes.
* **Model checking:** the team-app invitation action at 4–8 actors, the
  reduction workload matrix (independent, mostly independent,
  conflicting-safe, pool contention, write skew, crash/retry, team-app),
  fixture scenarios.
* **Discovery:** the commerce, workspace and evaluation applications and
  the corpus projects.

## Where results are

Current measurements are recorded where the script that produced them
writes (see each script's arguments) and summarized in the
[evidence index § 6](../research/evidence-index.md#6-performance). The
per-phase performance records, raw benchmark outputs and write-ups of the
development history are archived ([history](../HISTORY.md)).

## Known scale limits

The model-checking state space is exponential in the number of actors
(≈ 7× per actor on the team-app invitation benchmark); 8 actors finish
with the compact state storage (about 4.5 million states), 9 were not run. Classic DPOR saves memory but executes
far more transitions than sleep sets; source-DPOR is opt-in and slower than
sleep sets on highly conflicting safe workloads.

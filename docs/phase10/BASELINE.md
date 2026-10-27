# Phase 10 baseline

Everything in this file was measured at the start of Phase 10 on the clean
tree of commit `81d27e4` (the Phase 9 finalization), before any Phase 10 code
change, and the raw output is under `reports/phase10/baseline/`.

| | |
| --- | --- |
| platform | Darwin 27.0.0, arm64 (Apple Silicon) |
| rustc / Node | 1.98.1 / 22.23.2 |
| PostgreSQL used by the runs | 17.11 (Homebrew; the server binaries the replay discovers, as in Phase 9; `pg_config` on `PATH` reports 18.6, which no run used) |
| Phase 9 release binary | `reports/phase10/baseline/commitrift-phase9`, sha256 `c2f4d6d4…` (the Phase 9 final binary) |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace` | exit 0, **524 passed, 0 failed, 0 ignored** (`reports/phase10/baseline/test.txt`) |
| `cargo build --release` | exit 0 |

The test count equals the Phase 9 final report. The source digest of the
baseline tree is not recomputed here: `scripts/phase8-digest.py` was run only
after Phase 10 edits began, so the Phase 9 digest `df9f2a6d…` in
`docs/phase9/FINAL-REPORT.md` remains the reference for the Phase 9 snapshot.

## The Phase 9 reduction numbers, reproduced

`scripts/phase10-workloads.py` writes the team-app workloads (the extracted
`inviteTeamMember` action with a hypothetical `UNIQUE(teamId, email)`, N
copies of the actor). With the saved Phase 9 binary:

| workload | search | states | transitions |
| --- | --- | ---: | ---: |
| team-app, 3 actors | sleep sets | 280 | 294 |
| | no reduction | 280 | 588 |
| | classic DPOR | 6 626 | 6 625 |
| team-app, 4 actors | sleep sets | 1 984 | 2 291 |
| | no reduction | 1 984 | 5 344 |
| | classic DPOR | 3 476 615 | 3 476 614 |

These equal `docs/phase9/PERFORMANCE.md` §5. The explored graph of the
unreduced 4-actor search has **101 734 080** root-to-terminal schedules in
1 984 states.

## Profile of the classic DPOR

The classic search was instrumented (`explore_dpor_profiled`, switched on by
`COMMITRIFT_DPOR_PROFILE=1`; the instrumentation keeps side tables of states
and trace signatures, so it is a measurement tool, not the search). A trace
signature is the happens-before closure of every event, each event labelled
by its transition and its occurrence number, combined commutatively; two runs
with equal signatures are the same Mazurkiewicz trace.

| counter (team-app) | 3 actors | 4 actors |
| --- | ---: | ---: |
| transitions executed | 6 625 | 3 476 614 |
| maximal executions ("terminal nodes") | 1 777 | 984 511 |
| **distinct Mazurkiewicz traces** among them | **21** | **148** |
| distinct canonical terminal states | 12 | 32 |
| distinct states visited | 142 | 858 |
| transitions run below a state that an earlier frame had already opened | 6 416 (96.8 %) | 3 475 100 (99.96 %) |
| race requests whose backtrack point was already present | 3 597 | 2 036 419 |
| requests that fell back to "every enabled transition" | 0 | 0 |
| backtrack points added | 1 776 | 984 510 |

### What the profile says

* **The slowdown is duplicate exploration, not conservative dependencies.**
  The 4-actor search executed 984 511 maximal runs that contain 148 distinct
  traces: every trace was explored about **6 650 times**. No race request fell
  back to adding every enabled transition, so the reported cause is not an
  over-pessimistic fallback.
* **Missing sleep sets** is what the duplicate count measures. Classic DPOR
  without sleep sets is known to re-explore equivalent traces; here it does so
  by a factor of thousands because the actors conflict almost everywhere (one
  table, one sequence), so almost every backtrack point leads to a run that
  differs from an earlier one only by the order of independent steps in
  another part of the tree.
* **Missing state caching** is why those runs are so long: 99.96 % of the
  transitions are executed below a state the search had already opened. The
  stateless search pays for the whole subtree again each time.
* Not measured as causes, and not claimed: cyclic exploration (the transition
  system is acyclic within `maxSteps`), enabledness handling and repeated
  backtracking of the same point (the "already present" counter above is the
  nearest measure; it counts requests, not re-execution).
* Stateless DPOR cannot be fixed for this workload by a better backtracking
  rule alone: the number of distinct traces itself grows with the number of
  conflicting actors (21, 148 at 3 and 4 actors), while the number of states
  grows more slowly (280, 1 984). A state cache and sleep sets attack the two
  different parts of the redundancy.

Everything after this point is in `POR-ARCHITECTURE.md` and
`STATE-SPACE-PERFORMANCE.md`.

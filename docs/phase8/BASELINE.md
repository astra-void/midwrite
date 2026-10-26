# Phase 8 baseline

Recorded on 2026-10-26 before any Phase 8 change, on the machine all Phase 8
macOS numbers come from.

## Repository state

| | |
| --- | --- |
| branch | `phase-7` |
| commit | `6ada341b67c005254790d3dace9196c465bd0480` ("docs: Phase 7 design, evaluation and final report") |
| working tree | clean (`git status`) |
| tracked files | 783 |
| source digest | `5bf9103650ff0b4edb6b86e5037e6ed3838695f36f45f317313c8cbcdd708d0c` = `git ls-files -z \| xargs -0 shasum -a 256 \| shasum -a 256` |
| preserved binaries | `.corpora/bin/commitrift-phase7` (sha256 `da3686c4a40ae810067560b4b994ee7c368c4552e7810a9ba4d365c2d1cfa96e`, `cargo build --release` of the commit above), `.corpora/bin/commitrift-corpus-phase7` (built from `git archive HEAD` of the same commit) |

No commit, remote or push was made by Phase 8.

## Environment

| | |
| --- | --- |
| machine | Apple Silicon (arm64), macOS (Darwin 27.0.0, xnu-13432.1.9) |
| Rust | rustc 1.98.1 (Homebrew), cargo 1.98.1 |
| PostgreSQL | 17.11 (Homebrew) |
| Node.js | 22.23.2 |
| Drizzle / pg | 0.45.4 / 8.23.1 (pinned in the fixture lockfiles, installed from the local npm cache) |

## Gates (unchanged code)

| gate | result |
| --- | --- |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace` | **412 passed, 0 failed, 0 ignored** (≈124 s of test time) — the Phase 7 report's macOS count |
| `cargo build --release` | ok |

## Static analysis (Phase 7 binary, one run per depth, 8 threads)

`commitrift-corpus run --timings` on the four pinned projects:

| project | depth | findings | total ms | summaries ms | path analysis ms | expansions | reuses |
| --- | --- | --- | --- | --- | --- | --- | --- |
| midday | 3 | 282 | 283.8* | 38.1 | 14.5 | 2 816 | 1 489 |
| school-ledger | 3 | 332 | 166.2* | 40.3 | 23.2 | 2 496 | 5 099 |
| midday | 6 | 285 | 238.0 | 58.5 | 20.9 | 3 047 | 1 541 |
| school-ledger | 6 | 593 | 167.5 | 58.7 | 58.6 | 2 726 | 5 202 |
| midday | 12 | 285 | 228.9 | 46.1 | 21.0 | 3 095 | 1 546 |
| school-ledger | 12 | 593 | 170.6 | 60.6 | 59.9 | 2 754 | 5 210 |
| saas-starter | 3–12 | 23 | 4.6–9.3 | 0.7–1.1 | 0.6–1.0 | 52 | 43 |
| unnamed-ai-project | 3–12 | 17 | 6.7–11.7 | 0.6 | 0.3–0.4 | 61 | 23 |

\* first run after a build (cold caches). Medians of interleaved runs are in
[`PERFORMANCE.md`](PERFORMANCE.md). At depth ≥ 4 summary construction (the
IR inliner) and path analysis are the two depth-dependent shares;
lowering and resolution dominate midday and do not depend on depth.

## What Phase 7 did not do (the starting problems)

From `docs/phase7/FINAL-REPORT.md` and `docs/phase7/LIMITATIONS.md`, with
what the audit found in the code:

| limitation | code fact found in the audit |
| --- | --- |
| crash/retry/deadlock-victim schedules refused for original code | `source_verify::plan` returns `Err` on `TKind::Crash/Retry/Deadlock`; the coordinator has one `Worker` per actor, no restart |
| no external effects at runtime | `source.js` coordinates SQL only; the model's `Op::External` has no runtime counterpart |
| no `postgres-js` | `source.js` patches `pg.Pool` clients only |
| summaries per call instance | path summaries keyed by `CallScope` address; the inliner memo is keyed by (callee, *remaining depth*, callbacks) |
| schema declared by scenarios | `database.tables` is hand-written in every scenario |
| two TypeScript frontends | `analyzer/lower.rs` (2 918 lines) and `analyzer/concurrency.rs` (4 043 lines) both recognize Drizzle chains, transactions and calls; shared today: function identity (`identity.rs`), wrapper facts (`wrapper.rs`) |
| no resource limits for source workers | no `setrlimit`; a worker child that calls `setsid` escapes the process-group kill on macOS |

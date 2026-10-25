# Phase 3 baseline (Phase 2 state, before any Phase 3 change)

Recorded on 2026-10-25 from the working tree as found, before the rename
and before any Phase 3 code. All numbers below were measured in this
session unless marked *recorded* (taken from `reports/phase2/`). Raw
outputs are in `reports/phase3/baseline/`.

## Repository state

| Item | Observed |
| --- | --- |
| Git | `main` branch with **no commits**; every file is untracked. No Git remote is configured. |
| Workspace | 4 crates: `crashproof-core`, `crashproof-analyzer`, `crashproof-cli` (binary `crashproof`), `crashproof-corpus` (dev tool, `publish = false`) |
| Rust toolchain | rustc 1.98.1, cargo 1.98.1 (Homebrew) |
| Repository URL in `Cargo.toml` | `https://github.com/crashproof/crashproof` — **not verifiable** (no remote); treated as a placeholder |
| Source size | ~15k lines of Rust in `crates/*/src`, ~2.6k lines of Rust tests |
| Corpus checkouts | `.corpora/saas-starter` and `.corpora/midday` at the pinned revisions; both private projects present locally with the same 2 local changes recorded in Phase 2 |
| Saved binaries | `.corpora/bin/crashproof-phase1` (recorded); `.corpora/bin/crashproof-phase2` and `crashproof-corpus-phase2` copied from this baseline's release build |

## Quality gates

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | **156 passed, 0 failed, 0 ignored** (`reports/phase3/baseline/cargo-test.txt`) |
| `cargo build --release` | ok |

Test breakdown: core unit 69, analyzer unit 8, `tests/analysis.rs` 32,
`tests/fixtures.rs` 3 (53 single-file fixtures), `tests/phase2.rs` 22,
`tests/projects.rs` 1 (21 multi-file projects), `tests/cli.rs` 19, corpus
runner 1, doc-test 1.

## Corpus findings (default `recommended` policy)

Re-running the Phase 2 corpus runner on the pinned corpus reproduces the
recorded reports exactly:

```text
crashproof-corpus compare reports/phase2/corpus-phase2.json reports/phase3/baseline/corpus-phase2-rerun.json
0 added, 0 removed, 0 changed
crashproof-corpus compare reports/phase2/corpus-next.json reports/phase3/baseline/corpus-next-rerun.json
0 added, 0 removed, 0 changed
```

Original (pre-review) configurations, `corpus-phase2-rerun.json`:

| Project | Revision | Files | Functions | With effects | Truncated | Diagnostics by code | Warnings |
| --- | --- | --- | --- | --- | --- | --- | --- |
| saas-starter | `6e33e58b` | 41 | 156 | 28 | 0 | CP1001 1, CP1004 11, CP1005 3, CP9001 6 | 1 |
| midday | `51587319` | 2022 | 14827 | 1030 | 10 | CP1001 40, CP1002 9, CP1004 116, CP1005 15, CP9001 89 | 49 |
| school-ledger (private) | `4b97f34` + 2 local changes | 462 | 6391 | 526 | 12 | CP1004 123, CP9001 194 | 0 |
| unnamed-ai-project (private) | `39fcfc7` + 2 local changes | 104 | 648 | 41 | 0 | CP1004 8, CP9001 9 | 0 |

Post-review configurations (`corpus-next-rerun.json`; not an independent
measurement): midday 36 warnings (13 CP1001 demoted to `info` by
`bestEffort`), school-ledger with `getDb()` + `exclude` 10 CP9001 and
1 truncated function.

### CP9001 by cause (original configurations)

| Project | Unrecognized calls | Truncated functions | Unmodeled DB method | Raw SQL |
| --- | --- | --- | --- | --- |
| saas-starter | 6 | 0 | 0 | 0 |
| midday | 62 | 10 | 10 | 7 |
| school-ledger | 180 | 12 | 0 | 2 |
| unnamed-ai-project | 9 | 0 | 0 | 0 |

## Baseline precision (labels written by the Phase 2 implementer)

`crashproof-corpus precision` over `corpora/labels` (reviewed findings with
a decided label only; 95% Wilson intervals):

| Code | Found | Reviewed | TP | Intended risk | FP | Unclear | Hazard precision | Bug precision |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CP1001 | 41 | 41 | 13 | 24 | 1 | 3 | 0.97 (0.87–1.00) | 0.34 (0.21–0.50) |
| CP1002 | 9 | 9 | 2 | 4 | 2 | 1 | 0.75 (0.41–0.93) | 0.25 (0.07–0.59) |
| CP1004 | 258 | 45 | 12 | 30 | 1 | 2 | 0.98 (0.88–1.00) | 0.28 (0.17–0.43) |
| CP1005 | 18 | 8 | 0 | 7 | 1 | 0 | 0.88 (0.53–0.98) | 0.00 (0.00–0.32) |

These are source-level judgements by the same agent that implemented
Phase 2. None was confirmed by a maintainer, and none was reproduced at
runtime.

## Performance (release build, `scripts/bench.py`, 10 runs after 1 warm-up)

Measured on this machine (Apple Silicon macOS, 8 analysis threads by
default), `reports/phase3/baseline/bench-phase2-rerun*.tsv`:

| Project | Median wall (8 threads) | Peak RSS | Median wall (1 thread) | Peak RSS (1 thread) | Recorded Phase 2 median / RSS |
| --- | --- | --- | --- | --- | --- |
| saas-starter | 0.0089 s | 6.8 MiB | 0.0100 s | 6.0 MiB | 0.0076 s / 6.8 MiB |
| midday | 0.1735 s | 73.1 MiB | 0.3765 s | 61.3 MiB | 0.1563 s / 73.5 MiB |
| school-ledger | 0.1270 s | 75.9 MiB | 0.2553 s | 64.5 MiB | 0.1177 s / 76.3 MiB |
| unnamed-ai-project | 0.0105 s | 8.2 MiB | 0.0160 s | 7.2 MiB | 0.0099 s / 8.1 MiB |

The same binary is 6–17% slower today than the recorded Phase 2 medians;
RSS is within 1%. This is run-to-run and machine-state variance, and is the
noise floor for Phase 3 comparisons: Phase 3 performance is compared against
**this** re-run, not against the recorded values.

## Unsupported patterns and known blind spots (carried into Phase 3)

From `docs/LIMITATIONS.md`, `docs/phase2/FOLLOW-UP.md` and the counts above:

* Class methods and `this.method()` (11 of 158 context CP9001 calls in midday).
* Callbacks other than inline transaction callbacks.
* General helper return-value correlation.
* Retry, redelivery and reconciliation outside the analyzed function.
* Adjacent but unrelated effects are paired (main false-positive class).
* Truncation at 512 paths (22 functions across the corpus).
* Memory: whole-project IR retained for cross-module summaries
  (1.4–8× Phase 1 peak RSS).
* No runtime reproduction of any finding.

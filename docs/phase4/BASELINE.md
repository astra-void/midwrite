# Phase 4 baseline (frozen before any Phase 4 change)

Recorded on 2026-10-25, before modifying implementation code. Raw logs:
`reports/phase4/baseline/`.

## Source revision

* Git: branch `main` with **no commits**; every file is untracked. There is
  no revision hash to cite and no remote.
* The working tree was compared file by file with the supplied Phase 3
  digest (`digest.txt`, a 285-file text dump): **285 of 285 files identical**,
  none missing. Not in the digest: `.gitignore`, `Cargo.lock`,
  `crates/commitrift-cli/src/bin/crashproof.rs` (present on disk, consistent
  with the Phase 3 report). Nothing changed between the digest and the start
  of Phase 4.
* `digest.txt` itself (10 MB) is an input artifact, not source; Phase 4 adds
  it to `.gitignore` and leaves the file in place.

## Environment

| | |
| --- | --- |
| Machine | Apple Silicon Mac (10 cores, 16 GiB), macOS 27.0.1 |
| Rust | rustc 1.98.1, cargo 1.98.1 (MSRV in `Cargo.toml`: 1.97) |
| Runtimes | Node.js v22.23.2, Bun 1.3.3 |
| PostgreSQL | 17.11 (Homebrew) |
| Isolation | macOS `sandbox-exec` |
| Linux | none native. Docker Desktop 29.7.2 was installed but its daemon was stopped; Phase 4 started it to obtain a Linux kernel (linuxkit 7.0.12, aarch64). No GitHub-hosted runner is available (no remote). |

## Quality gates

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace` | **217 passed, 0 failed, 0 ignored, 0 skipped** (matches the Phase 3 report) |
| `cargo build --release` | ok |

Per suite: analyzer unit 8, `analysis.rs` 32, `fixtures.rs` 3, `phase2.rs`
22, `phase3.rs` 5, `projects.rs` 1, `traces.rs` 16, CLI unit 1, `cli.rs` 24,
`replay.rs` 14, core unit 76, corpus unit 1, replay unit 13, doc-test 1.
The 14 replay tests executed (Bun, PostgreSQL and sandbox-exec present);
none skipped.

## Replay scenarios (Phase 3)

| Harness | Runtime | Cases | What it shows |
| --- | --- | --- | --- |
| `order-vulnerable` | Bun + a hand-written Drizzle-shaped adapter over Bun's SQL client | 6 | CP1001 reproduced (kill and exception); compensation differs between exception and kill |
| `order-outbox` | same | 5 | transactional outbox: no loss at 5 failure points, duplicate delivery after a relay crash |
| `invoice-pattern` | same | 3 | isolated re-write of a midday code shape (not midday) |

No harness used the real `drizzle-orm` package or Node.js.

## Trace coverage and corpus findings

| Project | Diagnostics | Traces (complete / approximated / incomplete) |
| --- | --- | --- |
| fixtures | 84 | 63 (61 / 2 / 0) |
| saas-starter | 21 | 15 (14 / 1 / 0) |
| midday | 269 | 180 (126 / 7 / 47) |
| school-ledger (private) | 317 | 123 (30 / 0 / 93) |
| unnamed-ai-project (private) | 17 | 8 (8 / 0 / 0) |

Corpus revisions are the Phase 3 ones (saas-starter `6e33e58b`, midday
`51587319`, school-ledger `4b97f34` + 2 local changes, unnamed-ai-project
`39fcfc7` + 2 local changes); all four were checked out locally.

## Performance (same machine)

From the Phase 3 binary rebuilt from the digest and measured in the Phase 4
session (`reports/phase4/perf/bench-phase3-*.tsv`, median of 7 runs):

| Project | Wall, 8 threads | Peak RSS, 8 threads | Wall, 1 thread | Peak RSS, 1 thread |
| --- | --- | --- | --- | --- |
| midday | 0.165 s | 65.5–68.6 MiB (two rounds) | 0.376 s | 54.5 MiB |
| school-ledger | 0.121–0.122 s | 73.6–74.2 MiB | 0.251 s | 61.5 MiB |
| saas-starter | 0.009 s | 7.1 MiB | 0.010 s | 6.3 MiB |
| unnamed-ai-project | 0.011 s | 8.4 MiB | 0.016 s | 7.2 MiB |

## Known limitations at the start of Phase 4

From `docs/phase3/FINAL-REPORT.md`, `docs/phase3/SECURITY.md` and
`docs/LIMITATIONS.md`:

1. Replay harnesses used Bun and a minimal client, not real Drizzle code.
2. Isolation existed only on macOS; Linux refused without an unsafe opt-in.
3. No upstream application bug was runtime-confirmed.
4. The automated blind review agreed with only 3 of 7 Phase 2 "true
   positive" labels; the labels are not ground truth.
5. Retries and recovery outside an analyzed function were only declarable
   per external effect.
6. 140 corpus traces were incomplete.
7. Harnesses had to be written entirely by hand.
8. No release workflow, no CI, no Git history.
9. Known false-positive classes: unrelated adjacent effects, recovery outside
   the analyzed code; known false negatives: class methods and
   `this.method()`, callbacks, raw SQL, truncation at 512 paths.

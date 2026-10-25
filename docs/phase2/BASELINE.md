# Phase 2 baseline (Phase 1 as found)

Recorded on 2026-10-24, before any Phase 2 change to implementation code.
Machine: Apple M1 Pro, 16 GiB, macOS; `rustc 1.98.1`, `cargo 1.98.1`.
The repository had no commits; the Phase 1 release binary was copied to
`.corpora/bin/crashproof-phase1` (git-ignored) so that later comparisons run
the exact Phase 1 analyzer.

## Toolchain checks

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | **102 passed, 0 failed** |
| `cargo build --release` | success |

Test breakdown: `crashproof-core` unit tests 48; `crashproof-analyzer` unit
tests 3, `tests/analysis.rs` 32, `tests/fixtures.rs` 3; `crashproof-cli`
`tests/cli.rs` 15; doc-tests 1.

## Fixture coverage

50 fixture files with `// @expect` annotations, checked by
`crates/crashproof-analyzer/tests/fixtures.rs` (exact set of `(code, line)`
pairs, plus confidence and function where annotated):

| Directory | Files | Purpose |
| --- | --- | --- |
| `fixtures/vulnerable/` | 28 | expected hazards (CP1001–CP1005) |
| `fixtures/safe/` | 10 | expected silence |
| `fixtures/unsupported/` | 12 | expected CP9001 limitations, one parse error |

Phase 1 output on `fixtures/` (`crashproof check fixtures --config
fixtures/crashproof.json --format json`, saved as
`reports/phase2/baseline/fixtures-phase1.json`): 49 of 50 files analyzed (the
syntax-error fixture fails to parse, as expected), 92 functions, 51
diagnostics: CP1001 ×26, CP1002 ×4, CP1003 ×3, CP1004 ×4, CP1005 ×2,
CP9001 ×12; confidence high ×32, medium ×6, low ×13.

## Fixed evaluation corpus

Defined in `corpora/manifest.json` (public, pinned) and
`corpora/manifest.local.json` (private checkouts on this machine). Each
project has a configuration in `corpora/configs/`, written from the
project's database and client modules before any result was inspected.

| Project | Origin | Revision | Kind |
| --- | --- | --- | --- |
| saas-starter | github.com/nextjs/saas-starter | `6e33e58b1e553a41fe22e6b941a7229a002de361` | Next.js + Drizzle + Stripe |
| midday | github.com/midday-ai/midday | `51587319f26a0ffaa9dfccab1920373cb65689b7` | large monorepo: Drizzle, job queues, email, payments |
| school-ledger | local checkout | `4b97f34` (2 locally modified files) | Next.js + Drizzle, no external effects |
| unnamed-ai-project | local checkout | `39fcfc7` (locally modified) | Next.js + Drizzle + `fetch` |

Measurement: `scripts/bench.py --runs 10` (one warm-up run, then 10 runs,
each a fresh process; wall time with `perf_counter`, peak RSS from
`getrusage` of the child). Raw rows: `reports/phase2/baseline/bench-phase1.tsv`;
last-run reports: `reports/phase2/baseline/bench-phase1.<project>.json`.

| Project | Files | Functions | Median wall (s) | Min–max (s) | Peak RSS (MiB) | Diagnostics |
| --- | --- | --- | --- | --- | --- | --- |
| saas-starter | 41 | 156 | 0.0084 | 0.0078–0.0095 | 4.9 | 25 |
| midday | 2022 | 14827 | 0.2423 | 0.2388–0.2507 | 20.5 | 164 |
| school-ledger | 462 | 6391 | 0.0914 | 0.0904–0.1002 | 9.6 | 60 |
| unnamed-ai-project | 104 | 648 | 0.0119 | 0.0113–0.0121 | 5.5 | 19 |

Diagnostic counts by code (Phase 1, default `--fail-on warning`):

| Project | CP1001 | CP1002 | CP1003 | CP1004 | CP1005 | CP9001 |
| --- | --- | --- | --- | --- | --- | --- |
| saas-starter | 1 | 0 | 0 | 14 | 3 | 7 |
| midday | 0 | 0 | 0 | 67 | 17 | 80 |
| school-ledger | 0 | 0 | 0 | 10 | 0 | 50 |
| unnamed-ai-project | 0 | 0 | 0 | 5 | 0 | 19 |

Of 123 warnings across the four projects, 96 are CP1004 and 20 are CP1005;
only one is a cross-resource finding (CP1001). These counts are measured,
not adjudicated: whether each is a real hazard is assessed in
`CORPUS-EVALUATION.md`.

## Correctness problems confirmed before Phase 2

Each was reproduced with the Phase 1 binary on a small input.

1. **Helper parameters treated as root handles (high-confidence false
   positive).** A same-file helper `saveOrder(db: Database, id)` that writes on
   its parameter `db`, called as `await saveOrder(tx, id)` inside
   `db.transaction(async (tx) => ...)`, is reported as CP1004 *high* ("commits
   independently of the transaction"): the inlined write is analyzed as a
   write on the root handle `db`, although the caller passed the transaction
   handle.
2. **Correlated conditions.** `if (flag) write(); if (!flag) publish();`
   yields CP1001 (medium) for a path that cannot execute.
3. **Single-file summaries only.** Calls of functions imported from other
   modules stay unknown effects (CP9001 when after a commit, silent before).
   In the corpora, most data-access code lives in separate modules
   (`lib/db/queries.ts`, `packages/db/src/queries/*`), so their effects are
   invisible to callers.
4. **Variant duplicates.** One write after a branch is reported once per
   distinct preceding commit (four CP1004 at `saas-starter`
   `app/(login)/actions.ts:210`), because a finding is keyed by the pair of
   operations.
5. **CP1004/CP1005 dominate output** (see above), although two sequential
   writes or two external calls are not necessarily meant to be atomic.
6. **Recognition strength does not affect confidence.** A `this.db`
   member-path match (only names checked) can produce `high` confidence.
7. Calls inside `Promise.all([...])` of same-file helpers are not
   summarized, and database handles obtained from a call (`getDb().insert(...)`,
   used throughout school-ledger) are not recognized at all.

Items 1–6 are addressed in Phase 2 (see `PRECISION-CHANGES.md` and
`CROSS-MODULE-ANALYSIS.md`); item 7 is recorded in `docs/LIMITATIONS.md`.

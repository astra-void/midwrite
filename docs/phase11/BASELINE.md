# Phase 11 baseline

Measured at the start of Phase 11, before any Phase 11 code change. Raw
output: `reports/phase11/baseline/`.

## Revision

| | |
| --- | --- |
| branch | `phase-8` |
| commit | `accbcf5` ("docs(phase10): design, correctness, evidence and final report") |
| working tree | clean: the Phase 10 work had been committed (`6cfb4a7`, `8e2f044`, `accbcf5` on top of `81d27e4`); nothing uncommitted had to be preserved |
| source digest (`scripts/phase8-digest.py`, 593 files) | `59275b50edea91f11ac8ee9e5622e8630338ffc12c3e4a2d0f5b6b2cca1817c7` — **equal to the Phase 10 final digest** |
| release binary | sha256 `ec9c019f0a01ad1b…` — **the Phase 10 final binary**; saved as `reports/phase11/baseline/commitrift-phase10` (not committed) |
| platform | Darwin 27.0.0 arm64, rustc 1.98.1, Node 22.23.2, PostgreSQL 17.11 (Homebrew) |

## Quality gates

Two runs were made. The first (`reports/phase11/baseline/test.txt`) ran in
the working tree while the Phase 11 fixture directory was being created and
stopped at the first failing test binary: one Node runtime unit test
(`unknown scenarios and prototype keys are refused`) failed once and passed
3 of 3 times when rerun alone (a timing-sensitive test under load); it is
not used as the baseline.

The baseline is the second run, on a clean `git archive` of `accbcf5` with
the fixtures' `node_modules` copied in (`reports/phase11/baseline/clean/`):

| gate | result |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace --no-fail-fast` | **551 passed, 4 failed, 2 ignored** (21.3 min) |
| `cargo build --release` | exit 0, sha256 `ec9c019f…` |

The 4 failures (`application_code_never_runs_without_the_explicit_authorization`,
`declared_programs_are_not_eligible_for_source_verification`,
`crash_verification_never_runs_without_the_explicit_authorization`,
`executing_the_application_still_needs_the_explicit_flag`) are static tests
whose first reported problem is "the harness command file does not exist"
when the fixtures' `dist/` directories have not been built yet: the archive
has no `dist/` (it is ignored by git) and these tests ran before the tests
that compile the fixtures. Rerun after the compilation, all 4 pass
(`clean/rerun-4.txt`), giving the Phase 10 total of **555 passed**. This is
a pre-existing test-order dependency, not a regression; it does not occur
in a working tree where the fixtures were built before.

## What the Phase 10 tree provides for Phase 11

Inspected before designing (`ARCHITECTURE.md` says how each piece is used):

* **Extraction**: `commitrift-analyzer::concurrency::TsActorProvider`
  lowers one function (selected by a source selector, `identity.rs`) to the
  concurrency IR, inlining helpers across modules up to depth 3, binding
  Drizzle tables, declared externals and transaction-retry helpers, and
  reporting wrapper facts (`wrapper.rs`). Its output does not depend on
  argument values.
* **Schema**: `commitrift-analyzer::schema` reads Drizzle tables, keys,
  unique sets (partial indexes marked), foreign keys, defaults; the scenario
  loader reads tables `from` it and refuses what it cannot model.
* **Scenario loader**: `commitrift-model::scenario::load_str` builds a
  `Model`, validates initial rows (not null, CHECK, keys) and refuses
  actors whose wrapper invocation is conditional without a declared
  contract.
* **Engine**: `engine::{initial_state, enabled, apply}` with a `Recorder`
  returning closed statements and their results.
* **Model checking**: `report::check`, sleep sets by default, source-DPOR
  opt-in; witnesses replay strictly (`witness::replay`, `check_schedule`).
* **Original-code verification**: `--verify-source --allow-execution` with a
  harness manifest (`commitrift-source.json`).
* **Invariants**: written by scenario authors only; no origin metadata.
* **Contracts**: `entry.invocation/preconditions` in scenarios; nothing
  project-wide.

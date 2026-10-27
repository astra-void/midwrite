# Phase 12 baseline

Measured at the start of Phase 12, before any Phase 12 change took effect.
Raw output: `reports/phase12/baseline/`.

## Revision

| | |
| --- | --- |
| branch | `phase-8` |
| commit | `6657faf` ("chore: state the copyright holder in both license files") on top of the committed Phase 11 work (`5ab9877` docs, `c0794cf`, `616f4c2`, `b0c4860`, …) |
| working tree | clean: the Phase 11 work and the license edits had been committed before Phase 12 started; nothing uncommitted had to be preserved |
| source digest (`scripts/phase8-digest.py`, 640 files) | `ad9499493ca541585dda5de9afff3b750943490fb8b628a2077f42a690cb91a4` — **equal to the Phase 11 final digest** |
| release binary | sha256 `0643249f1b9274ca…` — **the Phase 11 final binary**; saved as `reports/phase12/baseline/commitrift-phase11` (ignored by git) |
| platform | Darwin 27.0.0 arm64, rustc 1.98.1, Node 22.23.2, PostgreSQL **server 17.11** (Homebrew `postgresql@17`, the server CommitRift's tests start); the `psql`/`initdb` first on `PATH` are the libpq 18.6 client tools |

## Quality gates

The gates ran on a copy of the committed tree in a private scratch directory
with its own target directory, so that Phase 12 edits could not reach them.

| gate | result |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace --no-fail-fast` | **583 passed, 0 failed, 3 ignored** (24.5 min) |
| `cargo build --release` | exit 0, sha256 `0643249f…` |

Ignored: `regenerate_fixtures`, `dump_seed` (Phase 10 tools) and `dump_ir`
(Phase 11). This re-establishes the Phase 11 count after its final
categorization change, which the Phase 11 report had not rerun in full.

**A first attempt is not a baseline.** An earlier run was started in the
working tree; its shell script was stopped but its `cargo test` child kept
running, compiled early Phase 12 edits, and wrote into the same log as the
clean run (`reports/phase12/baseline/contaminated/`, kept with a note). The
clean run was repeated into a fresh file; the numbers above are from it.

## What Phase 12 starts from

* `schema.rs` already read `defaultRandom()` (as the database expression
  `gen_random_uuid()`) and `$defaultFn` (as an application default), but
  the scenario loader refused every such primary key: 42 (midday), 21
  (school-ledger) and 2 (unnamed-ai-project) refused tables.
* A scenario read one schema module; tables were bound to source identifiers
  by name.
* Every extraction discovered the project's files and built a module
  resolver again (98 % of midday's 30 s discovery).
* Re-exports, method calls, call-returned database handles and projected
  `returning({...})` were unknown operations.

# Phase 13 baseline

Measured at the start of Phase 13, before any Phase 13 change took effect.
Raw output: `reports/phase13/baseline/`.

## Revision

| | |
| --- | --- |
| branch | `phase-8` |
| commit | `32efbca` ("docs(phase12): design, evaluation, evidence and final report") |
| working tree | **clean**: the Phase 12 report described its work as uncommitted, but it had been committed (`67966c2` … `32efbca`) before Phase 13 started; nothing had to be preserved |
| source digest (`scripts/phase8-digest.py`, 702 files) | `803fbbd12f6bc6f1ccd9ddef2e28a7c12b7709e9402d380f4fa753abab6c9881` — **equal to the Phase 12 final digest** |
| release binary | sha256 `7da740b2f5f3618f…` — **the Phase 12 final binary**; saved as `reports/phase13/baseline/commitrift-phase12` (ignored by git) |
| platform | Darwin 27.0.0 arm64, rustc 1.98.1, Node 22.23.2, PostgreSQL server 17.11 (Homebrew `postgresql@17`) |

## Quality gates

Run on a copy of the committed tree in a private scratch directory with its
own `CARGO_TARGET_DIR` (the Phase 12 learning: a baseline run in the working
tree can compile early edits).

| gate | result |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace --no-fail-fast` | **606 passed, 0 failed, 3 ignored** — the Phase 12 final count |
| `cargo build --release` | exit 0, sha256 `7da740b2…` |

## What Phase 13 starts from

* Row locks are exclusive: the store maps a row to one holder; `UPDATE`,
  `DELETE` and `SELECT … FOR UPDATE` take the same lock.
* Foreign keys are read from Drizzle schemas but reported as
  `not modeled`; a candidate where one actor locks a referenced row gets the
  `foreign-key-locks` model-limitation assumption, and two first-party
  schedules diverge at runtime (`createWorkspace × createWorkspaceLocked`,
  and the same with the Server Action).
* `Op::Return` carries no value; a function's return value is not part of
  the state, and there is no step between a request's last statement and
  its response.
* Reads through midday's `withReplicas` database (typed
  `Awaited<ReturnType<typeof connectDb>>`) are 752 unknown operations.
* School-ledger's 9 candidates are all `UNSUPPORTED`.
* `INSERT … ON CONFLICT … DO UPDATE` is an unknown operation.

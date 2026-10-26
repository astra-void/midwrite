# Phase 9 baseline

Established on 2026-10-26 before any Phase 9 change, on the Phase 8
commit (`4dc01af`), and compared with what the Phase 8 report recorded.

| item | Phase 8 report | re-established |
| --- | --- | --- |
| host | macOS arm64 (Darwin 27.0.0) | same machine |
| `cargo fmt --all -- --check` | clean | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean | clean |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace` | 452 passed | **452 passed, 0 failed** (`reports/phase9/baseline/cargo-test.txt`) |
| source digest (`scripts/phase8-digest.py`) | `d371ba90…` (465 files) | **identical** `d371ba90935868cc5bc3a5c12b5a31ce9bc8f9a8a027626d9a0d1f5b9aefd4a3` |
| `cargo build --release` | ok | ok; the binary is kept as `reports/phase9/baseline/commitrift-phase8` (git-ignored, 9.5 MiB) and is the "old" side of every comparison below |
| PostgreSQL | 17.11 | 17.11 (Homebrew), `psql` 18.6 client |
| Node.js | 22.23.2 | 22.23.2 |

## What the Phase 8 transition system does with an unanswered operation

Read from `crates/commitrift-model/src/engine.rs` before changing it:

* `COMMIT` is one transition (`Op::Commit` in `step_thread`): the server
  commits, the response is delivered, the actor advances. There is no state in
  which a COMMIT was submitted and its outcome is unknown.
* `Crash` is enabled between two transitions of a process; it aborts the open
  transactions of the process's actors and marks them `Crashed`.
* "Lost acknowledgment" is therefore only "crash after the operation and
  before the next one". The model's committed state after such a crash equals
  what a real server holds when the *response* is lost, but the client side of
  that history is not represented: the model cannot say that the client does
  not know, and a reconciliation read cannot race with a COMMIT that the
  server has not completed yet.
* `Model::process_of` already lets actors share a process (one `apps` map,
  crash together); source verification refused it
  (`verify_schedule`: "one process per actor").
* There is no connection pool: `Conn::Tx` is the actor's transaction,
  `Conn::Pool` an unbounded pool.
* The runtime crash checkpoint is "announced and held" (never sent) or "waiting
  on a lock"; a kill with a released COMMIT running is refused
  (`docs/phase8/CRASH-RETRY-SEMANTICS.md`).

## Phase 8 model-checker cost (before)

Measured with the saved Phase 8 binary on the extracted team-app action with a
hypothetical `UNIQUE(teamId, email)` (the Phase 8 benchmark workload;
`fixtures/phase7/scenarios/team-app-invite-double-submit.json`, N copies of
the actor, sleep-set reduction on):

| actors | states | transitions | visited bytes | bytes per state | peak RSS |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 4 | 1 984 | 2 291 | 6.7 MB | 3 361 | 17 MiB |
| 5 | 13 984 | 18 042 | 56.9 MB | 4 066 | 91 MiB |
| 6 | 97 408 | 140 975 | 463 MB | 4 758 | 740 MiB |

`visitedBytes` was `key length + 96` per stored state, where the key is the
flat canonical encoding. About 98 % of it is the encoding itself: every
row carries its column names as strings, every statement is rendered with
`format!("{:?}")`, every locals map repeats its keys. Memory therefore grows
with the *size* of a state, not only with their number — which is what Phase 9
addresses (`STATE-SPACE-STORAGE.md`).

## Where the Phase 8 limitations end up

`docs/phase8/LIMITATIONS.md` lists what Phase 9 targets. The mapping is in
`LIMITATIONS.md` of this directory.

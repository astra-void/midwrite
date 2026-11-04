# Phase 16 baseline

Recorded before any Phase 16 change, on 2026-10-29. Logs:
`reports/phase16/baseline/`.

## Working tree

* Branch `phase-8`, HEAD `1efa383` ("docs(phase15): design, evaluation,
  evidence and final report"). The Phase 15 work, uncommitted at the end of
  Phase 15, has since been committed (`77927d7` … `1efa383`), including the
  post-freeze clippy fix (`07f6559`, `map(&close)` → `map(close)` in
  `crates/commitrift-model/src/engine.rs`). The only untracked path was
  `.claude/` (tool settings; left alone).
* Source digest (`scripts/phase8-digest.py`, 927 files):
  `98567599d96cefdc985b3f0b380f2a14276dd46a838cd08d8cf9c2526f9be045` —
  **identical to the digest the Phase 15 report records for its fixed tree
  (§8a)**, so the tree at the start of Phase 16 is the Phase 15 final tree.
* macOS: Darwin 27.0.0 arm64, rustc 1.98.1 (clippy 0.1.98), Node 22.23.2,
  PostgreSQL 17.11 (Homebrew). Docker Desktop's daemon was not running.
* Pinned packages used by the corpus: school-ledger zod **4.6.5**, midday
  zod `^4.3.6` (catalog), drizzle-orm 0.45.4 in the fixtures. The Phase 14
  zod model was written against zod 3.25.76 semantics; the zod 4.6.5 and
  3.25.76 packages used for the differential matrix are the copies in the
  local pnpm store (never fetched, never part of the repository).

## Quality gates on a frozen copy

`rsync` copy of the tree (without `target/`, `.git/`), own
`CARGO_TARGET_DIR`.

| gate | result |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace --no-fail-fast` | **751 passed, 0 failed, 3 ignored** (71 test targets) |
| `cargo build --release` | exit 0; `commitrift` sha256 `903356e5…` — byte-identical to the Phase 15 report's release binary |

This matches the Phase 15 final record (751 / 0 / 3). The release binary is
kept (outside the repository) as the "Phase 15 binary" for every
comparison in this phase.

## Known gate failures carried in

| failure | state at baseline |
| --- | --- |
| Linux clippy `needless_borrows_for_generic_args` (rustc 1.99.1) at `engine.rs:1181` | fixed in `07f6559`; a 1.99 toolchain is not installed on this Mac, so it can only be re-checked on Linux |
| `python3 scripts/validate-schemas.py --builtin reports/phase13/evidence/*.json` | **exit 1**: `workspace-app-discovery.json: no schema for kind/version ('commitrift.discovery', 1)` (reproduced on macOS) |

## The Phase 15 binary on the fresh Phase 16 set

Scored with `scripts/phase16-eval.py` (the Phase 15 scorer with only the
paths changed, written before the set existed) right after the set was
frozen, reading aggregates only: **TP 1, FP 0, FN 17, TN 19** (precision
1.000, recall 0.056). Raw: `reports/phase16/eval/phase15-binary.json`.

## Corpus reference (Phase 15 report)

| project | effectful | candidates | model-checkable | wall | peak RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| saas-starter | 9 | 2 | 1 | 0.04 s | 13 MiB |
| midday (declared replica factory) | 81 | 4 | 0 | 1.32 s | 148 MiB |
| school-ledger | 70 | 9 | 0 | 0.76 s | 121 MiB |

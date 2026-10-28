# Phase 15 baseline

Recorded before any Phase 15 change, on 2026-10-28.

## Working tree

* Branch `phase-8`, HEAD `4c338ad` ("docs(phase14): design, evaluation,
  evidence and final report"). The Phase 13 and Phase 14 work, uncommitted
  at the end of Phase 14, has since been committed in five commits
  (`e1a30d5` … `4c338ad`). The only untracked path was `.claude/`
  (tool settings, not part of the project; left alone).
* Source digest (`scripts/phase8-digest.py`, 879 files):
  `e486816bed5cb6a1cc11aa1e1d710239b8a6cf222e97a2510f2544dc8e1b44cb` —
  **identical to the digest recorded by the Phase 14 final report**, so the
  tree at the start of Phase 15 is the Phase 14 final tree.
* macOS: Darwin 27.0.0 arm64, rustc 1.98.1, Node 22.23.2, PostgreSQL 17.11
  (Homebrew, server accepting connections on `/tmp:5432`; tests use their
  own disposable clusters).

## Quality gates on a frozen copy

Run on an `rsync` copy of the tree (without `target/` and `.git/`) with its
own `CARGO_TARGET_DIR`, so later edits could not leak in. Logs:
`reports/phase15/baseline/`.

| gate | result |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace --no-fail-fast` | **734 passed, 0 failed, 3 ignored** (69 test targets) |
| `cargo build --release` | exit 0 |

This matches the Phase 14 final record (734 / 0 / 3).

## Reference binary

`target/release/commitrift` was rebuilt from the unchanged tree
(`cargo build --release -p commitrift-cli`) and kept as the "Phase 14
binary" for comparisons (sha256 `059b1150…`). Its sha256 differs from the
Phase 14 report's release binary (`1f035c61…`), which was produced by a
workspace-wide `cargo build --release`; the source is the same.

## The two historical false negatives, reproduced

| case | Phase 14 binary | cause |
| --- | --- | --- |
| Phase 14 set **P14-E21** `saveDisplayName` (read-your-writes through a lagging replica) | `NO_VIOLATION_WITHIN_BOUNDS` | the synthesized row's `displayName` (`"displayName-1"`) equals the argument the actor writes: the stale read returns the same text |
| Phase 13 set **E26** `createFolder` (UNIQUE NULLS DISTINCT at the root) | `NO_VIOLATION_WITHIN_BOUNDS` | both actors get `parentId = 1`; the defect needs `parentId = null`, which synthesis never generates |

Note: the "E26" of the Phase 14 set is a different case (`claimJob`,
`Date#getTime()`); the null-argument case named in the Phase 15 brief is
the Phase 13 set's E26.

## Docker

At the start of the cycle the Docker Desktop daemon was not running
(`docker info`: socket missing). See `FINAL-REPORT.md` §8 for the Linux run.

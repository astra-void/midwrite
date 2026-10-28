# Testing

Contribution rules (fixtures for every behavior change, deterministic
output, no unearned confidence) are in [`CONTRIBUTING.md`](../../CONTRIBUTING.md).
This page describes the test suite and how the phase reports ran it.

## Quality gates

Every phase report records these four gates on its final tree:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace --no-fail-fast
cargo build --release
```

Without `COMMITRIFT_REQUIRE_REPLAY=1`, tests whose runtime prerequisites
are missing print `SKIPPED` and pass — that is **not** runtime
verification. With it, a missing prerequisite or a blocked isolation
backend fails the test.

Runtime prerequisites: PostgreSQL server binaries (`initdb`, `pg_ctl`,
`postgres`; `COMMITRIFT_PG_BIN` selects a directory), Node.js ≥ 22, Bun
(Phase 3/4 replay tests), an isolation backend (macOS `sandbox-exec`, or
Linux bubblewrap with unprivileged user namespaces), and the fixtures'
pinned packages and builds, for example:

```bash
(cd fixtures/replay/drizzle-node && npm ci --ignore-scripts && npm run build)
(cd fixtures/phase7/team-app && npm ci --ignore-scripts && npm run build)
```

`scripts/phase12-linux-docker.sh` lists every fixture that needs this. Some
static tests also depend on pre-built fixture `dist/` directories
([Phase 11 baseline](../phase11/BASELINE.md)).

Test counts by phase (final tree, macOS): Phase 6 381, Phase 7 412,
Phase 8 452, Phase 9 524, Phase 10 555 (2 ignored), Phase 11 583 (3
ignored), Phase 12 606 (3 ignored). The ignored tests are tools, not
skipped checks (`regenerate_fixtures`, `dump_seed`, `dump_ir`). Phase 13's
final count is not yet recorded.

## Where tests live

| crate | tests | covers |
| --- | --- | --- |
| `commitrift-analyzer` | `tests/fixtures.rs`, `projects.rs`, `analysis.rs`, `traces.rs`, `phase2.rs` … `phase13.rs`, `phase13_resolution.rs`, `concurrency.rs`, `differential.rs` | `@expect` fixtures, multi-file projects, extraction, source resolution |
| `commitrift-model` | `engine.rs`, `properties.rs`, `por*.rs`, `dpor.rs`, `sdpor.rs`, `pg_matrix.rs`, `postgres.rs`, `lock_ground_truth.rs`, `phase7.rs` … `phase13.rs` | model semantics, reductions vs the unreduced search, PostgreSQL lockstep matrices, ground-truth replay |
| `commitrift-discover` | `discovery.rs`, `phase13.rs`, `dump.rs` | detectors, synthesis, contracts, cache invalidation |
| `commitrift-replay` | `source_lifecycle.rs`, `proxy_postgres.rs` | crash/restart, hangs, limits, detached processes, proxy fates |
| `commitrift-cli` | `cli.rs`, `model_check.rs`, `replay.rs`, `phase7.rs` … `phase13.rs` | end to end: exit codes, reports, HTML escaping, authorization, original-code runs |
| `runtime/node` | `test/runtime.test.js`, `test/postgres-js.test.js` | run by the CLI tests with `node --test` |

## Fixtures

* `fixtures/vulnerable/`, `safe/`, `unsupported/`: single files with
  `// @expect CP1001 line=12 confidence=high function=completeOrder`,
  `// @expect none` or `// @expect parse-error`; the set of `(code, line)`
  pairs must match exactly.
* `fixtures/projects/`: multi-file projects for cross-module behavior.
* `fixtures/replay/`: runnable replay harnesses.
* `fixtures/concurrency/`, `fixtures/phase7` … `fixtures/phase13`:
  model-check scenarios and first-party applications for original-code
  verification. The single-file `@expect` harness skips them.
* `fixtures/phase11/labels.json`, `fixtures/phase12/eval-labels.json`,
  `fixtures/phase13/eval-labels.json` (+ `FROZEN.json`): labeled
  evaluation sets; never edit labels ([evaluation methodology](../research/evaluation-methodology.md)).

Never add a test that executes code from `.corpora/` or any external
repository.

## Linux

`scripts/linux-replay-docker.sh` (Phase 4) and the per-phase
`scripts/phase{8,9,10,12,13}-linux-docker.sh` run fmt, clippy, the full
suite with required replay, the release build and the phase's evidence in a
Debian 12 container, as a non-root user, with only the bubblewrap-needed
security options relaxed. The `ci/linux-replay.Dockerfile` image is shared.
`.github/workflows/ci.yml` and `release.yml` exist; their header says they
have not run on GitHub-hosted runners.

## Known intermittent tests

* `deadlock_workloads` (Phase 6 oracle): PostgreSQL chose the other
  deadlock victim once on Linux (Phase 8); timing-steered.
* `claim-skip-locked-2/serializable`: an unresolved intermittent failure
  (Phase 9, [test reliability](../phase9/TEST-RELIABILITY.md)).
* Two Phase 7/8 HTML tests read committed generated reports as input; if
  the evidence run that writes them was blocked, they fail until the
  report is regenerated.

## Report schemas and documentation links

```bash
python3 scripts/validate-schemas.py [--builtin] REPORT.json...   # needs jsonschema unless --builtin
python3 scripts/check-doc-links.py                                # relative links and anchors in README, CONTRIBUTING, docs/
python3 scripts/check-doc-links.py --historical report            # report, but do not fail on, docs/phaseN/
```

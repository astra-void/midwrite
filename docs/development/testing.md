# Testing

Contribution rules (fixtures for every behavior change, deterministic
output, no unearned confidence) are in [`CONTRIBUTING.md`](../../CONTRIBUTING.md).
This page describes the test suite and how it is run.

## Quality gates

A change is ready when these four gates pass:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
MIDWRITE_REQUIRE_REPLAY=1 cargo test --workspace --no-fail-fast
cargo build --release
```

Without `MIDWRITE_REQUIRE_REPLAY=1`, tests whose runtime prerequisites
are missing print `SKIPPED` and pass — that is **not** runtime
verification. With it, a missing prerequisite or a blocked isolation
backend fails the test.

Runtime prerequisites: PostgreSQL server binaries (`initdb`, `pg_ctl`,
`postgres`; `MIDWRITE_PG_BIN` selects a directory), Node.js ≥ 22, Bun
(single-request replay tests), an isolation backend (macOS `sandbox-exec`, or
Linux bubblewrap with unprivileged user namespaces), and the fixtures'
pinned packages and builds, for example:

```bash
(cd fixtures/replay/drizzle-node && npm ci --ignore-scripts && npm run build)
(cd fixtures/apps/team-app && npm ci --ignore-scripts && npm run build)
```

`scripts/linux-docker.sh` lists every fixture that needs this. Some
static tests also depend on pre-built fixture `dist/` directories.

Two more switches have the same meaning:
`MIDWRITE_REQUIRE_ZOD=1` (the zod differential needs Node.js and the
pinned zod packages, located with `MIDWRITE_ZOD3_DIR` and
`MIDWRITE_ZOD4_DIR`) and `MIDWRITE_REQUIRE_SCHEMA=1` (report-schema
tests). `fixtures/apps/retry` needs `npm ci && npm run build` for its
original-code tests.

The ignored tests are tools, not skipped checks (`regenerate_fixtures`,
`dump_seed`, `dump_ir`).

## Where tests live

| crate | tests | covers |
| --- | --- | --- |
| `midwrite-analyzer` | `fixtures.rs`, `projects.rs`, `analysis.rs`, `traces.rs`, `transactions.rs`, `value_layer.rs`, `recovery.rs` (traces, replay, recovery contracts), `wrappers.rs`, `drizzle_schema.rs`, `schema_modules.rs`, `replicas.rs`, `source_resolution.rs`, `drizzle_statements.rs`, `js_values.rs`, `zod.rs`, `multi_row_locks.rs`, `nulls_not_distinct.rs`, `concurrency.rs`, `differential.rs` | `@expect` fixtures, multi-file projects, value layer, extraction, schema facts, source resolution; golden data in `tests/data/` |
| `midwrite-model` | `engine.rs`, `properties.rs`, `por.rs`, `por_adversarial.rs`, `por_audit.rs`, `por_mutation.rs`, `por_regressions.rs`, `dpor.rs`, `sdpor.rs`, `pg_matrix.rs`, `postgres.rs`, `lock_ground_truth.rs`, `defaults_and_joins.rs`, `crash_retry_plans.rs`, `in_flight.rs`, `process_pool.rs`, `state_storage.rs`, `generated_values.rs`, `foreign_keys.rs`, `drizzle_statements.rs`, `js_values.rs`, `generated_id_invariants.rs`, `multi_row_locks.rs`, `nulls_not_distinct.rs` | model semantics, reductions vs the unreduced search, PostgreSQL lockstep matrices, ground-truth replay |
| `midwrite-discover` | `discovery.rs`, `dump.rs`, `replicas.rs`, `input_domains.rs`, `generated_ids.rs`, `multi_row_locks.rs`, `nulls_not_distinct.rs`, `zod.rs` | detectors, synthesis, world search, contracts, cache invalidation |
| `midwrite-replay` | `source_lifecycle.rs`, `proxy_postgres.rs` | crash/restart, hangs, limits, detached processes, proxy fates |
| `midwrite-cli` | `cli.rs`, `model_check.rs`, `replay.rs`, `server_actions.rs`, `crash_retry.rs`, `in_flight.rs`, `connection_loss.rs`, `discover_commerce.rs`, `generated_ids.rs`, `responses.rs`, `retry_wrapper.rs`, `input_worlds.rs`, `loops.rs`, `report_schemas.rs` | end to end: exit codes, reports, HTML escaping, authorization, original-code runs, report schemas |
| `runtime/node` | `test/runtime.test.js`, `test/postgres-js.test.js` | run by the CLI tests with `node --test` |

## Fixtures

* `fixtures/vulnerable/`, `safe/`, `unsupported/`: single files with
  `// @expect MW1001 line=12 confidence=high function=completeOrder`,
  `// @expect none` or `// @expect parse-error`; the set of `(code, line)`
  pairs must match exactly.
* `fixtures/projects/`: multi-file projects for cross-module behavior.
* `fixtures/replay/`: runnable replay harnesses.
* `fixtures/concurrency/`, `fixtures/scenarios/`, `fixtures/model/`:
  model-check scenarios (hand-written, crash/retry, in-flight,
  connection-loss), lock ground truth, report-schema negative cases.
* `fixtures/apps/`: first-party applications for extraction, discovery
  and original-code verification.
* `fixtures/reports/`: report files that tests read as input (replay,
  model-check, discovery, Drizzle SQL probes).
* `fixtures/eval/p11` … `p16`: labeled and frozen evaluation sets; never
  edit labels ([evaluation datasets](../research/evaluation-datasets.md)).

The single-file `@expect` harness skips everything but the first three
directories.

Never add a test that executes code from `.corpora/` or any external
repository.

## Linux

`scripts/linux-replay-docker.sh` and `scripts/linux-docker.sh` run fmt, clippy, the full
suite with required replay, the release build and the evidence runs in a
Debian 12 container, as a non-root user, with only the bubblewrap-needed
security options relaxed. The `ci/linux-replay.Dockerfile` image is shared.
`.github/workflows/ci.yml` and `release.yml` exist; their header says they
have not run on GitHub-hosted runners.

## Rules for runtime tests

Tests that start PostgreSQL or run workers follow these rules, learned
from flakes:

1. **No fixed sleeps as synchronization.** Tests wait for an observable
   condition with a deadline: a proxy event (`OperationHeld`,
   `OperationCompleted`), a `pg_stat_activity` state, a worker checkpoint
   or a row in a table. The deadline only turns a hang into an
   INCONCLUSIVE result.
2. **Each test owns its cluster and its directory:** `initdb` in a private
   temporary directory, trust authentication, Unix socket only,
   `listen_addresses=''`, removed on every exit path.
3. **Scenario files that tests generate live outside the repository**, so
   a test never leaves a modified fixture behind.
4. **Wall-clock-sensitive runtime tests are serialized** (a `SERIAL`
   mutex), so a loaded machine does not change which of two valid server
   behaviors is observed.
5. **A blocked precondition is reported, not skipped silently**; under
   `MIDWRITE_REQUIRE_REPLAY=1` it fails.
6. **Timeouts, proxy errors and worker deaths are inconclusive**, never
   counted as agreement and never upgraded.
7. **Expected-result lists are not widened to fit a flake.** An expected
   verdict changes only when the semantics change, with the reason in the
   commit. The only retried observation is the deadlock victim (at most 3
   attempts, counted).

## Known intermittent tests

* `deadlock_workloads` (lockstep oracle): PostgreSQL chose the other
  deadlock victim once on Linux; timing-steered.
* `claim-skip-locked-2/serializable`: an unresolved intermittent failure.
* Two HTML tests read committed generated reports as input
  (`fixtures/reports/`); if those are regenerated incorrectly, they fail.

## Report schemas and documentation links

```bash
python3 scripts/validate-schemas.py [--builtin] REPORT.json...   # needs jsonschema unless --builtin
python3 scripts/check-doc-links.py                                # relative links and anchors in README, CONTRIBUTING, docs/
```

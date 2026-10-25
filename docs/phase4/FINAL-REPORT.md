# Phase 4 final report

Question: *can the Phase 3 workflow — static finding → failure trace →
controlled failure → observed PostgreSQL state → verification report — be
used on real TypeScript code by a developer who did not build CommitRift?*
Measured answer: **closer, not yet independently**. The workflow now runs
on Node.js with the real Drizzle ORM, on Linux with OS isolation, and starts
from a generated, finding-linked harness skeleton; a developer still writes
the observations and the invariant, and nothing has run on an external
application or on GitHub-hosted CI.

Everything below was measured on 2026-10-25 on one Apple Silicon Mac
(macOS 27.0.1) and in an unprivileged Debian 12 container on the same
machine (Linux 7.0.12, aarch64). Raw data: `reports/phase4/`. Goals that
were not reached are marked as such.

## 1. Implementation

| Milestone | Status |
| --- | --- |
| 0 Baseline | **Done** — [`BASELINE.md`](BASELINE.md): 217/217 tests, tree identical to the Phase 3 digest. |
| 1 Native Node.js replay | **Done** — one protocol client for Node.js and Bun (`runtime/node`), manifest `runtime` with version and entry-point preflight, cause-chain detection of injected faults. [`NODE-DRIZZLE.md`](NODE-DRIZZLE.md) |
| 2 Real Drizzle ORM | **Done** — `fixtures/replay/drizzle-node` (drizzle-orm 0.45.4, pg 8.23.1, lockfile), scenarios A–D, 16 cases, driver-level checkpoints (`runtime/node/pg.js`). |
| 3 Linux isolation and CI | **Done locally, CI unverified** — bubblewrap backend, probed, fail closed; full suite on Linux; CI workflow written, not run on GitHub. [`LINUX-ISOLATION.md`](LINUX-ISOLATION.md) |
| 4 Trace-to-harness generator | **Done** — `commitrift init-replay`, demonstrated on the Drizzle fixture end to end. [`HARNESS-GENERATION.md`](HARNESS-GENERATION.md) |
| 5 Recovery-aware analysis | **Done, narrow** — function-level `recoveryContracts` with cited evidence; provenance model; runtime recovery observations. [`RECOVERY-SEMANTICS.md`](RECOVERY-SEMANTICS.md) |
| 6 Verification workflow / regression | **Done** — stable case keys, `replay --baseline/--regression-output`, verification schema v2, regression schema v1, HTML finding→case table and evidence provenance. |
| 7 Real-world validation | **Partial** — corpus regression and trace analysis done; blind human-review workflow built; **0 human reviews**; **no external application reproduced**. [`REAL-WORLD-EVALUATION.md`](REAL-WORLD-EVALUATION.md) |
| 8 Performance | **Done** — no static regression after fixing one; replay 40 % faster for observation-heavy harnesses. [`PERFORMANCE.md`](PERFORMANCE.md) |
| 9 Release engineering | **Partial** — packaging script (2 targets verified), release workflow (not run), README; no Git history or remote. [`RELEASE-READINESS.md`](RELEASE-READINESS.md) |
| 10 Tests and security | **Done** — 245 tests on both platforms; review with two fixes in the generator. [`SECURITY-REVIEW.md`](SECURITY-REVIEW.md) |

### Architectural decisions

* **No second replay engine.** Node.js support is a runtime client plus a
  manifest field; supervisor, protocol, failure modes, PostgreSQL
  supervision, cleanup and reports are the Phase 3 code. Bun and Node
  harnesses share one JavaScript client.
* **Checkpoints from the driver.** `instrumentPgPool` observes the SQL
  Drizzle actually sends and PostgreSQL's replies, so application code stays
  uninstrumented (analyzed code must not contain checkpoints) and a commit is
  claimed only when the server acknowledged it as one.
* **Compile, don't load.** Node harnesses run `tsc` output; replay never
  installs or compiles and blocks with an instruction instead.
* **bubblewrap** for Linux, with a minimal filesystem view rather than
  "everything read-only", because read-only bind mounts do not stop Unix
  socket connections; no host-`/proc` fallback, because it would leak other
  processes' environments.
* **Stable identity through the analysis root.** Fingerprints contain paths
  relative to the root, so a harness in a subdirectory declares
  `analysis.root: ".."`; generated cases link by exact fingerprint.
* **Placeholders fail closed** (`COMMITRIFT-TODO` in manifest, setup SQL or
  compiled entry point → refused).
* **Contracts change presentation, never existence**: same fingerprint,
  trace and location; severity lowered only under `recommended`.
* **Schema versions**: verification reports v2 add optional fields only;
  v1 reports still load (used as regression baselines in tests).

### Files

New: `runtime/node/` (client, `pg` adapter, event-log transport, types,
9 `node --test` cases); `fixtures/replay/drizzle-node/` (application,
harness, manifest, lockfile, completed `generated-harness/`);
`crates/commitrift-replay/src/isolation.rs`;
`crates/commitrift-core/src/recovery.rs`;
`crates/commitrift-cli/src/init_replay.rs`;
`crates/commitrift-analyzer/tests/phase4.rs`; `ci/linux-replay.Dockerfile`;
`.github/workflows/{ci,release}.yml`; `scripts/{linux-replay-docker.sh,
validate-schemas.py, package-release.sh, demo-phase4.sh}`;
`corpora/configs/midday.phase4-contracts.json`,
`corpora/manifest.phase4-contracts.json`; `docs/phase4/` (this directory and
`schemas/`); `reports/phase4/`.

Changed: `runtime/typescript/commitrift-replay.ts` (re-export);
`commitrift-replay` (`lib.rs`, `manifest.rs`, `safety.rs` — isolation moved
out, `supervisor.rs`, `postgres.rs` — batched observations, `observe.rs`);
`commitrift-core` (`config.rs`, `diagnostic.rs`, `verification.rs`,
`analysis/render.rs`, `lib.rs`); `commitrift-analyzer` (`lib.rs`,
`project.rs`, `tests/traces.rs` +1 test); `commitrift-cli` (`lib.rs`,
`replay.rs`, `html.rs`, `render.rs`, `tests/replay.rs`); `commitrift-corpus`
(`traces`, `review-packet`, `review-check`; a deprecation-proof atomic
update); `README.md`, `CONTRIBUTING.md`, `docs/LIMITATIONS.md`,
`docs/ARCHITECTURE.md`, `.gitignore`. Untouched: `docs/phase2/`,
`docs/phase3/`, `reports/phase2/`, `reports/phase3/`, `corpora/labels/`,
diagnostic codes, fingerprints, the JSON check report schema, the trace
schema, baselines, SARIF fingerprint key.

### Incomplete features

Human review (workflow only); reproduction in an external application; CI
and release runs on GitHub; x86_64 targets; drivers other than node-postgres;
resource limits for harnesses; a source-patch mode for the generator (not
needed by the driver-level design, not implemented).

## 2. Validation

| | macOS arm64 (sandbox-exec) | Linux aarch64 (bubblewrap) |
| --- | --- | --- |
| `cargo fmt --check`, `cargo clippy -D warnings` | clean | clean (rustc 1.99.1) |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace` | **245 passed, 0 failed, 0 skipped** | **245 passed, 0 failed, 0 skipped** |
| `cargo build --release`; build without `replay` feature | ok | ok |

245 = 217 (Phase 3) + 28: `phase4.rs` 5 (contracts), `traces.rs` +1
(Drizzle findings pinned), CLI unit +2, `replay.rs` +12 (Node runtime
client [9 `node --test` cases], real Drizzle ×2, Node isolation, unbuilt Node
entry point, generator ×3, completed generated harness, regression, detached
grandchildren, HTML of v1 reports), core unit +2, replay unit +6. Skipped:
none on either platform.

**Negative CI check** (Linux, Docker's default seccomp, no user namespaces):
the CLI replay is *blocked* (exit 3, nothing executed); 15 replay tests
**fail** under `COMMITRIFT_REQUIRE_REPLAY=1`; none skips silently.

**Drizzle replay** (both platforms, identical outcomes): 16/16 cases as
declared — A dual write reproduced by kill and by exception (5/5 with
`--repeat 5`); B Drizzle transaction rolls back before COMMIT, with the
uncommitted writes invisible from another session; C an event published
inside a transaction survives its rollback by exception, by kill and by a
COMMIT that PostgreSQL rejects on a deferred unique constraint (no injection
at all); D outbox keeps the intent at every failure point and delivers a
duplicate after a relay crash. Exceptions and kills are reported distinctly
(`threw`/exit 1 vs `killed`/signal 9; `injectedFaultSurfaced`).

**Phase 3 regression**: `order-vulnerable`, `order-outbox`,
`invoice-pattern` replayed against their Phase 3 reports as baselines:
3 still reproduced + 3 still not; 0 + 5; 3 + 0; nothing changed.

**Generated harness**: skeleton refused while unfinished (13 placeholders
listed); after 66 changed lines, 5/5 cases as expected on both platforms,
linked to the exact fingerprint `4f53a6215ac16913`.

**Schemas**: all Phase 4 verification (v2), regression (v1) and trace
reports validate, corrupted copies are rejected; SARIF of the fixtures (87
results) and midday (269, with and without contracts) validates against the
OASIS 2.1.0 schema.

## 3. Correctness

| | |
| --- | --- |
| Confirmed application bugs | **None.** |
| Runtime-reproduced windows | First-party fixtures only: Drizzle A (2 cases) and C (3 cases), the Phase 3 fixtures, the generated harness; the isolated invoice pattern (not midday). |
| Potential hazards (corpus, `recommended`) | saas-starter 1, midday 49, school-ledger 0, unnamed-ai-project 0 warnings — unchanged. |
| Intended risks | Declarable per effect and now per function; the evidence-cited midday contract variant moves 8 of 49 warnings to info (none labeled true positive). |
| False positives | No new sound suppression; unrelated-state relations still need value flow (deferred). |
| Incomplete traces | 140, unchanged, **all** caused by unknown calls inside the failure window (top midday causes listed). |
| Diagnostic changes on the corpus | **0** in all three configurations (reports byte-identical to Phase 3). |
| Independent reviews | **0 human reviews.** Review packets for 12 midday findings are ready; labels from Phase 2/3 remain one reviewer plus an automated second opinion. |
| Confidence upgrades from runtime results | none (by design and by test). |

Remaining analysis limitations: [`../LIMITATIONS.md`](../LIMITATIONS.md).

## 4. Performance

Static analysis, Phase 3 binary (rebuilt from the digest) vs Phase 4, same
session ([`PERFORMANCE.md`](PERFORMANCE.md)):

| Project | Wall 8 threads | Peak RSS 8 threads | Wall 1 thread |
| --- | --- | --- | --- |
| midday | 0.165 → 0.164 s | 65.5–68.6 → 66.2–67.6 MiB | 0.371–0.373 → 0.373–0.374 s |
| school-ledger | 0.121–0.122 → 0.121–0.122 s | 73.6–74.2 → 73.1–73.5 MiB | 0.251–0.254 → 0.249–0.252 s |

Within noise; `check` output byte-identical; memory by phase identical.
One regression (+3.5 % single-threaded on school-ledger) was caused by the
first version of the contract hook and fixed. Trace overhead unchanged.
Replay (separately): Node + Drizzle 746 → 446 ms per run after batching
observations; Bun fixture 184 ms; cluster set-up included.

## 5. Public beta

| | |
| --- | --- |
| Installation | from source (`cargo install --path crates/commitrift-cli`); locally built archives |
| Supported platforms | macOS arm64 and Linux aarch64 (both: static analysis and isolated replay, full suite). Linux and macOS x86_64: unverified. Windows: no. |
| Reproducible demo | `scripts/demo-phase4.sh` (all steps automated except writing observations and the invariant) |
| Remaining security concerns | no independent audit; no resource limits; macOS processes leaving the group survive; Linux sandbox exposes the project tree (files and any sockets in it); `sandbox-exec` deprecated |
| Release blockers | Git history and a verified remote; a green GitHub CI run incl. required Linux replay; tagged release run; security review; a human-reviewed precision baseline; crates.io/npm packaging decisions |

### Proposed next priorities

1. A human review of the 12 prepared midday packets (ideally with a midday
   maintainer) before any precision claim.
2. Push to a real repository and get `ci.yml` green on Ubuntu 24.04 and
   macOS; then a tagged release run.
3. One authorized reproduction in an external application with a small
   footprint (a project whose tests already start PostgreSQL), using
   `init-replay`.
4. Driver adapters beyond node-postgres (postgres.js, Neon serverless) and a
   generic external-effect adapter.
5. Harness resource limits and an independent review of the isolation code.

Postpone: class-method analysis, broader ORM support, symbolic execution, a
hosted dashboard.

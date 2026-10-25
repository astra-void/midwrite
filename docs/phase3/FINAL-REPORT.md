# Phase 3 final report

Question: *can CommitRift turn a selected failure window into observable
runtime evidence?* Measured answer: **yes, for an authorized, instrumented
harness**. The path static finding → failure trace → controlled injection →
observed PostgreSQL state → verification report → HTML view works end to
end, and reproduces the CP1001 window of a first-party fixture in 5 of 5
runs with a real SIGKILL after a verified commit. It does **not** yet say
anything at runtime about real applications: the one real-world pattern
reproduced was reproduced in an isolated re-write, and a blind second review
suggests Phase 2's "likely bug" labels were optimistic.

Everything below was measured on 2026-10-25 on one Apple Silicon Mac; raw
data is under `reports/phase3/`. Hypotheses and goals are marked as such.

## 1. Rename summary

| What | Change |
| --- | --- |
| Crates | `crashproof-{core,analyzer,cli,corpus}` → `commitrift-{core,analyzer,cli,corpus}`; new `commitrift-replay`. Rust paths `crashproof_*` → `commitrift_*`. |
| Executables | `commitrift`; plus a legacy `crashproof` executable with identical behavior and a deprecation note on stderr. |
| Configuration | `commitrift.json`; `crashproof.json` read only when `commitrift.json` is absent. When both exist, only `commitrift.json` is read and a note names the ignored file; never merged. |
| Environment | `COMMITRIFT_THREADS`; `CRASHPROOF_THREADS` honored when it is unset. |
| Output branding | `tool.name` `commitrift` in JSON and SARIF, texts and explanations. |
| Unchanged on purpose | Diagnostic codes `CP1001`–`CP9001`; fingerprints (0 changed identities on the corpus); JSON report schema version 1; baseline format (old baselines load); SARIF `partialFingerprints` key `crashproof/v1` (it names the fingerprint algorithm; changing it would reset code-scanning history). |
| Repository URL | The old `https://github.com/crashproof/crashproof` could not be verified (the repository has no Git remote and no commits), so it was removed rather than renamed; SARIF no longer carries an `informationUri`. |
| Preserved history | `docs/phase2/`, `reports/phase2/`, `corpora/labels/` and the `.trail/` learnings are untouched. No Git operation was performed (the working tree had no commits). |

Tests: `legacy_crashproof_executable_behaves_like_commitrift`,
`legacy_configuration_is_a_fallback_and_never_merged`,
`legacy_thread_variable_is_honored`, `discovery_prefers_the_new_name_and_never_merges`.

## 2. Architecture summary

Details: [`ARCHITECTURE.md`](ARCHITECTURE.md), [`TRACE-FORMAT.md`](TRACE-FORMAT.md),
[`REPLAY-PROTOCOL.md`](REPLAY-PROTOCOL.md), [`SECURITY.md`](SECURITY.md).

* **Failure Trace IR** (`commitrift-core::trace`): the reported path, every
  event as an ordered step (reads, writes, transaction begin/commit/rollback,
  external, unknown, branch decisions, returns, unsupported flow) with
  locations, cross-module call chains and transaction ownership; an explicit
  injection boundary with checkpoint hints; a `SIMULATED` crash prediction
  from the analysis' transaction model; validation against that model;
  stable ids without line numbers; `complete`/`approximated`/`incomplete`.
  Derived from the existing analysis without changing it.
* **Replay protocol**: a validated manifest; JSON lines over a Unix socket;
  the harness blocks at each checkpoint until the supervisor answers
  `continue`/`throw`, or is killed (SIGKILL to its process group) while
  waiting — no race, no timing dependence. Failure modes `processKill` and
  `exception` are reported separately.
* **Runtime supervisor** (`commitrift-replay`): disposable PostgreSQL
  cluster per invocation (Unix socket only), fresh database per run, control
  run, pre-injection checks from a separate session, observations before /
  after / after recovery, declared invariants, deadlines, cleanup, network
  isolation via `sandbox-exec` (fail closed elsewhere), cleared environment,
  redaction.
* **Verification evidence model** (`commitrift-core::verification`):
  `STATIC_CANDIDATE`, `SIMULATED`, `RUNTIME_REPRODUCED`,
  `RUNTIME_NOT_REPRODUCED`, `INCONCLUSIVE`, separate from `confidence`;
  runtime levels only from replay, only against harness-declared invariants.

## 3. Implementation status

| Milestone | Status |
| --- | --- |
| 0 Rename and baseline | **Done** ([`BASELINE.md`](BASELINE.md)). |
| 1 Failure Trace IR, generation, validation, `commitrift trace`, schema | **Done**. JSON Schema in `schemas/`. |
| 2 Deterministic fault injection (harness protocol, supervisor, isolation, PostgreSQL fixture, real process termination) | **Done** for the documented scope: one database provider (local disposable PostgreSQL), macOS network isolation, hand-written harnesses (Bun). Docker not used. Linux isolation not implemented (refuses without explicit opt-in). |
| 3 Evidence levels, assertions, verification report, `commitrift replay` | **Done**. |
| 4.1 Safe transactional-outbox fixture | **Done** (including duplicate delivery). |
| 4.2 Recovery semantics | **Done, narrow**: `bestEffort`, `reconciled`, `idempotentConsumer` declarations with provenance; nothing inferred. |
| 4.3 Effect relationships | **Investigated**: name-based argument provenance implemented as a descriptive field; it does not separate bugs from non-bugs, so it is not used for severity. |
| 4.4 Class methods | **Not implemented** (lowest priority; needs receiver typing and override rules). |
| 5 Local viewer | **Done**: `commitrift report`, static HTML, no scripts, CSP. |
| 6 Independent evaluation | **Partial**: static regression done; one isolated real-world pattern reproduction; blind *automated* second review of 18 findings. No human reviewer, no maintainer contact, nothing reproduced in an original application. |
| 7 Performance and memory | **Done**: profiled by phase; two targeted fixes; peak RSS below Phase 2. |
| 8 Tests | **Done** (below). |
| 9 Documentation and demo | **Done**: this directory, README, CONTRIBUTING, ARCHITECTURE, DIAGNOSTICS, LIMITATIONS, `scripts/demo.sh` ([`DEMO.md`](DEMO.md)). |

## 4. Correctness

`cargo test --workspace` with `COMMITRIFT_REQUIRE_REPLAY=1` (replay tests may
not skip): **217 passed, 0 failed, 0 ignored, 0 skipped**
(`reports/phase3/cargo-test.txt`). Baseline: 156.

| Suite | Phase 2 | Phase 3 |
| --- | --- | --- |
| core unit (incl. trace simulation, verification levels, config discovery, recovery config) | 69 | 76 |
| analyzer unit / `analysis.rs` / `fixtures.rs` / `phase2.rs` / `projects.rs` | 8 / 32 / 3 / 22 / 1 | unchanged |
| `traces.rs` (static traces: commit→external, external→commit, external in transaction, outbox, cross-module, tx handle to helper, conditional, unknown, truncated, exclusive branches, ids, duplicates, round trip, harness findings) | — | 16 |
| `phase3.rs` (recovery declarations: positive, negative, strict, never inferred from names) | — | 5 |
| `cli.rs` (incl. legacy executable/config/env, trace command, HTML escaping) | 19 | 24 |
| `replay.rs` (PostgreSQL integration: reproduction ×3 runs, outbox, authorization, missing harness, production-looking targets, secrets and network isolation, timeout and cleanup, failed setup, unreachable checkpoint, failed pre-check, missing runtime, determinism, code-selection ambiguity, static analysis never executes code, HTML of verification reports, isolated invoice pattern) | — | 14 |
| replay unit (manifest safety, invariants, event measures, redaction, private roots, sandbox profile) | — | 13 |
| cli unit / corpus unit / doc-test | 0 / 1 / 1 | 1 / 1 / 1 |

Other gates: `cargo fmt --check` and `cargo clippy --workspace --all-targets
-D warnings` clean; `cargo build --release` and the CLI without the `replay`
feature build. All single-file and multi-file fixtures pass. SARIF output
(fixtures 84 results, midday 269, midday with baseline 269) validates against
the OASIS SARIF 2.1.0 schema; all trace reports (fixtures and four corpus
projects, 389 traces) and verification reports validate against the new
JSON Schemas, which reject injected errors (`schema-validation.txt`,
`trace-coverage.txt`). 0 of 389 traces fail validation against the analysis
model.

**Regressions:** none in diagnostics (0 added/removed/changed in three corpus
configurations). One intended fixture-harness change: `tests/fixtures.rs`
skips `fixtures/replay/` (multi-file harnesses, covered by `traces.rs`).

**New negative tests** include: outbox and mutually exclusive branches yield
no trace; CP9001 never yields a trace; recovery never inferred from names;
idempotent consumers never lower severity; failures outside the boundary do
not reproduce; exception compensation does not reproduce while a crash does;
replay refuses unauthorized runs, external database URLs, credential
variables, shell strings and unknown manifest fields; secrets never reach the
harness or the report; HTML never contains a script element.

**Remaining limitations:** [`../LIMITATIONS.md`](../LIMITATIONS.md) and
[`PRECISION.md`](PRECISION.md).

## 5. Runtime reproduction

Fixture `fixtures/replay/order-vulnerable` (CP1001 in `completeOrder`, trace
`tr-c649a775ffafff7d`), case `kill-after-db-commit`, checkpoint
`after-db-commit` (label `orders`), mode `processKill`, `--repeat 5`:

| | PostgreSQL `orders.status` (`order-1`) | Event log `order.completed` |
| --- | --- | --- |
| Before (fresh database each run) | `pending` | 0 |
| At the checkpoint, from a separate session | `paid` (commit verified) | — |
| After SIGKILL | **`paid`** | **0** |

The process ended by signal 9 at the checkpoint in every run (operation not
completed). The declared invariant `paid-order-has-event` (a paid order has
its event) **failed in 5 of 5 runs**: `RUNTIME_REPRODUCED`, consistent, with
the trace's prediction matching the observed state. The same window with an
injected exception also reproduces (5/5); in the variant with a `catch`
compensation an exception does not reproduce (5/5) while a crash does (5/5).
Negative cases (kill before the write, kill after the publish) do not
reproduce (5/5 each). The transactional-outbox version does not reproduce at
any of 5 failure points (25 runs) and exposes a duplicate delivery after a
relay crash. Full tables: [`REPRODUCTION-RESULTS.md`](REPRODUCTION-RESULTS.md).

## 6. Real-world evaluation

| | |
| --- | --- |
| Corpus revisions | saas-starter `6e33e58b1e553a41fe22e6b941a7229a002de361`, midday `51587319f26a0ffaa9dfccab1920373cb65689b7`, school-ledger `4b97f34` + 2 local changes (private), unnamed-ai-project `39fcfc7` + 2 local changes (private). Analyzed statically only; never executed. |
| Findings | Identical to Phase 2 (saas-starter 21, midday 269, school-ledger 317, unnamed-ai-project 17 diagnostics). |
| Traces | 15 / 180 / 123 / 8; incomplete: 0 / 47 / 93 / 0. |
| Findings reviewed | 18, blind, by a separate automated session (not a human): 11/18 agreement with Phase 2 labels (κ 0.38); 3 of 7 "likely bug" labels confirmed (midday 1/5). |
| False positives eliminated | None. No precision change was found that is sound: the relationship heuristic would have demoted 3 labeled true positives. |
| Reproduced | midday finding `34107b6f5fb66fb4` (invoice finalized before the generate-invoice job): **isolated reproduction of the suspected pattern** in `fixtures/replay/invoice-pattern` (5/5), not in midday; a client retry repairs the state; maintainers not contacted; no upstream issue filed. |
| Unverified | Every other corpus finding. |

## 7. Performance

Same-session comparison with the Phase 2 binary ([`PERFORMANCE.md`](PERFORMANCE.md)):

| Project | Wall time, 8 threads (Phase 2 → 3) | Peak RSS, 8 threads | Peak RSS, 1 thread |
| --- | --- | --- | --- |
| midday | 0.1676 → 0.1694 s (+1.1%) | 75.5 → **67.4 MiB (−11%)** | 61.1 → 54.0 MiB |
| school-ledger | 0.1225 → 0.1227 s (+0.2%) | 76.4 → **72.8 MiB (−5%)** | 64.6 → 60.7 MiB |
| saas-starter | 0.0087 → 0.0092 s (+0.5 ms) | 6.8 → 7.0 MiB | 6.0 → 6.3 MiB |
| unnamed-ai-project | 0.0106 → 0.0110 s (+0.4 ms) | 8.3 → 8.3 MiB | 7.2 → 7.2 MiB |

Causes: the first Phase 3 build regressed peak RSS by ~4–5 MiB because a new
IR field grew every node from 208 to 232 bytes. Profiling by phase showed
most of midday's retained IR was `Vec` growth slack (~610 bytes per node).
Shrinking bodies after lowering and boxing the rarely present recognition
brought `Node` to 176 bytes and peak live heap from 34.8 to 27.4 MiB
(midday, 1 thread). The remaining small-project regression (+0.4–0.5 ms,
+0.2–0.3 MiB) is attributed to the larger binary (hypothesis; not
investigated). In-process phase timings are equal within ±2%. Replay is
measured separately: 2.45 s for the 7 runs of `order-vulnerable` (cluster
creation ~0.8 s, ~0.19 s per run).

## 8. Next priorities

**Would most improve real-world adoption** (opinion, based on this phase):

1. *Human* review of the corpus labels, ideally with maintainers, before any
   further precision claims; the automated second review already moved the
   picture.
2. Harness ergonomics: replay adapters for the real `drizzle-orm` package and
   a Node runtime, and a harness template generated from a trace (with the
   invariant left for the author to write).
3. Recovery declarations at the function or job level (a BullMQ processor
   with retries is the commonest "intended risk").
4. Linux network isolation (user and network namespaces), so replay can run
   in CI.

**Postpone:** class-method resolution, relationship heuristics, a hosted or
multi-user viewer, broader ORM support.

**Before a first public beta:** a real Git history and a verified repository
URL; CI on Linux and macOS (replay required on at least one); a human-reviewed
precision baseline; published JSON Schemas versioned with the binary; a
security review of `commitrift-replay` and the sandbox profile; and a
decision on how long to ship the legacy `crashproof` executable and
configuration fallback.

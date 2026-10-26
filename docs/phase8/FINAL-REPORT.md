# Phase 8 final report

Question: *can a model counterexample that crashes a process and retries
an operation be checked against the application's original code — with a
real kill, a real new process and PostgreSQL's real answer — and can the
analysis be made more reusable, schema-aware and faster without changing
its results?*

Measured answer: **yes, for a documented subset.** CommitRift now kills a
worker running original TypeScript with SIGKILL at a checkpoint it
verifies, observes PostgreSQL ending the worker's sessions and the
committed state after the crash, starts the retry as a new process, routes
external effects through test doubles that outlive the worker, and compares
everything with the model. 21 first-party scenarios on node-postgres and
postgres-js (crash/retry counterexamples, safe alternatives, lock waits,
real 40001 retries) matched the model in **105 of 105** runs on
PostgreSQL 17.11 (macOS) and **63 of 63** on PostgreSQL 15.19 (Linux). Summaries are reused across depths and call instances,
schemas are read from Drizzle source and compared with real catalogs, and
the static analyzer's output is byte-identical to Phase 7 while deep-call
analysis is up to 25 % cheaper and model checking 2.0–2.4× faster.

Measured on 2026-10-26 on one Apple Silicon Mac (macOS, Darwin 27.0.0,
PostgreSQL 17.11, Node.js 22.23.2, Drizzle 0.45.4, node-postgres 8.23.1,
postgres-js 3.4.5) and in a Debian 12 container on the same machine
(see §2). Raw data: `reports/phase8/`.

## 1. Implementation

| area | change | document |
| --- | --- | --- |
| crash/retry runtime | plan steps `Crash`/`Restart`; checkpoints verified (announced-and-held operation, or lock wait confirmed by `pg_stat_activity`); SIGKILL of the process group; PostgreSQL session termination observed with timeout; committed rows after the crash read; one worker process per execution (attempt), new pid and connections; `client_connection_check_interval` for crash plans | [`CRASH-RETRY-SEMANTICS.md`](CRASH-RETRY-SEMANTICS.md) |
| external effects | `sourceEffect` through the application's provider seam to coordinator-hosted test doubles with the declared deduplication; effects survive kills | same |
| model integration | effects recorded as actions; crash predictions (next operation, committed rows), restart predictions, effect/attempt/crash comparison, invariant on observed rows **and** effect log; recovery probes for safe variants; evidence classification; report schema 3 | [`ORIGINAL-CODE-RECOVERY.md`](ORIGINAL-CODE-RECOVERY.md), [`DIFFERENTIAL-VERIFICATION.md`](DIFFERENTIAL-VERIFICATION.md) |
| application retry loops | `extraction.transactionRetries` binds a helper that re-runs a transaction after 40001/40P01 to the model's `Begin { retry }`; the loop itself runs | same |
| postgres-js | adapter on the same coordinator: hold in `Query.prototype.handle` (version-pinned), driver-internal queries reported, `SET TRANSACTION` with its BEGIN, lock waits by `application_name` | [`POSTGRES-JS-ADAPTER.md`](POSTGRES-JS-ADAPTER.md) |
| shared semantic facts | `analyzer::semantic`: chain decomposition and syntax facts shared by the static lowering, the concurrency extractor and the schema extractor | [`SEMANTIC-FACTS.md`](SEMANTIC-FACTS.md) |
| summaries | depth-canonical inliner memo with exact counter replay; path summaries instantiated across corresponding call instances (`PathMode::Shared`, default) | [`PARAMETERIZED-SUMMARIES.md`](PARAMETERIZED-SUMMARIES.md) |
| schema | Drizzle `pgTable`/`pgEnum`/`pgSchema` facts without execution; scenarios read tables `from` source with conflicts and visible overrides; fact origins; catalog comparison (`commitrift schema`, and in every source run) | [`SCHEMA-EXTRACTION.md`](SCHEMA-EXTRACTION.md), [`SCHEMA-FIDELITY.md`](SCHEMA-FIDELITY.md) |
| optimization | copy-on-write explorer state (locals, row chains), no IR clone per step, fast hashing; shared IR strings, cached per-summary facts | [`STATE-SPACE-OPTIMIZATION.md`](STATE-SPACE-OPTIMIZATION.md) |
| security | per-worker rlimits, run-token sweep of detached processes, worker self-exit, coordinator `Drop`, `waitid(WNOWAIT)` before group kill | [`SECURITY.md`](SECURITY.md) |
| reports | HTML lane timeline (executions, PostgreSQL, channel doubles; ✕ SIGKILL, ↻ retry), schema facts and fidelity; JSON schema v3 | `docs/phase8/schemas/` |

Summary representation: an IR expansion of the callee with symbolic
parameter handles and values (Phase 5/7), memoized by `(callee, callback
context)` plus the depth only when the expansion depends on it; path
summaries are exit-grouped trace fragments (Phase 7) shared through a
verified node/transaction/variable bijection. Cache strategy: demand-driven
(expansions on request, path summaries on reach, shapes only for callees
called twice).

## 2. Correctness

| gate | macOS arm64 (PostgreSQL 17.11) | Linux aarch64 container (Debian 12, PostgreSQL 15.19, Node 22.23.4, bubblewrap 0.8.0) |
| --- | --- | --- |
| source digest (`scripts/phase8-digest.py`) | `d371ba90935868cc5bc3a5c12b5a31ce9bc8f9a8a027626d9a0d1f5b9aefd4a3` (465 files) | identical: `d371ba90…` (copied tree, `.git` kept) |
| `cargo fmt --all -- --check` | clean | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean | clean |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace` | **452 passed, 0 failed, 0 ignored** (Phase 7: 412); Node runtime tests 12/12 | **452 passed, 0 failed, 0 ignored** (third attempt; see below) |
| `cargo build --release` | ok | ok |
| original-code evidence (`scripts/phase8-evidence.py`) | 21 scenarios × 5 runs: 105/105 followed and matched | 21 scenarios × 3 runs: 63/63 followed and matched, same evidence level per scenario |
| report schemas | 21 v3 reports, 0 errors, corrupted copies rejected | 21 v3 reports, 0 errors |

The Linux column comes from `scripts/phase8-linux-docker.sh`
(`reports/phase8/linux/`). Docker Desktop was started for these runs and
quit afterwards; the image was the cached Phase 4 image, and the
container's `npm ci` fetched the fixtures' locked packages from the npm
registry. The tests and source runs themselves ran without network.

New tests: model `phase8.rs` (5: probes, crash plans at effects and lock
waits, refusal of autocommit-after-release), replay `source_lifecycle.rs`
(6: planned kill, restart + dedup, failed restart, hang, CPU limit,
detached grandchild), analyzer `phase8.rs` (8: schema facts, no execution,
object-form constraints, scenario linking, conflicts and overrides,
partial/NULLS NOT DISTINCT, retry-helper binding with and without binding)
and `semantic` unit tests (3), CLI `phase8.rs` (18: every scenario family
on original code, divergence at the crash point, authorization, no invented
retry, schema agreement and drift on PostgreSQL, postgres-js parity,
crashes/lock waits/40001 on postgres-js, version refusal, POR soundness on
all Phase 8 models, HTML escaping and timeline), JS `postgres-js.test.js`
(3). Existing tests changed only mechanically (a field added to
`VerifyRequest`, `ExtractRequest`, `RunRequest`; label fields read through
`&*`; phase 8 fixtures excluded from the single-file fixture test like
phase 7's).

**Problems found and fixed during Phase 8** (each covered by a test):

1. A crash scheduled after the blocker released its lock was labeled "not
   executed" although PostgreSQL had already run the statement: crash
   checkpoints now distinguish waiting (confirmed in `pg_stat_activity`)
   from possibly-executed, and refuse the autocommit case.
2. With PostgreSQL's default settings a backend blocked on a lock did not
   notice its killed client (> 10 s): crash plans set
   `client_connection_check_interval`; without it the run is reported
   inconclusive, never assumed rolled back.
3. Drizzle's ROLLBACK after a failure was expected at the failing
   statement's source; it is issued by the transaction call.
4. A retried BEGIN lost its caller frame across `return helper()`; the
   worker now restores unambiguous chains from the same execution.
5. postgres-js's connection-time catalog query was held and blocked its
   own connection; driver-internal queries now pass through (reported).
6. postgres-js row counts (`count`) were not reported (node-postgres uses
   `rowCount`).
7. Reports with only postgres-js fields were labeled schema 2; caught by
   the schema validator.
8. The first shared-summary version hashed every call and cost as much as
   it saved; gated and cached.
9. Labels for a recovery probe without crash would have claimed "recovery";
   such probes are `ORIGINAL_CODE_INVARIANT_HELD`.
10. (Linux, first attempt — `reports/phase8/linux-attempt1/`) the Phase 8
    container script copied the tree without `.git`; the isolation backend
    finds the project root by the Git work tree, so inside bubblewrap the
    `file:` dependency on `runtime/node` was invisible and **every**
    source run was blocked. The evidence script still exited 0 with 0 runs
    per scenario. Fixed: `.git` is kept (as in the Phase 7 script), and
    `scripts/phase8-evidence.py` now exits 1 unless every scenario ran N
    times, all followed and matched. The container's `cargo test` now runs
    with `--no-fail-fast` (the first attempt stopped at the first failing
    test binary).
11. (Linux, second attempt — `reports/phase8/linux-attempt2/`)
    `a_detached_grandchild_does_not_survive_the_run` asserted the macOS
    mechanism (the run-token sweep kills the grandchild's host pid). Under
    bubblewrap the worker has its own PID namespace: the grandchild's
    reported pid is namespace-local and the kernel kills it when the
    namespace ends. The test now asserts the property on both platforms —
    no process carrying the grandchild's marker survives (`/proc` on Linux,
    `ps` on macOS) — and the sweep mechanism only on macOS.

**Environment incident (macOS).** During the final macOS rerun an
external disk cleanup outside this work deleted every fixture's
`node_modules` (and emptied `target/`; the runs used a separate target
directory). All source runs were then `blocked` — reported as blocked,
never as passing: the evidence script exited 1, 19 tests failed
(`reports/phase8/final-cargo-test-deps-deleted.txt`,
`evidence-run-deps-deleted.log`). The locked dependencies were reinstalled
(`npm ci --ignore-scripts`, the same versions) and everything was rerun.
In that rerun one test failed because it ran before the evidence
rerun restored the report it reads: `the_html_report_escapes_…` takes
`reports/phase8/model-check/checkout-crash-after-charge.json` as input
(`final-cargo-test-stale-report.txt`). The macOS numbers in the table are
from a further full run at the final digest.

**Not fixed — a flaky Phase 6 test.** In the second Linux run the Phase 6
oracle test `deadlock_workloads` failed once: PostgreSQL aborted the other
transaction than the one the model names as deadlock victim (the oracle
steers the victim by timing; Phase 8 did not change this code). It passed
in the third full run, in 20 of 20 targeted repetitions in the same
container at the final digest (`reports/phase8/linux-deadlock-repeat/`),
and in every macOS run. It is recorded in
[`LIMITATIONS.md`](LIMITATIONS.md), not hidden or retried away.

No disagreement between model and original code remained in any of the
105 macOS and 63 Linux runs.

## 3. Runtime verification

| | |
| --- | --- |
| original-code scenarios | 21 (shop-app 13, pgjs-app 6, team-app 2) |
| crash/retry counterexamples reproduced (`ORIGINAL_CODE_CRASH_REPRODUCED`) | 5: double charge after a lost response (node-postgres and postgres-js), lost event of a dual write, duplicate publication by a restarted outbox relay, duplicate receipt after redelivery |
| safe/idempotent alternatives verified with a real crash and retry (`ORIGINAL_CODE_RECOVERY_REPRODUCED`) | 7: idempotent checkout, outbox no-loss, claimed-effect redelivery, crash while waiting and crash of the lock holder (both drivers) |
| other counterexamples on original code | 6: concurrent duplicate claims, duplicate warehouse notice after a real 40001 retry (both drivers), invite double submit and accept race on postgres-js, the idempotent checkout with two different request ids |
| invariant held on a probe without crash (`ORIGINAL_CODE_INVARIANT_HELD`) | 3: claimed effect with concurrent consumers, notify after commit through a real 40001 retry, exhausted retries |
| repetitions | 5 per scenario on macOS (105 runs, all matched); 3 per scenario in the Linux container (63 runs, all matched) |
| PostgreSQL | 17.11 (macOS), 15.19 (Linux) |
| drivers | node-postgres 8.23.1, postgres-js 3.4.5, Drizzle 0.45.4 |
| crash checkpoint evidence | per crash: pid, `killedBySignal`, the held operation that was never sent (or the confirmed lock wait), sessions ended (15–50 ms), committed rows equal to the model's |
| retry evidence | per restart: new pid ≠ previous, attempt number, previous outcome, mechanism (declared) |
| persistent state | final modeled rows equal to the model's in every run; schema fidelity `agree` on every run database read from source |

## 4. Performance

[`PERFORMANCE.md`](PERFORMANCE.md):

* Static analysis, Phase 7 → Phase 8, same findings: school-ledger total
  −6 % (depth 3) to −15 % (depth 8); summary construction −13 % to −25 %;
  path analysis −7 % to −19 %; midday summary construction −7 % to −16 %;
  expansions −20 % to −36 %; memo hit rate 65 → 75 % (school-ledger,
  depth 8); peak RSS −4 to −10 MiB at depths 4–12.
* Model checking, identical states: 4 actors 97 → 48 ms, 5 actors 936 →
  427 ms, 6 actors 9.0 → 3.8 s; reduction removes 57–61 % of
  transitions; peak memory unchanged.
* Verification: ≈ 2.0–2.4 s per original-code run; a crash and retry add
  ≈ 0.1–0.2 s; postgres-js costs the same as node-postgres.

## 5. Real-world validation

* **Confirmed original-code first-party cases**: 21 scenarios above
  (first-party fixtures written to contain these patterns — evidence about
  the patterns, not about external applications).
* **Model-only cases**: none new.
* **Adapted reproductions**: none; the pgjs-app reuses the team-app actions
  unchanged with saas-starter's client structure.
* **External application findings**: saas-starter — schema read
  statically confirms the missing `UNIQUE(team_id, email)`; the
  application and its migrations were **not executed** (not authorized);
  the finding remains a Phase 6/7 model counterexample confirmed for model
  SQL only. Static analysis of all four corpus projects is unchanged.
* **Independent human reviews**: none. 10 blind review packets for the
  first-party recovery findings are prepared and unreviewed.
* **Cases requiring further evidence**: any external application's crash
  behavior; real providers' idempotency (only declared doubles were used);
  servers without `client_connection_check_interval`.

## 6. Definition of done

| # | requirement | status |
| --- | --- | --- |
| 1 | Phase 1–7 tests remain covered | yes (all pass; four mechanically adapted) |
| 2 | shared semantic facts reduce duplicated recognition | yes, for chain decomposition and syntax facts; values/handles/completion not migrated |
| 3 | parameterized summaries reused across call sites | yes (inliner across depths; path summaries across instances) |
| 4 | instantiation preserves transaction and failure semantics | yes (22 022 analyses × 4 depths, 0 differences; byte identity) |
| 5 | deep-call analysis benchmarked and improved | yes (up to −25 % summary construction, −15 % total) |
| 6 | Drizzle schema extracted without executing code | yes |
| 7 | extracted constraints compared with a real PostgreSQL schema | yes (agreement and injected drift) |
| 8 | actual process termination | yes (SIGKILL, verified) |
| 9 | at least one real retry/redelivery mechanism | yes: the application's own 40001 retry loop runs; client/queue/supervisor retries as declared contracts executed by the coordinator |
| 10 | a model crash/retry counterexample checked against original code | yes (5) |
| 11 | a first-party crash/recovery violation reproduced | yes (5/5 each) |
| 12 | a safe alternative verified within the same bounds | yes (7) |
| 13 | postgres-js adapter | yes (3.4.5) |
| 14 | `pg` source verification still works | yes (Phase 7 tests unchanged) |
| 15 | unknown/in-flight outcomes not reported as certain | yes (checkpoint rules, inconclusive on unobserved termination) |
| 16 | POR regression and independence tests valid | yes (+ Phase 8 models) |
| 17 | diagnostics and reports compatible | yes (byte-identical static output; schema 1/2 reports unchanged; 3 only with Phase 8 fields) |
| 18 | performance measured | yes |
| 19 | security and authorization intact | yes (strengthened) |
| 20 | reports distinguish model, PostgreSQL and original-code evidence | yes |

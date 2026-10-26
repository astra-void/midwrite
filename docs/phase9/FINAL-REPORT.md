# Phase 9 final report

Phase 9 makes CommitRift treat an operation whose acknowledgment is lost
(a COMMIT or autocommit write on the wire when the client dies) as what it
is: an outcome the client does not know, which the server may complete
before, during or after the client's recovery. This report records what was
implemented, what the finalization verified, and what remains open. Every
number comes from a file under `reports/phase9/`; nothing is estimated
unless it says so.

**Snapshot.** Base commit `1e51c71` plus the uncommitted finalization
changes listed below; source digest (`scripts/phase8-digest.py`, 513 files)
**`df9f2a6d6da35d6fa107e69a91b2ece1acd45ec1235804dd96dba7bdfd15a832`**; final
release binary sha256 `c2f4d6d4…`. Nothing was committed or pushed.

## 1. Finalization changes

The audit changed code in three places. Each change has a test that
fails without it:

| change | why | test |
| --- | --- | --- |
| `engine.rs` `claim_lease` records `Access::Pool` | **DPOR soundness bug**: a task waiting for a pooled connection was not ordered after the release that handed it over, so DPOR missed 1 of 4 terminal states on "3 tasks, pool of 2" (verdict unaffected there) | `dpor.rs` `in_flight_and_pool_workloads_keep_every_terminal_state` (new; 10 workloads) |
| `keytable.rs` test seam `intern_hashed` | the old collision test never produced equal hashes | `colliding_hashes_do_not_merge_states` (now 600 keys with one 64-bit hash), `slot_collisions_do_not_merge_states` |
| `phase9.rs` test | HTML escaping of every string field of fresh v4 reports | `every_string_of_a_fresh_phase9_report_is_escaped` |

New scripts: `phase9-report-check.py` (schema + semantic validation with
negative tests), `phase9-final-tables.py`, `phase9-profile-bench.py`,
`phase9-linux-docker.sh`.

## 2. Implemented features (implementation and tests both complete)

* **In-flight operation lifecycle**: an operation submitted and not
  acknowledged becomes an orphan the server completes in a separate
  `ServerComplete` transition, at any later point (`OPERATION-LIFECYCLE.md`,
  `FORMAL-MODEL.md`).
* **Physical state and client knowledge kept apart**: `Ambiguity`
  (physical world, possible worlds) is part of the canonical state; worlds
  are explicit branches and are never eliminated by an observation; the twin
  analysis reports what the client could observe (`EPISTEMIC-SEMANTICS.md`;
  `states_that_differ_only_in_unresolved_outcomes_are_not_merged`,
  `reading_durable_state_does_not_make_the_retry_safe`).
* **Protocol fault injection** on node-postgres: three fates,
  `not-reached`, `answered-and-withheld`, `held-for-later-delivery`
  (`PROTOCOL-FAULT-INJECTION.md`).
* **Original-code reproduction** of in-flight failures, shared-process task
  crashes and pool starvation for the first-party `ledger-app`.
* **Shared processes and bounded pools** in the model (FIFO leases; crash
  kills every task), stuck-state classification `PoolStarvation`,
  `ApplicationWaitCycle`, `Unexplained` (database deadlocks are resolved by
  a `Deadlock` transition and never reported as stuck).
* **DPOR** (`--dpor`, opt-in, classic stateless, refused with `always`
  invariants and `maxContextSwitches`).
* **Compact visited-state storage**: interned fragments, varint key table,
  exact byte comparison.
* **Script-free HTML timelines**, **report schema v4** (additive; reports
  without Phase 9 features keep schema 1–3).

## 3. Correctness: quality gates

| gate | macOS arm64 (final snapshot) | Linux container (same digest) |
| --- | --- | --- |
| `cargo fmt --all -- --check` | exit 0 | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 | exit 0 |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace` | exit 0: **524 passed, 0 failed, 0 ignored** | exit 0: **524 passed, 0 failed, 0 ignored** |
| `cargo build --release` | exit 0 | exit 0 |
| Phase 9 original-code evidence | 10 scenarios × 5 runs, all followed and matched | 10 scenarios × 3 runs, all followed and matched |
| report schema + semantic check | 51 reports valid, 394 corruptions rejected; + 21 Phase 8 regression reports | 11 reports valid, 194 corruptions rejected |

Environment, macOS: Darwin 27.0.0 arm64, rustc 1.98.1, Node 22.23.2,
PostgreSQL 17.11 (Homebrew). Linux: Debian 12 (bookworm) container on Docker Desktop (linuxkit 7.0.12, aarch64), rustc 1.99.1, Node 22.23.4, PostgreSQL 15.19, bubblewrap 0.8.0, tests as a non-root user.
Test count history: Phase 8 baseline 452; Phase 9 at `1e51c71` 521; final
524 (+3 above). The interrupted and pre-fix gate runs are kept in
`reports/phase9/final/aborted-run/` and `pre-fix/` (the pre-fix run passed
521/521 before the fix).

No failure of any kind occurred in the final runs: no build failure, no
test failure, no infrastructure failure, no timeout. Docker Desktop was not
running at the start of the session and was started; that is the only
infrastructure intervention.

## 4. PostgreSQL evidence (`POSTGRESQL-VALIDATION.md`)

First-party `ledger-app`, drizzle-orm 0.45.4 / `pg` 8.23.1, PostgreSQL
17.11, 5 runs per scenario, 50 runs in total, 0 inconclusive, 0 diverged,
0 statements outside the model:

| ambiguous outcome | scenario | result |
| --- | --- | --- |
| A. COMMIT not received | `ledger-idempotent-commit-not-reached` | proxy held and discarded the COMMIT; backend `idle in transaction` before the kill; session ended 16 ms after it; durable ledger empty; no ack; idempotent retry wrote 1 row — 5/5 |
| B. COMMIT completed, answer withheld | `ledger-blind-commit-in-flight` | proxy observed `CommandComplete COMMIT` and withheld it; worker SIGKILLed; 1 payment durable (read from PostgreSQL after the session ended); blind retry → 2 payments — 5/5 |
| C. COMMIT delivered after recovery started | `ledger-reconciled-commit-in-flight` | COMMIT held with the session open; durable ledger empty after the kill; the restarted client's reconciliation read returned **no row**; then the held COMMIT was delivered and completed (1 row); the retry inserted → 2 payments — 5/5 |
| autocommit write, answered | `ledger-autocommit-blind-in-flight`, `ledger-settle-update-answered` | durable despite the kill (`INSERT 0 1`; `orders.status = paid`) — 5/5 each |
| autocommit write, not reached | `ledger-settle-update-not-reached` | not applied (`unpaid`) — 5/5 |
| idempotency key in all three fates | `ledger-idempotent-commit-{not-reached,answered,held}` | 1 row each — 5/5 each |

Recovery strategies (model = bounded exhaustive search; original code = one
schedule per run):

| strategy | model | original code |
| --- | --- | --- |
| blind retry | unsafe | unsafe (B) |
| lookup-then-insert | **unsafe** (the delayed completion) | **unsafe** (C) |
| unique idempotency key + `ON CONFLICT DO NOTHING` | no counterexample within bounds | holds in all three fates |
| lookup + unique key, concurrent second delivery | no counterexample (305 states) | not run |
| lookup only, concurrent second delivery | unsafe without any fault | not run |
| transactional outbox | no counterexample with an in-flight fault (72 states) | Phase 8 crash probe only |

## 5. Original-code verification: scope

Everything above ran the **first-party** fixture, written for this
purpose. No external application was executed in Phase 9. The pinned
external corpus was only analyzed statically (§9). The saas-starter finding
remains a model counterexample confirmed with model SQL, as in Phase 8.
Model-only results are labelled `model-counterexample` / "no
counterexample within bounds" and are never reported as original-code
verification.

Shared process and pools on original code: three tasks of one process
(one pid) killed by one SIGKILL, with one task's INSERT answered and
withheld — 5/5. Pool starvation with a pool of 1 was confirmed by
quiescence (803 ms), pool counters (1 waiting, 0 idle of 1) and 0 lock
waits — 5/5. **Not run on original code**: pool capacity 2 (model only, no
counterexample), an application wait cycle, and a real PostgreSQL deadlock
in the ledger app (PostgreSQL deadlocks are covered by the model-SQL replay
matrix, not by original code).

## 6. DPOR

* **Differential suite** (`tests/dpor.rs`, release, final build): 14
  targeted workloads, 10 in-flight/pool workloads (new) and seeds 1–700 —
  682 compared, 18 skipped because one search hit the 2 M-state limit.
  Terminal-state sets, verdicts, counterexample existence, stuck states,
  unsupported behavior and step truncation equal the unreduced search; DPOR
  counterexamples replay on the model. 7/7 tests pass.
* **Phase 9 scenarios, three modes**: the 21 scenario/bound combinations
  of `reports/phase9/final/reduction-modes.md` (Phase 9 scenarios,
  recovery-strategy scenarios, Phase 8 delivery scenarios with one in-flight
  fault) give the same verdict and the same counterexample step kinds under
  full search, sleep sets and DPOR.
* **Bug found and fixed**: refinement 5 in `DYNAMIC-POR.md` (pool hand-over
  not in happens-before). The random generator produces no pools, which is
  why 700 seeds never caught it.
* **Dependencies audited**: crashes, in-flight crashes, retries and deadlock
  victims are `Global`; lost acknowledgments use the actor plus the shared
  fault budget; `ServerComplete` uses the actor plus the rows it commits;
  pool arrival, release and (now) claim use `Pool(p)`; a blocked step is
  dependent with everything.
* **Restriction kept**: `always` invariants and `maxContextSwitches` refuse
  DPOR (it does not visit the intermediate states of skipped interleavings).
  No argument for lifting this was made.
* **Trade-off measured**: DPOR saved 0.1–0.5 MiB of peak RSS and executed
  up to ~1 500× more transitions (team-app, 4 actors: 30.4 s vs 35 ms). It
  stays opt-in.

## 7. Memory

Same workload, same states and transitions, peak RSS (`PERFORMANCE.md` §2):

| actors | states | Phase 8 | Phase 9 | ratio |
| ---: | ---: | ---: | ---: | ---: |
| 5 | 13 984 | 90.9 MiB | 12.1 MiB | 7.5× |
| 6 | 97 408 | 742.9 MiB | 26.7 MiB | 28× |
| 7 | 669 568 | 2 934.5 MiB | 118.5 MiB | 25× |
| 8 | 4 544 512 | not run (≈ 23.6 GiB of keys, extrapolated) | 532.4 MiB | — |

Exploration time was equal or 5–13 % lower. The key costs more CPU to build
(24.5 % vs 17.8 % of samples) and much less to look up (1.5 % vs 8.7 %).
Exactness was tested with forced 64-bit hash collisions (above) and by
comparing against the flat encoding (`phase9_storage`).

## 8. Report schemas and HTML

* `scripts/phase9-report-check.py` on every generated Phase 9 report and
  every committed v1–v3 model-check report: all valid; **394 corruptions
  rejected, 0 accepted** (missing fields, wrong numeric types, invalid
  enums, contradictory evidence labels, verdict without a counterexample,
  fate mismatches, an acknowledged in-flight operation, truncated proxy
  observations, unknown wire events, a connection without `Connected`,
  malformed task references, impossible transitions, restarts of
  never-killed pids, a Phase 9 field in a v3 report). The 21 Phase 8
  regression reports produced by the final binary are schema 3 and valid
  (Phase 9 fields absent).
* HTML: `every_string_of_a_fresh_phase9_report_is_escaped` replaces every
  string and map key of four fresh v4 reports that the report type accepts
  (231–402 per report in a `--nocapture` run during the audit; scenario,
  actor, task and process names, source
  paths, SQL, wire events, problems, invariant names, knowledge) with a
  numbered `<script>`/`<img>` payload: no raw element in the page, the CSP
  `script-src 'none'` present. The test reads no committed report.
* Wire log: no parameter values, rows, ordinary SQL or authentication data;
  `req-1` occurs 0 times in the 11 final wire logs.

## 9. Compatibility

* Static analysis: **144/144** outputs (4 projects × 3 depths × 3 path modes
  × json/sarif/text/trace) byte-identical to Phase 8 (`CORPUS-EVALUATION.md`).
* Model checking: **40/40** Phase 6–8 scenarios identical apart from
  `visitedBytes`.
* Phase 8 original-code regression with the final binary: 21/21 scenarios,
  3/3 runs each followed and matched; same verdict, evidence level and
  observed invariant as the Phase 8 report.

## 10. Cross-platform

`scripts/phase9-linux-docker.sh reports/phase9/linux` (the Phase 8 isolation
model unchanged: `seccomp=unconfined` and `systempaths=unconfined` only for
bubblewrap's user namespaces, no added capabilities, non-root user, the
repository mounted read-only and copied in with `.git`).

* **Source digest in the container: `df9f2a6d…`, identical to the macOS
  snapshot** (`reports/phase9/linux/digest.txt`, 513 files).
* fmt, clippy, the full workspace test suite with `COMMITRIFT_REQUIRE_REPLAY=1`
  (524 passed, 0 failed, 0 ignored, nothing skipped) and the release build:
  exit 0. The suite includes the proxy tests, node-postgres runtime
  verification, process crash/restart, shared-process, pool, in-flight
  COMMIT, HTML and DPOR differential tests.
* Phase 9 original-code evidence on PostgreSQL 15.19 under bubblewrap
  (every report records `bubblewrap: new user, network (loopback only), PID,
  IPC, UTS and cgroup namespaces`): 10 scenarios × 3 runs, all followed and
  matched, the same verdicts, fates and evidence levels as on macOS
  (PostgreSQL 17.11); the reconciliation race (C) reproduced 3/3.
  `ledger-pool-of-two` has nothing to run on either platform.
* Report check: 11 reports valid, 194 corruptions rejected.
* Not compared across platforms: timings (the container is a VM),
  benchmarks, the corpus identity (the private projects are not in the
  container) and the Phase 8 regression evidence (macOS only in this
  finalization; Phase 8 ran it on Linux).

Cross-platform equivalence is therefore claimed for the test suite and the
Phase 9 evidence at one source digest, on PostgreSQL 15 and 17.

## 11. Known limitations

* **postgres-js in-flight**: not supported (no proxy integration); only its
  Phase 8 crash/retry scenarios are reproduced.
* **`LostCommitted` / `LostNotCommitted`** (live process, broken
  connection): explored in the model, refused at runtime (UNSUPPORTED).
* **Retries of an actor in a shared process**: refused (UNSUPPORTED).
* **Pool capacity 2, application wait cycles, real deadlocks**: not run on
  original code (§5).
* **Pool arrival at start-up** follows declaration order; other start-up
  orders are not explored (`LIMITATIONS.md`).
* **DPOR** is evidence-checked, not proved. A soundness bug was found during
  finalization and fixed. Pool coverage rests on targeted workloads only.
* **Recovery probes** are one schedule per run; "no counterexample" is
  bounded.
* **Network behavior** is limited to three fates (no delay distributions,
  reordering, duplication, partial frames); one in-flight operation per
  connection; held COMMITs complete atomically.
* **Real-world review**: no independent human review of any finding; the
  Phase 8 review packets remain unreviewed; Phase 9 adds no external
  application evidence.
* **`claim-skip-locked-2/serializable`**: not reproduced in 10 further runs.
  It remains an unresolved intermittent failure; the autovacuum hypothesis is
  untested (`TEST-RELIABILITY.md`).
* Two Phase 7/8 HTML tests read committed generated reports as input.

## 12. Remaining research problems

1. **Runtime realization of a broken connection with a live process**
   (`LostCommitted`/`LostNotCommitted`). The model's most general fault has
   no original-code evidence. It needs the proxy to sever a connection
   without killing the worker, and the driver's error path to be observed.
2. **postgres-js and other drivers** under the proxy. The fault model is
   driver-independent; the evidence is not.
3. **External applications**. All in-flight evidence comes from a fixture
   written for it. The open question is whether real code bases contain
   reconcile-then-retry patterns that the delayed-completion race breaks.
   That needs authorized execution and human review.
4. **DPOR that pays off**. Classic DPOR without sleep sets executes 10³×
   more transitions than sleep sets plus the compact cache on these
   workloads. Source-/optimal-DPOR, or DPOR with sleep sets, would need a
   correctness argument for this transition system's state-dependent
   footprints. The pool bug shows how enabling dependencies escape a
   footprint relation designed for commutation.
5. **Pool generators for random differential testing**, and exploring
   start-up arrival orders.
6. **Server-side timeouts** (`idle_in_transaction_session_timeout`,
   `statement_timeout`) as fates of a held COMMIT.

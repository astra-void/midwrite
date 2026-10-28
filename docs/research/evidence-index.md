# Evidence index

Every measured result of Phases 2–15 that a reader might cite, with the
binary or source revision it came from, the environment, the baseline it
was compared with, its scope and limitations, and where the raw evidence
is. Method: [evaluation methodology](evaluation-methodology.md).

Rules for this page:

* Every number is copied from the named phase document or report file.
  Where a value is not recorded there, the cell says so.
* Results of **different revisions are never combined**. A row belongs to
  exactly one binary or source digest; comparisons name both sides.
* The categories are kept apart: static-analysis accuracy,
  candidate-discovery accuracy, model-checking correctness, PostgreSQL
  differential fidelity, original-code reproduction, performance.
* **No independent human review performed** in any phase. Labels and
  reviews were written by the implementer or by AI sessions; where an AI
  agent worked without access to the implementation, that is independence
  from the code, not from the project.
* School-ledger and unnamed-ai-project are private: only aggregate counts.

## Revisions and environments

| phase | base commit | source digest (`scripts/phase8-digest.py` unless noted) | release binary (sha256) | macOS environment | Linux container | tests (final) |
| --- | --- | --- | --- | --- | --- | --- |
| 2 | not recorded | not recorded | not recorded | Apple Silicon Mac | — | 156 passed |
| 3 | not recorded (no revision hash, Phase 4 baseline) | `digest.txt` text dump (285 files) | not recorded | Apple Silicon Mac, 2026-10-25 | — | 217 passed |
| 4 | none | 285 of 285 files identical to the Phase 3 digest at start | not recorded | Apple Silicon Mac, 2026-10-25 | Debian 12 (bubblewrap) | 245 passed (both) |
| 5 | none | not recorded | Phase 5 binary preserved: `71ad0af6…64ca` | 2026-10-25 | Debian 12, Linux 7.0.12 aarch64 | 300 passed (both) |
| 6 | not recorded | not recorded | Phase 6 binary sha in `reports/phase7/baseline/phase6-binary.sha256` | 2026-10-25, PostgreSQL 17.11 | Debian 12, PostgreSQL 15.19 | 381 passed (both) |
| 7 | baseline commit for Phase 8: `6ada341b` | baseline digest for Phase 8: `5bf91036…` (git-ls-files method) | Phase 7 binary `da3686c4…` | 2026-10-26, macOS 27.0.1, rustc 1.98.1, Node 22.23.2, PostgreSQL 17.11 | PostgreSQL 15.19, rustc 1.99.1 | 412 macOS, 411 Linux |
| 8 | `6ada341b` (+ uncommitted) | `d371ba90…` (465 files), identical on Linux | not recorded in the final report | 2026-10-26, Darwin 27.0.0, PostgreSQL 17.11, Node 22.23.2, Drizzle 0.45.4, pg 8.23.1, postgres-js 3.4.5 | Debian 12, PostgreSQL 15.19, Node 22.23.4, bubblewrap 0.8.0 | 452 passed (both) |
| 9 | `1e51c71` (+ uncommitted) | `df9f2a6d…` (513 files), identical on Linux | `c2f4d6d4…` | Darwin 27.0.0 arm64, rustc 1.98.1, Node 22.23.2, PostgreSQL 17.11 | Debian 12, rustc 1.99.1, Node 22.23.4, PostgreSQL 15.19, bubblewrap 0.8.0 | 524 passed (both) |
| 10 | `81d27e4` (+ uncommitted) | `59275b50…` (593 files), identical on Linux; benchmark and runtime evidence at `dac5dae3…` (differs only in a Python report checker) | `ec9c019f0a01ad1b…` | as Phase 9 | as Phase 9 | 555 passed, 2 ignored (both) |
| 11 | `accbcf5` (+ uncommitted) | `ad949949…` (640 files) | `0643249f1b9274ca…`; evidence produced by `6141d12d…` (before a reporting change) | as Phase 9 | **not run** | 583 passed, 3 ignored |
| 12 | `6657faf` (+ uncommitted) | `803fbbd1…` (702 files), identical on Linux | `7da740b2f5f3618f…` (Linux build `13366dec…`) | as Phase 9, PostgreSQL client tools 18.6 | as Phase 9 | 606 passed, 3 ignored (both) |
| 13 | `32efbca` (+ uncommitted) | baseline `803fbbd1…` (= Phase 12 final); final `dc3161e1…` (780 files; the evidence ran on `12453f6a…`: one script fix) | baseline `7da740b2…`; final `97e96ac0…` | Darwin 27.0.0 arm64, rustc 1.98.1, Node 22.23.2, PostgreSQL 17.11 | **not run** (Docker daemon unavailable) | run 1: 652 passed, 1 failed, 3 ignored; run 2: 653 passed, 0 failed, 3 ignored (`reports/phase13/final/`) |
| 14 | `32efbca` (+ uncommitted) | final `e486816b…` (879 files; run 1 on `88e074f4…`, which differs by one test expectation) | see `docs/phase14/FINAL-REPORT.md` §6 | as Phase 13 | **not run** (Docker daemon unavailable) | `reports/phase14/final/` |

Sources: each phase's `FINAL-REPORT.md` and `BASELINE.md`
([2](../phase2/FINAL-REPORT.md), [3](../phase3/FINAL-REPORT.md),
[4](../phase4/BASELINE.md), [5](../phase5/BASELINE.md),
[6](../phase6/BASELINE.md), [7](../phase7/FINAL-REPORT.md),
[8](../phase8/BASELINE.md), [9](../phase9/FINAL-REPORT.md),
[10](../phase10/FINAL-REPORT.md), [11](../phase11/FINAL-REPORT.md),
[12](../phase12/FINAL-REPORT.md), [13](../phase13/BASELINE.md)).

## 1. Static-analysis accuracy

`commitrift check` findings on real projects. No recall is claimed for
any row.

| phase | benchmark | binary | baseline | result | scope | limitations | raw evidence |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 2 | manual review of corpus findings | Phase 2 | Phase 1 (0 cross-resource findings in midday vs 49) | 103 findings reviewed, 97 with a decided label; window existed in 92; 27 judged likely bugs. Real window / likely bug (95 % Wilson): CP1001 (38) 0.97 (0.87–1.00) / 0.34 (0.21–0.50); CP1002 (8) 0.75 (0.41–0.93) / 0.25 (0.07–0.59); CP1004 (43) 0.98 (0.88–1.00) / 0.28 (0.17–0.43); CP1005 (8) 0.88 (0.53–0.98) / 0.00 (0.00–0.32) | saas-starter `6e33e58b`, midday `51587319`, two private projects | reviewer = Phase 2 implementer, from source; no maintainer confirmation; nothing reproduced | `reports/phase2/precision.txt`, `corpora/labels/` |
| 3 | blind automated re-review | Phase 3 | Phase 2 labels | 18 findings: 11/18 agreement (κ 0.38); 3 of 7 "likely bug" labels confirmed (midday 1/5) | sample of Phase 2 findings | a separate model session, not a human | `reports/phase3/blind-review.txt`, `blind-review-reasons.json` |
| 4 | human review workflow | Phase 4 | — | 0 human reviews; review packets for 12 midday findings prepared | — | unreviewed | `reports/phase4/review/` |
| 8 | review packets | Phase 8 | — | 10 blind review packets for first-party recovery findings, unreviewed | — | unreviewed | `reports/phase8/review-packets/` |

## 2. Candidate-discovery accuracy

`commitrift discover`, scored against labeled entries. "Model prediction"
= an entry has its own candidate whose actual scenario has a validated
model counterexample. "Static" = a hazard candidate exists.

| phase | benchmark | binary | result (TP / FP / FN / TN, precision / recall) | held out? | limitations | raw evidence |
| --- | --- | --- | --- | --- | --- | --- |
| 11 | first-party commerce application (8 pos / 10 neg / 3 unsupported) | Phase 11 evidence binary `6141d12d…` | model 8 / 1 / 0 / 9 (0.889 / 1.000); static 0.800 / 1.000; unsupported 3 of 3 reported; class agreement 7 / 9 | no — same author as the engine, written with the detector families in mind (an upper bound) | not independent | `reports/phase11/evidence/SUMMARY.md` |
| 11 | held-out Phase 7/8 fixtures (8 pos / 4 neg), **first scoring** | same | 4 / 2 / 4 / 2 (0.667 / 0.500) | yes (first scoring only) | — | `reports/phase11/eval/heldout-first-scoring.md` |
| 11 | held-out set after fixes it prompted | same | 7 / 0 / 1 / 4 (1.000 / 0.875) | **no** | post-fix | `reports/phase11/evidence/heldout-*.json` |
| 12 | Phase 12 evaluation set (15 entries, frozen), **first scoring** | Phase 12 | 4 / 0 / 3 / 6 (1.000 / 0.571) | fresh, frozen; same author as the implementation | not independent; original-code reproduction not measured (no harness) | `reports/phase12/eval/first-scoring.json`, `FROZEN.md` |
| 12 | same, after fixes it prompted | Phase 12 | 5 / 0 / 2 / 6 (1.000 / 0.714) | **no** | post-fix | `reports/phase12/eval/post-fix-scoring.json` |
| 13 | Phase 13 frozen set (27 entries, manifest `0a00594d…`) | Phase 12 binary `7da740b2…` | model 2 / 0 / 13 / 11 (1.000 / 0.133); static 0.800 / 0.267 | frozen before scoring | AI-authored (no `crates/` access); not blind | `reports/phase13/eval/phase12-binary.json` |
| 13 | same, **first scoring** | Phase 13 tree | model 3 / 0 / 12 / 11 (1.000 / 0.200); static 0.833 / 0.333 | yes | as above | `reports/phase13/eval/first-scoring.json` |
| 13 | same, after one change it prompted (`.for("share")` lowering) | Phase 13 final tree | model 4 / 0 / 11 / 11 (1.000 / 0.267); static 0.833 / 0.333 | **no** (post-feedback) | as above | `reports/phase13/eval/post-feedback-1.json` (reproduced by the final binary: `final-binary.json`) |
| 14 | Phase 13 frozen set | Phase 14 tree | model 7 / 0 / 8 / 11 (1.000 / 0.467); static 0.800 / 0.533 | **no** (used during development) | as above | `reports/phase14/eval/phase13-frozen-set.json` |
| 14 | Phase 14 set (38 entries, 12 families; `FROZEN.json` sha256 `778117d2…`) | Phase 13 final binary `97e96ac0…` | model 2 / 1 / 16 / 17 (0.667 / 0.111) | frozen before scoring | AI-authored without `crates/` access; not blind | `reports/phase14/eval/phase13-binary.json` |
| 14 | same, **first scoring** | Phase 14 tree `465e12d2…` | model 0 / 0 / 18 / 18 (— / 0.000) | yes | as above | `reports/phase14/eval/first-scoring.json` |
| 14 | same, post-feedback 1 and 2 | Phase 14 tree | 3 / 1 / 15 / 17 (0.750 / 0.167); 4 / 0 / 14 / 18 (1.000 / 0.222) | **no** | as above | `reports/phase14/eval/post-feedback-{1,2}.json` |
| 15 | Phase 15 set (30 entries, 9 input-domain families; `FROZEN.json` manifest `b11cc1a1…`) | Phase 14 binary (rebuilt, `059b1150…`) | model 2 / 0 / 11 / 14 (1.000 / 0.154) | frozen before scoring | AI-authored without `crates/` access; not blind | `reports/phase15/eval/phase14-binary.json` |
| 15 | same, **first scoring** | Phase 15 build `fd0e291b…` | model 4 / 0 / 9 / 14 (1.000 / 0.308) | yes | as above | `reports/phase15/eval/first-scoring.json` |
| 15 | same, post-feedback (final) | Phase 15 final tree `ced40150…` | 8 / 0 / 5 / 14 (1.000 / 0.615) | **no** | as above | `reports/phase15/eval/post-feedback-final.json` |
| 15 | Phase 13 frozen set; Phase 14 set (regression) | Phase 15 final tree | 8 / 0 / 7 / 11 (E26 → TP); 5 / 0 / 13 / 18 (E21 → TP) | **no** | as above | `reports/phase15/regression/` |

Regression of earlier benchmarks under a newer binary (not new estimates):

| set | binary A | binary B | result | raw |
| --- | --- | --- | --- | --- |
| Phase 11 first-party | Phase 11 | Phase 12 | model 8 / 1 / 0 / 9, static 0.800 / 1.000 — unchanged | `reports/phase12/regression/phase11-benchmarks.md` |
| Phase 11 held-out (post-fix) | Phase 11 | Phase 12 | 7 / 0 / 1 / 4 — unchanged | same |
| Phase 11 first-party and held-out | Phase 12 | Phase 13 | unchanged | `reports/phase13/regression/earlier-benchmarks.md` |
| Phase 12 set (post-fix) | Phase 12: 5 / 0 / 2 / 6 | Phase 13: 6 / 0 / 1 / 6 | `recordSignup` found (response step); `claimTask` still FN (no declared return-value property) | same |

Stage measurements on the Phase 13 frozen set (post-feedback): schema
16/16 tables; 27/27 labeled entries extracted; 21 of 27 with a bound
write; 4 of 15 positives with a candidate of the labeled class; 0 false
positives ([Phase 13 evaluation § 3](../phase13/EVALUATION.md#3-separate-measurement-problems-f6)).
Per-case causes: [Phase 13 evaluation § 2](../phase13/EVALUATION.md#2-per-case-first-scoring--post-feedback).

## 3. Model-checking correctness

Reductions compared with the unreduced search; none is a proof.

| phase | check | binary / digest | result | scope | limitations | raw evidence |
| --- | --- | --- | --- | --- | --- | --- |
| 6 | sleep-set reduction vs full exploration | Phase 6 | equal reachable states on 14 workloads and 1 500 random scenarios (150 in the test gate); independence audit of every independent pair in every reachable state | 2–4 actors | 3 reduction bugs found by random seeds and the audit, fixed | test suite; [Phase 6 POR](../phase6/PARTIAL-ORDER-REDUCTION.md) |
| 9 | classic DPOR differential (release, final build) | `df9f2a6d…` | 14 targeted + 10 in-flight/pool workloads + seeds 1–700: 682 compared, 18 skipped (2 M-state limit); terminal states, verdicts, counterexamples, stuck states equal; 21 scenario/bound combinations equal under full, sleep and DPOR | random generator produces no pools | one soundness bug found in finalization (pool hand-over), fixed | `reports/phase9/final/dpor-release-700-seeds.txt`, `reports/phase9/final/reduction-modes.md` |
| 10 | adversarial generator, release, 3 000 seeds | Phase 10 tree, release-mode test run | 3 000 compared, 0 unverified, all searches agree (sleep sets, classic DPOR, three source-DPOR configurations) | 9 profiles, 14 required feature combinations, each covered by 125–538 models | evidence-checked, not proved | `reports/phase10/evidence/adversarial-3000-run1.txt` |
| 10 | metamorphic transformations; regression fixtures; dependence audit | Phase 10 (`59275b50…`; `dac5dae3…` differs only in a Python report checker) | 208 cases preserved; 7 fixtures handled by every algorithm; 144 workloads, 84 077 independent pairs, 31 286 enabling pairs, 0 violations | — | deleting `RowRead`, `LockCheck`, `Wait`, `Seq` or `Budget` accesses is noticed by no workload | [Phase 10 final report § 3](../phase10/FINAL-REPORT.md#3-correctness) |
| 10 | defects found | — | 4: two source-DPOR development defects, the Phase 9 classic DPOR "only the last race" defect, the Phase 9 pool-claim defect | — | — | regression fixtures |
| 11 | counterexample validity on generated scenarios | `6141d12d…` | commerce: 22 candidates, 31 scenarios, 781 states; every counterexample replays strictly and violates its invariant; every emitted scenario gives `model-check` the same verdict and state count | first-party | — | `reports/phase11/evidence/` |
| 13 | reductions with the new operations (FK checks, lock modes, upserts, replica reads/replication, response step) | Phase 13 tree | independence audit clean; sleep sets, classic DPOR and source-DPOR give the same terminal states, verdicts, stuck states and unsupported behavior as the unreduced search | 20 FK workloads × 3 levels, a three-actor FK workload × 3, claim/atomic-claim × 3, signup with response, 6 upsert workloads × with/without a row × 2 levels, two writers and a replica reader | no counts of states recorded in the doc | `crates/commitrift-model/tests/phase13.rs` |

## 4. PostgreSQL differential fidelity

The model's SQL replayed in lockstep on PostgreSQL (or, Phase 13, an
independent ground truth replayed in the model).

| phase | check | environment | result | scope | limitations | raw evidence |
| --- | --- | --- | --- | --- | --- | --- |
| 6 | isolation matrix | PostgreSQL 17.11 (macOS) and 15.19 (Linux) | 60 configurations, **1 695 schedules, 0 disagreements** (after two model corrections the tests forced); every fixture counterexample confirmed | RC, RR, SER; small tables | SERIALIZABLE: PostgreSQL aborted 60 of 70 schedules the model allows (sequential scans), never the reverse | `reports/phase6/postgres-matrix-macos.txt`, `reports/phase6/linux/` |
| 7 | joins, projections, sequences + rollback, `ON CONFLICT DO NOTHING … RETURNING` | PostgreSQL 17.11 (Drizzle extraction doc); "both versions" (final report) | **102 schedules, 15 configurations, 0 disagreements** | RC, RR, SER | SER + join validated only on small workloads | [Phase 7 Drizzle extraction](../phase7/DRIZZLE-EXTRACTION.md#validation) |
| 8 | schema facts vs real catalog | PostgreSQL 17.11 / 15.19 | `agree` on every run database read from source; injected drift detected | first-party schemas | CHECK and partial-index predicates compared by presence only | [Phase 8 schema fidelity](../phase8/SCHEMA-FIDELITY.md) |
| 12 | Drizzle/PostgreSQL probes; model vs PostgreSQL on the same TypeScript | PostgreSQL 17.11 and 15.19 | 10 probe programs; **6 / 6** supported programs end in the same rows; lockstep oracle with generated keys agrees | generated values, defaults, `RETURNING` | one model limitation found (FK locks) | `reports/phase12/differential/` |
| 13 | independent PostgreSQL lock/FK ground truth | PostgreSQL 17.11 (Homebrew), disposable cluster | **837 cells, 0 INCONCLUSIVE**; a second run from `cells.json` identical | 5 schema variants (base, email, cascade, deferred, nulls) | produced by an AI agent without access to the implementation | `reports/phase13/lock-matrix/`, `fixtures/phase13/lock-matrix/cells.json` |
| 13 | ground truth replayed in the model | Phase 13 tree | **707 of 837 cells, 2 726 labeled steps, 0 disagreements**; with a mutated compatibility table: 109 disagreements | 130 cells not replayed (102 cascade/deferred, 15 FK-deadlock, 7 string concatenation, 4 bare `BEGIN`, 2 conditional) | 2 model mismatches found and fixed first | [Phase 13 lock differential § 7](../phase13/LOCK-DIFFERENTIAL.md#7-model-comparison) |
| 13 | FK / lock-mode workloads, every schedule in lockstep | PostgreSQL 17.11 | **543 schedules, 0 disagreements** | 20 workload pairs × 3 levels | — | [Phase 13 model correctness § 1](../phase13/MODEL-CORRECTNESS.md#1-model-vs-postgresql-semantics) |
| 13 | upsert workloads, every schedule in lockstep | PostgreSQL 17.11 | **396 schedules, 0 disagreements** | 6 pairs × with/without a row × 3 levels | `targetWhere` not modeled | same |

## 5. Original-code reproduction

Original (unchanged) first-party code under the counterexample's schedule,
on a disposable PostgreSQL. **No external, corpus or private application
was executed in any phase.**

| phase | application(s) | environment | result | scope | limitations | raw evidence |
| --- | --- | --- | --- | --- | --- | --- |
| 3 | `fixtures/replay/order-vulnerable` (replay harness) | macOS, 2026-10-25 | CP1001 window `RUNTIME_REPRODUCED` 5 of 5 runs (SIGKILL after a verified commit); outbox variant does not reproduce at any of 5 failure points (25 runs) | single request | — | `reports/phase3/replay/` |
| 3 | `fixtures/replay/invoice-pattern` | macOS | isolated re-write of a midday finding's pattern, 5/5 | not midday | maintainers not contacted | same |
| 4 | `fixtures/replay/drizzle-node` (real Drizzle) | macOS and Linux | 16/16 cases as declared; generated harness 5/5 | single request | — | `reports/phase4/replay/` |
| 7 | `team-app` `inviteTeamMember`, `acceptInvitation` | macOS, PostgreSQL 17.11, Drizzle 0.45.4 | `original-code-reproduced`, 5/5 each | first-party; concurrency | saas-starter's original not executed (model SQL confirmed only) | `reports/phase7/model-check/` |
| 8 | shop-app 13, pgjs-app 6, team-app 2 scenarios | macOS 17.11 / Linux 15.19 | **105 of 105** runs (macOS, 5 each) and **63 of 63** (Linux, 3 each) followed and matched: 5 crash counterexamples reproduced, 7 recoveries, 6 other counterexamples, 3 invariant-held probes | node-postgres 8.23.1, postgres-js 3.4.5 | retries other than the app's own loop are declared contracts | `reports/phase8/model-check/SUMMARY.md`, `reports/phase8/linux/evidence.txt` |
| 9 | `ledger-app` (in-flight fates, shared process, pool starvation) | macOS 17.11; Linux 15.19 | 10 scenarios × 5 runs (50), 0 inconclusive, 0 diverged; Linux 10 × 3 all matched; Phase 8 regression 21/21 × 3 | node-postgres | pool capacity 2, wait cycles, real deadlocks not run on original code | `reports/phase9/evidence/SUMMARY.md`, `reports/phase9/linux/evidence.txt`, `reports/phase9/phase8-regression/` |
| 10 | live connection loss and postgres-js in-flight fixtures | macOS 17.11 (digest `dac5dae3…`); Linux 15 | 22 × 5 = 110 runs: **21 of 22** reproduce or recover 5/5; the no-error-handler variant diverges 5/5 as expected; Linux 22 × 3 same outcome | node-postgres 8.23.1 (13 scenarios), postgres-js 3.4.5 (9) | closed socket only; COMMIT/autocommit only | `reports/phase10/evidence/runtime-final/`, `reports/phase10/linux/phase10-evidence.txt` |
| 11 | commerce application, discovered counterexamples | macOS 17.11, binary `6141d12d…` | **10 of 10** reproduced 3/3 (9 `original-code-reproduced`, 1 `original-code-crash-reproduced`) | first-party | Linux not run | `reports/phase11/evidence/SUMMARY.md` |
| 12 | `workspace-app`, discovered | macOS 17.11 and Linux 15.19, digest `803fbbd1…` | **4 of 6** reproduced 3/3; **2 diverged** 0/3 (foreign-key check lock not modeled) — identical on both platforms | first-party | the divergence is the FK-lock limitation lifted in Phase 13 | `reports/phase12/evidence/SUMMARY.md`, `reports/phase12/linux/phase12-evidence.txt` |
| 13 | `workspace-app` FK pairs (direct and Server Action) | macOS 17.11, Phase 13 tree | `original-code-reproduced` **3/3 and 3/3** (Phase 12: 0/3) | first-party | macOS only | `reports/phase13/fk-slice/` |
| 13 | `request-app`: `claim-race`, `signup-crash`, `reserve-race` | macOS 17.11, Phase 13 tree | 3/3 each (`signup-crash`: `original-code-crash-reproduced`); 5/5 counterexample scenarios with 0 divergences in total | first-party | raw reports for these three not found under `reports/phase13/` at the time of writing | [Phase 13 model correctness § 3](../phase13/MODEL-CORRECTNESS.md#3-source--model--original-code-fidelity) |
| 13 | replica read-your-writes | model only | counterexample in the model | — | no replication run; refused for original code | `fixtures/phase13/model/replica-read-your-writes.json` |
| 13 (closeout) | every Phase 13 slice rerun with the final binary | macOS 17.11, `12453f6a…` | 3/3 for claim-race, signup-crash, reserve-race and the direct FK pair; **2/3** for the Server-Action FK pair (coordinator race, fixed in Phase 14) | first-party | — | `reports/phase13/evidence/` |
| 14 | `fixtures/phase14/request-app`: multi-row INSERT + lost response + resubmission; COMMIT lost inside `withRetry` | macOS 17.11, Phase 14 tree | `original-code-crash-reproduced` 3/3; `original-code-connection-loss-reproduced` 3/3 (with the stated client-error handler; without it: diverged, the worker dies) | first-party | macOS only | `reports/phase14/evidence/` |

## 6. Performance

Same-machine comparisons; both binaries are named. Timings from different
phases' runs are not comparable with each other unless a row says so.

| phase | measurement | binaries compared | result | raw evidence |
| --- | --- | --- | --- | --- |
| 6 | `check` on midday; model checker | Phase 5 vs Phase 6 | `check` 0.206–0.209 s on both, identical output; model checker 2–4 actors 0.2–68 ms; sleep sets cut transitions 28–46 % at 3–4 actors, wall time 10–20 % | `reports/phase6/perf/` |
| 7 | path analysis, deep call chains | Phase 6 vs Phase 7 | path analysis −32 % to −55 % (midday, school-ledger, depths 3–8), identical findings; `check` midday 0.230 s at depth 3 (unchanged), 0.230 s at depth 6 (Phase 6: 0.260 s); original-code run ≈ 1.9 s vs ≈ 1.1 s model-SQL replay | `reports/phase7/perf/` |
| 8 | static analysis; model checking | Phase 7 vs Phase 8 | school-ledger total −6 % (depth 3) to −15 % (depth 8); model checking 4 actors 97 → 48 ms, 5 actors 936 → 427 ms, 6 actors 9.0 → 3.8 s; original-code run ≈ 2.0–2.4 s | `reports/phase8/perf/` |
| 9 | visited-state memory, same states | Phase 8 vs Phase 9 | 5 actors 90.9 → 12.1 MiB; 6 actors 742.9 → 26.7 MiB; 7 actors 2 934.5 → 118.5 MiB; 8 actors (4 544 512 states) 532.4 MiB (Phase 8 not run) | `reports/phase9/model-bench.md` |
| 10 | reductions, team-app 4 actors | Phase 9 vs Phase 10 (final binary) | classic DPOR 3 476 614 transitions, 30 362 ms (Phase 9) / 37 973 ms (Phase 10); sleep sets 2 291, 44 ms; source-DPOR stateful+sleep 574, 12 ms; team-app 7: sleep sets 1 084 010 (30.8 s, 117.9 MiB) vs source-DPOR 190 830 (6.0 s, 32.7 MiB) | `reports/phase10/final/por-bench.md` |
| 11 | discovery | Phase 11 (`6141d12d…`) | commerce 0.06 s (0.08 s with model checking of 31 scenarios), ≈ 10 MiB; midday 30 s, 73 MiB, ≈ 98 % extraction; warm cache 7.7 s | `reports/phase11/evidence/SUMMARY.md` |
| 12 | discovery with model checking | Phase 11 vs Phase 12 | midday 28.07 → 1.13 s, extraction 27 323 → 873 ms, peak RSS 73 → 112 MiB; school-ledger 3.36 → 0.53 s; cache cold 34.39 → 1.54 s, warm 7.21 → 1.06 s | `reports/phase12/corpus/` |
| 13 | discovery with model checking | Phase 12 binary vs Phase 13 tree, same machine and input | midday 1.05 s → 1.22 s (no declaration) / 1.29 s (declared factory), peak RSS 111 → 113 / 114 MiB; school-ledger 0.54 s / 80 MiB → 0.64 s / 91 MiB | `reports/phase13/analyzer/corpus/` |
| 13 | state-space overhead of `foreign-keys` | Phase 13, feature on vs off | 1.19× states on 2 discovered workspace-app scenarios, 1.00× on the others | `reports/phase13/compat/state-space-overhead.md` |

Note: Phase 12 reported midday 1.13 s and school-ledger 0.53 s; Phase 13
re-measured the Phase 12 binary at 1.05 s and 0.54 s in its own runs. The
values come from different runs and are not merged.

## 7. Compatibility and regression identity

Each phase compares the previous binary with the new one on identical
inputs.

| phase | static analysis (4 projects × depths 3, 6, 12 × 3 path modes × json, sarif, text, trace) | model checking (Phase 6–9 scenarios) | raw evidence |
| --- | --- | --- | --- |
| 8 vs 7 | 144/144 identical | 17/17 identical (tool version aside) | `reports/phase8/corpus-identity.txt` |
| 9 vs 8 | 144/144 identical | 40/40 identical apart from `visitedBytes` | `reports/phase9/corpus-identity.txt`, `reports/phase9/model-identity.txt` |
| 10 vs 9 | 144/144 identical | 56/56 identical (default search) | `reports/phase10/final/corpus-identity.txt` |
| 11 vs 10 | 144/144 identical | 56/56 identical | `reports/phase11/final/` |
| 12 vs 11 | 144/144 identical | 56/56 same verdicts and counts; JSON byte-identical for 43; 13 differ in `visitedBytes` (9) and helper return-slot attribution (4) | `reports/phase12/final/` |
| 13 vs 12 | 144/144 identical (`check-identity.txt`) | 56 scenarios: 47 byte-identical JSON; 9 differ only in `visitedBytes` | `reports/phase13/analyzer/check-identity.txt`, `reports/phase13/compat/` |
| 13 final vs 12 | 144/144 identical | 57 scenarios: 48 byte-identical; 9 differ only in `visitedBytes` | `reports/phase13/final/` |
| 14 vs 13 final | 144/144 identical | 57/57 byte-identical | `reports/phase14/compat/` |

## 8. Real-world corpus coverage (static discovery)

Nothing was executed. Counts only; no finding is claimed as a defect.

| project | measurement | Phase 11 | Phase 12 | Phase 13 | Phase 14 |
| --- | --- | --- | --- | --- | --- |
| saas-starter | effectful entries / candidates / counterexamples | 8 / 2 / 1 (heuristic, model only) | 8 / 2 / no new counterexample | candidates and statuses unchanged | 9 / 2 / 1 (unchanged statuses) |
| midday `51587319` | tables modeled / effectful entries / candidates | 8 / 0 / 0 | 50 / 0 / 0 | with the declared replica factory: 47 effectful entries, 3 candidates (1 UNSUPPORTED, 2 BLOCKED), 0 model-checkable | declared factory: 81 effectful, 4 candidates (1 INCONCLUSIVE, 3 BLOCKED), 0 model-checkable |
| school-ledger (private) | tables modeled / effectful entries / candidates / model-checkable | 9 / 0 / 0 / 0 | 30 / 21 / 9 / 0 (all UNSUPPORTED) | 30 / 21 / 9 / 0 (7 INCONCLUSIVE, 2 UNSUPPORTED) | 70 effectful / 9 / 0 (7 INCONCLUSIVE, 2 UNSUPPORTED) |
| unnamed-ai-project (private) | tables modeled / effectful entries / candidates | 7 / 1 / 0 | 9 / 3 / 0 | unchanged | 3 effectful / 0 candidates |

Sources: [Phase 11 real-world evaluation](../phase11/REAL-WORLD-EVALUATION.md),
[Phase 12 real-world coverage](../phase12/REAL-WORLD-COVERAGE.md),
[Phase 13 midday coverage](../phase13/MIDDAY-COVERAGE.md),
[Phase 13 school-ledger coverage](../phase13/SCHOOL-LEDGER-COVERAGE.md),
[Phase 13 source resolution](../phase13/SOURCE-RESOLUTION.md#effect-on-the-corpora).
Raw: `reports/phase11/evidence/corpus-*.json`, `reports/phase12/corpus/`,
`reports/phase13/analyzer/corpus/`, `reports/phase13/final/corpus/`,
`reports/phase14/corpus/` ([Phase 14 real-world results](../phase14/REAL-WORLD-RESULTS.md)).

Phase 15 (`reports/phase15/corpus/`, Phase 14 vs Phase 15 binary): saas-starter
unchanged; midday (declared factory) unchanged (4 candidates: 1
INCONCLUSIVE, 3 BLOCKED; 0 model-checkable); school-ledger 9 candidates,
now 9 UNSUPPORTED (the 7 formerly INCONCLUSIVE pass their authorization
guards with stated preconditions and stop at zod `strictObject`), 0
model-checkable ([Phase 15 real-world evaluation](../phase15/REAL-WORLD-EVALUATION.md)).

Phase 15 model-checking and differential evidence (`reports/phase15/evidence/`,
final tree `ced40150…`, PostgreSQL 17.11): 98 of 98 generated worlds agree
under the unreduced search, sleep sets, classic DPOR and source-set DPOR;
28 of 31 counterexample worlds confirmed in lockstep (166 outcomes, 0
disagreements; 3 replica models not replayable); two input-dependent
first-party defects reproduced on the original code 3/3 each, the safe
NULLS NOT DISTINCT counterpart not reproduced 0/3
([Phase 15 differential](../phase15/POSTGRESQL-DIFFERENTIAL.md)).

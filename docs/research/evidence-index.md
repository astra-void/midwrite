# Evidence index

The headline measured results a reader might cite, what they cover, the
tests and fixtures in the current tree that exercise the same behavior,
and where the archived raw evidence is. Method:
[evaluation methodology](evaluation-methodology.md); datasets:
[evaluation datasets](evaluation-datasets.md).

Rules for this page:

* Every number was recorded by the development phase named in the row,
  for that phase's binary; it is not re-measured here. The phase's design
  notes, final report, revision (base commit, source digest, release
  binary hash), environment and raw evidence are archived in the git tag
  `history/phase-records` ([development history](../HISTORY.md) explains
  how to read or restore them).
* Results of **different binaries are never combined**. Comparisons name
  both sides.
* The categories are kept apart: static-analysis accuracy,
  candidate-discovery accuracy, model-checking correctness, PostgreSQL
  differential fidelity, original-code reproduction, performance.
* **No independent human review performed.** Labels and reviews were
  written by the implementer or by AI sessions; where an AI agent worked
  without access to the implementation, that is independence from the
  code, not from the project.
* School-ledger and unnamed-ai-project are private: only aggregate counts.

Environments: an Apple Silicon Mac (macOS, PostgreSQL 17.11, Node 22.23)
and a Debian 12 container on the same machine (PostgreSQL 15.19,
bubblewrap 0.8.0). Later phases did not always repeat the Linux run; rows
say "macOS" where that matters.

## 1. Static-analysis accuracy

`midwrite check` findings on real projects. No recall is claimed.

| phase | benchmark | result | scope and limitations | in tree |
| --- | --- | --- | --- | --- |
| 2 | manual review of corpus findings | 103 findings reviewed, 97 with a decided label; window existed in 92; 27 judged likely bugs. Real window / likely bug (95 % Wilson): MW1001 (38) 0.97 (0.87–1.00) / 0.34 (0.21–0.50); MW1002 (8) 0.75 (0.41–0.93) / 0.25 (0.07–0.59); MW1004 (43) 0.98 (0.88–1.00) / 0.28 (0.17–0.43); MW1005 (8) 0.88 (0.53–0.98) / 0.00 (0.00–0.32) | saas-starter `6e33e58b`, midday `51587319`, two private projects; reviewer = implementer, from source; nothing reproduced | [`corpora/labels/`](../../corpora/labels), [`corpora/manifest.json`](../../corpora/manifest.json) |
| 3 | blind automated re-review | 18 findings: 11/18 agreement (κ 0.38); 3 of 7 "likely bug" labels confirmed | a separate model session, not a human | — |
| 4, 8 | human review packets | 12 midday findings and 10 first-party recovery findings prepared; 0 reviewed | unreviewed | — |
| current | dev / held-out precision and recall of `check` | — | [evaluation harness](../development/evaluation-harness.md) | [`corpora/eval/`](../../corpora/eval) |

The separate [2026-11-11 upstream review](../development/upstream-review.md)
scanned five backends, retained a record for every diagnostic, removed twenty
read-related false positives with an opt-in HTTP-method filter, and opened
four upstream issues. These are AI source reviews, not runtime reproduction,
maintainer confirmation, or a new precision/recall score.

The [2026-11-13 returned-function regression](../development/evaluation-harness.md#calls-through-factory-results-2026-11-13)
adds bounded static dispatch through factory results, verified by ten
targeted tests and a cross-module fixture. The
[evaluation report](../../reports/eval/2026-11-13-returned-functions.json)
has unchanged findings (413 dev, 32 held-out); it provides no evidence of a
real-project recall improvement. No corpus source was executed.

The [capture and module-binding extension](../development/evaluation-harness.md#lexical-captures-and-module-factory-bindings-2026-11-13)
retains all 413 prior development findings and adds 9 unclassified findings,
including exported entry contexts. Held-out findings remain unchanged
(30/32 hazard precision, gate PASS); known-positive recall is unchanged.
The new findings are not nine confirmed independent bugs, and no runtime
reproduction was performed. Raw record:
[`closure-environments.json`](../../reports/eval/2026-11-13-closure-environments.json).

The [subsequent nine-finding source review](../development/evaluation-harness.md#review-of-the-nine-closure-findings-2026-11-13)
labels 6 true-positive candidates, 2 intended risks and 1 infeasible-path
false positive. Seven overlap existing windows, and no runtime reproduction
or independent human validation was performed. Same-binary dev scoring is
395/414 hazard precision, with no unlabeled findings; audited known-positive
matches and held-out findings are unchanged.

The [cardinality-guard fix](../development/evaluation-harness.md#cardinality-guard-false-positive-fix-2026-11-13)
removes the reviewed impossible growth/shrink path and two equivalent
inner-handler reports. Seventeen targeted regressions preserve feasible
windows and reject unsafe array/count assumptions. Seven feasible pairs
become visible after pruning and were source-labeled; dev scoring is
400/418 hazard precision with no unlabeled findings. Audited matches and
held-out findings remain unchanged. Records:
[`cardinality-guards.json`](../../reports/eval/2026-11-13-cardinality-guards.json),
[`source review`](../../reports/eval/2026-11-13-cardinality-guards-review.json).

## 2. Candidate-discovery accuracy

`midwrite discover`, scored against labeled entries. "Model" = an entry
has its own candidate whose actual scenario has a validated model
counterexample. "Static" = a hazard candidate exists. Results are TP / FP
/ FN / TN (precision / recall). The sets are in
[`fixtures/eval/`](../../fixtures/eval) and described in
[evaluation datasets](evaluation-datasets.md).

| set | scored by | result | held out? |
| --- | --- | --- | --- |
| p11 first-party commerce (8 pos / 10 neg / 3 unsupported) | Phase 11 | model 8 / 1 / 0 / 9 (0.889 / 1.000); static 0.800 / 1.000; 3 of 3 unsupported reported | no — same author as the engine (an upper bound) |
| p11 held-out (8 pos / 4 neg) | Phase 11 | first scoring 4 / 2 / 4 / 2 (0.667 / 0.500); after the fixes it prompted 7 / 0 / 1 / 4 | first scoring only |
| p12 (15 entries) | Phase 12 | first scoring 4 / 0 / 3 / 6 (1.000 / 0.571); post-fix 5 / 0 / 2 / 6 | first scoring only; same author |
| p13 (27 entries) | Phase 12 binary; Phase 13 | Phase 12 binary 2 / 0 / 13 / 11 (1.000 / 0.133); first scoring 3 / 0 / 12 / 11 (1.000 / 0.200); post-feedback 4 / 0 / 11 / 11 | first scoring only; AI-authored without `crates/` access |
| p14 (38 entries, 12 families) | Phase 13 binary; Phase 14 | Phase 13 binary 2 / 1 / 16 / 17; first scoring 0 / 0 / 18 / 18; post-feedback 4 / 0 / 14 / 18 (1.000 / 0.222) | first scoring only; as above |
| p15 (30 entries, 9 input-domain families) | Phase 14 binary; Phase 15 | Phase 14 binary 2 / 0 / 11 / 14; first scoring 4 / 0 / 9 / 14 (1.000 / 0.308); post-feedback 8 / 0 / 5 / 14 (1.000 / 0.615) | first scoring only; as above |
| p16 (40 entries: 18 / 19 / 3 unsupported, 11 families) | Phase 15 binary; Phase 16 | Phase 15 binary 1 / 0 / 17 / 19 (1.000 / 0.056); first scoring 6 / 0 / 12 / 19 (1.000 / 0.333); post-feedback unchanged | first scoring only; as above |
| p12, p13, p14, p15 (regression under the Phase 16 binary) | Phase 16 | 6 / **1** / 1 / 5 (`transferCredits` deadlock vs a heuristic property, confirmed on PostgreSQL); 8 / 0 / 7 / 11; 7 / 0 / 11 / 18; 10 / 0 / 3 / 14 | no |

In tree: [`fixtures/eval/`](../../fixtures/eval) (sets, labels,
`FROZEN.json`), [`crates/midwrite-cli/tests/discover_commerce.rs`](../../crates/midwrite-cli/tests/discover_commerce.rs)
(the commerce application end to end), discovery reports used as test
inputs in [`fixtures/reports/discovery/`](../../fixtures/reports/discovery).

## 3. Model-checking correctness

Reductions compared with the unreduced search; none is a proof
([partial-order reduction](../architecture/partial-order-reduction.md#how-the-reductions-are-checked)).

| phase | check | result |
| --- | --- | --- |
| 6 | sleep sets vs full exploration | equal reachable states on 14 workloads and 1 500 random scenarios; independence audit of every independent pair in every reachable state; 3 reduction bugs found and fixed |
| 9 | classic DPOR differential | 682 random and targeted workloads compared: terminal states, verdicts, counterexamples, stuck states equal; one soundness bug in pool hand-over found and fixed |
| 10 | adversarial generator (3 000 seeds, 9 profiles); metamorphic transformations; dependence audit | all searches agree; 208 metamorphic cases preserved; 144 workloads, 84 077 independent and 31 286 enabling pairs, 0 violations; deleting `RowRead`, `LockCheck`, `Wait`, `Seq` or `Budget` accesses is noticed by no workload; 4 defects found (two source-DPOR, classic DPOR "only the last race", pool claim) |
| 11 | counterexample validity on generated scenarios | every counterexample replays strictly and violates its invariant; every emitted scenario gives `model-check` the same verdict and state count |
| 13 | reductions with FK checks, lock modes, upserts, replicas, response step | audit clean; all searches equal to the unreduced search |
| 15, 16 | generated worlds / emitted scenarios under the four searches | 98 of 98 and 197 of 197 agree |

In tree: [`crates/midwrite-model/tests/`](../../crates/midwrite-model/tests)
— `por.rs`, `dpor.rs`, `sdpor.rs`, `por_adversarial.rs`, `por_audit.rs`,
`por_mutation.rs`, `por_regressions.rs`, `foreign_keys.rs`,
`process_pool.rs`, `in_flight.rs`; model fixtures in
[`fixtures/model/`](../../fixtures/model).

## 4. PostgreSQL differential fidelity

The model's SQL replayed in lockstep on PostgreSQL, or an independent
ground truth replayed in the model
([validation](../architecture/postgres-model.md#validation-against-postgresql)).

| phase | check | result | limitations |
| --- | --- | --- | --- |
| 6 | isolation matrix, 17.11 and 15.19 | **1 695 schedules, 0 disagreements** (after two model corrections the tests forced) | SERIALIZABLE: PostgreSQL aborted 60 of 70 schedules the model allows (sequential scans), never the reverse |
| 7 | joins, projections, sequences + rollback, `ON CONFLICT DO NOTHING … RETURNING` | **102 schedules, 0 disagreements** | SER + join on small workloads only |
| 8 | schema facts vs real catalog | `agree` on every run database; injected drift detected | CHECK and partial-index predicates compared by presence only |
| 12 | Drizzle/PostgreSQL probes on the same TypeScript | 6 / 6 supported programs end in the same rows | found the FK-lock limitation (lifted in Phase 13) |
| 13 | independent lock/FK ground truth (AI-authored without implementation access) | **837 cells**, 707 replayed in the model: 2 726 labeled steps, 0 disagreements (a mutated compatibility table gives 109) | cascade/deferred, FK-deadlock and a few statement forms not replayed |
| 13 | FK / lock-mode and upsert workloads | **543** and **396 schedules, 0 disagreements** | `targetWhere` not modeled |
| 14 | multi-row inserts, predicates, timestamps, JSON | **669 schedules**, 0 disagreements besides the known one-way SSI difference | — |
| 15 | counterexample worlds | 28 of 31 confirmed (166 outcomes, 0 disagreements; 3 replica models not replayable) | — |
| 16 | NULLS NOT DISTINCT; multi-row locks; counterexample schedules; zod | **1 542** and **2 124 schedules**, 0 disagreements; 49 of 54 counterexamples confirmed (264 outcomes); zod 1 220 comparisons, 0 mismatches | multi-row lock order without `orderBy`: 24 of 51 schedules differ on an out-of-order heap (stated approximation) |

In tree: `crates/midwrite-model/tests/postgres.rs`, `pg_matrix.rs`,
`lock_ground_truth.rs` (with [`fixtures/model/lock-matrix/`](../../fixtures/model/lock-matrix)),
`drizzle_statements.rs`, `nulls_not_distinct.rs`, `multi_row_locks.rs`;
[`crates/midwrite-analyzer/tests/zod.rs`](../../crates/midwrite-analyzer/tests/zod.rs);
[`fixtures/reports/drizzle-sql/probe.json`](../../fixtures/reports/drizzle-sql/probe.json).

## 5. Original-code reproduction

Original (unchanged) first-party code under the counterexample's schedule,
on a disposable PostgreSQL. **No external, corpus or private application
has been executed.**

| phase | application(s) | result | in tree |
| --- | --- | --- | --- |
| 3 | `order-vulnerable` replay harness | MW1001 window reproduced 5 of 5 (SIGKILL after a verified commit); the outbox variant does not reproduce at any of 5 failure points | [`fixtures/replay/`](../../fixtures/replay), [`fixtures/reports/replay/`](../../fixtures/reports/replay), `crates/midwrite-analyzer/tests/recovery.rs`, `crates/midwrite-cli/tests/replay.rs` |
| 4 | `drizzle-node` (real Drizzle) | 16/16 cases as declared, macOS and Linux; generated harness 5/5 | [`fixtures/replay/drizzle-node`](../../fixtures/replay/drizzle-node) |
| 7 | `team-app` `inviteTeamMember`, `acceptInvitation` | `original-code-reproduced`, 5/5 each | [`fixtures/apps/team-app`](../../fixtures/apps/team-app), `crates/midwrite-cli/tests/server_actions.rs` |
| 8 | shop-app, pgjs-app, team-app (21 scenarios) | **105 of 105** runs (macOS) and **63 of 63** (Linux) matched: crash counterexamples, recoveries, lock waits, real 40001 retries | [`fixtures/apps/shop-app`](../../fixtures/apps/shop-app), [`fixtures/scenarios/crash-retry`](../../fixtures/scenarios/crash-retry), `crates/midwrite-cli/tests/crash_retry.rs` |
| 9 | `ledger-app` (in-flight fates, shared process, pool starvation) | 10 scenarios × 5 runs, 0 inconclusive, 0 diverged; Linux all matched | [`fixtures/apps/ledger-app`](../../fixtures/apps/ledger-app), [`fixtures/scenarios/in-flight`](../../fixtures/scenarios/in-flight), `crates/midwrite-cli/tests/in_flight.rs` |
| 10 | live connection loss; postgres-js in-flight | 21 of 22 scenarios reproduce or recover 5/5; the no-error-handler variant diverges as expected; Linux same | [`fixtures/apps/ledger-live`](../../fixtures/apps/ledger-live), [`fixtures/apps/ledger-pgjs`](../../fixtures/apps/ledger-pgjs), [`fixtures/scenarios/connection-loss`](../../fixtures/scenarios/connection-loss), `crates/midwrite-cli/tests/connection_loss.rs` |
| 11 | commerce application, discovered counterexamples | **10 of 10** reproduced 3/3 | [`fixtures/apps/commerce-app`](../../fixtures/apps/commerce-app), `crates/midwrite-cli/tests/discover_commerce.rs` |
| 12 | `workspace-app`, discovered | 4 of 6 reproduced; 2 diverged (FK check lock not modeled then), identical on both platforms | [`fixtures/apps/workspace-app`](../../fixtures/apps/workspace-app), `crates/midwrite-cli/tests/generated_ids.rs` |
| 13 | `workspace-app` FK pairs; `request-app` claim, signup crash, reserve races | 3/3 each (the earlier divergence resolved by FK lock modeling) | [`fixtures/apps/request-app`](../../fixtures/apps/request-app), `crates/midwrite-cli/tests/responses.rs` |
| 14 | `retry-wrapper-app`: lost response + resubmission; COMMIT lost inside `withRetry` | crash and connection-loss counterexamples reproduced 3/3 (macOS) | [`fixtures/apps/retry-wrapper-app`](../../fixtures/apps/retry-wrapper-app), `crates/midwrite-cli/tests/retry_wrapper.rs` |
| 15 | two input-dependent first-party defects | 3/3 each; the safe NULLS NOT DISTINCT counterpart not reproduced (0/3) | [`fixtures/apps/worlds-app`](../../fixtures/apps/worlds-app), `crates/midwrite-cli/tests/input_worlds.rs` |
| 16 | retry loop duplicate after a lost COMMIT acknowledgment | 3/3 | [`fixtures/apps/retry`](../../fixtures/apps/retry), `crates/midwrite-cli/tests/loops.rs` |

Replica read-your-writes counterexamples are model only
([`fixtures/model/replicas/`](../../fixtures/model/replicas)).

## 6. Performance

Same-machine comparisons recorded by the phase named; timings from
different phases are not comparable unless a row says so. Methodology and
scripts: [benchmarks](../development/benchmarks.md).

| phase | measurement | result |
| --- | --- | --- |
| 6 | `check` on midday; model checker | `check` ≈ 0.21 s, identical to the previous binary; model checker 2–4 actors 0.2–68 ms |
| 7 | path analysis, deep call chains | −32 % to −55 % with compact summaries, identical findings; original-code run ≈ 1.9 s |
| 8 | model checking | 4 actors 97 → 48 ms, 5 actors 936 → 427 ms, 6 actors 9.0 → 3.8 s |
| 9 | visited-state memory, same states | 7 actors 2 934.5 → 118.5 MiB; 8 actors (4 544 512 states) 532.4 MiB |
| 10 | reductions, team-app | 4 actors: classic DPOR 3.5 M transitions vs sleep sets 2 291 vs source-DPOR 574; 7 actors: sleep sets 1.08 M (30.8 s) vs source-DPOR 190 830 (6.0 s) |
| 12 | discovery with model checking | midday 28.07 → 1.13 s (shared project context), school-ledger 3.36 → 0.53 s |
| 13 | state-space overhead of `foreign-keys` | 1.19× states on 2 workspace-app scenarios, 1.00× on the others |

## 7. Compatibility and regression identity

From Phase 8 on, each phase compared the previous binary with the new one
on identical inputs: static analysis on 4 projects × depths 3, 6, 12 × 3
path modes × json, sarif, text, trace (144 outputs) stayed **identical**
in every phase; the model-checking scenarios kept the same verdicts and
state counts (differences only in `visitedBytes` and, once, helper
return-slot attribution). In tree: the versioning golden
[`crates/midwrite-analyzer/tests/data/versioning-golden.json`](../../crates/midwrite-analyzer/tests/data/versioning-golden.json)
and the report-schema tests (`crates/midwrite-cli/tests/report_schemas.rs`,
[`fixtures/model/report-schemas/`](../../fixtures/model/report-schemas)).

## 8. Real-world corpus coverage (static discovery)

Nothing was executed. Counts only; no finding is claimed as a defect.
Latest recorded state (Phase 16 binary):

| project | result |
| --- | --- |
| saas-starter | 1 model-checkable candidate (the invitation race; model counterexample, heuristic property) |
| midday `51587319` (declared replica factory) | 4 candidates: **1 model-checkable** (portal-settings read-modify-write, NO_VIOLATION_WITHIN_BOUNDS against a heuristic property over generated identity), 1 UNSUPPORTED, 2 BLOCKED (need declared invariants) |
| school-ledger (private) | 9 candidates, all UNSUPPORTED (blocker: zod `.superRefine()`), 0 model-checkable |
| unnamed-ai-project (private) | 0 candidates |

Earlier phases' counts and the blockers each phase removed are in the
archived records. Current blockers and the procedure:
[corpus evaluation](../development/corpus-evaluation.md);
corpus definition: [`corpora/`](../../corpora).

# Phase 13 evaluation: frozen set, precision and recall

Consolidates the plan's `PRECISION-RECALL` and the evaluation part of
`REAL-WORLD-EVALUATION` (the corpus is in `FINAL-REPORT.md` §6; there is no
separate `REAL-WORLD-EVALUATION.md`). The dataset itself:
`EVALUATION-DATASET.md`. The final Phase 13 binary reproduces the
post-feedback row exactly (`reports/phase13/eval/final-binary.json`).

**No independent human review performed.** The application and labels were
written by an AI agent working in the same session as the implementation,
without access to `crates/` or the in-progress Phase 13 code, and frozen
(`fixtures/phase13/FROZEN.json`, manifest sha256 `0a00594d…`, 29 files)
before any scoring. That is independence from the implementation, not from
the project; AI review is not independent review.

## 1. Scores (model prediction: an entry has its own candidate whose actual scenario has a validated model counterexample)

| set | binary | TP | FP | FN | TN | precision | recall |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Phase 13 frozen set | Phase 12 (`7da740b2…`) | 2 | 0 | 13 | 11 | 1.000 | 0.133 |
| Phase 13 frozen set, **first scoring** | Phase 13 | 3 | 0 | 12 | 11 | 1.000 | **0.200** |
| Phase 13 frozen set, after one change it prompted (**post-feedback, not held out**) | Phase 13 final | 4 | 0 | 11 | 11 | 1.000 | 0.267 |

Static hazard prediction on the same set: Phase 12 binary 0.800 / 0.267;
Phase 13 first scoring 0.833 / 0.333; post-feedback 0.833 / 0.333. Raw:
`reports/phase13/eval/{phase12-binary,first-scoring,post-feedback-1}.json`.
The FROZEN.json checksums are verified by `scripts/phase13-eval.py` before
every run.

The only change prompted by the set: `.for("share" | "key share" | "no key
update")` lowered to the model's lock strengths (`createTeamShared`, E04,
was an unknown operation), and discovery no longer counts a shared lock as a
protection. It is generic (the model had the strengths; the extractor did
not lower them); the score after it is reported as post-feedback.

## 2. Per case (first scoring → post-feedback)

| id | entry | label | outcome | cause when not TP/TN |
| --- | --- | --- | --- | --- |
| E01 | createTeam | + | TP | |
| E02 | createTeamLocked | − | TN | |
| E03 | createTeam ∥ createTeamLocked | + | (pair candidate: counterexample) | the foreign-key-lock pattern of Phase 12; scored with E01 |
| E04 | createTeamShared | + | FN → **TP** | `.for("share")` not lowered (fixed, post-feedback) |
| E05 | addMember | + | FN | lock cycle: the read-modify-write candidate's synthesis is INCONCLUSIVE |
| E06 | addMemberCounted | − | TN | |
| E07 | addComment ∥ deletePost | − | TN | |
| E08 | addBookmark ∥ deletePost | + | FN | no detection family pairs a parent DELETE with a child INSERT |
| E09 | holdSeat | + | FN | the property is a return value ("at most one caller told true"); the frozen contracts declare none, and no heuristic infers it |
| E10 | holdSeatAtomic | − | TN | |
| E11 | holdSeatDirect | + | FN | same as E09 |
| E12 | placeOrder | + | FN | multi-row `values([...])` not modeled (the response step that would expose the duplicate is implemented; the entry has no extracted effect) |
| E13 | placeOrderIdempotent | − | TN | |
| E14 | settleOrder | + | FN | higher-order retry wrapper (`withRetry(() => …)`) not resolved |
| E15 | settleOrderOnce | − | TN | |
| E16 | renameAndRead | + | FN | return-value property (read-your-writes) not declared |
| E17 | renameAndReadPrimary | − | TN | |
| E18 | renameInTransaction | − | TN | |
| E19 | addLabel | + | **TP** | (replica-backed check-then-act: new in Phase 13) |
| E20 | createTeamRoute (`withAuth` + zod) | + | FN | `schema.parse(input)`: a verified package contract, but its result (the transformed input) is not representable |
| E21 | inviteRoute | − | TN | |
| E22 | recordView (`onConflictDoUpdate`) | − | TN | |
| E23 | reserveHandle | + | FN | a column the model drops (`updatedAt`, timestamp) in the upsert's `set` |
| E24 | handleWelcomeJob | + | FN | timestamp column (`welcomedAt`) not modeled |
| E25 | handleWelcomeJobOnce | − | TN | |
| E26 | createFolder | + | FN | a conditional expression inside `and(...)` (`parentId === null ? isNull(...) : eq(...)`): predicate not a Drizzle operator call |
| E27 | runMaintenance | unsupported | reported | raw SQL |

**Precision stayed 1.000** throughout: no false positive. Recall is low
because 11 positives need constructs Phase 13 did not add to the extractor
(arrays/multi-row inserts, timestamp columns, higher-order wrappers, a
parent-delete × child-insert family, return-value properties without a
declaration). The same `upsert`, `response` and `returned` semantics *are*
verified on the first-party request application (`EVIDENCE` in
`FINAL-REPORT.md`) — the gap is source coverage and property declaration,
not model semantics.

## 3. Separate measurement problems (F6)

| stage | measured on | result |
| --- | --- | --- |
| schema recognition | frozen set | 16/16 tables modeled; foreign keys enforced in synthesized scenarios |
| entry extraction | frozen set | 27/27 labeled entries extracted |
| effect extraction | frozen set | 21 of the 27 labeled-effectful entries have a bound write; the 6 others are blocked by the constructs above |
| candidate discovery | frozen set (post-feedback) | 4 of 15 positives have a candidate of the labeled class; 1 negative has one (its scenario holds: no false positive) |
| model-checkable candidates | frozen set | every candidate's actual scenario loaded or was reported UNSUPPORTED/INCONCLUSIVE with the reason |
| counterexample correctness | frozen set + request-app + workspace-app | every counterexample replays and violates its invariant (validation); 0 false positives |
| original-code schedule reproduction | first-party request-app and workspace-app | 5/5 counterexample scenarios reproduced 3/3 runs (evidence); 0 divergences |
| property interpretation | — | return-value properties need a declaration; none is inferred |
| lock semantics | PostgreSQL ground truth | 707 cells / 2 726 labeled steps, 0 disagreements |

## 4. Earlier benchmarks (regression check, `reports/phase13/regression/earlier-benchmarks.md`)

| set | Phase 12 binary | Phase 13 binary |
| --- | --- | --- |
| Phase 11 first-party (commerce-app), model | TP 8 FP 1 FN 0 TN 9 (0.889 / 1.000) | **unchanged** |
| Phase 11 held-out (shop-app, team-app) | TP 7 FP 0 FN 1 TN 4 | **unchanged** |
| Phase 12 evaluation set (post-feedback in Phase 12) | TP 5 FP 0 FN 2 TN 6 | TP **6** FP 0 FN **1** TN 6 — `recordSignup` (crash before the response) is now found; `claimTask` stays FN (no return-value property declared) |

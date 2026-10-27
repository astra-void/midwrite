# Phase 12 evaluation (Milestone 10)

## What this evaluation is — and is not

* A **new** application, `fixtures/phase12/eval-app` (15 entries, a
  three-module schema behind a barrel, object and class repositories), and
  **new labels**, `fixtures/phase12/eval-labels.json`, written after the
  Phase 12 implementation and **before discovery was ever run on it**. The
  SHA-256 of the labels and of every source file were recorded before the
  first scoring: `reports/phase12/eval/FROZEN.md`.
* The labels are by **the same author as the implementation**. The set is
  fresh (it is not the Phase 11 held-out set, and it was not used to develop
  Phase 12), but it is **not an independent human evaluation**: the author
  knew the supported subset while writing it.
* The first scoring is recorded once (`scripts/phase12-eval.py --first`
  refuses to overwrite it) and is reported below unchanged. Fixes prompted by
  it are scored separately and are **not** held-out results.
* The Phase 11 numbers (first-party 0.889 / 1.000; held-out first scoring
  0.667 / 0.500; post-fix 1.000 / 0.875) remain the Phase 11 record; they
  are not relabeled or replaced.

Labels: 7 positive, 6 negative, 2 unsupported. Classes: check-then-act (4),
status-transition (2), read-modify-write (4), resubmitted-write (1),
ambiguous-commit (2), plus the 2 unsupported entries (a loop over an input
array; `fetch()` of a remote service).

## First scoring (frozen labels, before any change)

`reports/phase12/eval/first-scoring.json`

| stage | result |
| --- | --- |
| schema recognition | **6 / 6** expected tables modeled, 0 refused; **4 / 4** expected generated-key columns recognized (`accounts.id`, `events.id`, `members.id` database; `projects.id` application, `nanoid()`) |
| entry extraction | 15 / 15 labeled entries extracted |
| effect extraction | 14 / 15 entries with a bound database write (the unsupported loop entry has none: its insert is inside the loop) |
| candidate generation | 6 / 7 positives have a candidate of the labeled class; 3 of 6 negatives also have one (as expected: a candidate is a question, the model answers it) |
| model prediction (validated counterexample of the entry's own candidate) | **TP 4, FP 0, FN 3, TN 6 — precision 1.000, recall 0.571** (F1 0.727) |
| static hazard prediction | TP 6, FP 1, FN 1, TN 5 — precision 0.857, recall 0.857 |
| counterexample validity | 4 / 4 entries with a counterexample: every counterexample replays and violates its invariant |
| unsupported handling | 2 / 2 reported, 0 claimed safe |
| original-code reproduction | **not measured**: the evaluation application has no authorized harness |

False negatives of the first scoring:

1. `claimTask` (status-transition): read the assignee, then assign
   unconditionally. No candidate: the status-transition family requires a
   state column set to a *literal* state and an observable consequence
   (an insert or an external effect) — a lost claim's only consequence is
   the boolean the function returns, and **main-task return values are not
   modeled**.
2. `recordSignup` (resubmitted-write, declared crash retry): the candidate
   exists but is `NO_VIOLATION_WITHIN_BOUNDS` after 2 states. The entry has a
   **single** INSERT; the model has no transition between its commit and the
   response (the response is not a separate event), so the declared
   "crash after commit, then resubmit" cannot occur in the model.
3. `recordLogin` (ambiguous-commit): `INCONCLUSIVE` — synthesis required a
   fault-free solo run to reach the handler's second INSERT, which only runs
   after a failure; and the declared invariant's key `(accountId, kind)`
   could not be instantiated because the detector passed only
   parameter-valued columns (not the literal `kind = "login"`).

## After fixes prompted by the first scoring (NOT held out)

Two fixes for (3), both general: the repeated write of an ambiguous-commit
candidate is not a solo-run target (it is reached under the explored fault),
and declared invariants receive every written value. Scored with the same
frozen labels: `reports/phase12/eval/post-fix-scoring.json`.

| | TP | FP | FN | TN | precision | recall |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| first scoring | 4 | 0 | 3 | 6 | 1.000 | 0.571 |
| after fixes (not held out) | 5 | 0 | 2 | 6 | 1.000 | 0.714 |

(1) and (2) are left as documented limitations rather than patched for this
set (`LIMITATIONS.md`): both need model changes (return values; a response
event) that would alter the state spaces of every earlier scenario.

## The Phase 11 benchmarks under the Phase 12 binary

The Phase 11 first-party and held-out sets are rescored by the existing CLI
tests (`commitrift-cli` `phase11`, `commitrift-discover` `discovery`); their
results are in the final test run (`FINAL-REPORT.md`). They are regression
checks, not new accuracy estimates.

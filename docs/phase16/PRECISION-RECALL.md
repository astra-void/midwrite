# Phase 16 precision and recall

(Track F's F5–F8 and the brief's §8.4.)

**No independent human review was performed.** Every label of the sets
below was written and checked by AI agents, and an AI agent checking
another AI agent's results is not independent human validation. The
[review packet](#review-packet) at the end is prepared for developers who
review the sets later.

## Scoring rule (all sets)

The same rule as Phases 13–15 (`scripts/phase16-eval.py`, a copy of
`scripts/phase15-eval.py` with only its paths changed):

* A case is **predicted positive** when its entry's own candidate (for
  paired cases: the pair's candidate) has a `MODEL_COUNTEREXAMPLE` whose
  every counterexample is **validated**: it replays in the model and
  violates the declared property.
* `TP`/`FP`/`FN`/`TN` compare that prediction with the frozen label.
  An `UNSUPPORTED`, `BLOCKED`, `INCONCLUSIVE` or `NO_VIOLATION_WITHIN_BOUNDS`
  result on a positive case counts as **FN**: unsupported code is never
  scored as safe or as found.
* Cases **labeled** `unsupported` (code the subset is documented not to
  cover) are reported separately and are **not** in any TP/FP/FN/TN
  denominator. For them the scorer records whether the tool reported them
  as unsupported or claimed them safe (`claimedSafe`).
* Precision and recall are computed only against the frozen labels.

## Phase 16 frozen set (fresh, held out)

`fixtures/phase16/eval-app` with `fixtures/phase16/eval-labels.json`: 40
cases (18 positive, 19 negative, 3 unsupported) covering every family the
brief lists (§F5), frozen in `fixtures/phase16/FROZEN.json` (manifest
`a55cb43b…`, 38 files, verified at every scoring) **before** any
implementation was scored against it. See
[EVALUATION-DATASET.md](../../fixtures/phase16/EVALUATION-DATASET.md).

| scoring | binary | TP | FP | FN | TN | precision | recall | labeled unsupported (reported / claimed safe) |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| Phase 15 binary (reference) | Phase 15 release | 1 | 0 | 17 | 19 | 1.000 | 0.056 | 3 / 0 |
| **first scoring** (`reports/phase16/eval/first-scoring.json`) | integrated Phase 16, sha256 `393207d3…`, digest `538c7822…` | **6** | **0** | **12** | **19** | **1.000** | **0.333** | 3 / 0 |
| post-feedback-1 | after the integration fixes | 6 | 0 | 12 | 19 | 1.000 | 0.333 | 3 / 0 |
| post-feedback-final (`reports/phase16/eval/post-feedback-final.json`) | final release build | 6 | 0 | 12 | 19 | 1.000 | 0.333 | 3 / 0 |

The first score is recorded once (`first-scoring.json`, with the binary's
sha256 and the source digest beside it) and is not replaced, relabeled or
erased. No implementation change was made in response to an individual
Phase 16 failure: the integration fixes after the first scoring were
prompted by the historical sets (below), and they left this set's score
unchanged.

### Per family (first scoring)

| family | TP | FP | FN | TN |
| --- | ---: | ---: | ---: | ---: |
| zod strict objects | 1 | 0 | 0 | 1 |
| optional / nullable / default | 1 | 0 | 1 | 2 |
| validated authorization inputs | 0 | 0 | 1 | 1 |
| bounded retry loops | 0 | 0 | 2 | 2 |
| SQLSTATE retry logic | 0 | 0 | 2 | 2 |
| request JSON bodies | 1 | 0 | 0 | 1 |
| hoisted / conditional predicates | 1 | 0 | 1 | 2 |
| multi-row locks | 0 | 0 | 2 | 3 |
| `NULLS NOT DISTINCT` | 1 | 0 | 0 | 1 |
| generated identifiers | 0 | 0 | 2 | 2 |
| replica reads | 1 | 0 | 1 | 2 |
| labeled unsupported | — | — | — | — (3 reported, 0 claimed safe) |

### Why the 12 false negatives were missed

| cause | cases | first blocker |
| --- | ---: | --- |
| JavaScript standard-library calls are not modeled | 5 | `Set.prototype.has` on a module-level `Set`, `Number.isSafeInteger`, `Math.min` in the project's retry/backoff helper (`src/lib/pg-errors.ts`) and in an input check; the candidates are UNSUPPORTED (never safe) |
| no candidate detected: a status transition whose write is keyed by the id of a row read first, followed by an insert | 4 | detection does not pair the read with the id-keyed write |
| no candidate detected: a predicate built inline from a local that is not a held predicate | 1 | "`sibling` is not a Drizzle predicate the model resolves" |
| no candidate detected: a multi-row claim whose `limit` is not an integer literal | 1 | `Number.isSafeInteger`, then "`limit` is not an integer literal" |
| INCONCLUSIVE: no property instance | 1 | the case's property names no row the synthesized worlds contain |

Every miss is reported to the user as `UNSUPPORTED`, `INCONCLUSIVE` or no
candidate, never as `NO_VIOLATION_WITHIN_BOUNDS` with an unexplained gap.
The standard-library builtins (5 of the 12) are the largest single cause
and the most direct next step.

### Stage-level accuracy (first scoring, Phase 16 set)

| stage | result |
| --- | --- |
| source entries extracted | 40 of 40 labeled entries (71 entry candidates indexed) |
| relevant effects extracted | 39 effectful of 40 expected |
| candidates detected | positives with a candidate of the labeled class: 11 of 18; negatives with one: 10 of 19 |
| valid initial worlds synthesized | 24 worlds built, 6 invalid (rejected by the synthesized constraints), 0 duplicates |
| model-checkable candidates | 12 of 26 candidates (the report's `modelCheckable`); 10 reach a verdict (6 MODEL_COUNTEREXAMPLE, 4 NO_VIOLATION_WITHIN_BOUNDS) |
| counterexamples found | 6 entries |
| counterexamples validated | 6 of 6 (all replay and violate the declared property) |
| original-code reproductions | not measured on this set: the evaluation application has no authorized harness (by design: it is never executed). First-party reproductions are in [POSTGRESQL-DIFFERENTIAL.md](POSTGRESQL-DIFFERENTIAL.md) |
| false positives | 0 |
| false negatives | 12 |
| unsupported outcomes | 3 labeled-unsupported cases reported as unsupported, 0 claimed safe; 16 of 26 candidates UNSUPPORTED (12) or INCONCLUSIVE (4) |

Static hazard prediction (a candidate of the labeled class exists, before
model checking), for comparison: TP 12, FP 7, FN 6, TN 12, precision
0.632, recall 0.667. Model checking turns 7 static false alarms into
true negatives and costs 6 static hits that are UNSUPPORTED or
INCONCLUSIVE.

## Historical sets (regression, not fresh evidence)

These sets have shaped earlier phases and are regression checks, not
independent evidence. Each was re-run with the final Phase 16 binary
(`reports/phase16/regression/final-*.json`) and compared with the Phase 15
release binary's run of the same set.

| set | Phase 15 binary | Phase 16 final | change |
| --- | --- | --- | --- |
| Phase 11 first-party + held-out (`crates/commitrift-discover/tests/discovery.rs`) | labeled entries classified as labeled | passes; unsupported reported 2, missed 1, labels corrected 1 | the checkout route label (capability label "unsupported") now has a dated correction in `fixtures/phase11/labels.json` `corrections`: Phase 16 models `request.json()`; the label itself is unchanged |
| Phase 12 evaluation | TP 6, FP 0, FN 1, TN 6 | TP 6, **FP 1**, FN 1, TN 5 | `transferCredits`, see below |
| Phase 13 frozen | TP 8, FP 0, FN 7, TN 11 | TP 8, FP 0, FN 7, TN 11 | unchanged |
| Phase 14 frozen | TP 5, FP 0, FN 13, TN 18 | TP 7, FP 0, FN 11, TN 18 | +2 TP (loops, caught errors) |
| Phase 15 frozen | TP 8, FP 0, FN 5, TN 14 | TP 10, FP 0, FN 3, TN 14 | +2 TP |

Phase 12 and 13 figures are their own scorers' `stages.model.modelPrediction`
(the same prediction rule as above). An auxiliary tally written during
integration (`scripts/phase16-regression-tally.py`) counts one paired
Phase 13 case differently (9 / 0 / 6 / 11 for both binaries). It is not
used for any figure here; the scorer's own count is authoritative and
agrees with the Phase 15 record.

### The Phase 12 `transferCredits` false positive

Phase 16's lock-order candidates now cross keys (Track C's multi-row and
held-predicate work), so two concurrent transfers in opposite directions
(A→B and B→A) are explored. They deadlock: PostgreSQL aborts one with
SQLSTATE 40P01. The candidate's property is a heuristic "no invocation
fails". The schedule was replayed on PostgreSQL 17.11 with
`--validate-postgres`: **confirmed**, 11 database outcomes matched. The
Phase 12 label is "negative" for an environment of transfers *from one
wallet*. The counterexample lies outside that label's environment and
against a guessed property, so the scorer counts it as a false positive,
and it is reported as one. The label was not changed. The counterexample
is real PostgreSQL behavior, and it is labeled HEURISTIC_HYPOTHESIS in the
report.

### Two Phase 14/15 false positives found and fixed during integration

After the first integration, the Phase 14 set had a new FP
(`createCategory`, P14-E11) and so did the Phase 15 set (`createPage`,
P15-E07), both safe cases. Cause: actor-world search varied, between the
two actors, an argument that is part of the read check through a computed
value (`trim().toLowerCase()`) or a conditional held predicate. The two
actors then looked at different rows, the check legitimately passed for
both, and the heuristic property was violated in a world the code's own
check excludes. Fix (generic, `crates/commitrift-discover/src/worlds.rs`,
`actor_worlds`): an argument that takes part in a read predicate is not
varied between actors even when it is not a recognized key. Both sets
returned to FP 0, and the Phase 16 set's score did not change.

## Counterexample validity

Every counterexample on every set above is validated in the model (the
`allValidated` stage). On PostgreSQL: 49 of 54 counterexample schedules
from ten applications confirmed in lockstep, 0 disagreements over 264
compared outcomes; the remaining 5 are read-replica counterexamples, which
the lockstep harness (a single primary) cannot replay and which stay
model-only ([POSTGRESQL-DIFFERENTIAL.md](POSTGRESQL-DIFFERENTIAL.md)).

## Review packet

For a future independent reviewer. Each item is self-contained.

1. **Labels:** `fixtures/phase16/eval-labels.json`. Per case: `id`,
   `entry`, `class`, `label`, `family`, `property` (declared in
   `fixtures/phase16/eval-app/commitrift-contracts.json`), `environment`,
   `isolation`, `counterpart`, `inputSensitivity`, `why`. The file's
   `$comment` states who wrote the labels and what they had read (an AI
   agent that knew the documented supported subset: the set is fresh but
   not blind). Check each `why` against the source and PostgreSQL 15+
   semantics; record disagreements without editing a label (the file's
   `corrections` list, empty today, is the place).
2. **Freeze:** `python3 scripts/phase16-eval.py OUT.json` verifies
   `fixtures/phase16/FROZEN.json` before scoring; a changed source or
   label file refuses to score.
3. **Per-case evidence:** `reports/phase16/eval/first-scoring.json` →
   `stages.model.cases` (prediction, static prediction, candidates and
   their statuses).
4. **Reproduce a counterexample:** `commitrift discover
   fixtures/phase16/eval-app --model-check --emit-scenarios DIR`, then
   `commitrift model-check DIR/<file> --validate-postgres` (needs the
   PostgreSQL server binaries; nothing of the application is executed).
5. **Questions most worth an independent look:** (a) the 7 static false
   alarms that model checking clears: is each safe for every input, or
   only within the explored worlds? (b) the Phase 12 `transferCredits`
   deadlock: should a heuristic "no invocation fails" ever be reported for
   lock-order candidates? (c) the 4 status-transition misses: is the
   read-then-id-keyed-write pattern a hazard in every case labeled so?
6. **Record:** reviewer, date, case id, agree / disagree / unsure, reason.
   Report the agreement rate separately from the scores above.

# Phase 14 evaluation

Three measurements, kept apart: the **new Phase 14 set** (first scoring,
then post-feedback), the **Phase 13 frozen set** (post-feedback: it was
used during development), and the **earlier benchmarks** (regression
only). **No independent human review was performed** for any of them.

## 1. The Phase 14 evaluation set

`fixtures/phase14/eval-app`, `fixtures/phase14/eval-labels.json`: 38
entries (18 positive, 18 negative, 2 unsupported) in 12 construct families,
each with a vulnerable and a safe counterpart. Written by an AI agent that
did not read `crates/` or the Phase 14 work, frozen
(`fixtures/phase14/FROZEN.json`, sha256 of the manifest file `778117d2…`,
39 files) before any CommitRift command ran on it. That is independence from
the implementation, not from the project. Description of the set:
`fixtures/phase14/EVALUATION-DATASET.md`. Scorer: `scripts/phase14-eval.py`
(verifies the freeze first; model prediction = a validated
`MODEL_COUNTEREXAMPLE` of the entry's own candidate; a paired case needs a
candidate of exactly the pair).

Before the first scoring, only the aggregates of the Phase 13-binary
baseline were looked at; per-case results were read only after the first
scoring was recorded.

| scoring | binary | TP | FP | FN | TN | precision | recall |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| baseline | Phase 13 final (`97e96ac0…`) | 2 | 1 | 16 | 17 | 0.667 | 0.111 |
| **first scoring** | Phase 14 integrated (`577274ff…`, tree `465e12d2…`) | **0** | **0** | **18** | **18** | — | **0.000** |
| post-feedback 1 | + typed fills | 3 | 1 | 15 | 17 | 0.750 | 0.167 |
| post-feedback 2 (not held out) | + the changes below | 4 | 0 | 14 | 18 | 1.000 | 0.222 |

Without the two return-value positives whose property the application does
not declare (E20, E22): post-feedback 2 is TP 4 / FN 12 (recall 0.250).
Raw: `reports/phase14/eval/{phase13-binary,first-scoring,post-feedback-1,
post-feedback-2}.json`.

**Why the first scoring was zero.** Making time and JSON columns modeled
(the `timestamps`/`json` features, enabled in discovery's schema probe) let
those tables into synthesized scenarios, and row synthesis filled a NOT NULL
`timestamptz` column with text (`"createdAt-1"`), which the loader correctly
refuses: every candidate touching such a table became INCONCLUSIVE. The
baseline's two true positives and its false positive were among them. This
is an integration defect the new set exposed; the first scoring is kept as
measured.

**Changes prompted by the set** (each generic; none names an entry of it):

1. post-feedback 1 — synthesized rows fill time columns with a canonical
   time and JSON columns with `{}` (`synth.rs` `fill`).
2. post-feedback 2 — (a) a declared formula over return values also drives
   the `return-value` family, and invariant terms match an entry actor by
   its export name (`reserveSlot` for `src/services/slots.ts#reserveSlot`);
   (b) a counterexample on a table with a **partial unique index** the model
   does not enforce is INCONCLUSIVE, not a finding (this removed the one
   false positive); (c) module-level arrays of literals are values when
   `as const` or never mutated (JavaScript value semantics only).

Per family (post-feedback 2): multi-row insert 1/1 TP, atomic update 1/1,
higher-order 1/2, return-value 1/4 (1/2 of the declared ones), every other
family 0 TP; no false positive in any family.

**Remaining false negatives, first blocker** (post-feedback 2):

| case | family | first blocker |
| --- | --- | --- |
| E01 | arrays | no candidate: the check-then-insert runs per element of a constant array inside a loop; the key is not an argument |
| E03 | arrays | a call inside a conditionally evaluated operand |
| E05 | strings | the check and the insert share no argument-dependent key (the key is a computed string) |
| E10 | zod | the schema uses `.uuid()` (not modeled), so `parse` stays unknown |
| E12 | higher-order | the retry helper is a `for` loop (not in the supported subset) |
| E16 | replica | no detection family for a stale replica read used in a decision without a declared property |
| E20, E22 | return-value | property not declared (by design: never inferred) |
| E21 | return-value | NO_VIOLATION: the synthesized initial row already holds the value the actor writes (row fill and argument synthesis produce the same text), so a stale read is indistinguishable — a synthesis limitation, not a model result to rely on |
| E24 | fk-parent-child | the pair's path reads a caught error's properties (`error.code`), which are not modeled |
| E26 | timestamp | `now.getTime()` (time arithmetic is not modeled) |
| E28, E29 | json | a projection of a JSON column in `select({...})` is not lowered |
| E34 | unique | a route handler reading `await req.json()` (framework request objects are not modeled) |

Unsupported entries (E37, E38: raw SQL) are reported with their unknown
operations, never claimed safe.

## 2. The Phase 13 frozen set (post-feedback, not held out)

`reports/phase14/eval/phase13-frozen-set.json`, Phase 14 binary:

| | TP | FP | FN | TN | precision | recall |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Phase 13 first scoring (recorded) | 3 | 0 | 12 | 11 | 1.000 | 0.200 |
| Phase 13 final, post-feedback (recorded) | 4 | 0 | 11 | 11 | 1.000 | 0.267 |
| **Phase 14** (post-feedback) | **7** | **0** | **8** | **11** | 1.000 | **0.467** |

New true positives: E14 `settleOrder` (closure in `withRetry`, ambiguous
commit of a transaction), E20 `createTeamRoute` (zod `parse` lowered), E24
`handleWelcomeJob` (a redelivered message's declared insert). The pair E08
(`addBookmark × deletePost`, no foreign key) is a model counterexample of the
new parent-child family against a stated hypothesis, and E07 (with a foreign
key) holds — the scorer counts pairs separately. Remaining: E05 (lock cycle:
synthesis INCONCLUSIVE), E09/E11/E16/E23 (return-value properties not
declared in the frozen contracts), E12 (`values(order)` of a parameter
object), E26 (synthesis picks a non-null parent where the bug needs
`parentId = null`).

Static hazard prediction on the same set: precision 0.800, recall 0.533.

## 3. Earlier benchmarks (regression)

`reports/phase14/regression/earlier-benchmarks.md`: Phase 11 first-party and
held-out, and the Phase 12 set, give the same model scores as under the
Phase 13 binary. The Phase 12 set's unsupported entry `importTasks` is now
"missed" by its scorer's rule, because its loop over an input array is
modeled as a bounded synthesized array — the label's premise no longer
holds; the frozen label was not changed.

## 4. Stage by stage (Phase 14 set, post-feedback 2)

| stage | result |
| --- | --- |
| schema | 28/28 tables modeled |
| entry extraction | 38/38 |
| effect extraction | 31 of 38 labeled-effectful entries bind a write (Phase 13 binary: 26) |
| candidate discovery | 4 of 18 positives have a candidate of the labeled class (6 negatives do; their scenarios hold) |
| model-checkable candidates | 16 of 20 candidates reach a verdict |
| counterexample correctness | every counterexample replays and violates its invariant |
| original-code reproduction | not measured on this set (no authorized harness); see `REAL-WORLD-RESULTS.md` for the first-party runs |
| unsupported classification | 2/2 raw-SQL entries reported |

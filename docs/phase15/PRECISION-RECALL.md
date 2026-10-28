# Phase 15 evaluation: precision, recall and coverage failures

**No independent human review was performed** for any set.

## 1. The fresh Phase 15 set

`fixtures/phase15/eval-app`, `fixtures/phase15/eval-labels.json`: 30
entries (13 positive, 14 negative, 3 unsupported) in nine input-domain and
initial-state families, each positive with a safe counterpart. Written by
an AI agent that did not read `crates/`, `runtime/` or the Phase 14/15
design documents and ran no `cargo`/`commitrift` command; frozen before any
scoring (`fixtures/phase15/FROZEN.json`, manifest digest `b11cc1a1…`, 26
files; description `fixtures/phase15/EVALUATION-DATASET.md`). Independent
of the implementation, not of the project: the author read the documented
subset and Phase 14's notes on synthesis gaps. Scorer:
`scripts/phase15-eval.py` (the Phase 14 rule on the new paths).

Order of events: the set was written while the Phase 15 search was being
implemented (by me, without reading the set); before any case was looked
at, the Phase 14 binary and the then-current build were scored and only
the aggregates read; per-case results were read only after the first
scoring was recorded.

| scoring | binary | TP | FP | FN | TN | precision | recall |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Phase 14 binary (baseline) | `059b1150…` | 2 | 0 | 11 | 14 | 1.000 | 0.154 |
| **first scoring** (held out) | build `fd0e291b…` (world search, domains, relationship worlds) | **4** | **0** | **9** | **14** | 1.000 | **0.308** |
| post-feedback 1 (not held out) | + guard satisfaction, guarded-update family, boundary and actor worlds | 8 | 0 | 5 | 14 | 1.000 | 0.615 |
| post-feedback final (not held out) | final binary (+ literal unions, argument guards, declared key from the INSERT, unenforced-unique rule) | 8 | 0 | 5 | 14 | 1.000 | 0.615 |

Without the two cases whose property must be declared and is not (E20,
E21): first scoring TP 4 / FN 8 (recall 0.333); final TP 8 / FN 4 (0.667).
Raw: `reports/phase15/eval/{phase14-binary,first-scoring,post-feedback-1,
post-feedback-2,post-feedback-final}.json`.

The first scoring's two new true positives are exactly the two Phase 15
mechanisms: E01 (value distinction: a `stored-distinct` world) and E08
(nullable argument: an `undefined`/`null` world).

**Changes prompted by the set** (each generic; none names an entry):
guards over read rows (`satisfy_guard`, E18), the declared-only
guarded-update family and stored boundary worlds (E12, E14), actor worlds
for non-key arguments (E22), and — found by the regression sets during
this work — the restriction of actor worlds to independent requests (a
false positive on the Phase 13 set's E25 that never reached a scored
Phase 15 result).

**Remaining false negatives** (final):

| case | family | first blocker | category |
| --- | --- | --- | --- |
| E03 | value-distinction | no candidate: a read compares the stored theme with the argument, then an UPDATE writes the argument and a relative counter; no family covers it | missing candidate |
| E06 | nullable-argument | the null predicate is hoisted into a local (`const inFolder = … ? isNull(…) : eq(…)`) | unsupported source behavior |
| E10 | actor-keys | multi-row `FOR UPDATE` without `.limit(1)` | unsupported source behavior |
| E20 | authorization | admin-count write skew: the property is deliberately not declared | missing invariant |
| E24 | idempotency | the retry helper is a `for` loop | unsupported source behavior |

Per family (final): boundary 2/2, composite-unique 1/1, fk-parent-child
1/1, value-distinction 1/2, nullable-argument 1/2, authorization 1/2,
idempotency 1/2, actor-keys 0/1; no false positive in any family; every
counterexample validated (8/8); unsupported entries 3/3 reported. Paired
outcomes (positive and its safe counterpart both right): 2 → 4 → 8 of 13.

## 2. Stage by stage (first scoring → final)

| stage | first scoring | final |
| --- | --- | --- |
| schema | 28/28 tables | 28/28 |
| entry extraction | 30/30 | 30/30 |
| effect extraction | 26 of 30 labeled-effectful entries bind a write | 26 |
| candidate detection | 6 of 13 positives have a candidate of the labeled class | 8 |
| constraint extraction / input satisfiability | 8 candidates reached a checkable world | 13 |
| valid world generation | 28 worlds built for the set, 6 invalid/unreached (final, `reports/phase15/evidence/`) | — |
| model-checkability | 8/8 candidates of the labeled class reach a verdict | 13/13 |
| counterexample detection | 4 | 8 |
| counterexample validity | 4/4 replay and violate | 8/8 |
| relevant branch reachability | per candidate in `coverage.branchOutcomes` (solo runs) | — |
| original-code reproduction | not measured on this set (no authorized harness); see the worlds application (`POSTGRESQL-DIFFERENTIAL.md`) | — |

## 3. Historical sets (regression references, not held out)

Final binary; raw `reports/phase15/regression/`.

| set | Phase 14 binary | Phase 15 final | change |
| --- | --- | --- | --- |
| Phase 11 first-party (commerce-app) | TP 8 FP 1 FN 0 TN 9 | TP 8 FP 1 FN 0 TN 9 | none (`checkoutIdempotent` still FP) |
| Phase 11 held-out shop-app | TP 5 FP 0 FN 1 TN 4 | TP 5 FP 0 FN 1 TN 4 | none |
| Phase 11 held-out team-app | TP 2 FP 0 FN 0 TN 0 | TP 2 FP 0 FN 0 TN 0 | none |
| Phase 12 set | TP 6 FP 0 FN 1 TN 6 (2 unsupported) | TP 6 FP 0 FN 1 TN 6 | none |
| Phase 13 frozen set | TP 7 FP 0 FN 8 TN 11 | **TP 8** FP 0 FN 7 TN 11 | **E26 FN → TP** (`null:parentId`) |
| Phase 14 set (post-feedback) | TP 4 FP 0 FN 14 TN 18 | **TP 5** FP 0 FN 13 TN 18 | **E21 FN → TP** (`stored-distinct:accounts.displayName`) |

(Phase 12 and 13 tallies are computed from each scorer's per-case output
with its rule; the Phase 13 pair rule was applied as in its scorer.)

## 4. Why the Phase 14 set still misses 13 positives (4.4 of the brief)

| cause | cases |
| --- | --- |
| missing input values (fixed in Phase 15) | E21 → TP |
| missing candidate (no detection family) | E01 (constant array loop), E05 (computed key), E16 (replica read in a decision, no declared property) |
| unsupported source behavior | E03 (call in a conditional operand), E10 (zod `.uuid()`), E12 (`for`-loop retry helper), E24 (`error.code` of a caught error), E26 (`Date#getTime()`), E28/E29 (JSON projection in `select({...})`), E34 (`await req.json()`) |
| missing invariant (by design never inferred) | E20, E22 |
| invalid initial rows, unreached branches, external effects, exploration budget | none observed |

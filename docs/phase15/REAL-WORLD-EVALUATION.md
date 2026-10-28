# Real-world evaluation: midday and school-ledger

(This document covers what the brief called MIDDAY-EVALUATION and
SCHOOL-LEDGER-EVALUATION.)

Static discovery with bounded model checking; **no project was executed**.
Raw data: `reports/phase15/corpus/` (`scripts/phase15-corpus.py`: the
Phase 14 binary and the final Phase 15 binary on the same machine and
inputs, median of 3 runs, peak RSS the maximum). school-ledger is a private
checkout: only aggregate counts and generic causes are reported.

## Corpus metrics

| project | binary | extracted | effectful | unknown ops | candidates | model-checkable | model counterexamples | statuses | wall | peak RSS |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: |
| saas-starter | Phase 14 | 37 | 9 | 237 | 2 | 1 | 1 | 1 MODEL_COUNTEREXAMPLE (heuristic), 1 UNSUPPORTED | 0.04 s | 13 MiB |
| saas-starter | Phase 15 | 37 | 9 | 237 | 2 | 1 | 1 | unchanged | 0.04 s | 13 MiB |
| midday (declared replica factory) | Phase 14 | 1168 | 81 | 18 861 | 4 | 0 | 0 | 1 INCONCLUSIVE, 3 BLOCKED | 1.36 s | 144 MiB |
| midday (declared replica factory) | Phase 15 | 1168 | 81 | 18 861 | 4 | 0 | 0 | 1 INCONCLUSIVE, 3 BLOCKED | 1.32 s | 148 MiB |
| school-ledger | Phase 14 | 546 | 70 | 13 396 | 9 | 0 | 0 | 7 INCONCLUSIVE, 2 UNSUPPORTED | 0.71 s | 120 MiB |
| school-ledger | Phase 15 | 546 | 70 | 13 396 | 9 | 0 | 0 | **9 UNSUPPORTED** | 0.76 s | 121 MiB |

Original-code reproductions: none (no external project is executed).
Valid generated worlds / worlds explored are not meaningful here: no
candidate reached a checkable world in either project (below).

## school-ledger: the nine candidates

All nine are check-then-act candidates with a `HEURISTIC_HYPOTHESIS`
property (origin unchanged).

| | Phase 14 first blocker | Phase 15 | Phase 15 first blocker |
| --- | --- | --- | --- |
| 7 candidates (two provisioning entries and their server actions) | the inlined authorization guard denies: the synthesized actor argument has neither the role discriminator nor the role list the guard tests (INCONCLUSIVE) | the guard is **satisfied** with stated preconditions: the actor argument gets the discriminator literal the guard compares with and a role list containing the tested role ("SYNTHESIZED precondition … the scenario explores callers that satisfy this guard; it is not assumed of every caller"); no authentication is bypassed — the code still runs the guard | the next operation, input validation with zod **`z.strictObject(...)`**, is not modeled: **UNSUPPORTED** (validation is never treated as the identity) |
| 2 candidates (a test-support entry) | a method call on a value whose type is not resolved | unchanged | unchanged (UNSUPPORTED) |

Per candidate (aggregated, identical for the 7): required authorization
input — an actor of the staff kind holding the school-admin role (both
synthesized and stated); synthesized state count — 1 base world (the
search's further worlds stop at the same validation step); valid
actor-input combinations — the base world reaches the validation step;
relevant effects reached — none (the conflict's statements follow the
validation); model-checking outcome — not reached; remaining unknowns —
`strictObject`, then (from the extraction's unknown list) unresolved
method calls in a fingerprinting helper and loops over identity lists,
which may block next.

**Model-checkable school-ledger candidates: 0** (Phase 14: 0). The change
is a more precise and later first blocker, not a verification. No
counterexample was forced and none is claimed.

## midday: the four candidates (declared replica factory)

| candidate | Phase 14 | Phase 15 | first blocker now |
| --- | --- | --- | --- |
| read-modify-write of a customer's portal settings (replica read, primary update) | INCONCLUSIVE | INCONCLUSIVE | the scenario loads and both actors reach the replica `SELECT` and the primary `UPDATE` in the base world and in the further worlds (an actor-differ world over the enabled flag, a stored-equal world over the stored flag); the heuristic serial-outcome property still cannot be stated: every serial outcome of the portal id is a generated value (`nanoid(21)`), which an invariant term cannot express. World search does not change that |
| two team-membership write skews | BLOCKED | BLOCKED | no declared invariant (multi-row invariants are never inferred) |
| an insights-generation write skew | BLOCKED | BLOCKED | no declared invariant |

Replica semantics are the Phase 13 model (one asynchronous replica,
unbounded lag; reads through the replica never observe primary state
unless the model's replica has caught up). **Model-checkable midday
candidates: 0** (Phase 14: 0).

## What would unblock them

* school-ledger: zod `strictObject` (rejecting unknown keys needs the
  object's own keys as a model operation), then the next blockers in the
  provisioning path.
* midday: properties over generated identities ("the final portal id is
  one some serial execution generated"), which needs invariant terms over
  generated values; or a declared property the code's values can express.

## Evidence

No independent external defect is claimed. A model counterexample against
a heuristic hypothesis would not be a confirmed defect; none was produced.

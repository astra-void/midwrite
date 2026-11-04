# Phase 16 real-world coverage

(This document covers Track F's F2–F4 and the brief's §8.3.)

Static discovery with bounded model checking on the pinned corpus. **No
project was executed and no project file was modified.** school-ledger and
unnamed-ai-project are private checkouts: only aggregate counts and generic
causes are reported for them, never a path, selector or identifier.

Raw data: `reports/phase16/corpus/` (`scripts/phase16-corpus.py`: the
Phase 15 binary and the integrated Phase 16 release binary on the same
machine and inputs, median of 3 runs, peak RSS the maximum). The Phase 15
column is that binary re-measured today, not copied from the Phase 15
report.

## Corpus metrics

| project | binary | extracted | effectful | read-only | resources | unknown ops | candidates | model-checkable | model counterexamples | statuses | wall | peak RSS |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: |
| saas-starter | Phase 15 | 37 | 9 | 11 | 23 | 237 | 2 | 1 | 1 | 1 MODEL_COUNTEREXAMPLE (heuristic), 1 UNSUPPORTED | 0.03 s | 13 MiB |
| saas-starter | Phase 16 | 37 | 9 | 11 | 23 | 236 | 2 | 1 | 1 | unchanged | 0.03 s | 13 MiB |
| midday (declared replica factory) | Phase 15 | 1168 | 81 | 63 | 123 | 18 861 | 4 | 0 | 0 | 1 INCONCLUSIVE, 3 BLOCKED | 1.23 s | 149 MiB |
| midday (declared replica factory) | Phase 16 | 1168 | 86 | 84 | 123 | 35 302 | 4 | **1** | 0 | 1 NO_VIOLATION_WITHIN_BOUNDS, 1 UNSUPPORTED, 2 BLOCKED | 1.77 s | 230 MiB |
| school-ledger | Phase 15 | 546 | 70 | 27 | 68 | 13 396 | 9 | 0 | 0 | 9 UNSUPPORTED | 0.72 s | 121 MiB |
| school-ledger | Phase 16 | 546 | 70 | 38 | 68 | 14 061 | 9 | 0 | 0 | 9 UNSUPPORTED | 0.81 s | 131 MiB |
| unnamed-ai-project | Phase 15 | 33 | 3 | 7 | 3 | 881 | 0 | 0 | 0 | — | 0.03 s | 14 MiB |
| unnamed-ai-project | Phase 16 | 33 | 3 | 8 | 3 | 844 | 0 | 0 | 0 | — | 0.03 s | 15 MiB |

midday without the declared replica factory extracts the same 1168
entries but reaches no database handle (0 effectful), as in Phase 15.

**Why unknown operations went up on midday and school-ledger.** Phase 16
follows loop bodies (unrolled), caught-error handlers, `request.json()` and
held predicates, so more code is reached: on midday (declared) the
extraction now records 225 replica `SELECT`s (99 before), 67 transactions
(19) and 78 primary `UPDATE`s (30). Every operation inside the newly
reached code that the model does not support is still recorded, which is
where the extra unknown operations come from (largest: unresolved method
calls 4 987 → 11 182, unsupported transaction shapes 933 → 3 043,
inline-depth limit 365 → 1 389). Nothing previously recorded was dropped;
none of the new operations is approximated as harmless. The per-category
table is in `reports/phase16/corpus/SUMMARY.md`.

Original-code reproductions on the corpus: none (no external project is
executed).

## midday: the four candidates

With `corpora/contracts/midday.json` (the declared replica factory, as in
Phase 15). No business invariant was added: the two write skews still need
a declaration, and only templates are provided
([GENERATED-ID-INVARIANTS.md](GENERATED-ID-INVARIANTS.md#appendix-contract-templates)).

| stage | portal settings (`toggleCustomerPortal`, read-modify-write) | insight generation (`GenerateInsightsProcessor.process`) | `leaveTeam` (write skew) | `deleteTeamMember` (write skew) |
| --- | --- | --- | --- | --- |
| Phase 15 status | INCONCLUSIVE | BLOCKED (write skew, no invariant) | BLOCKED | BLOCKED |
| **Phase 16 status** | **NO_VIOLATION_WITHIN_BOUNDS** | **UNSUPPORTED** (check-then-act) | BLOCKED | BLOCKED |
| source extraction complete | yes | no: `?.` on a non-identifier base (line 89), `Promise.all` with a used result; at run time an unresolved method call (line 64) | yes | yes |
| scenario generated | yes (actual + distinct-key control; the control is unsatisfiable: one customer row) | yes | no (no property to check) | no |
| scenario loaded | yes | yes | — | — |
| property expressible | **yes, now**: "the final `portalId` is a value some serial execution of the same requests produces", stated over generated identity (actor + generator), no concrete ID fabricated | yes: UNIQUE(teamId, periodType, periodYear, periodNumber) on `insights`, SCHEMA_ENFORCED | needs a declaration | needs a declaration |
| property origin | HEURISTIC_HYPOTHESIS | SCHEMA_ENFORCED | — | — |
| model check completed | yes: 3 worlds (base, `actors-differ:params.enabled`, `stored-equal:customers.portalEnabled`), 21–24 states each, complete | no (the actors reach the unsupported call in every world) | — | — |
| property requires a declaration | no (heuristic, labeled as such) | no | **yes** (multi-row invariants are never inferred) | **yes** |
| unsupported semantics | none on the path | see extraction | none | none |

* **portal settings:** the Phase 15 blocker was that every serial outcome of
  the portal id is a generated value (`nanoid(21)`), which no invariant
  term could express. Track E's generated-identity terms express it
  exactly. The heuristic property holds within the bounds in all three
  worlds. That is a bounded result against a guessed property, not a
  proof and not a finding. The defect class the first-party slice shows
  (callers told a token that is not stored) needs a *declared* property
  about what callers are told; midday declares none, so none is
  claimed. **This is the one previously blocked corpus candidate that is
  now model-checkable.**
* **insight generation:** Phase 16 extracts more of the method (the loop
  over periods), so the candidate is now a check-then-act against the
  table's UNIQUE constraint rather than a write skew waiting for an
  invariant. Its first blocker is unchanged from Phase 15: an optional
  chain on a call result. The status is UNSUPPORTED, never safe. The
  appendix template for this entry still applies if a team wants to
  declare its own rule.
* **team membership:** BLOCKED, unchanged and intended.

**Model-checkable midday candidates: 1** (Phase 15: 0).

## school-ledger: the nine candidates (aggregate)

All nine are check-then-act candidates with a HEURISTIC_HYPOTHESIS
property, as in Phase 15.

| | Phase 15 first blocker | Phase 16 |
| --- | --- | --- |
| 7 candidates (two provisioning entries and their server actions) | authorization guard satisfied by stated synthesized preconditions; then zod **`z.strictObject(...)`** not modeled | the guard is satisfied by the same two stated preconditions (actor kind and role list; the guard still runs); **`strictObject` is now modeled and passes** with the conformed synthesized input; the next validation step, **`.superRefine()` on a zod array**, is not modeled → UNSUPPORTED |
| 2 candidates (a test-support entry) | method call on a value of unresolved type | unchanged (UNSUPPORTED) |

Per candidate (identical for each group of seven, aggregated):

* function identity: provisioning entries (private; not named here)
* candidate class: check-then-act; property origin: HEURISTIC_HYPOTHESIS
* first unsupported operation reached at run time: `.superRefine()` on an
  array schema (an arbitrary callback that adds issues through its
  context argument; a refinement callback is never assumed to accept)
* relevant zod schema: a strict object with string, enum and array
  fields; the strict-object, enum, string-length and array checks are now
  modeled and differentially checked against zod 4.6.5
  ([ZOD-VALIDATION.md](ZOD-VALIDATION.md))
* synthesized preconditions: 2 (stated in every scenario: "SYNTHESIZED
  precondition … the scenario explores callers that satisfy this guard;
  it is not assumed of every caller")
* valid worlds generated: the base world and up to three domain worlds
  per candidate; each reaches the same refinement step
* reached database effects: none (the conflicting statements follow the
  validation)
* model-checking result: not reached
* remaining limitations: after `.superRefine()`, the provisioning path
  still contains a Drizzle builder in a transaction that is not awaited
  where it is built, and unresolved method calls in a helper. Its
  entries now carry COMPLETED, BOUNDED_UNROLLING and TRUNCATED loop
  classifications; the TRUNCATED ones are a PostgreSQL error-classification
  helper whose loop condition is not a counter comparison.

**Model-checkable school-ledger candidates: 0** (Phase 15: 0). The blocker
moved one validation step later. Supporting `.superRefine()` faithfully
would mean interpreting arbitrary callbacks, which is outside the Phase 16
subset and a stated non-goal ("a complete Zod interpreter"). Treating the
refinement as always-accepting would bypass validation, which the brief
forbids. No counterexample was forced and none is claimed.

## saas-starter and unnamed-ai-project

* saas-starter: unchanged. 1 model-checkable candidate (a counterexample
  against a heuristic hypothesis, labeled as such) and 1 UNSUPPORTED.
* unnamed-ai-project: 3 effectful entries, 0 candidates in both phases (no
  concurrent hazard shape detected; read-only entries 7 → 8 because one
  more read path is extracted).

## Phase 15 → Phase 16 summary (§8.3)

| metric | Phase 15 | Phase 16 |
| --- | ---: | --- |
| midday effectful entries | 81 | 86 |
| midday candidates | 4 | 4 |
| midday model-checkable | 0 | **1** (NO_VIOLATION_WITHIN_BOUNDS, heuristic) |
| school-ledger effectful entries | 70 | 70 |
| school-ledger candidates | 9 | 9 |
| school-ledger model-checkable | 0 | 0 (blocker: zod `.superRefine()`, then an un-awaited builder) |
| saas-starter model-checkable | 1 | 1 |
| unnamed-ai-project candidates | 0 | 0 |

## Every remaining corpus blocker

| project | candidates | blocker | what would unblock it faithfully |
| --- | ---: | --- | --- |
| school-ledger | 7 | zod `.superRefine()` (array) | a declared or modeled refinement, e.g. modeling the specific uniqueness refinement pattern once it is verified against zod |
| school-ledger | 2 | method call on a value of unresolved type | type resolution of that value |
| midday | 1 | `?.` on a call result; `Promise.all` whose result is used | optional chains on non-identifier bases; `Promise.all` with a destructured result |
| midday | 2 | no declared invariant (write skew) | a team declaring its rule (templates provided) |

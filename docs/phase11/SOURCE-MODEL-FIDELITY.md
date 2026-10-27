# Source-to-model fidelity and counterexample validation

## Extraction completeness (before a scenario is explored)

* Every candidate lists the unknown operations of its entries and whether
  one lies on a path through the candidate's operations
  (`unsupportedOnPath`). Such a candidate is never reported
  `NO_VIOLATION_WITHIN_BOUNDS`; if the model reaches the operation the
  verdict is `UNSUPPORTED` (`unknown_sql_and_undeclared_calls_stay_visible`).
* Synthesis runs every actor through the engine; reaching an unsupported
  operation makes the scenario `unsupported` with the location.
* Callback invocation: wrapper facts from source; a conditional wrapper
  becomes a synthesized precondition naming its guards; an unknown
  invocation blocks the scenario until declared.
* Transaction ownership: inline `db.transaction(async (tx) => …)`, source
  helpers without a loop, declared helpers; anything else is unknown.
* Unknown external calls stay unknown unless declared; a declared external
  is an `External` operation with its channel contract.
* Schema: tables the loader refuses are listed with the reason; partial
  unique indexes are never global constraints; triggers, RLS and CHECK
  constraints the extractor does not read are not modeled.

## Counterexample validation

For every counterexample of every scenario (`pipeline.rs::model_check`):

1. **replayed**: the schedule replays strictly in the model
   (`witness::replay`, every transition enabled);
2. **violates**: the replayed, completed schedule violates the named
   invariant (`witness::check_schedule`);
3. **initial state valid**: the rows passed the loader (not null, CHECK,
   keys) and the source foreign keys;
4. **operations exercised**: which of the candidate's operations the
   counterexample executes (and which not);
5. **minimized**: the model checker's own minimization;
6. assumptions travel with the candidate and the emitted scenario.

A counterexample that fails 1 or 2 makes the candidate `INCONCLUSIVE`. On
the commerce application every counterexample replayed and violated
(`counterexamples_replay_violate_and_exercise_the_candidate`), and every
check-then-act counterexample exercised all of its candidate's operations.

## Model ↔ `commitrift model-check`

Every emitted scenario, loaded by `commitrift model-check`, gives the same
verdict and the same number of states as discovery reported
(`emitted_scenarios_load_and_give_the_same_verdicts_in_model_check`, ≥ 20
scenarios).

## Model ↔ original code

`commitrift model-check <emitted> --verify-source --allow-execution` runs the
actors' original code under the counterexample's schedule
(`reports/phase11/evidence/runtime-*.json`, 3 runs each):

| candidate | class | evidence | runs |
| --- | --- | --- | --- |
| CR-SYNTH-199193 `inviteMember` | check-then-act | original-code-reproduced | 3/3 |
| CR-SYNTH-05E13A `reserveItem` | read-modify-write (lost update) | original-code-reproduced | 3/3 |
| CR-SYNTH-9336E1 `reserveItem` × `reserveItemLocked` | read-modify-write | original-code-reproduced | 3/3 |
| CR-SYNTH-2551DF `claimTask` | status transition | original-code-reproduced | 3/3 |
| CR-SYNTH-EEF9EA `processRefund` | status transition (double payout) | original-code-reproduced | 3/3 |
| CR-SYNTH-BEDC65 `processRefund` | duplicate processing (at-least-once) | original-code-reproduced | 3/3 |
| CR-SYNTH-5E3D04 `checkout` | status transition (double charge) | original-code-reproduced | 3/3 |
| CR-SYNTH-1C8DF2 `checkoutIdempotent` | status transition (the FP's `charges` rows) | original-code-reproduced | 3/3 |
| CR-SYNTH-1C8DF2 `checkoutIdempotent`, distinct request keys | status transition | original-code-reproduced | 3/3 |
| CR-SYNTH-3E97D8 `checkout` | external-effect gap (crash, retry) | original-code-crash-reproduced | 3/3 |

Every statement, result, outcome and final table agreed with the model in
every run; the business functions ran unchanged. The harness
(`fixtures/phase11/commerce-app/verify/harness.ts`) provides what a server
provides (pool, session, provider test doubles). Synthesized arguments
reached it through the declared mapping in the contracts.

The serial-outcome property reads actor outcomes; Phase 11 lets the runtime
evaluate outcome terms when the run has no crash or retry and every
observed outcome equals the model's (checked before). No Phase 7–10
scenario uses outcome terms with source verification, so no earlier result
changes (checked: 0 of the source-verified fixtures).

## A defect found on the way

Discovery extracts far more functions than hand-written scenarios did and
hit a panic in the Phase 7 extractor: inlining a helper emits its return
slot with the caller's call span resolved in the callee's source, and an
offset inside a multi-byte character made the excerpt slice panic (on
school-ledger and unnamed-ai-project, Korean text and typographic quotes).
Fixed by widening the slice to character boundaries; the mislocated source
reference itself is kept, because correcting it would change source
references in existing model-check reports (`LIMITATIONS.md`). Regression
test: `source_excerpts_never_split_a_multi_byte_character` (fails on the
old code).

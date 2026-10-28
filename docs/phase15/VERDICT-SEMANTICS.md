# Track D: verdict semantics and coverage accounting

This document also covers what the brief called INPUT-COVERAGE.

## Status codes are unchanged

`MODEL_COUNTEREXAMPLE`, `NO_VIOLATION_WITHIN_BOUNDS`, `INCONCLUSIVE`,
`UNSUPPORTED`, `BLOCKED` and `STATIC_CANDIDATE` mean what they meant in
Phase 11–14. Phase 15 adds a **coverage** section to every model-checked
candidate and makes the finding text state the input bounds; it does not
redefine a code. A counterexample's property origin is still never
upgraded (`SCHEMA_ENFORCED`, `USER_DECLARED`, `FRAMEWORK_CONTRACT`,
`HEURISTIC_HYPOTHESIS`).

## Four dimensions, kept apart

```json
"coverage": {
  "mode": "relationship-search",
  "sourceSemantics": { "unknownOperations": 0, "unsupportedOnPath": false },
  "initialStates":   { "distinctExplored": 1, "relationships": ["actual: the synthesized initial state and arguments"] },
  "actorInputs":     { "distinctExplored": 2, "relationships": ["actual: the actors use the same key", "actual: parentId = null for every actor"] },
  "schedules":       { "algorithm": "sleep-sets", "worldsComplete": 2, "worldsTruncated": 0, "worldsUnsupported": 0 },
  "branchOutcomes":  { "relevantOutcomes": 8, "reachedOutcomes": 5, "basis": "branch directions taken by each actor running alone …" },
  "worlds": [ { "id": "base", "variant": "actual", "origin": "base", "status": "explored", "verdict": "NO_COUNTEREXAMPLE_WITHIN_BOUNDS" }, … ],
  "domainExhaustive": false,
  "classesExhaustive": true,
  "unexplored": ["values outside the listed relationship classes (each parameter's value domain is not enumerated)", …],
  "firstCounterexampleWorld": "null:parentId",
  "conclusion": "A counterexample was found in initial world `null:parentId` after exploring 2 world(s)."
}
```

(Real output of `discover fixtures/phase15/worlds-app --entry createFolder`.)

* **Source semantics**: unknown operations in the actors' entries and
  whether an unsupported operation is on the candidate's path. An
  unsupported operation the actors reach still makes the candidate
  `UNSUPPORTED` (unchanged).
* **Initial states** and **actor inputs**: the number of *distinct* row
  sets and argument vectors among the explored worlds of the actual
  configuration, and the relationships they establish.
* **Schedules**: per explored world, whether the schedule search completed
  within its bounds, was truncated, or reached unsupported behavior. A
  completed schedule search is complete **for that world only**.
* **Branch outcomes**: two per `JumpIf` in the actors' extracted code;
  reached = directions an actor took **running alone** on an explored
  world's final rows (concurrent schedules may take others). Counted, never
  estimated; no percentage is derived.

## What is (never) claimed

* `domainExhaustive` is **always false** for synthesized inputs: the value
  domains are not enumerated, only relationship classes. It exists so a
  reader cannot mistake a complete class search for a complete domain.
* `classesExhaustive` is true only when world search was on and every
  derived world was explored or was a duplicate (none invalid, unreached,
  unsupported, skipped for budget or after a counterexample).
* A `NO_VIOLATION_WITHIN_BOUNDS` finding now always ends with:
  "Input exploration: no counterexample among N explored initial world(s)
  of the actual configuration (R distinct row set(s), A distinct argument
  vector(s)); schedules completed within the bounds for the explored
  worlds. The input domain was not exhaustively explored[; and the actors
  contain operations the extractor did not understand]; general
  application safety is not established."
* A counterexample in a world other than the base world is reported as
  "In the initial world `<id>` (<relationships>): MODEL_COUNTEREXAMPLE …".
* With `--max-worlds 1` the mode is `single-world` and the unexplored list
  starts with "world search is off (--max-worlds 1): one synthesized
  initial world per variant".

## Unmodeled protections never become findings

Phase 14 made a counterexample on a table with an unmodeled partial unique
index `INCONCLUSIVE`. Phase 15 adds the same rule for a unique rule
`NULLS NOT DISTINCT` over a nullable column, which the model does not
enforce (it treats NULLs as distinct). This was found while building the
first-party application: once declared keys could be instantiated from the
INSERT (`SOURCE-COVERAGE.md`), the safe `createLabel` reported a model
counterexample; on PostgreSQL the original code does **not** reproduce it
(3/3 "not-reproduced", the duplicate is rejected). It is now
`INCONCLUSIVE` with the rule named.

## Caching

Discovery caches **extractions** only (keyed by every file and option they
read, `commitrift-discover-cache-4`). Worlds, invariants, verdicts and
coverage are recomputed on every run, so a change of input domains,
contracts, budgets, failure bounds or properties can never reuse a stale
verification result. Test:
`the_extraction_cache_never_carries_verification_results_across_input_domains`
(the same cache file, `--max-worlds` 8 → 1 → 8: counterexample, no
violation, counterexample). World fingerprints cover everything the model
checker is given and are used for deduplication within a run.

## Regression tests (`crates/commitrift-discover/tests/phase15.rs`)

| requirement | test |
| --- | --- |
| a single selected world is not described as all worlds | `e21_with_world_search_off_a_single_world_is_not_described_as_all_worlds` |
| budget truncation is reported | `a_world_budget_is_reported_as_unexplored_input` |
| unknown constraints remain visible | unknown nullability and unexplored classes in `unexplored` (all coverage tests); synthesis notes for guard preconditions (`a_synthesized_authorization_precondition_is_stated_never_assumed_silently`) |
| unsupported paths prevent strong conclusions | unchanged `UNSUPPORTED` status; Phase 11–14 tests |
| exhaustive finite-domain runs identified | `domainExhaustive` is false and `classesExhaustive` true only when every class ran (E21 test, budget test) |
| changing input domains invalidates results | cache test above |
| historical report versions remain compatible | `schemaVersion` 1, additive fields; 56/56 model reports and 144/144 static outputs identical (`FINAL-REPORT.md` §6) |

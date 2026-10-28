# Track C: feedback-guided world search (and state-space sampling)

Code: `crates/commitrift-discover/src/pipeline.rs` (`discover` world loop,
`run_world`, `unflipped`), `crates/commitrift-discover/src/worlds.rs`.
This document also covers what the brief called STATE-SPACE-SAMPLING.

## Loop

For each **actual** variant of a candidate, when `--model-check` is on and
`--max-worlds` > 1:

1. Build and check the base world (as before).
2. If it has no validated counterexample (or `--all-worlds`), collect
   pending worlds:
   * **domain worlds** (`domain_worlds`): every value class of every
     parameter domain, applied to all actors that read the path;
   * **actor worlds** (`actor_worlds`): a non-key argument the actors share
     differs (independent requests only);
   * **relationship worlds** (`relationship_worlds`, from the base world):
     for every column an actor writes from an argument, if a stored value
     equals the written value, a world where it differs
     (`stored-distinct:<table>.<column>`), and if it differs, one where it
     is equal (`stored-equal:…`); keys are never changed;
   * **boundary worlds** (`boundary_worlds`): a guard compares a column of
     a row the actor read with an integer → stored values operand − 1,
     operand, operand + 1 (`stored-boundary:<table>.<column>=<v>`), except
     values a declared `nonNegative` excludes.
3. Order by priority, then name (deterministic):
   `stored-distinct` of a column the actor also reads back (10),
   `stored-boundary` (15), `null` (20), `undefined` (22), actor worlds
   (26), operands (30–34), other `stored-distinct` (40), `stored-equal`
   (50).
4. Take the next world, build it with `build_world`, fingerprint it, check
   it. After every built world its solo runs' branch outcomes are merged;
   a domain world over a parameter that a branch has so far taken in one
   direction only moves ahead by 10 (its `feedback` says so).
5. Stop at the first **validated** counterexample (replays and violates)
   unless `--all-worlds`, or when `--max-worlds` attempts were made; every
   world not attempted is listed with the reason.

Controls and hypothetical variants keep one world (they never decide a
candidate's status).

## The two historical false negatives

* **P14-E21** (`saveDisplayName`): the base world stores the written text,
  the stale replica read returns it, the call returns `true`: no
  violation. `relationship_worlds` sees `accounts.displayName` written
  from `displayName` with an equal stored value, and the actor reads the
  table back → `stored-distinct:accounts.displayName` is first; it gives a
  counterexample (the call returns `false` although its write committed).
  The rule names no table, column or fixture.
* **Phase 13 E26** (`createFolder`): `parentId: number | null` →
  `null:parentId` for both actors → both inserts succeed under the NULLS
  DISTINCT unique key → the declared/heuristic property fails. The
  `parentId === null` branch, taken one way in the base world, is taken the
  other way in the null world (branch outcomes 3 → 5 of 8).

## Deduplication

`fingerprint` hashes everything the model checker is given (the scenario
document without its name and description: rows, arguments, invariants,
isolation, bounds, features, tables, overrides). Equal fingerprints are
explored once (`duplicate`). Worlds whose values differ are never merged.

## Budgets

| budget | flag | reported |
| --- | --- | --- |
| worlds per actual variant (attempts, base included) | `--max-worlds N` (default 8; 1 = off) | `budgets.maxWorldsPerVariant`, `worldsSkippedForBudget`, per-world `skipped-budget` |
| states per world | `--max-states` | per scenario `complete`; coverage `schedules.worldsTruncated` |
| states over all scenarios | `--max-total-states` | `scenariosSkippedForBudget` |
| wall time / memory per world | `--max-wall-ms`, `--max-memory-bytes` | as above |
| candidates | `--max-candidates` | `candidatesOverBudget` |
| explore after a counterexample | `--all-worlds` | `skipped-after-counterexample` otherwise |

Argument combinations are not enumerated as a Cartesian product: each
world changes one relationship (its own budget is the world budget).

## Feedback signals used, and not used

Used: validated counterexamples (stop), branch outcomes of solo runs
(order), stored-equals-written in the base world (generate), guards over
read rows and arguments (repair), unreached/invalid worlds (report).
Not used: code coverage as a goal (a world is never explored *because* it
raises coverage alone), concurrent-run branch outcomes (only solo runs are
observed; the coverage section says so).

## Reproduction

A validated counterexample of a candidate whose status is
`MODEL_COUNTEREXAMPLE` gets, with `--emit-scenarios <DIR>`, a
`<scenario>.counterexample.json`: candidate, class, variant, world and its
relationships, property and origin, status and finding, initial rows,
actor arguments, assumptions, bounds, algorithm, schedule, final terms,
source fingerprint (FNV-64 of the entry modules and schema modules), the
evidence level ("model counterexample (not an original-code
reproduction)") and the commands to replay it in the model and, with a
harness, on the original code. Every value is synthesized; nothing is read
from a database or the environment.

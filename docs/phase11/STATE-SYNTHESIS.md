# State synthesis and scenario construction

`commitrift-discover/src/synth.rs`. A generated scenario is an ordinary
scenario file (`schemaVersion` 1) with tables read `from` the Drizzle schema,
entries selected by selector, synthesized arguments and rows, invariants
with their origin, and bounds; it is loaded and validated by the scenario
loader like any other.

## Tables

Every table an actor reads, writes or joins, the property's table, and the
tables their foreign keys reference (transitively), ordered so that every
table follows the tables it references (PostgreSQL checks foreign keys row
by row when the runtime seeds them). Tables from several schema modules in
one scenario are refused (the loader reads one schema module).

## Arguments

From the parameter paths the extracted code reads, typed by the columns
they are compared with or written to: key-like integers (a primary,
foreign or unique column, or compared by equality in a predicate) get
distinct small integers in path order; other integers 3; booleans `true`;
text `alice@example.test` for e-mail paths, else `<field>-1`. Declared
`arguments` replace them; declared `values` replace single paths (for
validation code the model does not run, e.g. a role enum checked by a
wrapper).

Relations between actors (variants):

| relation | second actor |
| --- | --- |
| `same` | identical arguments |
| `distinct-key` (control) | the candidate's key paths changed (`+100`, `alice`→`bob`) |
| `distinct-row` | the written row's key changed, the group kept (write skew) |
| `swapped` | the two lock keys swapped (lock order) |
| `distinct-request-key` | only the idempotency key an effect sends changed (assumed client behavior) |

## Rows: running each actor alone

Rows are not invented from templates. Each actor runs **alone** in the
model's engine (`engine::{initial_state, enabled, apply}` with a recorder),
from the rows found so far:

1. **Pre-seeding** (source-grounded correlation). An object argument whose
   fields are columns of exactly one table including its single primary key
   is taken to be that table's row (preferring the table a foreign key of
   the code names: `invitedBy = $member.id` → `members`); the rows a
   candidate's UPDATE/DELETE targets write are seeded from the arguments,
   with the equality conditions of the read they follow (the write-skew
   rows are the checked group). Each is reported as a note.
2. **Loop** (at most 16 iterations): if the actor did not execute every
   operation of the candidate — an UPDATE/DELETE that changed no row does
   not count — the last read on its path that came back empty is satisfied
   by adding a row built from its *closed* predicate (`=`, `>=`, `>`, `<=`,
   `<`, `!=` against literals; contradictory equalities are refused). A
   joined read gets its missing base row or the missing row of a joined
   table from the join condition. A read the candidate needs empty — the
   check of a check-then-insert — is never satisfied. Count reads get rows
   until the count passes.
3. **Completion** of every added row: primary key (next integer or
   `<table>-n`), constant defaults, sequence values, not-null columns
   (`10`, `<column>-n`, `true`), parent rows for foreign keys (to depth 6),
   unique sets checked.
4. **Foreign keys of inserted rows**: the rows the actor inserts must find
   their parents; missing parents are added and the actor run again.
5. **Final round**: every actor runs again alone on the final rows and must
   still reach its operations; the rows each read returned are recorded (a
   property key may depend on them).

The result is `satisfied`, `unsatisfiable` (with the reason: a predicate
that cannot be satisfied, contradictory conditions, an unreachable
operation, a foreign key that cannot be met) or `unsupported` (the actor
reaches an operation the model does not support — the location is given).
The loader then checks not-null, CHECK constraints, primary and unique keys;
discovery checks the source's foreign keys. A scenario never reports a
violation caused by a state the real schema forbids — within the modeled
constraints; triggers, row-level security and constraints the extractor
does not read are not modeled (`LIMITATIONS.md`).

## Variants

| variant | kind | when |
| --- | --- | --- |
| `actual` | actual | always: the application as analyzed |
| `distinct-key` | control | multi-actor check-then-act, read-modify-write, status transition |
| `distinct-request-key` | actual (assumed client behavior, stated) | multi-actor candidates whose effects send an argument as idempotency key |
| `hypothetical-unique` | hypothetical | `--hypothetical`, check-then-act without a covering unique set |

Only `actual`-kind variants decide a candidate's status (the first with a
counterexample, else the first).

## Bounds

`maxSteps = max(64, 3 × operations × (1 + retries))`, `maxStates` from
`--max-states` (200 000), `maxWallMs` from `--max-wall-ms` (60 s),
`maxCrashes`, `maxRetries` (raised to the code's transaction-helper retries,
at most 2, so that a retried transaction runs as the code runs it),
`maxInFlight` from the candidate's environment. A total state budget
(`--max-total-states`) skips later scenarios and counts them.

## Stable artifacts

Scenario files are written with sorted keys (`serde_json` without
`preserve_order`), names `<candidate-id>-<variant>.json`, the project root
relative to the output directory. Two runs produce identical files
(`discovery_is_deterministic_and_scenarios_are_stable`). With harness
mappings in the contracts, a `sourceVerification` section is added;
running it still needs `--verify-source --allow-execution`.

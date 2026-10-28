# Track B: constraint-aware initial-state synthesis

Code: `crates/commitrift-discover/src/synth.rs` (`build_world`,
`satisfy_guard`, `argument_guard`, `stopped_at`),
`crates/commitrift-discover/src/worlds.rs` (`distinct_value`,
`apply_cells`, `identity_column`).

## Strategy: the engine is the constraint solver

The Phase 11 design is kept and extended: rows are found by running each
actor **alone** through the model's own engine and repairing what stopped
it, then every actor again on the final rows; the scenario loader then
checks every modeled constraint. Phase 15 adds no SMT solver: every gap
found in this cycle was a missing *repair*, not a search the engine could
not do (`INPUT-DOMAINS.md`, below).

## Constraints used, and their origin

| constraint | origin | how |
| --- | --- | --- |
| primary keys, unique keys | schema | `Rows::add` keeps keys unique; the loader re-checks |
| foreign keys | schema | parents added first; inserted rows' parents added; `fk_violations` |
| NOT NULL, defaults, types | schema | typed fills (Phase 14: time, JSON; uuid) |
| enum values | schema | `distinct_value` chooses another enum member |
| `varchar(n)` | schema | `distinct_value` stays within `n` |
| string-literal unions | source (annotation) | argument values (`INPUT-DOMAINS.md`) |
| read predicates | source | `row_for` satisfies the last empty read (Phase 11) |
| guards over a read row | source | **new**: `satisfy_guard` |
| guards over an argument | source | **new**: `argument_guard` |
| zod validation | source | extracted as code (Phase 14); unmodeled schemas stop synthesis as UNSUPPORTED |
| declared contract values, arguments | user | fixed; never varied |
| declared invariants | user | a stored boundary value a declared `nonNegative` excludes is not generated |

## Guards (B6)

When every read on an actor's path found a row and the actor still stopped
before the candidate's operations, a guard stopped it. Two repairs, each
tried once per (row or path, value), within the seeding bound:

* **stored row**: a branch the run evaluated compares a property of a row
  the actor read (`membership.role !== "admin"`, through the locals the
  condition tests) with a literal, or orders it against an integer
  (`org.vacancies > 0`); the stored row's column is set to a value that
  passes (the literal, or operand ± 1). Identity columns (primary, unique,
  foreign keys) are never changed.
* **argument**: a branch compares an argument path — followed through the
  locals an inlined callee binds (`isSchoolAdmin$actor.kind`) and through
  every assignment of a condition local — with a literal, or tests
  membership (`actor.schoolRoles.includes("school_admin")`); the argument
  is set for every actor of that entry. Declared values and paths the world
  chose are never changed.

Each repair is recorded in the scenario's synthesis notes as
`SYNTHESIZED precondition of <actor>: … (the scenario explores callers
that satisfy this guard; it is not assumed of every caller)`.
Authentication is never bypassed: the code still runs every guard; the
scenario only chooses a caller and rows for which it passes. When no
repair applies, the world stays unsatisfiable and the message now says
where the actor stopped (`(the actor ended after <location> with:
<failure>)`).

## Multiple worlds

`build_world(spec)` applies a world's **argument overrides** before rows
are synthesized (rows then follow the arguments: a `null` parent needs no
parent row) and its **cell overrides** after every actor's first solo run
(to the rows the actors need), then requires every actor to reach the
candidate's operations alone on the final rows. A world in which an actor
no longer reaches them is a valid state where the candidate does not
occur: reported `unreached`, not checked.

## Satisfiability vocabulary

| world outcome | meaning |
| --- | --- |
| `explored` | built, loaded, model-checked |
| `unreached` | built from valid inputs, but an actor stops before the candidate's operations |
| `invalid` | no valid initial state found for this world (a constraint cannot be met within the bounds) — not a proof that none exists |
| `unsupported` | the actor reaches an operation the model does not support |
| `duplicate` | same fingerprint as an explored world |
| `skipped-budget`, `skipped-after-counterexample` | not attempted |

`SATISFIABLE`/`UNSATISFIABLE`/`UNKNOWN`/`UNSUPPORTED`/`BUDGET_EXCEEDED` of
the brief map to `explored`/(none: nothing is proved unsatisfiable)/
`invalid`/`unsupported`/`skipped-budget`: an `invalid` world is never
reported as proof that no valid state exists.

## Provenance

Every world records its relationships in words, its origin (`base`,
`domain`, `relationship`), why it was ordered where it was (`feedback`),
its fingerprint, the synthesized rows and arguments, the synthesis notes
(guard preconditions, rows taken from argument objects, unmodeled NOT NULL
columns) and the property origins; the scenario file records the schema
facts the loader used and those it did not model.

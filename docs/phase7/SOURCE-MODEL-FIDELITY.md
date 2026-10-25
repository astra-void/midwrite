# Source-to-model fidelity

## Operation identity

Every extracted operation keeps a `SourceRef`: file, line, column, the
function it is written in (the static analyzer's name for it, by
position), the inlining call chain, a source excerpt, and — new in Phase 7,
in memory only to keep report schema v1 stable — the last line of its
span. `commitrift model-check` links it to the Effect IR effect at the same
position and to existing findings (Phase 6). At runtime a statement is
linked to an operation only when its innermost *application* stack frame
(source-mapped TypeScript) lies inside that span; a statement without an
application frame is linked by kind and table only and the comparison says
so. Links are never made by function or method names.

Chain from source to PostgreSQL, as recorded per statement in
`sourceVerification.statements`:

```
src/app/actions.ts:80:11 (model op, span 80–86)  ──  INSERT invitations {...}  (model transition #7)
      ▲ innermost application frame src/app/actions.ts:80:34
insert into "invitations" (...) values (default, $1, ...)   (Drizzle's SQL)  → 1 row
```

## Facts vs assumptions

The extraction report (`extraction[]` in report schema 2) separates:

* `sourceFacts` — what the extractor derived: the entry's identity, the
  wrapper's definition, how it invokes the callback, its guards and
  arguments;
* `scenarioAssumptions` — what the scenario declared: invocation mode,
  preconditions (authenticated user, validation succeeded). These are never
  displayed as proven; at runtime they are *established by the harness
  input* for that run only.

Other assumptions stay in the report's `assumptions` list: key-order
results without `orderBy`, sequence starts, unmodeled columns, initial
state, isolation, actor concurrency.

## Semantic coverage

`sourceCoverage` counts operations of the concurrency IR, not AST nodes:

* `representedOperations` — database statements, transaction control,
  external effects, application state;
* `assumedOperations` — operations resting on a declared assumption (the
  wrapper's own code around the call, each declared precondition);
* `unsupportedOperations` (with details) — `Unknown` operations.

The team-app and original saas-starter `inviteTeamMember` callbacks: 5
represented, 4 assumed (wrapper + 3 declared contract items), 0
unsupported.

## Strict validation

* Unsupported behavior never disappears: an expression the extractor did
  not understand inside a local operation (`let`, a condition, an
  external's payload) now turns that operation into `Unknown` at scenario
  load (`seal_unknowns`). Before Phase 7, evaluating it *failed the actor*,
  which silently turned unsupported source into a modeled exception.
* Reaching an `Unknown` makes the verdict `UNSUPPORTED`; an unreachable one
  changes nothing (`exception_handling_beyond_the_subset_is_unsupported_not_guessed`).
* Source verification refuses (`unsupported`) actors declared in the
  scenario instead of extracted, actors without a harness entry, entries
  whose harness selector differs from the actor's, and counterexamples with
  crashes, retries or deadlock victims.
* At runtime, a write the application sends outside the modeled function
  makes the verification `inconclusive`; an unexpected statement inside it
  makes it `diverged`. Neither is skipped to force the expected schedule.

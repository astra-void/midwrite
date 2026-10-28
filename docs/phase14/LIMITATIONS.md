# Phase 14 limitations

What Phase 14 lifted, what it still refuses or approximates, and what was
not measured. Earlier phases' limitations that this file does not lift
still apply (`docs/phase13/LIMITATIONS.md`; current list:
`docs/reference/limitations.md`).

## Lifted in Phase 14

* Callbacks passed to project functions (`withRetry(() => …)`): inlined
  where called, with their captured scopes (`SOURCE-COVERAGE.md`).
* zod `parse`/`safeParse` for a documented subset of schemas.
* Bounded arrays, strings (template literals, concatenation, the common
  methods), `undefined` distinct from `null`, `Map`/`Set` — under the
  `js-values` feature (`VALUE-SEMANTICS.md`).
* Multi-row `INSERT`, `inArray`/`notInArray`, conditional predicates,
  `timestamp`/`timestamptz`/`date` and `json`/`jsonb` columns (subsets;
  `DRIZZLE-DIFFERENTIAL.md`).
* `nanoid()` / `crypto.randomUUID()` as generated values.
* Ambiguous commit of a transaction retried by a catch block; a
  parent-delete × child-insert family; declared return-value properties.
* A load-dependent false divergence of the original-code runner (a waiting
  statement answered while the coordinator waited for the lock holder's
  COMMIT); generated keys at connection-loss and server-completion
  checkpoints; a local `EvalError::Unknown` treated as a catchable
  exception; serial-outcome properties that turned booleans and generated
  values into `null`.

## Refused (unsupported, never approximated)

* `values(obj)` with an object that is not a literal or `const` (the keys a
  caller passes are not known; Drizzle sends `DEFAULT` for a missing key).
* zod checks other than the listed ones (`email`, `uuid`, `regex`, ...),
  `optional`/`nullable`/`default`, `transform`/`refine`/`coerce`, unions,
  arrays, `passthrough`/`strict`.
* Array mutation, spread, index assignment, `===` on arrays/`Map`/`Set`,
  iteration over a `Map`/`Set`, `toLowerCase`/`toUpperCase` of non-ASCII
  text, `String()` of objects or generated values, a string method on a
  generated value.
* `for`/`while` loops, `for … of` over an array of unknown origin;
  framework request objects (`req.json()`, `Response`).
* Time arithmetic (`Date#getTime`, ordering clock readings), non-default
  Drizzle time modes, a `new Date()` held in a local and used by several
  statements; JSON comparisons and `->>`.
* Nested conditional predicates; `inArray` over a subquery or computed list.
* Multi-row statements in flight (a lost acknowledgment of a multi-row
  autocommit statement).

## Approximations and assumptions, stated

* `clock-readings-distinct`: distinct clock readings differ from each other
  and from explicit values; `now()` is the transaction's start time.
* `generated-values-distinct` now also covers `nanoid()`/`randomUUID()`.
* A partial unique index is still not modeled: a counterexample on a table
  that has one is reported INCONCLUSIVE, never as a finding.
* A zod object schema's input is assumed not to be an array.
* `as const` module arrays are assumed not mutated (casts are not tracked).
* The original-code harness of `fixtures/phase14/request-app` handles errors
  of checked-out node-postgres clients; without that, a broken connection
  during a COMMIT kills the worker (observed) and the retry never runs.
* Pre-existing JavaScript approximations listed in `VALUE-SEMANTICS.md`
  (`null + x`, mixed-type `===`, default parameters) are unchanged.

## Synthesis limitations that affect verdicts

* Row fill and argument synthesis can produce the same text, so an initial
  value can equal the value an actor writes; a stale read is then
  indistinguishable (Phase 14 set E21: NO_VIOLATION for a true positive).
* Arguments are not explored over `null`: a bug that needs a `null`
  argument (Phase 13 set E26) is not found.
* The 7 school-ledger INCONCLUSIVE candidates still stop at an inlined
  authorization guard that tests a field the synthesized argument lacks.

## Not measured / not independent

* **No independent human review performed.** Both evaluation sets were
  written by AI agents without access to the implementation; the Phase 14
  set was scored once before any change, then post-feedback.
* No PostgreSQL streaming replication was run: replica results remain
  model-only.
* Linux was not run (the local Docker daemon was unavailable); Windows was
  not run.
* Original-code verification covers first-party fixtures only; no corpus
  application was executed and no corpus finding is claimed as a defect.

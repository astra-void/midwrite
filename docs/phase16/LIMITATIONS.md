# Phase 16 limitations

The complete list after Phase 16. The authoritative user-facing list is
[docs/reference/limitations.md](../reference/limitations.md); each track
document has the details and evidence for its own area.

## What results mean

* **Bounded.** Two concurrent actors (three where a scenario declares
  them), small synthesized worlds, at most `--max-worlds` worlds per
  variant, loops unrolled at most 8 times.
  `NO_VIOLATION_WITHIN_BOUNDS` is never a proof of safety.
* **Inputs are explored by relationship class, not by value.** Every
  candidate's coverage says `Input domain exhaustive: no` and lists what
  was not explored.
* **Heuristic properties are guesses.** A `HEURISTIC_HYPOTHESIS`
  counterexample (for example the Phase 12 `transferCredits` deadlock
  against "no invocation fails") is real model behavior, and may be real
  PostgreSQL behavior, but it violates a rule CommitRift guessed. Only
  `USER_DECLARED` and `SCHEMA_ENFORCED` properties are the application's
  own rules.
* **No independent human review** of any evaluation set has been
  performed ([PRECISION-RECALL.md](PRECISION-RECALL.md)).

## Not modeled (always reported, never treated as safe)

| area | not covered | reference |
| --- | --- | --- |
| zod | refinements (`refine`, `superRefine`), transforms, preprocess, coerce, unions and discriminated unions; Yarn PnP or `paths`-aliased zod copies (version unknown → version-dependent behavior refused); output key order | [ZOD-VALIDATION.md](ZOD-VALIDATION.md#limitations) |
| loops | `for…in`, `for await`, labeled `break`/`continue`; a `while` bound whose counter start is not a known integer; more than 8 iterations (TRUNCATED); backoff delays (unknown calls) | [LOOP-SEMANTICS.md](LOOP-SEMANTICS.md#limitations) |
| caught errors | errors from lost connections (property `code` unknown); `instanceof` an application error class; error messages; driver stacks other than drizzle-orm 0.45 with pg 8 / postgres 3.4 | [LOOP-SEMANTICS.md](LOOP-SEMANTICS.md#caught-errors) |
| requests | `request.text()`, `formData()`, `arrayBuffer()`, query strings; a body read twice; Server Actions receiving `FormData`; tRPC procedures | [FRAMEWORK-REQUESTS.md](FRAMEWORK-REQUESTS.md#not-supported-documented) |
| predicates | a `let` predicate assigned again; clock readings in a held predicate; nested conditionals; helpers with more than `return <pred>`; operators outside the list (`like`, `between`, `exists`, raw `sql`) | [DRIZZLE-PREDICATES.md](DRIZZLE-PREDICATES.md#3-what-stays-unknown-never-approximated-as-true) |
| multi-row reads | `orderBy` other than the primary key; joins in multi-row mode | [MULTI-ROW-LOCKS.md](MULTI-ROW-LOCKS.md#7-limitations) |
| uniqueness | partial unique indexes; collation-aware equality; an NND rule with an unmodeled column | [NULLS-NOT-DISTINCT.md](NULLS-NOT-DISTINCT.md#approximations-and-limitations) |
| generated identity | quantifiers over generation events or attempts; distinguishing package-call sites; `UPDATE … RETURNING` row values | [GENERATED-ID-INVARIANTS.md](GENERATED-ID-INVARIANTS.md#7-approximations-and-limitations) |
| JavaScript | standard-library calls (`Set`, `Number.*`, `Math.*`); optional chains on call results; `Promise.all` with a used result; computed member access; recursion | [supported-patterns.md](../reference/supported-patterns.md) |

## Approximate (stated in each affected report)

* **Lock order of a multi-row lock without `.orderBy(primaryKey)`.** The
  model locks in key order; PostgreSQL locks in scan order. In a
  measurement with rows out of key order, 24 of 51 schedules differ.
* **`generatedBy`** matches any generation event of that actor and
  generator, not a specific occurrence.
* **Read replicas.** One asynchronous replica with unbounded lag.
  Counterexamples are model-only: the lockstep harness has no replica.
* **Generated identity on original code.** Which actor generated a value is
  not observable there, so such properties are reported `not-evaluated`.

## Platform and environment

* The lockstep matrices ran on PostgreSQL 17.11 (macOS) and 15.19 (Linux
  container), both with 0 disagreements
  ([QUALITY-GATES.md](QUALITY-GATES.md#linux)). PostgreSQL 16 was not run;
  PostgreSQL ≤ 14 has no NULLS NOT DISTINCT.
* The zod differential runs Node.js on the pinned zod packages. The test's
  default package paths are those of the development machine and must be
  set with `COMMITRIFT_ZOD3_DIR`/`COMMITRIFT_ZOD4_DIR` elsewhere (the Linux
  script installs them).

## Real-world reach

On the measured projects, most candidates still stop at an unsupported
construct:

* school-ledger: 0 of 9 model-checkable, stopping at zod `.superRefine()`
  and an unresolved method call;
* midday: 1 of 4 model-checkable, 2 needing a declared invariant, 1
  stopping at an optional chain.

The blockers are listed one by one in
[REAL-WORLD-COVERAGE.md](REAL-WORLD-COVERAGE.md#every-remaining-corpus-blocker).
On the Phase 16 frozen set, recall is 0.333. The largest single cause of
missed cases is JavaScript standard-library calls (5 of 12).

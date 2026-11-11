# Loops, retry loops and caught errors

Code: `crates/midwrite-analyzer/src/concurrency/loops.rs` (loops, update
expressions, caught errors, request bodies), hooks in
`crates/midwrite-analyzer/src/concurrency.rs`, the caught-error object in
`crates/midwrite-model/src/{ir,engine,jsfn}.rs`, retry-loop repeats in
`crates/midwrite-discover/src/detect.rs`.

Everything here is active only with JavaScript value semantics
(`ExtractRequest::js_values`, scenario feature `js-values`, always on in
`midwrite discover`). Without it, `for`/`while`/`do…while` are unsupported
operations and a caught error is opaque. Part of
[source extraction](source-extraction.md).

## Loops: unrolling, never merging

`for (init; test; update)`, `while (test)` and `do … while (test)` are
unrolled into the existing `Let`/`JumpIf`/`Jump` operations; there is no
loop operation in the IR. For every iteration the extractor lowers the
source again: the test (as a run-time `JumpIf !test → end`, skipped before
the first iteration of `do…while`), the body in its own scope (statements,
`await`s, transactions, generated values, inlined helpers and closures),
`continue` targets, a fresh per-iteration binding for `let`/`const` loop
variables (ECMAScript's `CreatePerIterationEnvironment`, so a closure
created in iteration *k* keeps iteration *k*'s binding), then the update.

`break` jumps past the unrolled loop; `return` and `throw` leave as
everywhere else; `try` blocks left by `break`/`continue` are closed.
Iterations are never merged and a statement is never split: a multi-row
`INSERT` in the body is one atomic statement per iteration, and a
transaction opened in the body is a new transaction per iteration.
Generated values are new identities in every iteration. `x++`, `++x`,
`x--`, `--x`, `x += e`, `x -= e` on locals are supported statement forms
(`x += e` is string concatenation when a side is statically a string).

### Classification

Each loop gets one classification, reported as an extraction assumption
with a stable prefix (`loop <CLASS> at <file:line:col>: ...`). Discovery
copies these assumptions (and the `caught-errors: ` and `request: ` ones)
into every candidate of the entry; `--explain-coverage` prints them.

| class | when | lowering |
| --- | --- | --- |
| `BOUNDED_UNROLLING` | the counter analysis establishes at most `MAX_UNROLL` (8) iterations | every iteration unrolled, nothing after the last |
| `TRUNCATED` | no bound is established (no test, `while (true)`, a bound from an argument, a counter assigned otherwise, more than 8 iterations) | 8 iterations, then the test once more and an `Op::Unknown` "loop may execute beyond the explored bound": executions within the bound are exact, one that would start another iteration stops as unsupported |
| `UNKNOWN` | the loop's condition is not modeled, or nested loops exceed `MAX_UNROLL_PRODUCT` (64) iterations | an `Op::Unknown` |
| `UNSUPPORTED` | `var`/`using` or destructuring in a `for` head | an `Op::Unknown` |
| `COMPLETED` | `for…of` over an array literal | unrolled; the assumption is added |

`for…of` over a list value (a bounded loop over a snapshot) and over a
`Map`/`Set` carry no assumption; `for…in`, `for await` and labeled
`break`/`continue` are refused.

### The counter analysis

A bound comes only from an analysis, never from a guess. It is
established when:

* the test is `x < N`, `x <= N`, `x > N` or `x >= N` (either side), `x` a
  local and `N` an integer the source fixes (a literal, a module-level
  `const`, or a local every assignment so far set to that integer);
* `x`'s value at the loop start is a known integer `A`;
* one increment toward the bound runs on every iteration: the `for`
  update, or for `while`/`do…while` a top-level body statement when the
  body has no `continue`;
* after unrolling, **every** assignment of `x` emitted inside the loop —
  inlined helpers, closures and spawned tasks included — moves it toward
  the bound by a constant (checked on the emitted code).

The bound is `ceil((N − A)/k)` iterations for a strict comparison,
`floor((N − A)/k) + 1` otherwise (0 when the start already fails the
test, at least 1 for `do…while`). Extra increments only end the loop
sooner: the run-time test stays in every iteration.

## Retry loops

A retry loop around a statement or a transaction re-executes its body for
real: the statements of attempt *k* are separate operations, generated
values are new events, a reused idempotency key is the same value, a
transaction is begun and committed again. The application's loop decides
the retries; the model adds none. Failures come from the model's existing
fault model ([failure model](../concepts/failure-model.md)): SQLSTATE
40001/40P01 from the modeled PostgreSQL, a lost acknowledgment with the
process alive (the server committed or not, the driver reports an error),
crashes. A failure caught by the loop's `catch` continues with the next
iteration; a crash runs no handler; a declared environment retry re-runs
the whole actor. A backoff `await sleep(ms)` or `setTimeout` promise is an
unknown call, so a path that reaches it is unsupported.

Retry helpers that are *not* loops are bound by a declared contract
(`extraction.transactionRetries`): the model's `Begin { retry }` predicts
the helper's BEGIN/40001/ROLLBACK/BEGIN, and during original-code
verification the application's own helper runs.

The discovery family `ambiguous-commit` pairs a write inside a `try` block
with the same write reachable from that `try`'s handler — the next
iterations of a retry loop ([scenario synthesis](scenario-synthesis.md)).

## Caught errors

A `catch (e)` whose body uses `e` other than as the argument of `throw`
(`e.code`, `e?.code`, `e.cause?.code`, `e instanceof Error`,
`isSerializationFailure(e)`, `last = e`) is an *inspecting* handler: its
`Op::Try` gets `inspect: Some(CaughtErrors)` and the catch parameter is
bound to the error object. A handler that only re-throws keeps the opaque
binding. At run time the failure carries its origin:

| failure origin | error object (a `Value::Row`) |
| --- | --- |
| a modeled statement's or COMMIT's database error | the verified shape below; without one, an object whose properties are unknown |
| `throw new Error("text")` (or another global built-in error constructor with at most one string literal) inside an inspecting `try` | `{ name, message, code: undefined, cause: undefined }`, `instanceof Error` |
| a re-thrown caught error (`throw e`) | the same object, unchanged |
| anything else: a lost connection, a failed child task, an evaluation error, a `throw` of another class or value | an object none of whose properties is known |

A listed property (an explicit `undefined` included) is its value; an
unlisted one is an **unsupported evaluation**, never `undefined`. This
holds however the object travels (helpers, locals, `e.cause`). An unknown
failure origin never becomes a PostgreSQL error.

### Verified error shapes

Read from the pinned packages: drizzle-orm 0.45.4 wraps every query error
in `DrizzleQueryError` (`query`, `params`, `cause`; no `code`; `name` is
`"Error"`); its node-postgres transaction sends COMMIT through the same
path, while its postgres-js transaction lets postgres-js issue COMMIT, so
a COMMIT error there is the driver's own. pg 8.23.1's `DatabaseError` and
postgres 3.4.5's `PostgresError` carry the SQLSTATE as `code`. The shape is
chosen from the `package.json` nearest to the module that declares
`drizzle-orm`:

| declared versions | statement errors | COMMIT errors |
| --- | --- | --- |
| drizzle-orm `0.45.x` with `pg` only | `DrizzleQueryError` (`code` undefined, `cause.code` = SQLSTATE) | `DrizzleQueryError` |
| drizzle-orm `0.45.x` with `postgres` only | `DrizzleQueryError` | driver error (`code` = SQLSTATE) |
| anything else | not verified (properties unknown) | not verified |

Only `code` and `cause.code` are modeled; message, query text, severity
and constraint name are unknown. The `caught-errors: ...` assumption states
which shape applies. Consequence worth knowing: with drizzle-orm 0.45,
`e.code === "40001"` or `e.code === "23505"` is **always false** for a
database error (the code is on `cause`), and the model shows such a retry
never running.

Not modeled: errors of a lost connection or the pool (driver-specific
messages and codes such as `ECONNRESET`), `instanceof` of a class other
than `Error` (`DrizzleQueryError`, application error classes), `message`
and `stack`. `try` inside a transaction callback stays refused
(PostgreSQL's aborted-transaction state is not modeled).

Tests: `crates/midwrite-cli/tests/loops.rs` (every loop shape and
caught-error case, plus recovery probes on the original code); fixtures
[`fixtures/apps/loops`](../../fixtures/apps/loops) and
[`fixtures/apps/retry`](../../fixtures/apps/retry) (route handler with a
retry loop, verified against the original code under a lost
acknowledgment).

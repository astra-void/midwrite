# Phase 16 track B: bounded loops, retry loops and caught errors

Code: `crates/commitrift-analyzer/src/concurrency/loops.rs` (new; loops,
updates, caught errors, requests), hooks in
`crates/commitrift-analyzer/src/concurrency.rs` (`statement`,
`try_statement`, `expression_statement`, `expr`, `identifier`, `member`,
`binary`, `call_value`, `inline_here`, `lower_main`, `for_of`, `emit`),
`crates/commitrift-model/src/{ir.rs,engine.rs,jsfn.rs}` (the caught error
object), `crates/commitrift-discover/src/detect.rs` (retry-loop repeats,
assumptions), `crates/commitrift-discover/src/synth.rs` (one line: such
code needs the `js-values` scenario feature),
`crates/commitrift-analyzer/src/concurrency/js_values.rs` (one line:
`value_root` accepts a module-level array of literals),
`crates/commitrift-discover/src/effects.rs` (`Ctx::local`: a local reached
again through its own definitions is a leaf; without it the value
analysis of unrolled loops' chained counters was exponential — the private
corpus' discovery ran for more than 19 CPU minutes before the guard, 7.6 s
(debug build) after).

Everything here is active only with JavaScript value semantics
(`ExtractRequest::js_values`, scenario feature `js-values`, always on in
`commitrift discover`). Without it the Phase 7–15 lowering is unchanged:
`for`/`while`/`do...while` are unsupported operations and a caught error is
opaque.

## Loops

### Lowering: unrolling, never merging

`for (init; test; update) body`, `while (test) body` and
`do body while (test)` are unrolled into the existing `Let`/`JumpIf`/`Jump`
operations; no new IR operation exists. For every iteration the extractor
lowers the source again:

1. the test (not before the first iteration of `do...while`), as a
   `JumpIf !test → end` evaluated at run time with the iteration's values;
2. the body, in a scope of its own: its statements, `await`s, transactions,
   generated values (`Value::Gen` counters are per actor and never reset,
   so every iteration's `nanoid()`/`defaultRandom()` is a new identity),
   inlined helpers and closures, in source order;
3. `continue` targets: the iteration's end;
4. for `let`/`const` declarations of a `for` head: a fresh binding holding
   the current value (ECMAScript's `CreatePerIterationEnvironment`, done
   *before* the update, so a closure created in iteration *k* keeps
   iteration *k*'s binding);
5. the update, as a statement.

`break` jumps past the unrolled loop; `return` and `throw` leave as
everywhere else; `try` blocks left by `break`/`continue` are closed
(`EndTry`, existing `jump_out`). Iterations are never merged and a
statement is never split: a multi-row `INSERT` inside the body stays one
atomic statement per iteration. A transaction opened inside the body is a
new transaction per iteration (its own `Begin`/`Commit`).

`x++`, `++x`, `x--`, `--x`, `x += e`, `x -= e` on local variables are new
statement forms (and `++x`/`x++` in expressions, hoisted; refused inside a
conditionally evaluated operand). `x += e` is a string concatenation when
either side is statically a string, as with `+`.

### Classification

Each loop gets one classification, reported as an extraction assumption
with a stable prefix (`loop <CLASS> at <file:line:col>: ...`). Discovery
copies these assumptions (and the `caught-errors: ` and `request: `
ones) into every candidate of the entry (`detect.rs`,
`base_assumptions`), so the discovery report shows them; model-check
reports list them under the actor's extraction assumptions.

| class | when | lowering |
| --- | --- | --- |
| `BOUNDED_UNROLLING` | the counter analysis below establishes at most `MAX_UNROLL` (8) iterations | every iteration unrolled, nothing after the last |
| `TRUNCATED` | no bound is established (no test, `while (true)`, a bound from an argument, a counter assigned otherwise, more than 8 allowed iterations) | 8 iterations (or the established count), then the test once more and an `Op::Unknown` "loop may execute beyond the explored bound": an execution that would start one more iteration stops as unsupported; executions within the bound are exact |
| `UNKNOWN` | the loop's condition is not modeled (`Expr::Unknown` on its first lowering), or nested loops exceed `MAX_UNROLL_PRODUCT` (64) iterations | an `Op::Unknown` |
| `UNSUPPORTED` | `var`/`using` or destructuring in a `for` head | an `Op::Unknown` |
| `COMPLETED` | `for...of` over an array literal (Phase 7 unrolling) | unchanged; the assumption is added |

`for...of` over a list value (Phase 14 `for_of_value`, a bounded loop over
a snapshot) and over a Map/Set are unchanged and carry no assumption;
`for...in` and `for await` are refused as before ("`for` loops are not part
of the supported concurrency subset", "this `for...of` head is not
modeled").

### The counter analysis (a bound only from a real analysis)

A bound is established when all of these hold:

* the test is `x < N`, `x <= N`, `x > N`, `x >= N` (either side), where `x`
  is a local variable and `N` an integer the source fixes: a literal, a
  negated literal, a module-level `const`, or a local every assignment
  lowered so far set to that same integer (`const_ints`, maintained by
  `emit`; two different values, or any non-integer value, make it unknown
  for the rest of the extraction);
* `x`'s value when the loop starts is such a known integer `A`;
* one increment toward the bound runs on every iteration: the `for`
  update (`x++`, `++x`, `x += k`, `x = x + k`, and the decrements for
  `>`/`>=`), or, for `while`/`do...while`, a top-level statement of the
  body when the body has no `continue` (a `continue` could skip it);
* after unrolling, **every** `Let` of `x` emitted inside the loop — the
  body, inlined helpers, closures and spawned tasks included — is
  `x + k` / `x - k` toward the bound, or one of the per-iteration copies.
  This check is on the emitted code, so a helper or closure that assigns
  the counter is seen.

The bound is then `ceil((N − A)/k)` iterations for a strict comparison,
`floor((N − A)/k) + 1` otherwise (0 when the start already fails the test;
at least 1 for `do...while`), where `k` is the guaranteed increment. Extra
increments elsewhere only end the loop sooner: the run-time test stays in
every iteration.

### Retry semantics

A retry loop around a statement or a transaction re-executes its body for
real: the statements of attempt *k* are separate operations, generated
values are new events, a reused idempotency key is the same value (the
same local), a transaction is begun and committed again. The application's
loop decides the retries; the model adds none:

* `Op::Begin.retry` stays the transaction helper's own (Drizzle's
  `transaction` retries nothing; declared retry helpers keep Phase 8
  semantics);
* failure injection is the model's existing one: SQLSTATE 40001/40P01
  from the modeled PostgreSQL, a lost acknowledgment with the process alive
  (`maxInFlight`, `TKind::LostCommitted`/`LostNotCommitted`: the server
  committed or not, the driver reports an error), crashes. A failure caught
  by the loop's `catch` continues with the loop's next iteration; a crash
  runs no handler; a declared environment retry (`OperationRetry`) re-runs
  the whole actor as before;
* a backoff `await sleep(ms)` of a package or a `setTimeout` promise is
  not modeled (an unknown call), unless a verified contract exists (none
  is added in this phase): a retry loop with a backoff call is unsupported
  on the path that reaches it.

The discovery detector `ambiguous_commit` (`detect.rs`) now also pairs a
write inside a `try` block with the same write reachable from that `try`'s
handler (`retry_reach`): the next iterations of a retry loop. Before, it
needed the repeat inside the handler region; with a `return` ending the
`try` block, the extracted code's dead `EndTry` made the next iteration
look like the normal continuation.

## Caught errors

### Which handlers get an error object

A `catch (e)` whose body uses `e` other than as the argument of `throw`
(`e.code`, `e?.code`, `(e as {...}).cause?.code`, `e instanceof Error`,
`isSerializationFailure(e)`, `last = e`) is an *inspecting* handler. Its
`Op::Try` gets `inspect: Some(CaughtErrors)` (a defaulted field: existing
scenarios and extractions serialize identically) and `error: Some(local)`;
the catch parameter is bound to `Binding::Caught(local)`. A handler that
only re-throws keeps the Phase 7 opaque binding.

At run time (`engine.rs`, `fail_thread_from`) the failure carries its
origin, and the handler of an inspecting `try` receives:

| failure origin | the error object (a `Value::Row`) |
| --- | --- |
| a database statement's or COMMIT's error (`db_failure`) | the shape of `CaughtErrors.statement` / `.commit` (below); without a verified shape, an object whose properties are unknown |
| `throw new Error("text")` (or `TypeError`, `RangeError`, ...: global built-in constructors, at most one string-literal argument) lowered inside an inspecting `try` block | `{ name, message, code: undefined, cause: undefined }`, `instanceof Error` |
| a re-thrown error object (`throw e` of a caught error) | the same object |
| anything else: a lost connection, a failed child task, an evaluation error, a `throw` of another class or value | an object none of whose properties is known |

Reserved keys (`$reason`: the failure text, so a re-throw fails with the
same text; `$isError`; `$unmodeled`: the origin is unknown) mark these
rows. `jsfn::at` reads a property of such a row through `caught_prop`: a
listed property (an explicit `undefined` included) is its value, an
unlisted one is an **unsupported evaluation**, never `undefined`; on an
`$unmodeled` row every property is unsupported. This holds however the
object travels: through a helper (`Binding::Caught` is passed as a
handle), a local (`const err = e`), a nested property (`e.cause` is itself
such a row). An unknown failure origin never becomes a PostgreSQL error.

`throw e` of a caught error is an `Op::Rethrow`: the object (and its text)
reaches the next handler unchanged. `throw new Error("text")` inside an
inspecting `try` block is lowered as the object then `Op::Rethrow` (the
trace still shows `throw Error("text")`); elsewhere it stays `Op::Throw`.

### How drizzle-orm and the drivers surface database errors (verified)

Sources read (installed packages in `fixtures/*/node_modules`):

* drizzle-orm **0.45.4** `pg-core/session.js`, `PgPreparedQuery.queryWithCache`:
  every query is `try { return await query() } catch (e) { throw new
  DrizzleQueryError(queryString, params, e) }` on every path (cache or
  not). `errors.js`: `class DrizzleQueryError extends Error` sets
  `query`, `params`, `cause`; no `code`, no `name` of its own (`name` is
  `"Error"`).
* drizzle-orm 0.45.4 `node-postgres/session.js` `transaction`: `begin`,
  the callback, then `await tx.execute(sql\`commit\`)` — the COMMIT goes
  through `queryWithCache`, so its error is a `DrizzleQueryError` too; the
  `catch` sends `rollback` and re-throws the original error.
* drizzle-orm 0.45.4 `postgres-js/session.js` `transaction`:
  `this.client.begin(async (client) => ...)` — postgres-js issues COMMIT
  itself: a COMMIT error is postgres-js's own error, not wrapped.
* pg-protocol (pg 8.23.1) `parser.js`: `message.code = fields.C` (the
  SQLSTATE) on the `DatabaseError`.
* postgres 3.4.5 `src/connection.js`: field `C` is mapped to `code`;
  `src/errors.js`: `PostgresError` copies the fields onto the error.

So `CaughtErrors` is chosen from the `package.json` nearest to the module
that declares `drizzle-orm` (`loops.rs`, `caught_shape`):

| declared versions | statement errors | COMMIT errors |
| --- | --- | --- |
| drizzle-orm `0.45.x` (an exact version or `^`/`~`/`=` range whose lower bound is 0.45.x) and `pg`, no other driver | `DrizzleQueryError` (`code` undefined, `cause.code` = SQLSTATE) | `DrizzleQueryError` |
| drizzle-orm `0.45.x` and `postgres`, no other driver | `DrizzleQueryError` | `Driver` (`code` = SQLSTATE) |
| anything else (other drizzle-orm versions, both or neither driver, another driver package) | not verified | not verified |

Only `code` is modeled (and `cause.code`); the message, the query text,
severity, detail, constraint name are unknown properties. The assumption
`caught-errors: ...` states which shape applies. Consequence worth
knowing: with drizzle-orm 0.45, `e.code === "40001"` and
`e.code === "23505"` are **always false** for database errors (the code is
on `cause`); the model shows such a retry never running
(`withdrawOnCode`, `insertOrNoteOnCode`). Phase 14's evaluation app's
`isForeignKeyViolation` has this shape.

Not modeled: errors of a lost connection or of the pool (driver-specific
messages and codes such as node-postgres' "Connection terminated
unexpectedly" or `ECONNRESET`; their shape inside `DrizzleQueryError` was
not verified), `instanceof` of a class other than `Error`
(`DrizzleQueryError`, `DatabaseError`, `PostgresError`), an error's
`message`/`stack`. `try` inside a transaction callback stays refused
(PostgreSQL's aborted-transaction state).

## Evidence

All commands from the repository root of the worktree, PostgreSQL 17.11
(`/opt/homebrew/opt/postgresql@17/bin`), Node 22.

* `target/debug/commitrift discover fixtures/phase16/retry --model-check`:
  `placeOrder` and `POST` (route handler, `request.json()`) are
  `ambiguous-commit` MODEL_COUNTEREXAMPLE against the declared
  `one-order-per-request`; `recordPayment` (ON CONFLICT DO NOTHING keyed by
  the reused idempotency key, UNIQUE) NO_VIOLATION_WITHIN_BOUNDS;
  `withdraw` (SERIALIZABLE, re-run on `cause.code === "40001"`)
  NO_VIOLATION_WITHIN_BOUNDS against `balance-non-negative`. Every
  candidate carries its `loop BOUNDED_UNROLLING` assumption.
* `target/debug/commitrift model-check <emitted placeOrder scenario>
  --verify-source --allow-execution --source-repeat 2`: `reproduced`, 2 of
  2 runs, evidence `original-code-connection-loss-reproduced`: the proxy
  broke the connection after PostgreSQL committed the INSERT, node-postgres
  reported an error, the original `for` loop ran the INSERT again, two
  `orders` rows with one `request_id` were observed.
* `COMMITRIFT_REQUIRE_REPLAY=1 cargo test -p commitrift-cli --test
  phase16_loops`: 10 passed. Besides the model tests of every loop shape
  and caught-error case, two recovery probes ran on the original code:
  `recordPayment` (its INSERT in a transaction) along the model's shortest
  schedule "COMMIT acknowledgment lost, PostgreSQL commits the held COMMIT,
  the loop runs the transaction again": `recovery-reproduced`
  (`ORIGINAL_CODE_RECOVERY_REPRODUCED`, invariant holds in 2 of 2 runs, one
  `payments` row); `withdraw` along "one withdrawal commits, the other gets
  40001, then commits": `invariant-held` (the original loop recognized
  `cause.code === "40001"` and re-ran the transaction).
* `withdrawOnCode` (the same loop testing `error.code`): with the invariant
  "both withdrawals succeed", the model finds a counterexample in which a
  withdrawal fails with 40001 (the wrapper's `code` is undefined), while
  `withdraw` has none.
* Phase 15 `P15-E24`/`P15-E25` and Phase 14 `P14-E12`/`P14-E13` (the
  `withRetry` `for` loop): the loop is now BOUNDED_UNROLLING (3 attempts);
  after a one-line change in `js_values.rs` (`value_root` accepts a
  module-level array of literals, so `RETRYABLE.includes(code)` in an
  `||` operand is a pure built-in call) the entries extract without unknown
  operations, and their first blocker is now precise: "the caught error
  comes from a failure whose error object is not modeled (connection
  terminated unexpectedly ...): its property `code` is unknown". Their
  labels assume the driver rejects with `ECONNRESET`; with drizzle-orm
  0.45 the wrapper's `code` is undefined for every query error, so whether
  `withRetry` retries at all after a lost connection is not established by
  the sources read here.
* The private corpus (static discovery, aggregate counts only): its 9
  candidates stay UNSUPPORTED with the same first blockers as Phase 15
  (zod `strictObject`, an unresolved call); their entries now carry 4
  BOUNDED_UNROLLING, 9 TRUNCATED and 8 COMPLETED loop classifications (per
  candidate) and 35 residual "beyond the explored bound" operations; the
  most frequent caught-error refusal (198 occurrences) is `instanceof` an
  application error class.
* Phase 14 `src/services/projects.ts#addTask` (`isForeignKeyViolation(e)`,
  `e.code === "23503"`) now extracts without unknown operations; under
  drizzle-orm 0.45 the test is false for a foreign-key violation (the code
  is on `cause`).

## Limitations

* `MAX_UNROLL` (8) and `MAX_UNROLL_PRODUCT` (64) are constants, not
  options. A retry loop with more than 8 attempts is TRUNCATED: an
  execution reaching attempt 9 is unsupported.
* A while-loop bound needs the counter's start value to be a known
  integer at the loop (a parameter or a value read from the database is
  not).
* A TRUNCATED loop's residual `Op::Unknown` lies after the loop: discovery
  treats an unknown operation before a candidate's last conflicting
  operation as on the path (`detect.rs`, `unsupported`), so writes after a
  TRUNCATED loop are blocked conservatively.
* Backoff delays are unknown calls; `for...in`, `for await`, labeled
  `break`/`continue` are refused.
* Caught-error shapes are verified for drizzle-orm 0.45 with pg 8 /
  postgres 3.4 only.

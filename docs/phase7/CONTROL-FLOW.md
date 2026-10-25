# Control flow in the concurrency extractor

What Phase 7 adds to the Phase 6 subset ([`../phase6/CONCURRENT-IR.md`](../phase6/CONCURRENT-IR.md)).
Tests: `crates/commitrift-cli/tests/phase7.rs` (control-flow section),
fixture `fixtures/phase7/control/src/orders.ts`.

## Exceptions: `try` / `catch` / `finally`

IR (model `ir.rs`), all local (not scheduling points):

* `Try { handler, error }` — push a handler on the thread; the failure's
  reason is stored in `error` when given (used only to re-throw);
* `EndTry` — pop it;
* `Rethrow { local }` — fail with the stored reason.

Engine (`fail_thread`): a failure of a thread with an active handler —
`throw`, a rejected awaited helper, a database error (e.g. 23505), a failed
`Promise.all` child surfacing at the join — resumes at the handler instead
of failing the actor. If the handler was entered *outside* a transaction
and the actor has a transaction open, that transaction is rolled back
first (Drizzle's `transaction` rolls back and re-throws before the outer
`catch` runs). Handlers are part of the canonical state.

Lowering:

```
try { B } catch (e) { C } finally { F }
  Try(h1) B EndTry F Jump(end)
  h1: Try(h2, err) C EndTry F Jump(end)
  h2: F Rethrow(err)
end:
```

`F` runs on the normal path, after a caught exception, and — re-throwing
afterwards — after an exception in `C` or an uncaught one (`try/finally`).
A `return` from an inlined helper leaves the helper's open `try` blocks
(`EndTry` before the return jump), so a helper's handler never catches its
caller's later failure (`a_helpers_handler_does_not_catch_its_callers_failure`).

**A crash is not an exception.** Crashes are environment transitions that
kill the process: no handler and no `finally` runs. The test
`exceptions_run_catch_and_finally_but_a_crash_runs_neither` shows, for
`processOrder` (charge → insert → catch: record failure → finally:
release), that every non-crashed final state has the release, and that a
crash right after the charge leaves the charge without a release.

Not modeled (→ `Unknown`, so `UNSUPPORTED` if reached):

* `try` inside a transaction callback (after a database error PostgreSQL's
  transaction is aborted; that state is not modeled);
* `return`/`break`/`continue` through a `finally`;
* inspecting the caught error (`e.code`, `e.message`, `instanceof`): the
  binding is opaque; only its truthiness is known;
* destructured catch parameters.

## Branches

* `switch` with strict equality, fall-through, `default` anywhere and
  unlabeled `break`. Case tests must be side-effect free (literals,
  variables, property reads): JavaScript evaluates them lazily, the model
  up front.
* `a ?? b`, and `a || b` / `a && b` whose *value* is used: hoisted as
  `t = a; if <short-circuit> skip; t = b`, so `b` (and any call in it) runs
  exactly when JavaScript runs it. (Phase 6 evaluated value-position
  `||`/`&&` to a boolean — a fidelity bug.) In conditions the Phase 6
  boolean form is kept.
* Object literals in value position (`return { error: "…" }`) are values
  (`Expr::Object`); a `const` object literal can be read property by
  property and passed to `values(obj)`.
* Correlated branches through helper return values use the Phase 6
  hoisting; discriminated returns work when the discriminant is a literal
  property of a returned object.

## Loops

Only `for (const x of [e1, e2, …])` over an array literal is unrolled
(elements evaluated first; `break`/`continue` supported). A loop over a
collection of unknown length is `Unknown`: one modeled iteration would not
represent all executions, and no bound is guessed.

## Asynchrony (unchanged from Phase 6)

`await` ordering, `Promise.all` children as concurrent tasks with a join,
fire-and-forget tasks whose failure terminates the process. No
completion order is invented for independent promises. `.then` chains,
timers and event emitters stay `Unknown`.

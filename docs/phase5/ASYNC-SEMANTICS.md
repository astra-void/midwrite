# Asynchronous and higher-order semantics

Code: `crates/commitrift-analyzer/src/lower.rs` (`plain_call`,
`concurrent`, `array_callback`, argument placeholders),
`crates/commitrift-core/src/analysis/summary.rs` (`Remap::execution`,
placeholders), `crates/commitrift-analyzer/src/builtins.rs`.

## Execution modes

Still `awaited | returned | fireAndForget | concurrent` per operation; no
new mode was needed. What the modes mean for the path model:

| Mode | Started | Caller waits for completion | Ordered with later operations |
| --- | --- | --- | --- |
| awaited / returned (async) | here | yes | yes |
| concurrent | here, together with others | yes, for the group | no (low confidence) |
| fireAndForget | here | no | no (medium confidence) |

"Effect completed" is not "committed": a write on a transaction handle
commits only when the root transaction's callback completes (unchanged
transaction model). Runtime replay remains the only source of observed
commit/visibility evidence; static modes never override replay verdicts.

## What Phase 5 adds

| Construct | Model |
| --- | --- |
| resolved call inside `await Promise.all([...])` (and `allSettled/race/any`) | summarized; ordered operations copied as `concurrent` (Phase 4: unknown effect) |
| `(async () => { … })()` | resolved to the arrow (`[immediately invoked]`) |
| `await f(cb)` where `f` awaits `cb()` | the callback is inlined at that point (`[callback passed for parameter #i]`) |
| `await withTimeout(op(), ms)` where the callee awaits/races its parameter | `op()` in the argument is a **placeholder**: kept (as `concurrent`) only if the resolved callee awaits that parameter; dropped otherwise and when the callee is unresolved (Phase 4: ignored as unawaited) |
| `await Promise.all(xs.map(cb))` | loop (`array callback`, zero or more times) calling `cb` concurrently |
| `xs.forEach(cb)`, unawaited `xs.map(cb)` | loop calling `cb`, operations fire-and-forget; dropped when `cb` is not summarized |
| `setTimeout/setInterval/queueMicrotask(cb)` | deferred: never part of the current sequence; the scheduling function is inert only if `cb` is |
| `new Promise(executor)` | executor runs immediately; its `resolve`/`reject` parameters are inert |
| modeled builtins in argument position | never placeholders |

The placeholder mechanism realizes the "argument evaluation vs promise
awaiting" distinction: the wrapper's own body is summarized once (inert for
`withTimeout`), and the uncertainty moves to the real operation:

```text
effect#1 unknown effect (unrecognized call): supabase.storage.from("vault").download(fileName) (Concurrent)
call#0 withTimeout(…) proven inert: `withTimeout` …
```

Never assumed: that a callback passed to a function is invoked (only when
the callee's summary invokes or lets it escape), or that `forEach` awaits.

## Errors

Unchanged and still distinct: `throw` and rejection leave the current
scope; `catch` handlers are entered as if the `try` failed before its
effects (approximated); `finally` runs on every exit; an exception inside a
transaction callback rolls it back; a callee's exception propagates out of
the call; `return` inside a catch/finally completes normally. Exceptions are
not crashes: compensation in a `catch` does not cover a process crash
(unchanged notes). A concurrent callee's rejection rejects the group.

## Not modeled

`.then(cb)/.catch(cb)/.finally(cb)` continuation bodies (their callbacks
stay separate functions; recognized operations ending in them are still
explicit executions); completion order inside a concurrent group; a timer
firing; promises stored and awaited later (`const p = op(); …; await p` is
only a lazy-builder case, as before); `for await`; unawaited calls of
resolved functions (`notify(id);` stays silent, as in Phase 4); captured
transaction handles inside callbacks (`items.map((i) => tx.update(...))` —
the callback's `tx` is not the enclosing transaction for the analysis, so
the call remains unknown).

## Differential evidence

`tests/differential.rs` runs five programs under Node.js with a recording
fake runtime and compares the operations with the static trace: ordered
programs (sequential, callback wrapper, `this` dispatch) match exactly;
the concurrent program agrees on the set and the first operation, and the
runtime order (slow publication last) differs from source order, which the
static trace correctly marks `approximated`; the rejected helper's run
performs only the commit, matching the static path's branch between the
commit and the publication. A finite set of agreeing runs is evidence for
these programs only.

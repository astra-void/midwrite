# Callbacks and higher-order wrappers

`crates/commitrift-analyzer/src/wrapper.rs`, `concurrency.rs`
(`entry_info`, `wrapper_facts`).

Passing a callback does not mean it runs. When a selected function was
reached through an `arg[N]` step, the extractor resolves the callee of that
call (same file, relative import, tsconfig `paths`, workspace package —
the analyzer's resolver; never by name) and reads the wrapper's source.

## Invocation facts (derived from source)

| `invocation` | Meaning |
| --- | --- |
| `returned-function-returns-promise` | the wrapper returns a function whose `return fn(...)` hands the callback's promise to its caller |
| `returned-function-awaits` | the returned function awaits `fn(...)` |
| `returned-function-fire-and-forget` | the returned function calls `fn(...)` without awaiting or returning it |
| `invoked-during-wrap` / `-unawaited` | the wrapper calls `fn` while wrapping |
| `stored` | the parameter is used other than by calling it (stored, pushed, passed on) |
| `not-invoked` | the wrapper never references it |
| `unknown` | the wrapper is not analyzed (package, method call, concise body, shadowing, several calls) |

The analysis is bounded: one level of returned function; only direct calls
of the parameter; any other use makes the result `stored`/`unknown`.

`conditional` is true when an earlier top-level statement of the invoking
function can exit (`if (!user) throw …`, `if (!result.success) return …`)
or can reject (`await getUser()`), or when the call is nested in a branch.
Those statements are listed as `guards` with locations; `arguments` lists
what the wrapper passes (`result.data, formData, user`).

## Declared contracts (supplied by the scenario)

`entry.invocation` (`awaited` / `fire-and-forget`) and
`entry.preconditions` (free text) are *assumptions*, reported separately
from source facts and never shown as proven:

| Source facts | Contract | Result (`contractStatus`) |
| --- | --- | --- |
| no wrapper (direct function) | — | `not-needed` |
| known, unconditional | none | `absent` (allowed) |
| known, conditional | none | **refused**: declare the conditions you assume |
| known | consistent | `consistent` |
| known | contradicting (`awaited` vs fire-and-forget) | **refused** |
| `stored` / `unknown` | none | **refused** |
| `stored` / `unknown` | given | `required` (the contract is the only basis) |
| `not-invoked` | any | **refused** |

The actor is the callback's body. The wrapper's own code around the call
(authentication lookup, validation) is not part of the actor: the
extraction records this as an assumption, and at runtime the coordinator
classifies the wrapper's statements as *outside the model* (only reads are
allowed there; a write outside the model makes the verification
inconclusive).

## Fixtures and tests

`fixtures/phase7/wrappers` (first-party wrappers of every shape, nested
higher-order functions, a tRPC-style procedure whose builder is a package
API, a callback factory, class and object methods) and
`fixtures/phase7/team-app` (Server Actions with `validatedActionWithUser`).
Tests: `crates/commitrift-analyzer/tests/phase7.rs`. Real-world: the
original saas-starter `inviteTeamMember` callback resolves its wrapper
through `@/lib/auth/middleware` (`returned-function-returns-promise`, three
guards) — [`CORPUS-EVALUATION.md`](CORPUS-EVALUATION.md).

Not handled: wrappers whose invocation depends on runtime data (retry
loops, middleware arrays), outer wrappers in a nested chain (only the
innermost call's wrapper is analyzed; an outer wrapper's behavior is part
of the declared contract), framework-registered callbacks (route tables,
event emitters).

# Phase 14 Track B: JavaScript value semantics

The model's values were small integers, strings, booleans, one `null`,
rows and Phase 12 generated values. Phase 14 adds bounded JavaScript
arrays, `undefined`, `Map` and `Set`, and the string and array built-ins
real code tests before it writes. Code: `model/src/value.rs` (values,
`Builtin`, `EachOp`), `model/src/jsfn.rs` (built-ins),
`analyzer/src/concurrency/js_values.rs` (lowering),
`discover/src/synth.rs` (argument typing). Tests:
`model/tests/phase14_values.rs` (28), `analyzer/tests/phase14_values.rs`
(11), synthesis unit tests; fixtures `fixtures/phase14/values-app` and
`fixtures/phase14/model/`.

## Versioning: the `js-values` feature

All of it is behind one switch, `ExtractRequest.js_values` / the scenario
feature `"js-values"`. Without it, extraction is the Phase 13 extraction:
the reachable-state digests of all 83 loadable Phase 6–13 fixture scenarios
are byte-identical to the tree before the change (16 pinned in
`phase_6_to_13_scenarios_keep_their_state_bytes`). Discovery extracts with
the switch on and gives a synthesized scenario the feature when an actor's
code uses one of the new forms (an array, a built-in call, a callback,
`undefined`, a list/map/set literal), so that the loader extracts what
discovery extracted (checked by the round trip, `REPLICA-INTEGRATION.md`).
Canonical state: the new values have their own encoding tags (6–9); a state
without them keeps its Phase 6–13 bytes.

## `undefined` and `null`

With the switch, `undefined` is a value: the identifier, `void <literal>`,
`let x;`, `return;`, a missing property, an out-of-range index, a missing
argument, a `find` miss, a `Map.get` miss, the result slot of a helper that
falls off its end. `x == null` / `x != null` test nullish; loose `a == b`
is `(nullish a && nullish b) || a === b`; `??` and `?.` test nullish
(`a?.b.c` and longer chains are refused). `undefined` never reaches SQL
(`value_sql` refuses it).

## Strings

| construct | semantics | refused |
| --- | --- | --- |
| template literal | `Concat` of ToString parts | — |
| `+` | concatenation when a side is statically a string (literal, template, string built-in, a `const` bound to one, a parameter annotated `string`); arithmetic otherwise | not inside queries |
| `String(x)` | ints within ±(2^53−1); `"null"`, `"undefined"`, `"true"`; arrays joined with `,` (nullish elements empty) | rows, generated values, `Map`, `Set`, larger ints |
| `.length`, `.indexOf` | UTF-16 code units | — |
| `includes`, `startsWith`, `endsWith` | ToString of the needle | — |
| `.split(sep)` | non-empty string literal separator | other separators |
| `toLowerCase`, `toUpperCase` | ASCII receivers | non-ASCII (the result depends on Unicode case mapping) |
| `.trim()` | the ECMAScript WhiteSpace + LineTerminator set (trims U+FEFF; not U+0085, U+180E, U+200B) | — |

## Arrays

Literals; `a[i]` (negative or out of range → `undefined`); `.at(k)` for an
integer literal; `includes`/`indexOf` with SameValueZero (an object needle
is refused); `join`; `map`/`filter`/`find`/`findIndex`/`some`/`every` with
one plain-parameter arrow whose body is one expression or `{ return e; }`:
the body is lowered as a conditional operand, so calls, `await`, `?:`,
value `&&`/`||`/`??` and `?.` inside it are unknown (pure built-ins are
allowed), and evaluation short-circuits as in JavaScript. `const [a, , b] =
v`; `for (x of v)` is a bounded loop over a snapshot. Arrays are values:
mutation (`push`, `splice`, `sort`, index assignment, spread) is refused,
and so is `===` involving an array, `Map` or `Set` (identity is not
modeled). Arrays from the database keep their Phase 7 forms.

## `Map` and `Set`

`new Map(...)`/`new Set(...)` (global constructors), `get`/`has`/`size` on
statically known collections, `set`/`add`/`delete`/`clear` as a rebinding of
the local — only for a `const` initialized with `new Map`/`new Set` in the
same function body and used only through its own methods. Any other use
(passed, returned, stored, captured by a nested function, a used result of
`set`/`add`) refuses mutation, with the reason as an assumption: an alias
would observe the mutation, which a value model cannot. Keys use
SameValueZero; object keys are refused; iteration over a `Map` or `Set` is
refused.

## Unknown at run time is unsupported, not an exception

A built-in met with a receiver it does not model at run time (non-ASCII
case mapping, the text of a generated value, an int beyond 2^53−1, object
identity) is `EvalError::Unknown`. Phase 14 makes the engine end that path
as **unsupported** instead of failing the thread: before, such an error
became an exception the application's `catch` could observe
(`model/src/engine.rs`: `settle_thread` leaves the thread on the step,
`step_thread` reports it). This also applies to the Phase 12 refusal to
order generated values. A `null`/`undefined` receiver stays a real
`TypeError`.

## Synthesis typing

A parameter path used only as an array (callbacks, `join`, element access,
`for…of`, destructuring) gets an array of at most 3 elements (the literal
needles of `includes`/`indexOf`, else one placeholder); one used only as a
string gets a string. A path used only through members strings and arrays
share (`includes`, `indexOf`, `length`) needs an annotation (`string`,
`string[]`, `Array<…>` on the parameter, an inline type literal, or a
same-module interface/alias); without one, synthesis refuses with that
reason instead of guessing. Arrays of objects and conflicting uses are
refused.

## State space and partial-order reduction

`analyzer/tests/phase14_values.rs` / `model/tests/phase14_values.rs`
compare full exploration, sleep sets, classic DPOR and Source-DPOR (cache on
and off) on 9 models with list- and set-valued locals at 3 isolation
levels: terminal states, verdicts, counterexample existence and stuck
states agree, and the independence audit is clean. New values are local
state (no new footprints).

## End to end

`fixtures/phase14/values-app`: `tags-lost-update` (split / includes / join /
template) finds a counterexample, `tags-locked` none; `handles-normalized`
(trim + template + `toLowerCase` make " ann"/"lee " equal "Ann"/"Lee")
finds one, `handles-raw` none with the same inputs; without the feature
all four are UNSUPPORTED. The declared model
`fixtures/phase14/model/tags-lost-update.json` is confirmed by the lockstep
PostgreSQL oracle (`--validate-postgres`, PostgreSQL 17.11).

## Unchanged approximations (stated, not fixed)

`null + x` is `null`; relational comparisons with `null` are `false`;
mixed-type `===` throws in the model; `+` on a value not statically known to
be a string is arithmetic; a default parameter applies only to a missing
argument (not an explicit `undefined`); `?.` only on a plain local base;
`typeof` of an undeclared global is refused; element access on a path
synthesizes an array even though strings index too.

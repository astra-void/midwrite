# Phase 13: source resolution (Track D, analyzer)

What the extractor (`crates/commitrift-analyzer/src/concurrency.rs`) now
resolves, what it deliberately does not, and why. Every resolution prints
an assumption naming what was followed. Tests:
`crates/commitrift-analyzer/tests/phase13_resolution.rs`.

## Implemented

| form | resolution | evidence kept |
| --- | --- | --- |
| function alias `export const canManage = isAdmin` (same module, imported, re-exported; aliases of aliases, bounded to 8 hops) | the aliased function is inlined | assumption "`canManage` is an alias of the function `isAdmin` of `<module>`" |
| `export const f = cache(fn)` with `cache` imported from `react` | the actor's **first** call of `f` inlines `fn`; a repeated call is `Op::Unknown` ("returns the memoized result of an earlier call, which is not modeled") | assumption naming the React `cache` contract and that no earlier call with the same arguments happened in the same request. A `cache` of any other package is not this contract |
| template literal without interpolation | a string literal | — |
| template literal with interpolation | still unknown, with the reason ("string concatenation is not modeled": the model's value layer has no string concatenation) | — |
| a module-level name used as a database (any name, not only configured receivers) | its value provenance (`docs/phase13/REPLICA-AWARE-ANALYSIS.md`): `primaryDb.insert(...)` of `const primaryDb = drizzle(...)` is a primary statement | origin printed for replicated databases |
| `await connectDb()`, `x.$primary`, `x.usePrimaryOnly()` as declarations or arguments | database handles | as above |
| package calls with a verified contract | still `Op::Unknown`, naming the contract (`PACKAGE-CONTRACTS.md`) | — |

Unchanged and already supported before Phase 13 (verified, not
reimplemented): exported `const`s holding arrow functions or function
expressions, object-literal methods (`m() {}`, `m: async () =>`, `m: f`),
class methods with constructor-stored handles, conditional expressions,
value-producing `&&`/`||`/`??` (hoisted, evaluated exactly when JavaScript
would), `?.` on a local, `switch`, `for (const x of [literal, ...])`,
`{ a, b: c }` destructuring, `Promise.all([...])`, async helper inlining
(fire-and-forget calls as detached tasks).

## Not implemented, with the reason

| form | why not |
| --- | --- |
| other wrapped callbacks (`export const f = wrap(async () => ...)`, call-returned functions, function factories) | how the returned function calls the callback (arguments, how often, whether at all) is not known without interpreting the wrapper; Phase 7's wrapper analysis decides only invocation, not argument mapping. They stay "`f` in `<module>` is not a function declaration" |
| inherited methods (`class B extends A`) | not needed by the measured blockers; refused as before ("extends another class") |
| `Array.prototype.includes/map/filter/some`, `new Map`/`new Set`, loops over collections of unknown length | the model's value layer has no arrays, maps or sets; these stay unknown |
| RegExp `.test`/`.exec` on module constants | no regular expressions in the value layer |
| string building (`` `${a}-${b}` ``, `+` on strings, `.slice`, `.toLowerCase`) | no string operations in the value layer |
| destructuring with defaults or nesting, rest patterns | unchanged |

The model crate was not changed by this track: forms whose *values* the
model cannot represent cannot be unblocked by the extractor alone.

## Effect on the corpora

`reports/phase13/analyzer/corpus/SUMMARY.md`; school-ledger in detail in
`SCHOOL-LEDGER-COVERAGE.md`.

| project | category | Phase 12 | Phase 13 |
| --- | --- | ---: | ---: |
| school-ledger | unresolved call | 606 | 484 |
| school-ledger | unresolved identifier | 646 | 594 |
| school-ledger | unknown operations (total) | 12 894 | 14 615 |
| school-ledger | inline depth limit | 871 | 1 137 |
| saas-starter, unnamed-ai-project | candidates, statuses, entry statuses | — | unchanged |

The total rises because aliases and React `cache` functions are now inlined
and expose their own unsupported operations (newly exposed behavior, not a
regression); the static `commitrift check` path does not use this extractor
and is byte-identical to Phase 12 (`reports/phase13/analyzer/check-identity.txt`,
144/144).

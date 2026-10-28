# Phase 13: school-ledger coverage (Track D checkpoint)

school-ledger is private: as in Phase 12, only aggregate counts and generic
causes are written here — no code, path or identifier. It was analyzed from
a read-only copy of its working tree (HEAD `d1fdf38`, without
`node_modules`) in a private scratch directory; the working tree itself was
neither modified nor executed. The Phase 12 binary reproduces the Phase 12
figures on this copy exactly (21 effectful entries, 9 candidates, 12 894
unknown operations). Raw data: `reports/phase13/analyzer/corpus/school-ledger-*.json`,
`reports/phase13/analyzer/school-ledger-candidates.json`.

| | Phase 12 binary | Phase 13 binary |
| --- | ---: | ---: |
| tables modeled | 30 | 30 |
| entries extracted | 546 | 546 |
| effectful / read-only entries | 21 / 29 | 21 / 30 |
| resources | 18 | 18 |
| unknown operations | 12 894 | 14 615 |
| candidates | 9 | 9 |
| statuses | 9 UNSUPPORTED | 7 INCONCLUSIVE, 2 UNSUPPORTED |
| model-checkable / counterexamples | 0 / 0 | 0 / 0 |
| wall / peak RSS | 0.54 s / 80 MiB | 0.64 s / 91 MiB |

## The nine candidates

All nine are check-then-act candidates on two actors whose property is a
HEURISTIC_HYPOTHESIS (at most one row per key; no unique constraint in the
schema). Phase 12's "7 calls whose resolved export is not a function
declaration, 2 unsupported expressions" are, precisely:

* **C1, C2, C4–C7, C9 (7 candidates)** — the first operation of the actor
  is an authorization check called through an **exported `const` function
  alias** (`export const a = b;` where `b` is a function declaration of the
  same module). The callee resolved to a variable declaration whose
  initializer is an identifier, which Phase 12's resolver did not follow.
* **C3, C8 (2 candidates)** — a test-support helper's **default parameter
  value is a template literal** whose interpolation is a call of a package
  function (`node:crypto` `randomUUID`) followed by a String method; the
  helper is called without that argument.

| | original blocker | resolved? | extracts | scenario built | model checking | counterexample | first remaining blocker |
| --- | --- | --- | --- | --- | --- | --- | --- |
| C1, C2, C4, C5, C6, C7, C9 | function alias | **yes** (alias followed) | yes | yes; synthesis **unsatisfiable** → INCONCLUSIVE | not reached | none | the synthesized argument is a table row without the field the (now inlined) authorization guard tests, so the guard denies and the actor throws before the candidate's operations |
| C3, C8 | template literal with an interpolated package call | no (string building is not modeled; the interpolation itself is a package call producing a random value) | yes | yes; synthesis unsupported → UNSUPPORTED | not reached | none | the same template literal, now reported as "string concatenation is not modeled" |

Property origin: HEURISTIC_HYPOTHESIS for all nine, unchanged (never
upgraded).

**Probe with a declared value.** With a scratch contracts file declaring
the guard's field for the three entries involved (`values`, not committed),
four of the seven reach the next operation: `Array.prototype.includes` on a
field of the argument — an array value the model cannot represent (no
arrays in the value layer), so they become UNSUPPORTED there; the other
three stay unsatisfiable on their own path. Behind that, on the path to the
candidate operations: a validation helper calling `safeParse` of a schema
passed as a parameter (unknown package behavior: the schema is not a
module-level zod value there), loops over collections of unknown length,
`new Map`/`new Set`, closures used as values, `instanceof`, a multi-row
insert, a row-lock shape that is not supported, Drizzle builders that are
not awaited, and caught errors whose properties are inspected (72
unsupported operations on the first candidate's actors in total). None of
these is unblocked by extractor changes alone; no
counterexample was forced, and none is claimed.

## Why the unknown count rose

12 894 → 14 615 (+1 721): aliases and React `cache`d functions are now
inlined (10 calls of a `cache`d function now reach its body), exposing
their own unsupported operations ("inline depth limit" 871 → 1 137,
"unresolved method call" 4 603 → 5 326, "other" 2 555 → 3 311) while
"unresolved call" fell 606 → 484 and "unresolved identifier" 646 → 594. One
more entry is read-only (a SELECT reached through an alias). The static
`commitrift check` output is byte-identical to Phase 12 for every format,
depth and path mode (`reports/phase13/analyzer/check-identity.txt`).

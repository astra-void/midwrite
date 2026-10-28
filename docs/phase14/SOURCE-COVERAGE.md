# Phase 14: source coverage (Tracks C and E)

What the extractor and discovery now understand that Phase 13 did not, how
each construct is lowered, and what is still refused. Value-domain
lowering (arrays, strings, `Map`/`Set`) is in `VALUE-SEMANTICS.md`; Drizzle
statements and predicates in `DRIZZLE-DIFFERENTIAL.md`.

## C2. Closures and higher-order wrappers

`analyzer/src/concurrency.rs`: an arrow function or function expression
passed as an argument to an inlined project function becomes a
`Binding::Closure`: the module and span of its function node, and a copy of
the scopes it closed over. When code calls a name bound to a closure, the
closure's body is inlined **at that call**, in the scopes of its definition
plus its own parameters.

| property | how it is preserved |
| --- | --- |
| callback identity | the exact function node (module, span); found again with `oxc_semantic` nodes, never by name |
| argument mapping | the call's arguments bind the closure's parameters like any inlined call (handles, values, destructuring) |
| captured values | the captured scopes name the same model locals as the defining function; an assignment after the capture is seen (JavaScript closures capture variables) |
| invocation timing | the body runs where, and as often as, the wrapper calls it; a closure that is never called contributes nothing; a closure passed on to a package call is that package call (unknown) |
| retry behavior | it is the wrapper's own code: `try { return await op() } catch { return await op() }` lowers to a `try` region and a handler that runs the body again |
| error propagation | the model's `try`/handler semantics; a transaction opened inside the callback is rolled back before the handler runs |
| transaction boundaries | `db.transaction(async (tx) => …)` inside the callback is the usual transaction |

A local binding shadows a module function of the same name. A function
value used other than by calling it is an unknown expression ("only modeled
when it is called"); a closure returned or stored is not followed. Wrapper
*facts* for entries (`withAuth(handler)` as an exported route) are the
Phase 7 mechanism, unchanged; a closure used as such a wrapper is refused.

**Ambiguous commit through a wrapper.** The ambiguous-commit family
(`discover/src/detect.rs`) also covers a write inside a transaction whose
COMMIT is in a `try` block and that the handler repeats (before: only
autocommit writes). Its fault assumption says so: "the acknowledgment of
the transaction's COMMIT can be lost after the server committed it". The
handler region is now computed by control-flow reachability (what the
handler reaches and the try block's normal continuation does not); the
Phase 11 straight-line approximation stopped at a conditional `return` in
the handler.

## C1. zod `parse` / `safeParse`

`analyzer/src/zod.rs` reads a schema's builder chain statically:
`z.object` (strip mode, keys in shape order), `z.string` with `min`, `max`,
`length`, `nonempty`, `trim`, `toLowerCase`, `toUpperCase`, `startsWith`,
`endsWith`, `includes` (literal arguments; a custom message allowed),
`z.number` with `min`/`gte`/`gt`/`max`/`lte`/`lt`/`positive`/
`nonnegative`/`negative`/`nonpositive`/`int`/`finite`, `z.boolean`,
`z.literal`, `z.enum`, nested schemas by module constant. The semantics are
those of zod 3.x read from `zod/v3/types.js` (3.25.76): string checks in
declaration order; `trim`/`toLowerCase`/`toUpperCase` rewrite the value for
the checks that follow and for the output; lengths are `.length`.

`schema.parse(input)` lowers to explicit steps: `typeof` tests, each check
as a jump to a failure block, each rewrite as a new local, the object's keys
read once; the result is the **parsed** value (not the input). A failure
throws (`ZodError`); `safeParse` yields `{ success, data }` instead, and its
`error` is not modeled (reading it is unknown). A schema passed as an
argument to a helper keeps its identity (`Binding::Zod`), so
`validate(schema, input)` helpers are followed.

Refused, by name: every other check (`email`, `uuid`, `regex`, `url`, ...),
`optional`/`nullable`/`default` (they depend on `undefined`, which the
Phase 13 value layer does not distinguish from `null`), `transform`,
`refine`, `coerce`, `preprocess`, unions, arrays, `passthrough`/`strict`.
A refused schema keeps the Phase 13 package-contract wording plus the
reason ("…; the schema is not modeled: zod string check `.email()` is not
modeled"). Assumption printed: a zod object schema's input is not an array
(zod rejects arrays; the model's object test does not distinguish them).

## E1. Return-value properties: declared templates only

`commitrift-contracts.json` `invariants[].property` accepts three
templates (`discover/src/contracts.rs`):

| template | formula | for |
| --- | --- | --- |
| `atMostOneSuccess { function?, returned }` | completions told `returned` ≤ 1 | at most one successful claim, unique reservation, no duplicate success after a retry |
| `exactlyOneSuccess { function? }` | successful completions = 1 | exactly one acknowledged logical operation |
| `everySuccessReturns { function?, returned \| argument }` | completions told the value = all completions | read-your-writes (`argument`: a parameter path, instantiated per scenario) |

A new discovery family, `return-value` (`declared_outcome`), builds a
candidate **only** for an entry named by such a declaration: the entry's
own effects, two concurrent calls with the same arguments (one call for
`everySuccessReturns`), the declared retry if any. Nothing infers these
properties: an entry without a declaration gets no candidate of this class,
and the frozen sets' undeclared return-value positives stay false
negatives.

## E2. Parent DELETE × child INSERT

Family `parent-child` (`parent_child`): a read finds the
parent row by a single unique key of the parent table, the read decides an
INSERT into another table whose column holds that key, and another entry
deletes parent rows by that key. Property `no-orphan-<child>`:
`count(child where column = k) > 0 ⇒ count(parent where key = k) > 0`.

| schema | property origin | what a counterexample means |
| --- | --- | --- |
| foreign key `NO ACTION`/`RESTRICT` child → parent | SCHEMA_ENFORCED (protection listed) | none expected: the database rejects one statement with 23503 — not by itself an application defect |
| `CASCADE`/`SET NULL`/`SET DEFAULT` | — | the scenario is refused (Phase 13 refusal) |
| no foreign key | HEURISTIC_HYPOTHESIS ("the existence check suggests the reference must hold") | a counterexample against a hypothesis, never a confirmed defect |

## F. Next step in every report

Each candidate carries `nextStep` (JSON) / "Next step:" (text, HTML): from
its status and its property's origin, what a developer can do next —
declare the property, rerun with `--model-check`, declare argument values,
emit the scenario and read the counterexample, or run the original code
with an authorized harness. `commitrift explain` now also lists `CP7103`.

# Abstract values, environments and provenance

Code: `crates/commitrift-core/src/absint/{value,env}.rs`,
`crates/commitrift-analyzer/src/values.rs`. Design rationale:
[`ANALYSIS-DESIGN.md`](ANALYSIS-DESIGN.md) §1–2.

## What a value is

An `AbsValue` over-approximates the set of runtime values an expression may
evaluate to, restricted to identities that decide something in CommitRift:

| Atom | Meaning | Used for |
| --- | --- | --- |
| `Lit` | primitive constant | branch facts, `switch` cases |
| `Function` | a function of the table | call targets, callbacks |
| `Class` | a class constructor | static method dispatch |
| `Object` | an allocation site of the current function | aliasing, receivers ([heap](HEAP-AND-ALIASES.md)) |
| `Record` | an object with known properties outside any heap (a returned literal) | provenance and dispatch across calls |
| `Param(i)`, `This` | summary inputs | substitution at call sites |
| `CallResult(k)` | what call site `k` returned (symbolic until summarized) | return-value propagation |
| `EffectResult(e)` | what effect `e` produced (a row, a response — never concrete) | provenance |
| `Opaque(n)` | a fresh unknown with an identity (an unknown call's result) | "same value" without knowing it |
| `Field(atom, name)` | a property of a symbolic atom, depth ≤ 3 | `order.id` |
| `Static(symbol)` | module-level data or an import (tables, constants, clients) | neither input nor unknown |

`Top` is "any value"; the empty set (bottom) is "unreachable".

**Lattice.** Sets ordered by inclusion, `Top` greatest. Join is union; more
than `MAX_ATOMS` (6) atoms widens to `Top` (counted). Atoms are stored
sorted and deduplicated (canonical form, deterministic output). Values with
zero or one atom are stored inline (no allocation). Chains are finite:
atoms per function are finite, `Field` depth is bounded, and the set size is
bounded.

**Not modeled.** TypeScript types (never consulted), numbers and strings
beyond literals, arithmetic, prototypes, getters (property reads never run
code in the model), promise state (a promise and its fulfillment share an
identity; `await` is the identity function on values).

## Environment

`Env<K>` maps semantic binding identities (Oxc symbol ids) to values. Same
names in different scopes are different keys. Updates are strong (a binding
is one variable). Joins are pointwise; a binding present on one side only
keeps its value (it is out of scope on the other path). Bindings written
from a nested function (`captured_writes` in the symbol table) always read
as `Top`: any call might run that closure.

Identifiers resolve, in order: environment → same-file function →
class → `const` literal → import / module-level binding (`Static`) → `Top`
(a variable of an enclosing function, or not assigned yet).

## Transfer functions (frontend)

The lowerer carries a `ValueState` (environment + heap, copy-on-write) in
source order and applies:

| Construct | Effect on the state |
| --- | --- |
| parameters | `param#i` (destructured: property paths of it; defaults joined) |
| `const/let x = e`, patterns | `bind_pattern`: object/array patterns read properties `"a"`, `"0"`, …; rest → `Top`; defaults joined |
| `x = e` | strong update; compound assignments → `Top`; destructuring assignment targets → `Top` |
| `x++` | `Top` |
| call | value recorded per call site: `EffectResult` (recognized effect), `CallResult` (summarized call), `Opaque` (other) |
| `new C()` | instance object (known class) or `Opaque` |
| `if` / `?:` / `&&` / `switch` / `try` | each arm from a copy of the state, joined after (handlers start from the join of before and after the block) |
| loops | fixpoint, see [`PATH-ANALYSIS.md`](PATH-ANALYSIS.md) |
| `return e` | function return value ⊔= exported `eval(e)`; `$ret` facts |

Each transfer function is a composition of joins, reads and constructors and
is monotone; widening only replaces a value with `Top`. Seeded property tests
(`absint::properties`) check upper bound, commutativity, idempotence,
associativity up to widening, chain termination, and monotonicity of
substitution and field reads.

## Provenance

Each recognized write and external effect records a `Provenance`: the
symbolic atoms its arguments contain, each **exact** (the argument, or a
property of it, transitively, *is* that value) or **derived** (the value
appears inside a computation: template, arithmetic, an argument of a nested
call), plus an `unknown` flag when some argument is `Top`. Table arguments
(`insert(orders)`) and `Static` data are ignored. Provenance is expressed in
the frame of the function the IR belongs to; when a callee is inlined, its
atoms are substituted (`Param` → argument values, the callee's
`EffectResult` → the copied effect, callee `CallResult`/`Opaque` → fresh
identities for that instantiation), and a caller's `CallResult(k)` is
replaced by the instantiated return value of call `k`. Example (from
`tests/phase5.rs`):

```ts
const created = await createOrder(input);   // returns await db.insert(...).returning()
const payload = { orderId: created[0].id };
const request = payload;
await queue.publish("order.created", request);
// publish inputs: result of `db.insert(orders)`.0.id  (exact)
```

## Limits

Variables of enclosing functions read as `Top` inside callbacks (captured
values are not bound from the defining frame); `var` re-declarations in
loops are re-bound like `let`; property values of objects from unanalyzed
code are `Field` paths, not values; at most 16 inputs per effect (more →
`unknown`).

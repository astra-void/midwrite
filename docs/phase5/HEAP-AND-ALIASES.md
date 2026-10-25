# Bounded heap and aliases

Code: `crates/commitrift-core/src/absint/heap.rs`; used by
`crates/commitrift-analyzer/src/values.rs` and `lower.rs`.

## Objects

One abstract object per **allocation site** per function: object literals,
array literals (elements are properties `"0"`, `"1"`, …), and `new C(...)`
(kind `instance(C)`, used for [method dispatch](CLASS-RESOLUTION.md)).
Each object has known properties, and three flags:

* `open` — may have unlisted properties (a spread of an unknown value, a
  computed key, an accessor, a write with an unknown key);
* `escaped` — reachable from code the analysis does not see;
* `summary` — stands for several concrete objects (the site was evaluated
  again on the same path, i.e. in a loop).

Two bindings holding the same `Object(o)` atom are aliases:

```ts
const payload = { orderId: order.id };
const request = payload;
request.orderId = other;          // strong update of o.orderId
await gateway.charge(payload);    // sees `other`
```

## Updates

| Write | When | Effect |
| --- | --- | --- |
| strong | target is exactly one non-summary object | replace the property |
| weak | target may be several objects, or a summary object | join into the property |
| unknown target | target is a parameter, a field path, `Top` | nothing can be updated; the written value escapes (counted) |
| unknown key | `o[k] = v` with non-literal `k` | object becomes open, every property `Top`; `v` escapes |

Reads: a known property returns its value; an absent property returns `Top`
(prototype properties are not modeled — never `undefined`); an escaped object
returns `Top` for every read; symbolic receivers give `Field` paths.

## Escape

An object escapes, transitively over reachable objects, when it is passed to
any call that is not a modeled non-mutating builtin or a recognized effect
(recognized effects are assumed not to mutate their arguments), stored into
an escaped object or an unknown target, returned, or when a closure defined
in the current function is passed to a call (every object bound in the
environment escapes: the closure may run later and change what it
captures). Mutating builtins (`Object.assign(target, …)`, `arr.push(...)`)
make their target open. Invalidation is selective: unrelated objects keep
their properties (`tests/phase5.rs::strong_updates_replace_and_escapes_invalidate`,
`absint::heap::tests::escape_is_transitive_and_selective`).

## Leaving the function

Objects never leave their function as heap references: return values and
effect provenance are **exported** — each non-escaped, non-summary object
becomes a `Record` of its (exported) properties, depth ≤ 3, ≤ 16
properties; others become `Top`.

## Resource identity

Effects carry their table (`orders`) as before and, now, the provenance of
their arguments. A symbolic resource such as "orders row with id
`input.id`" is the pair (table, provenance of the `where`/`values`
arguments). CommitRift never claims two operations touch different rows:
predicates are not interpreted, so disjointness is never concluded
([`EFFECT-RELATIONSHIPS.md`](EFFECT-RELATIONSHIPS.md)).

## Not modeled

Heap state across functions (only exported records), prototype chains,
`Map`/`Set` contents, array lengths and element order beyond literal
indices, getters/setters (an accessor makes its object open), `delete`
(the symbol-table rule from Phase 2 still disables object services),
mutation by unanalyzed code of objects that did not visibly escape (for
example through a module-level reference) — module-level data is `Static`
and never tracked.

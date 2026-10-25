# Phase 5 analysis design

This document fixes the abstractions *before* they were implemented, with
the problem each one solves (from [`BASELINE.md`](BASELINE.md)). Where the
implementation deviated, the per-topic documents and
[`FINAL-REPORT.md`](FINAL-REPORT.md) say so.

## Deviations found during implementation

* **`Static` atom** added: module-level data and imports must be neither
  provenance inputs nor "unknown" (otherwise every `eq(orders.id, id)`
  would make provenance unknown).
* **Return facts are bound only when the caller tests the binding**, and
  the trace merge at call exits keys return facts by **liveness** (any fact
  outside the call mentions the variable), not by the `call#N:` prefix: the
  prefix rule lost a feasible path on the corpus (school-ledger CP1004s)
  once a callee was itself inlined.
* **Specialization limit falls back** to the context-free expansion instead
  of refusing to summarize (refusing was a regression versus Phase 4).
* **Loops**: an optimistic first pass keeps its IR when the entry state is
  already a fixpoint; the loop-head order ignores bindings and objects
  created inside the body.
* **Array callbacks** (`Promise.all(xs.map(cb))`, `xs.forEach(cb)`) were
  implemented as loops in the IR (`LoopKind::Callback`), not as a new
  execution mode.
* **`.then(cb)` continuations** were not implemented (unchanged from Phase 4).
* **Inert callees with reads** are still inlined (their reads appear in
  traces and trace ids depend on steps), only callees without any recognized
  operation become empty `inert` scopes.
* Constructor parameter properties are not bound to instance fields.

## 0. Principles

1. **Extend, do not replace.** The structured Effect IR, the bounded path
   walker, the replay-based detector and the trace builder stay. Phase 5
   adds a *value layer* that the frontend computes while lowering, and
   *summary semantics* that the core inliner consumes.
2. **Values only where they decide something.** A value is tracked only
   insofar as it can change: which function a call reaches, which object a
   method is dispatched on, whether a callee may perform an effect, which
   values an effect's arguments derive from, and which branch arms are
   feasible.
3. **Every loss is explicit.** Joins that exceed a bound produce `Top`
   (counted as a widening), unresolved calls keep an unknown effect whose
   detail says why, assumptions are recorded on the call site and in the
   diagnostic.
4. **No new definition of "complete".** A trace is complete exactly when
   its failure window contains no unknown step and no unsupported control
   flow (Phase 3 definition). Phase 5 can only change *which steps are
   unknown*, and only by resolving them through an analysis fact.

## 1. Abstract domains

### 1.1 Abstract values (`commitrift_core::absint::value`)

Concrete semantics approximated: the set of JavaScript values an
expression may evaluate to at a program point, *restricted to the
identities CommitRift cares about*.

```text
AbsValue = Top | { atoms: sorted set of Atom, |atoms| ≤ K }     (K = 6)
Atom     = Lit(literal)            primitive constant
         | Function(f)             a function of the function table
         | Class(c)                a class constructor
         | Object(o)               an abstract allocation site of the current heap
         | Param(i) | This         the i-th argument / receiver (summary inputs)
         | CallResult(k)           the value call site k returned (symbolic)
         | EffectResult(e)         the value effect e produced (a row, a response)
         | Opaque(n)               a fresh unknown value with identity (unknown call result)
         | Field(atom, name)       a property of a symbolic atom (depth ≤ 3)
         | Import(module, name)    an imported binding not yet linked
         | Builtin(b)              a modeled global (`Promise`, `setTimeout`, …)
```

* Bottom is the empty set (unreachable), Top is "any value". Order is set
  inclusion with Top greatest. Join is union; a union larger than K becomes
  Top (*widening*, counted). The height is finite because a function has
  finitely many atoms and `Field` depth is bounded, so every ascending chain
  stabilizes; the K bound makes stabilization fast.
* `Opaque` and `CallResult` give *identity without knowledge*: the value
  `id` returned by an unknown call is unknown, but two effects that both use
  `id` provably share it.
* `await` is the identity on abstract values: a promise and its fulfillment
  value share an abstract identity. Promise *state* is not a value; it is
  modeled by execution modes (§5).
* TypeScript types are never consulted. A value is a database handle only
  through the existing recognition rules; the value layer never invents a
  handle.

### 1.2 Environment (`absint::env`)

`Env = BindingId → AbsValue` keyed by **Oxc semantic symbol ids**, never by
names, so shadowing, parameters and captured variables are distinct. Join is
pointwise; a binding missing from one side joins with its declaration-time
value. Variables written from a nested function (another closure may change
them at any call) always read as Top.

### 1.3 Heap (`absint::heap`)

Concrete semantics approximated: the objects created in the current
function and their data properties.

```text
Heap   = ObjId → Object
Object = { kind: plain | array | instance(class), props: name → AbsValue,
           open: bool      (unknown properties: spread, computed write),
           escaped: bool   (reachable from code we do not see),
           summary: bool   (stands for several concrete objects) }
```

* One abstract object per **allocation site** (object/array literal,
  `new C(...)`) per function. An allocation site evaluated again on the same
  path (a loop) turns its object into a summary object.
* **Strong update** (replace) only when the target value is exactly one
  `Object(o)` atom and `o` is not a summary object; otherwise **weak update**
  (join). A write through a symbolic receiver (`Param`, `Field`, …) cannot
  be applied and is recorded as a mutation of an unknown object.
* **Escape**: an object passed to an unknown call, stored into an escaped
  object, or captured by a closure that may run later is marked escaped,
  transitively over reachable objects. Reads from escaped objects return
  Top. Unrelated objects keep their properties (selective invalidation).
* Reads of an absent property of a closed, non-escaped plain object give
  Top (prototype properties are not modeled), never `undefined`.

### 1.4 Effect summaries (`absint::summary`)

Per function, context-insensitive part:

```text
Summary = { inert: Inert, params: [ParamUse], returns: AbsValue,
            return_facts: per return exit, facts about `$ret` }
Inert   = Proven | NotInert(reason)          (lattice: Proven ⊑ NotInert)
ParamUse= { awaited, invoked, escaped: bool }
```

*Inert* means: on every path the function performs no database write, no
transaction, no configured external effect, no unknown call in any
position, and every call it makes is (a) a modeled builtin without
consistency-relevant effect, (b) a call of an inert function, or (c) an
invocation of a parameter, which makes inertness conditional on the
argument. Database reads are allowed (they commit nothing and are excluded
from failure windows). This is *stronger* than the Phase 4 inlining view:
Phase 4 ignored `return pkg()` in async callees and unawaited calls; for
inertness every call must be accounted for.

## 2. Transfer functions (frontend, `commitrift-analyzer/src/values.rs`)

| Construct | Transfer |
| --- | --- |
| `const/let x = e` | `env[x] := eval(e)` |
| `{a, b: c, ...r} = e` | `env[a] := read(v,"a")`, `env[c] := read(v,"b")`, `env[r] := Top` |
| `[a, b] = e` | `read(v,"0")`, `read(v,"1")` |
| `x = e` | strong update of `env[x]` (flow-sensitive) |
| `o.p = e` | heap write (strong/weak, §1.3) |
| `o.p` | heap read; on symbolic atoms `Field(atom,p)` |
| `{…}`, `[…]` | allocate at site; properties evaluated |
| `new C(…)` | allocate `instance(C)`; constructor parameter properties bound to arguments |
| function / arrow expression | `Function(f)` |
| call | `CallResult(k)` for a call site, `EffectResult(e)` for a recognized effect, `Opaque(n)` otherwise; arguments that are objects escape unless the callee is resolved |
| `await e` | `eval(e)` (identity) |
| `c ? a : b`, `a ?? b`, `a \|\| b` | join |
| `if`, `switch`, `?:`, `&&` | lower arms from a copy of the state, join after |
| loops | Kleene iteration of the body (value only, IR discarded) from the entry state; after `MAX_LOOP_ITERATIONS` (2) changed bindings are widened to Top; the body is then lowered once from the fixpoint |
| `return e` | `returns ⊔= eval(e)`; literal properties of a returned object literal become facts about `$ret` |
| `throw` | state dropped on that path |

Transfer functions are monotone: each is a composition of joins, reads and
constructors that preserve inclusion; widening replaces a value only with a
larger one (Top).

## 3. Context sensitivity

The inliner already substitutes database-handle arguments when it copies a
callee summary (`save(tx)` vs `save(db)` stay distinct without
specialization). Phase 5 adds **parameter-relevant context abstraction**:
the memo key of an expanded callee becomes

```text
(callee, remaining depth, [(param i, Function f) for each invoked param])
```

i.e. only arguments that decide *which code runs* (callback identities)
enter the key; constants and handles do not. The number of
specializations per callee is bounded (`MAX_SPECIALIZATIONS` = 8); beyond
it the call stays unknown with that reason (counted).

## 4. Interprocedural summaries and fixpoints

* **Inertness** is a greatest fixpoint over the project call graph,
  computed SCC by SCC (Tarjan) in reverse topological order: every function
  starts `Proven`, and is demoted when one of its facts refutes it. For a
  safety property ("never performs an effect") the greatest fixpoint is
  sound: an effect would have to occur at some finite call depth. Recursion
  needs no depth cut. Iteration inside an SCC is bounded by its size × 2;
  exceeding it demotes the whole SCC (never observed, counted).
* **Effect summaries** remain the Phase 2 inlined IR (ordered effects,
  transactions, branches, exits). A call is now replaced by:
  * the callee's expanded IR, as before — also for calls in concurrent
    position (`Promise.all` elements), with every ordered effect copied as
    `concurrent`;
  * an **empty call scope** when the callee is inert under this call's
    arguments: the call is resolved, it does nothing durable or visible;
  * an unknown effect with the reason otherwise (unchanged behaviour).
* **Return values** are substituted on instantiation: `Param(i)` → the
  caller's argument value, the callee's `EffectResult(e)` → the copied
  effect's new id, and the caller's `CallResult(k)` → the instantiated
  return value. Provenance therefore survives calls and returns.
* **Return facts**: a callee's facts about `$ret` are renamed to the
  caller's `const` binding of the call result, so `if (result.ok)` after
  `const result = await save()` is correlated with the callee's returns
  through the existing feasibility check.

## 5. Asynchronous and higher-order semantics

Execution modes stay `awaited | returned | fireAndForget | concurrent`.

* A resolved call in concurrent position is inlined with all ordered
  effects marked concurrent (completion order undetermined, low
  confidence), exactly like direct `Promise.all` elements.
* An IIFE `(async () => …)()` resolves to its function.
* A parameter **invoked** by the callee (`await action()`) is linked, at
  each call site, to the function value passed for it (inline arrow or named
  function), giving the callback's effects in order.
* A parameter **awaited** by the callee (`await p`, `Promise.race([p, …])`)
  marks the caller's argument as consumed by the callee: an unknown call in
  that argument becomes an unknown effect (concurrent when raced) instead
  of being ignored as an unawaited call.
* `setTimeout`/`setInterval`/`queueMicrotask` callbacks are **deferred**:
  never part of the current sequence; the scheduling function is inert only
  if the callback is.
* `new Promise(executor)` invokes the executor immediately; resolver
  parameters are inert.
* `arr.forEach(async cb)` does not await callbacks and
  `Promise.all(arr.map(cb))` runs them concurrently; neither is treated as a
  sequential loop.

Unsupported: continuation callbacks (`.then(cb)`) are not inlined; their
bodies stay separate functions.

## 6. Class and method resolution

Classes are collected per module (declaration, `export default class`,
`const C = class {}`) with methods, static methods, arrow-valued fields,
constructor parameter properties and the `extends` target (local or
imported, linked across modules like functions).

* `new C(…).m()` / `const x = new C(…); x.m()`: the dynamic class is exactly
  `C`; `m` is looked up in `C` and its ancestors. **Exact** dispatch.
* `this.m()` inside an instance method of `C`: `this` may be any subclass
  of `C`. Resolved only if no analyzed subclass of `C` defines `m`
  (closed-world assumption over the analyzed files) and the file never
  assigns `this.m`, `C.prototype.m` or uses `Object.defineProperty` /
  `Object.assign` on `this`/prototypes. If an ancestor is not analyzed
  (package base class) and `m` is found below it, the resolution is
  **assumed** (an unanalyzed constructor could install an instance property
  `m`): it is used, recorded on the call site, and named in the
  diagnostic's limitations.
* Several possible targets → not resolved (detail lists them). Methods are
  never resolved by name alone.

## 7. Path-sensitive analysis

The bounded path walker is kept; it already carries per-path facts. Phase 5
adds facts that come from values instead of syntax:

* property tests `x.p` / `x.p === lit` on a `const` binding whose
  references are only member reads (it never escapes and is never written
  through) — so a getter or an aliasing write cannot change it — become
  facts on the path `x#id.p`;
* callee return facts (§4).

No constraint solver: facts are literal (in)equalities, truthiness and
object-ness, compared pairwise.

## 8. Effect relationships

Each recognized effect records its **inputs**: the atoms its arguments'
values contain, each marked *exact* (the argument or one of its properties
*is* that value) or *derived* (the value occurs inside a computation such
as a template). After inlining, inputs are expressed in the analyzed
function's frame. For a finding with two operations:

| Classification | Condition |
| --- | --- |
| `provenShared` | the second operation's exact inputs contain the first's result (`EffectResult`) or a value both use exactly |
| `possiblyRelated` | a shared atom only through a derived input or a joined value |
| `noRelationEstablished` | all inputs of both sides known, nothing shared |
| `unknown` | an input is Top / escaped |
| `provenDisjoint` | reserved; **never emitted** in Phase 5 (disjointness of database rows is not provable without predicates) |

Used for explanation only; no severity or suppression changes.

## 9. Unknown-call handling

An unknown call stays an unknown effect unless a fact resolves it: a
function, method or callback target plus an inlined summary, or an inertness
proof. When a callee is resolved but not inert, its unknown calls appear at
their own locations (the uncertainty moves to the real operation, e.g. from
`withTimeout(...)` to `supabase.storage.from(...).createSignedUrl(...)`).

## 10. Soundness assumptions (documented, not proven)

1. Closed world for classes: subclasses exist only in analyzed files.
2. Modeled builtins are not monkey-patched (`Promise`, `setTimeout`,
   `Math`, `JSON`, `Date`, `Object.keys`, …), and `console.*` output is not a
   consistency-relevant effect.
3. Inertness ignores getters/setters invoked by property reads and
   `toString`/`valueOf` coercions.
4. Prototype mutation is detected only syntactically in the same file.
5. All Phase 1–4 assumptions (configured receivers trusted, every awaited
   operation may fail, …) still hold.

## 11. Deliberate approximations

Loops iterate the value state to a fixpoint but effects are still modeled
as zero or one iteration; `Field` depth 3; K = 6; array elements are
properties `"0"`, `"1"`… only for literals; template literals and arithmetic
give derived provenance, never exact values.

## 12. Performance implications

The value state is per function and dropped after lowering, except return
values, return facts and effect inputs (boxed, optional per effect, so the
size of an IR node stays unchanged). Inertness is linear in the call graph
plus SCC iterations. Concurrent-call inlining increases IR size; the memo
table already shares expansions. Loop fixpoints re-lower a loop body at most
`MAX_LOOP_ITERATIONS` times without retaining IR. Expected: lowering +10–30 %,
summaries depending on newly inlined calls; to be measured
([`PERFORMANCE.md`](PERFORMANCE.md)).

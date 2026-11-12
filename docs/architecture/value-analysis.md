# Value analysis and interprocedural summaries

The static analyzer (`check`, `trace`, `ir`) runs a bounded abstract
interpreter alongside lowering. It tracks values only where they decide
something: which function a call reaches, which object a method is
dispatched on, whether a callee may perform an effect, which values an
effect's arguments derive from, and which branch arms are feasible. It
extends the structured Effect IR, the path walker and the detector of
[`ARCHITECTURE.md`](../ARCHITECTURE.md); it does not replace them.

Code: `midwrite-core::absint` (`value`, `heap`, `env`, `summary`,
`relation`), `midwrite-analyzer` (`values.rs`, `classes.rs`, `link.rs`,
`builtins.rs`, `lower.rs`), `midwrite-core::analysis::{summary,paths}`.
Tests: `crates/midwrite-analyzer/tests/value_layer.rs`,
`crates/midwrite-analyzer/tests/differential.rs`, unit tests in `absint`.

Principles:

* **Every loss is explicit.** A join beyond a bound is `Top` (counted as a
  widening); an unresolved call keeps an unknown effect whose detail says
  why; assumptions are recorded on the call site and in the diagnostic.
* **No new definition of "complete".** A trace is complete exactly when
  its failure window holds no unknown step and no unsupported control flow.
  The value layer only changes *which* steps are unknown, and only by
  resolving them through a fact.
* **TypeScript types are never consulted.** A value is a database handle
  only through the recognition rules; the value layer never invents one.

## Abstract values

```text
AbsValue = Top | { atoms: sorted set of Atom, |atoms| ≤ 6 }
Atom     = Lit(literal)          primitive constant (branch facts, switch cases)
         | Function(f)           a function of the table (call targets, callbacks)
         | Class(c)              a class constructor (static dispatch)
         | Object(o)             an allocation site of the current function (aliasing, receivers)
         | Record                an object with known properties outside any heap (a returned literal)
         | Param(i) | This       summary inputs, substituted at call sites
         | CallResult(k)         what call site k returned (symbolic until summarized)
         | EffectResult(e)       what effect e produced (a row, a response; never concrete)
         | Opaque(n)             a fresh unknown with an identity (an unknown call's result)
         | Field(atom, name)     a property of a symbolic atom, depth ≤ 3
         | Static(symbol)        module-level data or an import: neither an input nor unknown
```

Sets are ordered by inclusion with `Top` greatest; join is union; more than
6 atoms widen to `Top`. Atoms are stored sorted and deduplicated, values
with 0–1 atoms inline. Chains are finite (finitely many atoms per function,
bounded `Field` depth, bounded set size). `Opaque` and `CallResult` give
*identity without knowledge*: two effects that both use the result of one
unknown call provably share it. `await` is the identity on values (a
promise and its fulfillment share an identity); promise *state* is modeled
by execution modes, not values.

**Environment.** Bindings are keyed by Oxc symbol ids (shadowing and
captured variables are distinct). Updates are strong, joins pointwise.
Bindings written from a nested function always read as `Top` (any call
might run that closure). An identifier resolves, in order, to: the
environment, a same-file function, a class, a `const` literal, an import or
module-level binding (`Static`), else `Top`.

**Transfer functions** (in source order, on a copy-on-write `Env × Heap`):
destructuring reads properties `"a"`, `"0"`, … (rest → `Top`); compound
assignment and `x++` → `Top`; a call yields `EffectResult` (recognized
effect), `CallResult` (summarized call) or `Opaque`; `new C()` allocates an
instance of a known class; each arm of `if`/`?:`/`&&`/`switch`/`try` starts
from a copy and joins after; `return e` adds to the function's return value
and, for literal returns, states `$ret` facts. Each transfer function is
monotone; widening only replaces a value with `Top`. Seeded property tests
(`absint::properties`) check the lattice laws.

## Heap and aliases

One abstract object per allocation site per function (object and array
literals, `new C(...)`), with known properties and three flags: `open`
(unlisted properties: spread, computed key, accessor), `escaped` (reachable
from code the analysis does not see), `summary` (stands for several
concrete objects: the site ran again in a loop).

| write | when | effect |
| --- | --- | --- |
| strong | the target is exactly one non-summary object | replace the property |
| weak | several possible objects, or a summary object | join |
| unknown target | a parameter, a field path, `Top` | nothing updated; the written value escapes |
| unknown key | `o[k] = v`, non-literal `k` | object open, every property `Top` |

An absent property reads as `Top` (prototypes are not modeled; never
`undefined`); an escaped object reads `Top` everywhere. An object escapes,
transitively, when passed to any call that is not a modeled non-mutating
builtin or a recognized effect, stored into an escaped object, returned, or
when a closure of the function is passed to a call. Invalidation is
selective. Objects leave a function only as exported `Record`s (depth ≤ 3,
≤ 16 properties).

**Provenance.** Each recognized write and external effect records the
symbolic atoms its arguments contain (≤ 16), each **exact** (the argument
or one of its properties *is* that value) or **derived** (inside a
template, arithmetic, a nested call), plus an `unknown` flag. Table
arguments and `Static` data are ignored. When a callee is inlined its atoms
are substituted into the caller's frame, so provenance survives calls and
returns.

## Interprocedural summaries

A function summary has two parts: the **effect summary** (the callee's
lowered IR with its calls expanded, now with abstract argument values,
return values and return facts) and the **inertness / parameter-use
summary**.

*Inert* means: on every path the function performs no database write, no
transaction, no configured external effect and no unknown call, and every
call it makes is a modeled builtin, a call of an inert function, or an
invocation of a parameter (inertness is then conditional on the argument).
Database reads are allowed.

`absint::summary::solve` computes strongly connected components of the
project call graph (callees and function-valued arguments are edges) and
processes them callees first:

* **inertness** is a greatest fixpoint: every member starts inert and is
  demoted by a refuting fact. For a safety property this is sound (an
  effect would occur at a finite depth), so recursive cycles that do
  nothing are inert without a depth cut. Each component has an iteration
  budget (2 × size + 2); exhausting it demotes the component with a reason;
* **parameter use** (`awaited`, `invoked`, `escaped`) is a least fixpoint.

At each call the inliner (`inline_calls_with`):

1. determines the callee: the linked function, or, for a call of a
   parameter, the function the enclosing call passed for it (callback
   context);
2. if the callee is inert under this call's callbacks, replaces the call by
   an empty `CallScope` marked `inert` (also beyond the depth bound and in
   recursion); a callee with database reads is still inlined so traces
   show its reads;
3. otherwise expands and copies it as before, substituting handles,
   `Param`/`This`, the callee's effect results and fresh identities per
   instantiation, and records the instantiated return value for the
   caller's `CallResult`;
4. else keeps an unknown effect whose detail says why the callee is not
   inert ("calls `render(…)`, which is not inert: …").

**Context sensitivity.** The memo key is `(callee, callback context)` plus
the remaining depth only when the expansion depends on it: only
function-valued arguments for parameters the callee may run enter the key;
constants and handles are substituted on copy. At most 8 callback contexts
are cached per callee (`MAX_SPECIALIZATIONS`); additional contexts are
expanded without caching. The depth bound and recursion guard still apply.
The specialization-limit counter records cache bypasses, rather than
discarded callback effects. Cache occupancy must not change coverage.

**Return facts.** A function's literal returns state `$ret === lit`;
object-literal returns state `$ret` is an object and `$ret.p === lit`. When
the caller binds the result (`const r = await f()`) and tests the binding,
the callee's `$ret` facts are renamed to `r` (property facts only when every
reference of `r` is a property read). Recursive calls are never inlined; a
recursive call of a non-inert callee stays an unknown effect ("recursive").

## Asynchronous and higher-order semantics

Execution modes stay `awaited | returned | fireAndForget | concurrent`.

| construct | model |
| --- | --- |
| resolved call inside `await Promise.all/allSettled/race/any([...])` | summarized; ordered operations copied as `concurrent` (low confidence) |
| `(async () => { … })()` | resolved to the function (immediately invoked) |
| `await f(cb)` where `f` invokes its parameter | the callback is inlined at that point (`[callback passed for parameter #i]`) |
| `await withTimeout(op(), ms)` where the callee awaits/races its parameter | `op()` is a placeholder kept (as `concurrent`) only if the resolved callee awaits that parameter |
| `await Promise.all(xs.map(cb))` | loop (`LoopKind::Callback`, zero or more times) calling `cb` concurrently |
| `xs.forEach(cb)`, unawaited `xs.map(cb)` | loop calling `cb`, fire-and-forget |
| `setTimeout` / `setInterval` / `queueMicrotask(cb)` | deferred: never part of the current sequence; the scheduler is inert only if `cb` is |
| `new Promise(executor)` | executor runs immediately; `resolve`/`reject` are inert |

Never assumed: that a callback passed to a function runs (only when the
callee's summary invokes or leaks it), or that `forEach` awaits. Not
modeled: `.then/.catch/.finally` continuation bodies, completion order
inside a concurrent group, promises stored and awaited later, `for await`,
unawaited calls of resolved functions, transaction handles captured by
callbacks. Exceptions are not crashes: compensation in a `catch` does not
cover a process crash.

`tests/differential.rs` runs small programs under Node.js with a recording
fake runtime and compares the operations with the static trace: ordered
programs match exactly; for a concurrent program the set and first
operation agree and the trace is correctly marked `approximated`.

## Class and method resolution

Classes are collected per module (declarations, `export default class`,
`const C = class {}`) with methods, static methods, instance fields holding
functions, accessors, the explicit constructor, and names changed
dynamically (`this.m = …`, `C.prototype.m = …`,
`Object.defineProperty/defineProperties/assign/setPrototypeOf`).
Superclasses are linked across modules like functions.

| call | receiver | lookup starts at |
| --- | --- | --- |
| `new C().m()`, `const x = new C(); x.m()` | exact: class `C` | `C` |
| `this.m()` in an instance method or arrow field of `C` | `C` or any subclass | `C` |
| `C.m()`, `Imported.m()`, `this.m()` in a static method | static | `C` |
| `super(…)`, `super.m()` | the parent class | parent |

A dynamically changed `m` anywhere in the chain → not resolved. An instance
field `m` wins over a prototype method; an accessor or data field named `m`
first → not resolved. For `this` receivers, any analyzed subclass that
declares or changes `m` → not resolved (closed world over the analyzed
files). Not found and the chain ends in an unanalyzed superclass → not
resolved. Found, but the chain ends in an unanalyzed superclass, or the
exact receiver escaped before the call → resolved under a recorded
**assumption**: findings through such a call get confidence ≤ medium and a
note. Receivers that are parameters, properties or unknown call results
are never resolved: methods are never matched by name. Constructor
parameter properties are not bound to fields.

## Path facts and loops

The path walker of `ARCHITECTURE.md` gains facts from values:

* **member facts**: `x.p` in a condition is a fact on `x#id.p` when `x` is
  never reassigned, every reference is a property read, and `x`'s value
  has no accessors (an object literal of this function, a record, a
  database row, an analyzed call's result). Objects from parameters or
  unknown calls are excluded: a getter could change between reads;
* **return facts** of callees, as above;
* **input facts**: a fact variable whose value is a snapshot of a
  parameter path is an input of the function (`FunctionIr::fact_inputs`):
  a parameter never reassigned (`status#4` = `param#2`), a binding
  destructured from one (`const { status } = params`), or a property of
  the rest object of one (`data#7.status` = `param#1.status` for
  `const { id, teamId, ...data } = params`, unless `status` is one of the
  keys taken out). The destructured parameter must have no reference
  other than property reads and that destructuring; the rest object no
  reference other than property reads and the payload of a Drizzle-shaped
  write (`.update(t).set(data)`, `.insert(t).values(data)`). Each call
  records its arguments as the callee receives them
  (`LocalCall::entry_values`, taken before the call marks objects it
  passes as escaped: primitive literals, the caller's parameter paths,
  closed object-literal records of those). An object literal written as
  the argument and evaluated again in a loop shares its allocation with
  earlier iterations, so the heap holds a summary it cannot export; its
  entry value is then read from the literal itself: literal-valued
  properties keep their literal, other properties are unknown, and a
  spread, computed key or accessor leaves the whole value unknown. On
  instantiation, an arm or
  exit fact on an input whose argument is a literal that contradicts it
  becomes `Never`, so that arm is infeasible for that call only. An input
  the caller forwards from its own parameters becomes an input of the
  caller's expansion and is decided one call further out, provided that
  parameter still holds what the caller received: a destructuring
  pattern, or a binding never reassigned, written through or
  method-called, whose only reference other than property reads and write
  payloads is the forwarding call. A property the
  argument does not list stays undecided (prototype properties are not
  modeled).
* **deferred request methods**: a `fetch` call matched by an `httpMethods`
  rule except for its method, which comes from an options parameter spread
  last into the options or passed as them (`fetch(url, { ...options,
  headers })`), is an unknown effect carrying `DeferredHttpMethod`. Inside
  the function it stays unknown, exactly as a call with unresolved options.
  On instantiation (`Remap::deferred_http`) the call's entry value for that
  parameter decides it: no argument (`LocalCall::argument_count`), or a
  closed record without `method`, gives the method the options set before
  the spread (GET when nothing did and the target is a URL string); a
  record's literal `method`, or a set of literals, gives those. Every
  method listed makes it the configured effect, with its recognition and
  provenance; otherwise it is an ordinary unknown call. A parameter of the
  caller passed on defers it again. Entry values keep a set of literals
  (`exists ? "POST" : "PUT"`) instead of widening it to unknown; facts on
  inputs still need a single literal to be decided. Unlike input facts, a
  key the closed record does not list reads as absent here: `method` is
  not a property of `Object.prototype`.

Facts remain literal (in)equality, truthiness and object-ness, compared
pairwise; there is no constraint solver. When leaving an inlined call,
return facts are part of the merge key when their variable is **live after
the call** (mentioned by a fact outside it); keying by the call prefix lost
a feasible path once a callee was itself inlined
(`return_facts_of_nested_calls_never_merge_away_feasible_paths`).

Loops: effects are modeled as zero or one iteration (exactly once for
`do`-while), with medium confidence; the *value state* is iterated. An
optimistic pass keeps its IR when the entry is already a fixpoint;
otherwise up to 3 value-only passes join the head state, widening changed
bindings to `Top` from the second; if still unstable the head is havocked
(every binding `Top`, every object escaped); the body is then lowered once
from the fixpoint. A per-function budget of 48 dry runs bounds nested
loops.

## Effect relationships (`valueRelation`)

For a two-operation finding, `valueRelation` compares both operations'
provenance in the analyzed function's frame (so it works across summarized
calls):

| `status` | condition |
| --- | --- |
| `provenShared` | the second operation's arguments contain, exactly, the first's result (`derivedFromResult`), or both use one value exactly (`sameValue`) |
| `possiblyRelated` | a shared value only through a computation |
| `noRelationEstablished` | every argument value known, nothing shared — **not** a proof of independence |
| `unknown` | some argument value unknown (escaped, captured, spread, …) |
| `provenDisjoint` | reserved, never produced: database predicates are not interpreted |

It is descriptive only: it never removes or demotes a finding, and neither
fingerprints nor trace ids depend on it. On the reviewed corpus findings
the status did not separate true positives from intended risks. The older
name-based `relationship` field (shared argument *names* within one
function) is likewise descriptive.

## Soundness assumptions

1. Closed world for classes: subclasses exist only in the analyzed files.
2. Modeled globals (`Promise`, timers, `Math`, `JSON`, `Object.keys`, …)
   are not replaced; `console` output is not a consistency-relevant effect;
   `drizzle-orm` query helpers (`eq`, `sql`, …) only build expressions.
3. Property reads run no code (an accessor makes its object open; member
   facts require objects without accessors).
4. Prototype and instance mutation is detected only syntactically in the
   declaring files.
5. Recognized effects do not mutate their arguments.
6. Module-level data is static.
7. The recognition trust model of [`LIMITATIONS.md`](../LIMITATIONS.md#recognition-trust-model)
   holds (configured receivers are trusted; every awaited operation may
   fail).

Deliberate approximations: `Field` depth 3, 6 atoms per value, array
elements only at literal indices, template literals and arithmetic give
derived provenance only, variables of enclosing functions read `Top` inside
callbacks.

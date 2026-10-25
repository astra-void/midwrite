# Interprocedural summaries

Code: `crates/commitrift-core/src/absint/summary.rs` (inertness and
parameter use), `crates/commitrift-core/src/analysis/summary.rs` (the
inliner), `crates/commitrift-analyzer/src/link.rs` (linking),
`crates/commitrift-analyzer/src/lower.rs` (facts).

A Phase 5 function summary has two parts:

1. the **effect summary** — still the callee's lowered IR with its calls
   expanded (ordered effects, transactions, branches, exits; Phase 2), now
   with abstract argument values, return values and return facts; and
2. the **inertness / parameter-use summary** — a fixpoint over the whole
   call graph.

## 1. Facts from the frontend

While lowering a body, every call expression (awaited or not) is accounted
for:

| Call | Fact |
| --- | --- |
| recognized write, transaction, external effect, unmodeled DB method | refutes inertness ("performs a database write (`db.update(orders)`)") |
| recognized read | nothing (reads commit nothing) |
| resolved target (function, import, method, constructor, `super`) | `CallFact` with argument kinds (function value, own parameter, other) |
| call of an own parameter | parameter `invoked` |
| `await p`, `return p` (async), combinator element `p` | parameter `awaited` |
| modeled builtin ([`builtins.rs`](../../crates/commitrift-analyzer/src/builtins.rs)) | inert; its callback argument must be inert (or is an invoked parameter) |
| anything else, `import()`, unknown tags, generators, labeled jumps, `process.exit` | refutes inertness; parameters passed to it `escaped` |

Facts are interned per module (targets and labels), and a body that already
refutes inertness keeps only the calls that forward its parameters.

## 2. Inertness and parameter use (fixpoint)

`solve` computes strongly connected components (iterative Tarjan; callees
*and* function-valued arguments are edges) and processes them callees first:

* **inertness**: greatest fixpoint — every member starts inert and is
  demoted by a refuting fact, a non-inert callee, or a function argument
  that is not inert for a parameter the callee may run (`invoked` or
  `escaped`), or a non-function value passed for an invoked parameter;
* **parameter use**: least fixpoint — a parameter passed on to a callee's
  awaited/invoked/escaped parameter becomes so too.

The greatest fixpoint is sound for a safety property: an effect would have
to occur at a finite call depth, so a recursive cycle whose bodies perform
nothing is inert, and no depth cut is needed. Each component has an
iteration budget (2 × size + 2); exhausting it demotes the component with an
explicit reason (never happened on the corpus: `fixpointBudgetExhausted` 0).

Reasons propagate in a bounded form: "calls `render(…)` (`render`), which is
not inert: calls `renderEmail(component)`, which is not linked: … package".

## 3. Using summaries at call sites

At each call the inliner (`inline_calls_with`):

1. determines the callee — the linked function, or, for a call of a
   parameter (`await action()`), the function the enclosing call passed for
   that parameter (**callback context**);
2. if the callee is **inert under this call's callbacks**, replaces the call
   by an empty `CallScope` marked `inert` (also beyond the depth bound and
   for recursive calls); a callee with database reads is still inlined so
   that traces keep showing its reads;
3. otherwise expands it (memoized) and copies it into the caller as before,
   substituting handles (Phase 2) and now abstract values: `Param`, `This`,
   the callee's effect results, call results and opaque values (fresh per
   instantiation), and records the instantiated return value for the
   caller's `CallResult`;
4. keeps it an unknown effect when it has no recognized operation and is not
   inert — the detail now says why it is not inert.

**Context sensitivity.** Memo key: `(callee, remaining depth, callbacks)`
where `callbacks` lists only function-valued arguments for parameters the
callee may run (from the solved parameter use). Constants and handles never
enter the key (handles are substituted on copy, Phase 2). At most
`MAX_SPECIALIZATIONS` (8) distinct callback contexts are expanded per callee;
beyond that the callee is expanded without callbacks (its parameter calls
stay unknown — the Phase 4 behavior) and the event is counted
(`specializationLimit`: midday 57, school-ledger 123).

**Return values and facts.** A function's return value is the join of its
returned values (exported records; `undefined` if it may fall off the end;
not stored when uninformative). Function-level `return` of an object literal
with literal properties states facts `$ret is an object`, `$ret.p === lit`;
of a literal, `$ret === lit`. At a call whose result is bound by
`const r = await f()` and whose binding the caller actually tests, the
callee's `$ret…` facts are renamed to `r#id…` (property facts only if every
reference of `r` is a property read). The path walker keeps traces apart
that differ in such facts while the binding is live (see
[`PATH-ANALYSIS.md`](PATH-ANALYSIS.md)).

**Concurrency and placeholders** are described in
[`ASYNC-SEMANTICS.md`](ASYNC-SEMANTICS.md).

## 4. Recursion

Recursive calls are still never *inlined* (inlining a cycle is
unbounded): a recursive call of a non-inert callee stays an unknown effect
with the reason "recursive". Inertness covers the recursive case exactly.

## 5. Cache behavior (corpus, final binary)

| | saas-starter | midday | school-ledger | unnamed-ai-project |
| --- | --- | --- | --- | --- |
| functions / inert | 156 / 44 | 14 827 / 4 269 | 6 391 / 1 889 | 648 / 179 |
| SCCs / recursive | 156 / 0 | 14 827 / 6 | 6 391 / 0 | 648 / 0 |
| fixpoint rounds | 185 | 16 206 | 7 115 | 832 |
| expansions / memo hits | 52 / 43 | 2 816 / 1 489 | 2 496 / 5 099 | 61 / 23 |
| callback specializations | 0 | 59 | 45 | 0 |
| calls resolved as inert | 14 | 1 050 | 1 148 | 18 |
| calls summarized (Phase 4 → 5) | 40 → 58 | 1 150 → 2 382 | 1 310 → 2 554 | 23 → 41 |

Memo hit rate: midday 35 %, school-ledger 67 %. The memo table lives only
during inlining.

## Example

```ts
async function execute(action: () => Promise<void>) { await action(); }
export async function completeOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  await execute(async () => { await queue.publish("order.completed", { id }); });
}
```

IR (`commitrift ir`):

```text
call#0 execute(…) inlines `execute` …
  call#1 action() inlines `completeOrder::<anonymous@4:17>` … [callback passed for parameter #0]
    effect#2 external effect (message-publish): queue.publish(…) …
```

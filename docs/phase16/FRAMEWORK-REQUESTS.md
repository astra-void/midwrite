# Phase 16 track B: request bodies and responses

Code: `crates/commitrift-analyzer/src/concurrency/loops.rs`
(`request_param`, `http_call`, `new_response`, `response_value`), hooks in
`concurrency.rs` (`lower_main`, `call_value`, `expr`, `identifier`,
`member`). JavaScript value semantics only (always on in `commitrift
discover`).

## The supported subset

A narrow, source-grounded form: a Fetch-style handler (Next.js Route
Handlers, plain Fetch handlers) whose parameter is declared with the
global `Request` type (not shadowed by an import, class or declaration of
the module) or `NextRequest` imported from `next/server`.

```ts
export async function POST(request: Request) {
  const body = (await request.json()) as { requestId: string };
  ...
  return Response.json({ status }, { status: 201 });
}
```

* **The parsed body is the actor's argument.** The parameter is bound to
  `Binding::Request(<param>)`; `await request.json()` (no arguments,
  awaited) is the local holding the actor's argument for that parameter.
  Discovery synthesizes it like any other argument from the paths the code
  reads (`$request.requestId`); a harness builds a `Request` from it
  (`fixtures/phase16/retry/verify/harness.ts`).
* JSON values only: missing properties (absent keys), `null`, nested
  objects and arrays. The synthesized argument has no `undefined` inside
  (stated assumption `request: ... holds JSON values only`).
* **A body `json()` rejects is not explored.** Modeling the rejection
  faithfully would need the body text (not the parsed value) as the
  argument; the stated assumption is that the body is valid JSON.
* **The body is read once.** A second `request.json()` (on any path the
  extractor lowers, including a later loop iteration) is an unsupported
  operation: Fetch rejects it (the body is used).
* Any other use of the request (`request.headers`, `request.url`,
  `request.formData()`, passing `request` to a helper) is an unsupported
  expression or operation, named.

### Responses

`Response.json(body, init?)` (global `Response`), `NextResponse.json(body,
init?)` (`next/server`) and `new Response(body, init?)` are the value

```
{ status: <init.status, 200 without one>, body: <the value given> }
```

`body` is the value passed before its serialization: a property holding
`undefined` or a non-JSON value is not normalized (stated). `headers` and
`statusText` in `init` are accepted and not modeled; any other `init`
property, a spread, or a non-literal `init` is an unknown expression. The
value is what return-value invariants see (`"returned": {"status": 201,
"body": {...}}`); the `Respond` delivery boundary is unchanged (the scenario
loader inserts it for actors declared as request handlers).
`Response.redirect`, `Response.error`, streaming bodies are not modeled.

Nothing executes the handler and no HTTP framework is modeled: routing,
middleware, cookies, `params` of dynamic segments are outside the subset.

## Not supported (documented)

* **Server Actions** (`"use server"` functions receiving `FormData`): no
  runnable vertical slice exists in this phase; `formData.get(...)` stays
  unknown. Server actions called with plain arguments are entries as
  before (Phase 7).
* **tRPC** procedures (`publicProcedure.input(schema).mutation(...)`): not
  supported; the procedure builder is an unknown call.
* `request.text()`, `request.formData()`, `request.arrayBuffer()`, query
  strings (`new URL(request.url).searchParams`).

## Evidence

* `fixtures/phase16/retry/src/app/api/orders/route.ts#POST` is an
  extracted route-handler entry with no unknown operation; discovery
  reports its `ambiguous-commit` candidate (through `placeOrder`'s retry
  loop) as MODEL_COUNTEREXAMPLE with the `request: ` assumptions.
* `fixtures/phase16/loops/src/shapes.ts#POST`: with an invariant on
  `"returned": {"status": 201, "body": {"ok": true}}` the model finds no
  counterexample (the response value is what the code returns), and the
  body's `label` reaches the database; `#PUT` (two reads of the body) is
  refused ("read twice").
* Phase 11's labeled `src/app/api/checkout/route.ts#POST`
  (`fixtures/phase11/labels.json`: "unsupported: request.json() is not
  modeled") is now extracted without unknown operations: the label's
  premise no longer holds (see the final report: the Phase 11 evaluation
  test's count of reported unsupported entries changes from 3 to 2).

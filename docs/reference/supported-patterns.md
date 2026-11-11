# Supported patterns

What each analysis recognizes in the current tree. Anything
not listed is reported as unknown (`MW9001`, an `Unknown` operation,
`UNSUPPORTED`) rather than guessed. Gaps: [limitations](limitations.md).
Concept summary: [supported semantics](../concepts/supported-semantics.md).

## Static analyzer (`check`, `trace`, `ir`)

| pattern | support |
| --- | --- |
| Drizzle writes `db.insert/update/delete(...)…` | executed when awaited, returned from an `async` function, run by `Promise.all`, or ended with `.execute()`/`.then()`; a builder that is only constructed is not a write |
| Drizzle reads `db.select(...)`, `db.query.<t>.findFirst/findMany`, `db.$count` | recognized; never a commit |
| `db.transaction(async (tx) => …)` | writes on `tx` commit when the callback completes; nested `tx.transaction` = savepoint; writes on `db` inside are not part of the transaction; `tx.rollback()` is an exit |
| builders stored in a `const` and awaited later | `const q = db.update(t).set(v); await q;` |
| receivers | configured names, renamed imports, `const` aliases, `importedFrom` checks, `drizzle(...)` handles, argument-free configured calls (`getDb()`) |
| member receivers | configured member paths (`this.db`, `ctx.db`, `this.queue`); a weaker `member-path` match; unconfigured lookalikes are `MW9001` |
| external effects | configured `receiver.method(...)` or `function(...)`, optionally followed by `.then/.catch/.finally`; first-argument rules; best-effort |
| control flow | sequences, `if`/`else`, `?:`, `&&`/`\|\|`/`??`, `switch` with fall-through, `return`, `throw`, `try`/`catch`/`finally`, loops (effects zero or one iteration; values to a fixpoint), `Promise.all/allSettled/race/any` over array literals, `Promise.all(xs.map(cb))`, `xs.forEach(cb)` |
| functions | declarations, expressions, arrows, class and object methods, `export default`, Next.js route handlers and Server Actions; nested functions analyzed separately |
| same-file and imported helpers | awaited or `async`-returned calls summarized up to `--max-call-depth` (3), never recursively; named, aliased, default and namespace imports; `export { } from`, `export *`; relative specifiers, `tsconfig` `paths`/`baseUrl` (single `*`, relative `extends`), workspace packages (`package.json` `name` + `exports`/`main`) |
| object services | `const orders = { async markPaid() {…} }`, same file or imported, unless its properties are assigned or deleted in its file |
| classes | `new C().m()`, `const x = new C(); x.m()`, `this.m()` without analyzed overrides, static methods, `super`, inheritance across modules; a package superclass → recorded assumption, ≤ medium |
| callbacks and wrappers | callbacks invoked by analyzed functions (≤ 8 contexts per callee), inert wrappers such as `withTimeout(op(), ms)`, IIFEs; `setTimeout` callbacks are deferred |
| inert helpers | calls of functions that perform nothing durable, visible or unknown on any path are resolved at any depth |
| handle arguments | a transaction handle, configured receiver (or configured call) or `drizzle(...)` instance passed as a direct identifier is bound to the callee's parameter |
| branch correlation | contradictory decisions on never-reassigned variables, `const` literals, literals returned by inline transaction callbacks and by summarized helpers, properties of unescaped objects |

Source: [README § supported patterns](../../README.md#supported-patterns),
[`docs/LIMITATIONS.md` § support levels](../LIMITATIONS.md#support-levels).

## Concurrency extractor (`model-check`, `discover`)

How these constructs are lowered: [source extraction](../architecture/source-extraction.md).
Forms marked *js-values* need JavaScript value semantics (scenario feature
`js-values`, always on in `discover`).

| construct | notes |
| --- | --- |
| entries by `file` + `function` or source selector (`module#name`, `…/arg[N]`, `/return`, `/method[m]`, `/static[m]`, `/property[p]`, `file@line:col`) | wrapped callbacks need the wrapper's invocation to be known or declared |
| `db.query.T.findFirst({ where })`; `select().from(T).where(p).limit(1)`; `select({ n: count() })` | rows in primary-key order without `orderBy` (stated assumption) |
| `.for("update" \| "no key update" \| "share" \| "key share")`, `skipLocked` / `noWait` | `skipLocked`/`noWait` with `update` only on single-row selects |
| multi-row `select().from(t).where(inArray(t.id, ids)).for(…)`, `skipLocked`, `noWait`, `.orderBy(<primary key>)`, `.limit(n)` | row locks taken one row at a time (a statement can hold a prefix while it waits); without `orderBy` the model uses primary-key order (an approximation: PostgreSQL follows the plan) |
| inner and left joins on column equality, projections, row arrays (`length`, `[0]`) | no aliases, no locks with joins |
| `insert(T).values({…})`, `values([{…}, …])` (multi-row, *js-values*), `onConflictDoNothing()`, `returning()`, `returning({ alias: t.col })` | a multi-row insert is one atomic statement; `values(obj)` of a parameter object is unknown |
| `onConflictDoUpdate({ target, set, setWhere })`, `excluded` | not `targetWhere` |
| `update(T).set({…}).where(p)`, `delete(T).where(p)`, `returning()` count/presence | row values of `UPDATE/DELETE … RETURNING` not modeled |
| predicates `eq ne gt gte lt lte and or not isNull isNotNull inArray notInArray`; `sql` templates with column refs, literals, locals and `+ - *`; conditional predicates | — |
| predicates held in locals: `const p = and(eq(..), ..)`, aliases, `cond ? isNull(c) : eq(c, v)`, `cond ? pred : undefined`, project helpers whose body is `return <predicate>` | operands captured at the declaration (as Drizzle does); a reassigned `let` is refused |
| `db.transaction(async (tx) => …, { isolationLevel })`, `throw` rolls back | `try` inside the callback is unknown |
| `try`/`catch`/`finally`, `switch`, `??`, value-position `\|\|`/`&&`, `for…of` over literal arrays, destructured parameters | a crash runs no handler |
| `for (let i = 0; i < N; i++)`, `while`, `do … while` with an analyzed counter (≤ 8 iterations), `break`, `continue`, early `return`, `try` inside the loop (*js-values*) | unrolled per iteration; classified `BOUNDED_UNROLLING`, `TRUNCATED`, `UNKNOWN`, `UNSUPPORTED` or `COMPLETED` ([loops](../architecture/loops-and-errors.md)) |
| `catch (e)` reading `e.code`, `e.cause?.code`, `e instanceof Error`, helpers like `isSerializationFailure(e)` (*js-values*) | only for failures of modeled statements and modeled `throw`s, with drizzle-orm 0.45 + `pg`/`postgres` error shapes verified ([caught errors](../architecture/loops-and-errors.md#caught-errors)) |
| arrays (literals, indexing, `includes`/`indexOf`/`join`, `map`/`filter`/`find`/`some`/`every` with a one-expression arrow, destructuring, `for…of` over a snapshot), strings (template literals, `+`, `String(x)`, `length`, `includes`/`startsWith`/`endsWith`, `split` by a literal, ASCII case mapping, `trim`), `undefined`, `Map`/`Set` locals used through their own methods (*js-values*) | values, not references: mutation of arrays (`push`, `splice`, index assignment), `===` on arrays/maps/sets, iteration over `Map`/`Set` and anything else are unknown ([value semantics](../concepts/value-semantics.md)) |
| closures passed to inlined project functions (`withRetry(() => …)`) (*js-values*) | inlined where and as often as the wrapper calls them; captured variables shared |
| helper inlining across modules (≤ `maxInlineDepth`, default 3), `Promise.all([...])`, detached calls | — |
| barrels, `export * as`, const object repositories, class instances with constructor-stored handles, `this.m()`, `getDb()` factories, typed `db` parameters | union handle types bound only when inlined with a known handle |
| exported function aliases (≤ 8 hops), React `cache(fn)` (first call) | — |
| generated keys: `uuid().defaultRandom()`, `$defaultFn` with recognized generators (`crypto.randomUUID()` …), sequences, identity, `gen_random_uuid()`; `nanoid()` / `randomUUID()` calls | identities, not concrete values ([schema extraction](../architecture/schema-extraction.md#defaults-and-generated-values)) |
| Drizzle schemas: `pgTable`, `pgEnum`, `pgSchema`, `.references()`, `foreignKey()`, unique sets incl. `NULLS NOT DISTINCT`, several modules | [schema extraction](../architecture/schema-extraction.md) |
| `timestamp`/`timestamptz`/`date` and `json`/`jsonb` columns, `defaultNow()`, `new Date()` | with the scenario features `timestamps` / `json`; time arithmetic and JSON operators unknown |
| `withReplicas` from `drizzle-orm/pg-core`, declared replica factories, `$primary`, `usePrimaryOnly()` | reads routed to the replica only with the `replicas` feature |
| zod schemas: `z.object`, `z.strictObject`, `z.looseObject`, `.strict()`/`.strip()`/`.passthrough()`, `optional`/`nullable`/`nullish`/`default(literal)`, arrays, `uuid`, `email`, `.regex(/literal/)`, enums over `const` arrays; `parse`/`safeParse` | version-aware (zod 3 / zod 4); refinements, transforms, preprocess, coerce, unions refused by name ([zod](zod.md)) |
| `export async function POST(request: Request)` with `await request.json()`; `Response.json`, `NextResponse.json`, `new Response` | the parsed body is the actor's argument (valid JSON assumed, read once); the response is `{ status, body }` ([requests](../architecture/source-extraction.md#requests-and-responses)) |
| `unique().on(..).nullsNotDistinct()`, column `.unique(name, { nulls: "not distinct" })` | enforced in INSERT, multi-row INSERT, UPDATE, `ON CONFLICT`, and the wait on an uncommitted equal key |
| entry return values (literals, locals, query results, row counts, comparisons, helper results, responses) | recorded only when an invariant reads them |
| invariant formulas over generated identities: `generatedBy`, `sameGenerated`, `distinctGenerated`, `same`, `forEachActor`, `forEachRow`, `countMatching`/`rowMatching`, `returned`, `argument` | matching by actor and generator, not occurrence ([invariants](../concepts/invariants.md)) |
| external effects declared in `extraction.externals` / contracts | — |

## Input synthesis

| pattern | how discovery uses it |
| --- | --- |
| `x: T \| null`, `x?: T`, `x: T \| undefined`, default values | `null`/`undefined` worlds (never without an annotation) |
| a string-literal union, inline or a type alias in the module or imported relatively (`kind: RouteKind`) | argument values are its members |
| `if (x > 10)`, `x === "admin"`, `flag === true` over an argument | operand and boundary worlds |
| a guard over a read row (`if (!m \|\| m.role !== "admin") throw`, `if (q.remaining <= 0) throw`) | the stored row is set to pass it (stated precondition); stored boundary worlds |
| a guard over an argument, also inside an inlined helper (`actor.kind === "staff" && actor.roles.includes("admin")`) | the argument is set to pass it (stated precondition) |
| an UPDATE/INSERT writing an argument to a column a row already has | `stored-distinct` / `stored-equal` worlds |
| a read, a guard on one of its columns, then a relative UPDATE of that column (`c = c - 1` via Drizzle's `sql` template) | `guarded-update` candidates when a property over the table is declared |

Details: [scenario synthesis](../architecture/scenario-synthesis.md);
zod-validated arguments: [zod § input domains](zod.md#input-domains-and-synthesis).

## Runtime coordination

| | supported |
| --- | --- |
| replay harnesses (`replay`) | Node.js ≥ 22 (compiled JavaScript) and Bun ≥ 1.2; node-postgres (`pg`) adapter |
| original code (`--verify-source`) | Node.js ≥ 22 with node-postgres 8.23.1 or postgres-js 3.4.5, drizzle-orm 0.45.4 |
| PostgreSQL | disposable local clusters; measured on 17.11 and 15.19 |
| isolation | macOS `sandbox-exec`, Linux bubblewrap with unprivileged user namespaces |

# Supported patterns

What each analysis recognizes, as of the Phase 13 working tree. Anything
not listed is reported as unknown (`CP9001`, an `Unknown` operation,
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
| member receivers | configured member paths (`this.db`, `ctx.db`, `this.queue`); a weaker `member-path` match; unconfigured lookalikes are `CP9001` |
| external effects | configured `receiver.method(...)` or `function(...)`, optionally followed by `.then/.catch/.finally`; first-argument rules; best-effort |
| control flow | sequences, `if`/`else`, `?:`, `&&`/`\|\|`/`??`, `switch` with fall-through, `return`, `throw`, `try`/`catch`/`finally`, loops (effects zero or one iteration; values to a fixpoint), `Promise.all/allSettled/race/any` over array literals, `Promise.all(xs.map(cb))`, `xs.forEach(cb)` |
| functions | declarations, expressions, arrows, class and object methods, `export default`, Next.js route handlers and Server Actions; nested functions analyzed separately |
| same-file and imported helpers | awaited or `async`-returned calls summarized up to `--max-call-depth` (3), never recursively; named, aliased, default and namespace imports; `export { } from`, `export *`; relative specifiers, `tsconfig` `paths`/`baseUrl` (single `*`, relative `extends`), workspace packages (`package.json` `name` + `exports`/`main`) |
| object services | `const orders = { async markPaid() {…} }`, same file or imported, unless its properties are assigned or deleted in its file |
| classes (Phase 5) | `new C().m()`, `const x = new C(); x.m()`, `this.m()` without analyzed overrides, static methods, `super`, inheritance across modules; a package superclass → recorded assumption, ≤ medium |
| callbacks and wrappers (Phase 5) | callbacks invoked by analyzed functions (≤ 8 contexts per callee), inert wrappers such as `withTimeout(op(), ms)`, IIFEs; `setTimeout` callbacks are deferred |
| inert helpers (Phase 5) | calls of functions that perform nothing durable, visible or unknown on any path are resolved at any depth |
| handle arguments | a transaction handle, configured receiver (or configured call) or `drizzle(...)` instance passed as a direct identifier is bound to the callee's parameter |
| branch correlation | contradictory decisions on never-reassigned variables, `const` literals, literals returned by inline transaction callbacks and by summarized helpers, properties of unescaped objects |

Source: [README § supported patterns](../../README.md#supported-patterns),
[`docs/LIMITATIONS.md` § support levels](../LIMITATIONS.md#support-levels).

## Concurrency extractor (`model-check`, `discover`)

| construct | since | notes |
| --- | --- | --- |
| entries by `file` + `function` or source selector (`module#name`, `…/arg[N]`, methods, object properties, returned functions) | 6; 7 | wrapped callbacks need the wrapper's invocation to be known or declared |
| `db.query.T.findFirst({ where })`; `select().from(T).where(p).limit(1)`; `select({ n: count() })` | 6 | rows in primary-key order without `orderBy` (stated assumption) |
| `.for("update")` with `skipLocked` / `noWait` | 6 | |
| `.for("no key update" \| "share" \| "key share")` | 13 | |
| inner and left joins on column equality, projections, row arrays (`length`, `[0]`) | 7 | no aliases, no locks with joins |
| `insert(T).values({…})` (one row), `onConflictDoNothing()`, `returning()` | 6; 7 | |
| `returning({ alias: t.col })` | 12 | |
| `onConflictDoUpdate({ target, set, setWhere })`, `excluded` | 13 | not `targetWhere` |
| `update(T).set({…}).where(p)`, `delete(T).where(p)`, `returning()` count/presence | 6; 7 | row values of `UPDATE/DELETE … RETURNING` not modeled |
| predicates `eq ne gt gte lt lte and or not isNull isNotNull`; `sql` templates with column refs, literals, locals and `+ - *` | 6 | a conditional expression inside `and(...)` is not a predicate |
| `db.transaction(async (tx) => …, { isolationLevel })`, `throw` rolls back | 6 | |
| `try`/`catch`/`finally`, `switch`, `??`, value-position `\|\|`/`&&`, `for…of` over literal arrays, destructured parameters | 7 | |
| helper inlining across modules (≤ `maxInlineDepth`, default 3), `Promise.all([...])`, detached calls | 6 | |
| barrels, `export * as`, const object repositories, class instances with constructor-stored handles, `this.m()`, `getDb()` factories, typed `db` parameters | 12 | union handle types bound only when inlined with a known handle |
| generated keys: `uuid().defaultRandom()`, `$defaultFn` with recognized generators (`crypto.randomUUID()` …), sequences, identity, `gen_random_uuid()` | 7; 12 | |
| Drizzle schemas: `pgTable`, `pgEnum`, `pgSchema`, `.references()`, `foreignKey()`, unique sets, several modules | 8; 12; 13 | |
| `withReplicas` from `drizzle-orm/pg-core`, declared replica factories, `$primary`, `usePrimaryOnly()` | 13 | reads routed to the replica only with the `replicas` feature |
| exported function aliases (≤ 8 hops), React `cache(fn)` (first call) | 13 | |
| entry return values (literals, locals, query results, row counts, comparisons, helper results) | 13 | recorded only when an invariant reads them |
| external effects declared in `extraction.externals` / contracts | 6; 11 | |

Sources: [Phase 6 concurrent IR § TypeScript subset](../phase6/CONCURRENT-IR.md#the-typescript-subset-tsactorprovider),
[Phase 7 Drizzle extraction](../phase7/DRIZZLE-EXTRACTION.md),
[Phase 12 architecture](../phase12/ARCHITECTURE.md),
[Phase 13 source resolution](../phase13/SOURCE-RESOLUTION.md),
[Phase 13 advanced Drizzle](../phase13/ADVANCED-DRIZZLE.md).

## Input synthesis (Phase 15)

| pattern | how discovery uses it |
| --- | --- |
| `x: T \| null`, `x?: T`, `x: T \| undefined`, default values | `null`/`undefined` worlds (never without an annotation) |
| a string-literal union, inline or a type alias in the module or imported relatively (`kind: RouteKind`) | argument values are its members |
| `if (x > 10)`, `x === "admin"`, `flag === true` over an argument | operand and boundary worlds |
| a guard over a read row (`if (!m \|\| m.role !== "admin") throw`, `if (q.remaining <= 0) throw`) | the stored row is set to pass it (stated precondition); stored boundary worlds |
| a guard over an argument, also inside an inlined helper (`actor.kind === "staff" && actor.roles.includes("admin")`) | the argument is set to pass it (stated precondition) |
| an UPDATE/INSERT writing an argument to a column a row already has | `stored-distinct` / `stored-equal` worlds |
| a read, a guard on one of its columns, then a relative UPDATE of that column (`c = c - 1` via Drizzle's `sql` template) | `guarded-update` candidates when a property over the table is declared |

Details: [Phase 15 input domains](../phase15/INPUT-DOMAINS.md),
[constraint synthesis](../phase15/CONSTRAINT-SYNTHESIS.md).

## Runtime coordination

| | supported |
| --- | --- |
| replay harnesses (`replay`) | Node.js ≥ 22 (compiled JavaScript) and Bun ≥ 1.2; node-postgres (`pg`) adapter |
| original code (`--verify-source`) | Node.js ≥ 22 with node-postgres 8.23.1 or postgres-js 3.4.5, drizzle-orm 0.45.4 |
| PostgreSQL | disposable local clusters; measured on 17.11 and 15.19 |
| isolation | macOS `sandbox-exec`, Linux bubblewrap with unprivileged user namespaces |

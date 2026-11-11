# Source extraction (TypeScript → model IR)

`crates/midwrite-analyzer/src/concurrency.rs` (`TsActorProvider`) lowers
one entry function of a project — and the helpers it calls, inlined up to
`maxInlineDepth` (default 3) — into the model checker's IR: statements on
connections and transactions, local computation, branches and jumps,
external effects, tasks, `Unknown` operations. It is used by the scenario
loader (`model-check`) and by `discover`, which calls it with exactly the
request a generated scenario's loader will make, so discovery and
model checking see the same code.

Related pages: [schema extraction](schema-extraction.md),
[loops, retries and caught errors](loops-and-errors.md),
[zod validation](../reference/zod.md),
[supported patterns](../reference/supported-patterns.md),
[limitations](../reference/limitations.md#concurrency-extractor-model-check-discover).

## Inputs

| input | source |
| --- | --- |
| entry | `file` + `function`, or a **source selector** (below) |
| tables | declared in the scenario, or read from Drizzle schema modules (`pgTable`, `pgEnum`, `pgSchema`), several modules, bound by resolved declaration ([schema extraction](schema-extraction.md)) |
| configuration | `databaseReceivers` (default `db`), `extraction.externals`, `transactionRetries` |
| versioned features | `ExtractRequest.return_values` (only when an invariant reads them), `replicas` and declared `replica_factories`, `js_values` (scenario feature `js-values`; always on in `discover`) — see [versioned semantics](../concepts/supported-semantics.md#versioned-semantics) |

## Function identity and source selectors

`identity.rs`. A function is identified by **its module and the line and
column where its syntax node starts** (its *anchor*):
`src/app/actions.ts@43:3`. This is the position `midwrite check` uses to
name anonymous functions (`<anonymous@43:3>`), so the extractor and the
static analyzer refer to the same function by position — never by text or
name. Two anonymous callbacks with identical text are different functions.
Every extraction report carries `entry.anchor`, `entry.file`/`entry.endLine`
(the function's region, used at runtime to classify statements as inside or
outside the modeled function) and `entry.staticName`.

| selector | function |
| --- | --- |
| `src/app/actions.ts#inviteTeamMember/arg[1]` | argument 1 of the call that initializes `inviteTeamMember` |
| `src/factory.ts#makeHandler/return` | the function `makeHandler` returns (its single `return <expr>`) |
| `src/service.ts#OrderService/method[place]` | instance method |
| `src/service.ts#OrderService/static[purge]` | static method |
| `src/handlers.ts#handlers/property[create]` | function-valued object-literal property (incl. method shorthand) |
| `src/handlers.ts#router/property[archive]/arg[0]` | steps chain |
| `src/app/actions.ts@43:3` | the function starting at 43:3 |

Bindings are top-level declarations (exported or not, `export default`).
Every step must resolve to exactly one syntax node, otherwise the selector
is an error with the reason (`the call has no argument 2`, `the function
has 2 return statements: select the function by its anchor`). Functions
created dynamically (`new Function`, `eval`), stored in maps, or class
fields holding arrow functions are selected by anchor. Arguments are the
selected function's parameters; simple object-pattern parameters
(`async ({ input }) => …`) are bound whole and each property read once.

## Callbacks and higher-order wrappers

`wrapper.rs`, `concurrency.rs` (`entry_info`). Passing a callback does not
mean it runs. When an entry is reached through an `arg[N]` step, the
extractor resolves the callee of that call (same file, relative import,
tsconfig `paths`, workspace package — never by name) and reads the
wrapper's source:

| `invocation` | meaning |
| --- | --- |
| `returned-function-returns-promise` | the wrapper returns a function whose `return fn(...)` hands the callback's promise to its caller |
| `returned-function-awaits` | the returned function awaits `fn(...)` |
| `returned-function-fire-and-forget` | the returned function calls `fn(...)` without awaiting or returning it |
| `invoked-during-wrap` / `-unawaited` | the wrapper calls `fn` while wrapping |
| `stored` | the parameter is used other than by calling it |
| `not-invoked` | the wrapper never references it |
| `unknown` | the wrapper is not analyzed (package, method call, concise body, shadowing, several calls) |

The analysis is bounded (one level of returned function, direct calls of
the parameter). `conditional` is set when an earlier statement of the
invoking function can exit or reject (`if (!user) throw`, `await
getUser()`), or the call is nested in a branch; those statements are
listed as `guards`, and `arguments` lists what the wrapper passes.

`entry.invocation` (`awaited` / `fire-and-forget`) and
`entry.preconditions` are **declared contracts**, reported apart from
source facts and never shown as proven:

| source facts | contract | result (`contractStatus`) |
| --- | --- | --- |
| no wrapper | — | `not-needed` |
| known, unconditional | none | `absent` (allowed) |
| known, conditional | none | **refused**: declare the conditions you assume |
| known | consistent / contradicting | `consistent` / **refused** |
| `stored` / `unknown` | none / given | **refused** / `required` (the contract is the only basis) |
| `not-invoked` | any | **refused** |

The actor is the callback's body. The wrapper's own code around the call
(authentication lookup, validation) is not part of the actor: the
extraction records it as an assumption, and at runtime its statements are
*outside the model* (reads allowed; a write makes a verification
inconclusive). Only the innermost wrapper of a chain is analyzed.

**Closures** (`Binding::Closure`): an arrow function or function
expression passed as an argument to an *inlined* project function keeps
its function node (module, span) and the scopes it closed over. When code
calls a name bound to a closure, the body is inlined at that call, as
often as the wrapper calls it, with the call's arguments bound to its
parameters. Captured names are the same model locals as in the defining
function, so an assignment after the capture is seen. A retry wrapper
(`try { return await op() } catch { return await op() }`) therefore runs
the body again in its handler. A closure that is returned, stored or
passed to a package call is not followed (unknown).

## Control flow

* **Exceptions.** `try { B } catch (e) { C } finally { F }` lowers to
  `Try`/`EndTry`/`Rethrow` handlers (local operations, part of the
  canonical state): `F` runs on the normal path, after a caught exception,
  and — re-throwing afterwards — after an exception in `C`. A failure of a
  thread with an active handler (`throw`, a rejected awaited helper, a
  database error, a failed `Promise.all` child at the join) resumes at the
  handler; if the handler was entered outside a transaction that is now
  open, the transaction is rolled back first (Drizzle's `transaction`
  rolls back and re-throws). A `return` from an inlined helper closes the
  helper's open `try` blocks. **A crash is not an exception:** no handler
  and no `finally` runs. `try` inside a transaction callback and
  `return`/`break`/`continue` through a `finally` are unknown. Inspecting
  a caught error: [caught errors](loops-and-errors.md#caught-errors).
* **Branches.** `switch` (strict equality, fall-through, `default`
  anywhere; case tests must be side-effect free), `a ?? b`, and
  value-position `a || b` / `a && b` hoisted so that `b` runs exactly when
  JavaScript runs it; conditions keep the boolean form. Object literals in
  value position are values. Correlated branches through helper return
  values work when the discriminant is a literal.
* **Loops.** `for…of` over an array literal is unrolled; with JavaScript
  value semantics, bounded `for`/`while`/`do…while` loops are unrolled
  with a classification ([loops](loops-and-errors.md)). No bound is ever
  guessed.
* **Asynchrony.** `await` ordering; `Promise.all` children as concurrent
  tasks with a join; fire-and-forget calls as detached tasks whose failure
  terminates the process. No completion order is invented for independent
  promises. `.then` chains, timers and event emitters are unknown.

## Drizzle statements

Four layers are kept apart: (1) source-level query construction in the
extractor, (2) abstract statement semantics in the model
(`StmtKind`/`ClosedStmt`, `exec.rs`), (3) the SQL a backend runs (the
oracle's model SQL, or Drizzle's own SQL observed at runtime), (4) what
PostgreSQL did. Statements are compared on results, not only on final
rows: a different returned row can change later control flow. Database
semantics: [PostgreSQL model](postgres-model.md).

Lowered forms (the full list is in
[supported patterns](../reference/supported-patterns.md#concurrency-extractor-model-check-discover)):

* reads: point and predicate selects, `count`, inner and left joins on
  column equality (no aliases, no locks; rows nested by table name as
  Drizzle returns them), projections of whole tables or columns (computed
  fields unknown), row arrays (`rows.length`, `rows[0]`), multi-row
  `SELECT` with row locks (`many: ManyRows`);
* writes: `insert … values` (one row or several), `onConflictDoNothing`,
  `onConflictDoUpdate({ target, set, setWhere })` with `excluded`,
  `update`, `delete`, `returning()` (counts, presence, inserted rows,
  `returning({ alias: t.col })`);
* `.for("update" | "no key update" | "share" | "key share")`,
  `skipLocked`/`noWait`; transactions with an isolation level;
* constants: TypeScript `enum`s and `const` objects of literals, local or
  imported; `const obj = {...}` passed to `values(obj)` (properties
  evaluated at the declaration);
* omitted columns take their default: constants, sequences, generated
  values ([schema extraction](schema-extraction.md#defaults-and-generated-values)).

**Held predicates.** Drizzle builds an `SQL` object when an operator is
called, so a predicate stored in a local carries the operand values of the
call, not of the use. A `const p = <init>` (or a `let` the module never
assigns again) whose initializer is predicate-shaped — a call of `eq ne gt
gte lt lte and or not isNull isNotNull inArray notInArray` imported from
`drizzle-orm`, an alias of a held predicate, `test ? predA : predB`,
`test ? pred : undefined`, or a call of a non-`async` project function
whose body is `return <predicate>` — is lowered **where it is declared**:
JavaScript operands are evaluated into fresh locals there; a conditional
becomes `(which AND a) OR (NOT which AND b)` with `which = !!test`, exact
under three-valued logic; an `undefined` predicate stands for what
`undefined` means at its use (`true` in `and`/`where`, `false` in `or`) and
is refused where it has no meaning (`not(p)`). Every use adds the
assumption "the Drizzle predicate `p` … is built where it is declared".
Unknown held predicates make the query an `Op::Unknown` (never `true`): a
`let` the module assigns again (textual, conservative), a clock reading in
the predicate, operators outside the list, nested conditionals, helpers
that do more than `return <pred>`, operators not imported from
`drizzle-orm`.

## Requests and responses

A Fetch-style handler whose parameter is declared with the global
`Request` type (not shadowed) or `NextRequest` from `next/server`:

* `await request.json()` (no arguments, awaited) is the local holding the
  **actor's argument** for that parameter; discovery synthesizes it from
  the paths the code reads (`$request.requestId`). JSON values only; a body
  that `json()` would reject is not explored (stated assumption: the body
  is valid JSON). The body is read once: a second `request.json()` on any
  lowered path is unsupported. Any other use of the request (`headers`,
  `url`, `formData()`, passing it to a helper) is unsupported, named.
* `Response.json(body, init?)`, `NextResponse.json(body, init?)` and
  `new Response(body, init?)` are the value `{ status, body }` (`status`
  200 without `init.status`); `headers`/`statusText` are accepted and not
  modeled; other `init` forms are unknown; `Response.redirect`,
  `Response.error` and streaming bodies are not modeled.

No HTTP framework is modeled (routing, middleware, cookies, dynamic
`params`). Server Actions receiving `FormData` and tRPC procedures are not
supported; Server Actions called with plain arguments are ordinary entries.

## Return values and the response step

When an invariant reads return values (`"returned": v` in an `actors`
term), the extractor emits `Op::ReturnValue { expr }` (a local operation)
before the entry's `Op::Return`: literals, locals, query results, mutation
row counts, comparisons, helper results, awaited promises, early returns,
expression-bodied arrows, responses as above. An expression that is not
understood stays `Expr::Unknown`, and the loader refuses an invariant that
reads that actor's return value; nothing is invented. Otherwise
`ReturnValue` is not extracted and the state space is unchanged.

An actor with `"response": true` is a request handler whose response is a
separate visible step `Op::Respond`, inserted by the loader before every
`Op::Return` of the main code. A crash between the last database operation
and `Respond` leaves the work committed and the client uninformed; a
declared `onCrash` retry then runs the request again. Error responses (a
thrown handler) are not a separate step. Outcome semantics and invariants:
[invariants](../concepts/invariants.md),
[failure model](../concepts/failure-model.md).

## Cross-module resolution

| pattern | resolution |
| --- | --- |
| `export { x } from`, `export { a as x } from`, `export *`, `export * as ns` | followed to the declaring module; a name provided by two `export *` modules is ambiguous and not followed; cycles are reported |
| `obj.m(...)` on a module-level `const obj = { ... }` (same module or imported) | the method is inlined; `this.other()` resolves to the same object. Refused when the module writes `obj.p = …`, `delete obj.p`, `Object.assign(obj, …)` or the literal has a spread |
| `inst.m(...)` on `const inst = new C(args)` or a local `new C(tx)` | the method is inlined with a `this` binding: constructor parameter properties and `this.f = param` map fields to constructor arguments (handles stay handles). Refused: `extends`, constructors doing anything else, field initializers with values |
| `getDb().insert(...)`, `const db = getDb()` | a zero-argument call whose **declared return type** is a `drizzle-orm` `…Database` type (directly, `Promise<…>`, or a type alias) is a root database handle; bodies are not interpreted |
| `fn(db: Database)` entry parameter | a root handle when `Database` resolves as above; otherwise Drizzle calls on it are "database handle of an unresolved type" (union types such as `Database \| Transaction` are bound only when inlined with a known handle) |
| `db.transaction(async (tx) => repo.save(tx, …))` | the handle survives object and class methods: an operation on `tx` stays in the transaction, one on `db` stays autocommit |
| `export const a = b` | function aliases followed (same module, imported, re-exported; at most 8 hops), with an assumption naming the alias |
| `export const f = cache(fn)`, `cache` from `react` | the actor's **first** call inlines `fn`; a repeated call is unknown (memoized result); `cache` of any other package is not this contract |
| any module-level name used as a database | its value provenance (`handles.rs`): primary, replicated (`withReplicas` from `drizzle-orm/pg-core`, or a declared replica factory), ambiguous, transaction, unknown — never by name alone in a project that builds a replicated database; `x.$primary`, `x.usePrimaryOnly()` are primary handles |
| template literal without interpolation | a string literal |
| a call into a package | unknown, unless a verified contract applies (below); no package is executed or assumed pure |

**Package contracts** (`packages.rs`). Every package call is in one
category: a **verified built-in contract** (`packages::CONTRACTS`, written
from the package's documented API at named versions: `bcryptjs`/`bcrypt`
`hash`, `compare`, `genSalt`; `nanoid()` and `randomUUID()` as generated
values; React `cache` as above; zod schemas, now modeled — see
[zod](../reference/zod.md)), a **declared external contract**
(`externals` of `midwrite-contracts.json`, lowered to an external effect),
or **unknown package behavior**. A contract matches an exact package
specifier and export, or a method of a module-level value built from that
package; a project module behind a path alias never matches. No contract
invents a value: a salted hash or a password comparison stays an unknown
operation whose detail names the contract and why its result is not
representable.

## Shared project context

`ProjectContext` (`context.rs`), one per project root and shared by every
extraction of a run: one module discovery and resolver (tsconfig `paths`,
workspaces), sources read once, a compact `ModuleSummary` per file
(imports, exports, `export *`, declarations), memoized export resolution,
import closures and type facts. It holds compact facts, not ASTs (an Oxc
AST lives in a per-use arena), and only resolves *where* names are
declared; what a declaration means is decided by the same lowering code.
`discover --cache FILE` adds an on-disk extraction cache keyed by
everything an extraction reads; with several schema modules a change
invalidates only entries whose import closure reaches it.

Three components read TypeScript — the static analyzer's lowering, this
extractor and the schema extractor. They share **recognition**
(`semantic.rs`: call/member chains looked through parentheses, `as`, `!`,
`satisfies` and optional chaining; property keys; dotted member paths;
literal strings) and function identity, and keep their own **semantics**:
the static analyzer builds a sequential effect with failure paths, the
extractor a concurrent state transition.

## Source-to-model fidelity

Every extracted operation keeps a `SourceRef`: file, line, column, the
function it is written in (by position), the inlining call chain
(`callChain`), a source excerpt and the end of its span. A helper's return
slot is attributed to the call site in the caller's module. At runtime a
statement is linked to an operation only when its innermost *application*
stack frame (source-mapped TypeScript) lies inside that span; links are
never made by function or method names.

Each extraction report separates `sourceFacts` (derived: entry identity,
wrapper definition, invocation, guards, arguments) from
`scenarioAssumptions` (declared: invocation mode, preconditions), and keeps
other assumptions (key order without `orderBy`, sequence starts, unmodeled
columns, loop classifications, caught-error shapes, request assumptions,
held predicates) in `assumptions`. `sourceCoverage` counts operations, not
AST nodes: **represented**, **assumed** (resting on a declared assumption)
and **unsupported** (`Unknown`, with details).

## What it never does

* Execute, import or evaluate project or package code.
* Drop an operation it does not understand: unknown expressions inside
  local operations are sealed into `Unknown` operations with their reason
  and location (`seal_unknowns`); a scenario that reaches one is
  `UNSUPPORTED`, an unreachable one changes nothing. A local step whose
  evaluation is undetermined ends the path as unsupported, never as a
  catchable exception.
* Invent a value: a local assigned only by an unsupported operation is
  recorded (`Op::Unknown.defines`) so the path ends as unsupported.

Tests: `crates/midwrite-analyzer/tests/{concurrency,wrappers,source_resolution,schema_modules,drizzle_statements,js_values,multi_row_locks,zod}.rs`,
`crates/midwrite-cli/tests/{server_actions,loops,responses}.rs`; fixtures
[`fixtures/apps/wrappers`](../../fixtures/apps/wrappers),
[`fixtures/apps/control`](../../fixtures/apps/control),
[`fixtures/apps/workspace-app`](../../fixtures/apps/workspace-app),
[`fixtures/apps/locks`](../../fixtures/apps/locks),
[`fixtures/apps/loops`](../../fixtures/apps/loops).

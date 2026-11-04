# Source extraction (TypeScript → model IR)

`crates/midwrite-analyzer/src/concurrency.rs` (`TsActorProvider`) lowers
one entry function of a project — and the helpers it calls, inlined up to
`maxInlineDepth` (default 3) — into the model checker's IR: statements on
connections and transactions, local computation, branches and jumps,
external effects, tasks, `Unknown` operations. It is used by the scenario
loader (`model-check`) and by `discover`, which calls it with exactly the
request a generated scenario's loader will make, so discovery and
model checking see the same code.

## Inputs

| input | source | since |
| --- | --- | --- |
| entry | `file` + `function`, or a **source selector** (`module#name`, `…/arg[1]` for a wrapped callback, methods, object properties, returned functions) | 6; selectors 7 |
| tables | declared in the scenario, or read from Drizzle schema modules (`pgTable`, `pgEnum`, `pgSchema`), several modules, bound by resolved declaration | 6; source 8; multi-module 12 |
| configuration | `databaseReceivers` (default `db`), `extraction.externals`, `transactionRetries` | 6, 8 |
| features | `ExtractRequest.return_values` (only when an invariant reads them), `replicas` and declared `replica_factories` | 13 |

## What it resolves

* **Wrappers** (Phase 7): how a higher-order wrapper invokes a callback
  (returned, awaited, fire-and-forget, during wrap, stored, never), its
  guards and arguments; a conditional or unknown invocation needs a
  declared contract. Only the innermost wrapper of a chain is analyzed.
  [Phase 7 callback resolution](../phase7/CALLBACK-RESOLUTION.md).
* **Control flow** (Phase 7): `try`/`catch`/`finally`, `switch`, `??`,
  value-position `||`/`&&`, `for…of` over array literals, destructured
  parameters. [Phase 7 control flow](../phase7/CONTROL-FLOW.md).
* **Drizzle** (Phases 6–13): point and predicate selects, `count`, inner
  and left joins with column equality, projections, `insert … values`
  (one row), `onConflictDoNothing`, `onConflictDoUpdate({ target, set,
  setWhere })` (13), `update`, `delete`, `returning()` (counts, presence,
  inserted rows; `returning({ alias: t.col })` 12), `.for("update" | "no
  key update" | "share" | "key share")` (strengths 13; `skipLocked`/`noWait`
  with `update` only), transactions with an isolation level.
  [Phase 7 Drizzle extraction](../phase7/DRIZZLE-EXTRACTION.md),
  [Phase 13 advanced Drizzle](../phase13/ADVANCED-DRIZZLE.md).
* **Generated values** (Phase 12): `uuid().defaultRandom()`, `$defaultFn`
  with a recognized generator (import-checked), sequences and identity
  columns; identity is the generation event.
  [Phase 12 generated-value semantics](../phase12/GENERATED-VALUE-SEMANTICS.md).
* **Cross-module structure** (Phase 12): barrels and re-exports, const
  object repositories, class instances with constructor-stored handles,
  `this.m()`, `getDb()`-style factories, typed `db` parameters;
  transaction handles survive repository calls.
  [Phase 12 architecture](../phase12/ARCHITECTURE.md#2-cross-module-resolution-milestone-5).
* **Database-handle provenance** (Phase 13, `handles.rs`): primary,
  replicated (`withReplicas` from `drizzle-orm/pg-core`, or a declared
  factory), ambiguous, transaction, unknown — never by name alone in a
  project that builds a replicated database.
  [Phase 13 replica-aware analysis](../phase13/REPLICA-AWARE-ANALYSIS.md).
* **Function aliases and React `cache`** (Phase 13): `export const a = b`
  followed (≤ 8 hops); `cache(fn)` from `react` inlines `fn` at the first
  call only. [Phase 13 source resolution](../phase13/SOURCE-RESOLUTION.md).
* **Package contracts** (Phase 13, `packages.rs`): `bcryptjs`/`bcrypt`,
  `zod` calls are named by a verified contract but stay `Unknown`, because
  their results are not representable.
  [Phase 13 package contracts](../phase13/PACKAGE-CONTRACTS.md).
* **Return values** (Phase 13): `Op::ReturnValue` for the entry's return
  expression when requested. [Phase 13 return values](../phase13/RETURN-VALUES-AND-RESPONSES.md).
* **zod validation** (Phase 16, `zod.rs`): object schemas with
  `strictObject`/`strict`/`strip`/`passthrough`, optional/nullable/default,
  string formats, a regex subset and arrays become a `zodCheck` /
  `zodData` built-in evaluated by `midwrite_model::zodfn`; the zod major
  version is read from the project's `package.json` metadata.
  [Phase 16 zod validation](../phase16/ZOD-VALIDATION.md).
* **Loops and caught errors** (Phase 16, `concurrency/loops.rs`): bounded
  `for`/`while`/`do…while` loops unroll into `Let`/`JumpIf`/`Jump` with a
  classification (COMPLETED, BOUNDED_UNROLLING, TRUNCATED, UNKNOWN,
  UNSUPPORTED); a `catch` may read a database error's SQLSTATE only for
  verified origins (`Op::Try.inspect`).
  [Phase 16 loop semantics](../phase16/LOOP-SEMANTICS.md).
* **Requests** (Phase 16): `await request.json()` in a route handler is the
  entry's argument; `Response.json(v)` is its return value.
  [Phase 16 framework requests](../phase16/FRAMEWORK-REQUESTS.md).
* **Held predicates and multi-row reads** (Phase 16): predicates in `const`
  locals, aliases, conditionals and `return <pred>` helpers; multi-row
  `SELECT` (with `FOR UPDATE/SHARE`, `NOWAIT`, `SKIP LOCKED`, `LIMIT n`,
  `orderBy(primaryKey)`) as `many: ManyRows`.
  [Phase 16 predicates](../phase16/DRIZZLE-PREDICATES.md),
  [multi-row locks](../phase16/MULTI-ROW-LOCKS.md).

## Shared project context

`ProjectContext` (Phase 12, `context.rs`): one module discovery, one
resolver, sources read once, compact module summaries, memoized export
resolution, closures and type facts, reused by every extraction of a run.
`discover --cache FILE` adds an on-disk extraction cache keyed by
everything an extraction reads; a schema change invalidates only entries
whose import closure reaches it.
[Phase 12 architecture § 1](../phase12/ARCHITECTURE.md#1-shared-project-context-milestone-6).

## What it never does

* Execute, import or evaluate project or package code.
* Drop an operation it does not understand: unknown expressions are sealed
  into `Unknown` operations with their reason and location (a Phase 7
  fix); a scenario that reaches one is `UNSUPPORTED`.
* Invent a value: a local assigned only by an unsupported operation is
  recorded (`Op::Unknown.defines`) so the path ends as unsupported.

Each extraction reports what was **represented**, **assumed** (with the
assumption) and **unsupported**
([Phase 7 source-model fidelity](../phase7/SOURCE-MODEL-FIDELITY.md)). The
gaps that block real code today are listed in the
[limitations reference](../reference/limitations.md#unsupported-source-constructs).

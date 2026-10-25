# The concurrency IR

`crates/commitrift-model/src/ir.rs`. It is derived from source (or declared in
a scenario) and kept separate from the Effect IR, which it does not replace.

## Actors, tasks, code

An `ActorSpec` has a name, the function it runs (`FunctionRef`: name and
file), positional arguments bound to parameter names, *code vectors*
(index 0 is the main task; others are spawned tasks), an optional operation
retry policy and a description. A scenario places each actor in a *process*
(default: its own).

## Operations

| IR operation | Specification term | Scheduling |
| --- | --- | --- |
| `Db { conn, Select { table, pred, lock, into } }` | DatabaseRead (`LIMIT 1`, key order; `FOR UPDATE` / `SKIP LOCKED` / `NOWAIT`) | visible; may wait |
| `Db { conn, SelectCount { … } }` | DatabaseRead (aggregate) | visible |
| `Db { conn, Update / Insert / Delete }` | DatabaseWrite (`affected` receives the row count; `ON CONFLICT DO NOTHING`) | visible; may wait |
| `Begin { isolation, retry }`, `Commit`, `Rollback` | Begin/Commit/RollbackTransaction | visible |
| `External { channel, payload, key }` | ExternalEffect (with idempotency key) | visible |
| `AppRead`, `AppWrite` | Read/WriteApplicationState (shared in-process variables) | visible |
| `Let`, `JumpIf`, `Jump` | local computation, Branch | folded |
| `Spawn { children, detached }`, `Join` | Spawn / Await of concurrent tasks (`Promise.all`, fire-and-forget) | folded (`Join` blocks until children finish) |
| `Throw`, `Return` | failure / completion | folded |
| `Unknown { detail }` | UnknownOperation | visible; reaching it makes the result `UNSUPPORTED` |
| (transition `Crash`) | ProcessCrash | environment |
| (transition `Retry`, `Begin.retry`) | Retry (operation re-execution, transaction re-execution) | environment / local |

`conn` is `Tx` (the actor's open transaction: a `tx` handle) or `Pool` (an
autocommit statement on a pooled connection: the root `db` handle). A
statement distinguishes *start* (snapshot, candidate rows) from *completion*
when it waits (`Resume`), and its transaction visibility follows the
isolation rules.

Expressions (`value.rs`) are literals, locals, columns of the row under
evaluation (`@col`), field reads, `!`, `is null`, `+ - *`, comparisons,
`and`/`or`, and `Unknown`. They are evaluated in **SQL mode** (three-valued
logic: comparison with `null` is unknown and does not match) inside
statements and **JavaScript mode** (strict equality, truthiness,
short-circuit; reading a property of `null` is a `TypeError`) elsewhere.
Arithmetic overflow is an error, never a wrap-around.

## Provenance kept on every operation

`SourceRef`: origin (`extracted` from TypeScript or `declared` in the
scenario), file, line, column, the function the operation is written in,
the call chain through which it was inlined (outermost first), a source
excerpt, the Effect IR effect id at the same location, and the fingerprints
of existing findings involving that effect (linked by
`commitrift model-check`; a sequential finding is never promoted to a
concurrency finding). Arguments are concrete values from the scenario;
transaction ownership is explicit (`conn`); branch conditions are the
lowered conditions themselves.

## Ordering

| Source pattern | IR | Ordering |
| --- | --- | --- |
| `await a(); await b();` | sequential operations | `b` starts after `a` settled successfully |
| `await Promise.all([a(), b()])` | `Spawn` + `Join` | `a` and `b` are concurrent tasks; the join waits for both; a failure surfaces at the join, the other task continues |
| `void a(); b()` / unawaited call | `Spawn { detached }` | the task may run after the caller returns; its failure crashes the process |
| callbacks of `db.transaction(cb)` | `Begin` … `Commit` (a `throw` rolls back) | sequential within the callback |
| anything else (`.then`, timers, loops, `try`) | `Unknown` | not ordered: unsupported |

No ordering is invented: concurrent tasks of one actor are scheduled like
actors, except that statements on the same transaction connection are
serialized (as the driver does).

## The TypeScript subset (`TsActorProvider`)

`crates/commitrift-analyzer/src/concurrency.rs` lowers one function (and the
helpers it calls, inlined up to `maxInlineDepth`, default 3) of a project,
using the analyzer's module resolution (relative imports, tsconfig `paths`,
workspace packages) and the configured `databaseReceivers` (default `db`).
Table identifiers map to scenario tables (`extraction.tables`, default the
same name); property names must equal declared columns.

Supported:

* `db.query.T.findFirst({ where })`; `const [x] = await
  db.select().from(T).where(p).limit(1)` with `.for("update")`,
  `.for("update", { skipLocked: true })`, `{ noWait: true }`;
  `select({ n: count() })` → `SelectCount`;
* `db.update(T).set({...}).where(p)`, `db.insert(T).values({...})` with
  `.onConflictDoNothing()`, `db.delete(T).where(p)`, `.returning()` used only
  for its length/truthiness (→ affected count); `sql` templates with column
  references, literals, locals and `+ - *`;
* predicates `eq ne gt gte lt lte and or not isNull isNotNull`;
* `db.transaction(async (tx) => …, { isolationLevel })`; `tx.` statements run
  on the transaction; a `throw` inside rolls back;
* `if`/`else`, `return`, `throw`, `const`/`let`/assignment, destructuring of
  a returned row, ternaries and `a?.b` on locals (hoisted exactly);
* calls of functions in the same file or imported from analyzed files
  (inlined with fresh locals; `return` jumps to the end of the inlined
  region); calls matching `extraction.externals` → `External`;
* `await Promise.all([...])` → `Spawn`/`Join`; unawaited external or helper
  calls → detached tasks.

Lowered to `Unknown` (with the detail and location): loops, `try`, `switch`,
`break`, raw `db.execute`, unbound tables or non-column properties,
unsupported predicate operators or Drizzle methods (`findMany`, multi-row
`values`, …), unawaited builders, `??`, recursion, calls beyond the inline
depth, callees that cannot be resolved, and any expression it does not
understand (`Expr::Unknown`). Findings rely on these never being skipped.

Assumptions recorded during extraction (shown in the report), for example:
`findFirst` without `orderBy` returns the first row in primary-key order
(the oracle sends `ORDER BY` the key; PostgreSQL itself does not promise an
order).

## Declared programs (`dsl.rs`)

Scenarios and tests can declare an actor's program instead of extracting
it; such operations are labeled `declared` everywhere (report, oracle SQL
origin). Syntax: see the module documentation; one statement per line,
`@column` for columns, `begin [isolation …] [retry n] … commit`, `let x =
select T where … [for update [skip locked | nowait]]`, `update`, `insert …
[on conflict do nothing] [into n]`, `delete`, `if … { } else { }`, `fail`,
`return`, `external ch [key e] [set k = e]`, `spawn [detached] { } { }`,
`join`, `read_app`/`write_app`, `unknown "…"`.

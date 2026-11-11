# Midwrite

**Failure-path analysis for TypeScript.** Midwrite is a local, open-source
static analyzer that looks for places where a crash, restart, timeout or failed
call *between two independently committed side effects* can leave your system
half-done: the database says an order is paid, but the `order.completed` event
was never published.

A real example: [Kan's checkout webhook](https://github.com/kanbn/kan/issues/697)
creates a workspace before its subscription record. If the second write fails,
Stripe's retry finds the existing workspace and skips the missing subscription:

```ts
// Reduced from Kan's checkout.session.completed handler.
if (!existingWorkspace) {
  await workspaceRepo.create(db, workspaceInput);  // commits
  // A failure here leaves a workspace without its subscription.
  await subscriptionRepo.create(db, subscriptionInput);
}
// On retry, existingWorkspace is present: neither create runs.
```

Midwrite reported this through cross-module call summaries (`MW1004`).
The failure path was checked against the source and
[reported upstream](https://github.com/kanbn/kan/issues/697); it has not been
runtime-reproduced or confirmed by the maintainer. The
[five-repository review](docs/development/upstream-review.md) records four
upstream reports, every diagnostic's review, and the HTTP false-positive fix.

> **Documentation: [`docs/README.md`](docs/README.md)** — installation,
> quickstart, CLI reference, supported technologies, the verification
> workflow, evidence levels, the [current limitations](docs/reference/limitations.md),
> architecture, the [evidence index](docs/research/evidence-index.md) and
> the historical phase reports.

```ts
export async function completeOrder(orderId: string) {
  await db.update(orders).set({ status: "paid" }).where(eq(orders.id, orderId));
  // <- a crash here leaves a paid order without an event
  await queue.publish("order.completed", { orderId });
}
```

```text
warning[MW1001]: database commit followed by an independent external effect
  --> src/orders.ts:4:9 (in `completeOrder`, confidence: high)
    |
  4 |   await queue.publish("order.completed", { orderId });
    |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  = `db.update(orders)` commits before `queue.publish("order.completed", { orderId })` (message-publish); a failure in between leaves the database changed without the external effect
  failure sequence:
    1. `db.update(orders)` (2:9) commits immediately: it is not inside a recognized transaction.
    2. The process crashes or is restarted, or the call fails, after the commit and before `queue.publish("order.completed", { orderId })` completes.
    3. `queue.publish("order.completed", { orderId })` is never performed.
    4. Result: the database change is durable but the external effect never happens, and nothing records that it is still owed.
  help: Record the intent in the same database transaction as the state change (transactional outbox) and perform the external effect from a relay that retries until it succeeds, or make a reconciliation job derive the missing effect from database state.
```

> **Status: experimental (Phase 13 working tree; Phase 13 changes are
> uncommitted).** Beyond the static analysis below, Midwrite model-checks
> bounded concurrent and crash/retry scenarios against a model of PostgreSQL
> (Phase 6), runs counterexamples against the original application code on a
> disposable database (Phases 7–10), discovers candidates and synthesizes
> scenarios without a hand-written scenario (Phase 11), and models
> generated keys, multi-module schemas (Phase 12), foreign-key and row-lock
> modes, return values, the response step, read replicas and upserts
> (Phase 13). All original-code evidence comes from first-party fixtures;
> no independent human review has been performed. Current state:
> [`docs/README.md`](docs/README.md); current limitations:
> [`docs/reference/limitations.md`](docs/reference/limitations.md). The
> history below describes Phases 2–5.
>
> The static analysis (`midwrite check`) is deliberately narrow:
> Drizzle ORM on PostgreSQL and configured external effects, one function at a
> time plus bounded summaries of the functions it calls, in the same file or
> imported from other analyzed modules. Expect false positives and false
> negatives; both are documented below and in
> [`docs/LIMITATIONS.md`](docs/LIMITATIONS.md). The Phase 2 evaluation on
> real projects, with manually reviewed precision, is in
> [`docs/phase2/`](docs/phase2/FINAL-REPORT.md). Phase 3 added failure traces
> and opt-in runtime replay that turns a selected failure window into observed
> evidence: [`docs/phase3/`](docs/phase3/FINAL-REPORT.md). Phase 4 made that
> workflow usable on real code: replay with **Node.js and the real Drizzle
> ORM**, isolated replay on **Linux** (bubblewrap), a **harness generator**
> (`midwrite init-replay`), regression comparison of replay results and
> function-level recovery contracts: [`docs/phase4/`](docs/phase4/FINAL-REPORT.md).
> Phase 5 extended the static engine into a bounded interprocedural abstract
> interpreter: abstract values and a per-function heap, provenance across
> calls and returns, method dispatch on known receivers, callbacks and
> wrappers, concurrent calls, inertness summaries over the call graph, and
> value-flow relationships between the operations of a finding:
> [`docs/phase5/`](docs/phase5/FINAL-REPORT.md).
> No bug in an external application has been runtime-confirmed.

> **Renamed.** Midwrite was called *CommitRift* until 2026-11-04 and
> *CrashProof* before Phase 3. Diagnostic codes moved from the `CP` prefix to
> `MW` (`CP1001` is now `MW1001`; `midwrite explain`, recovery contracts and
> every other code input still accept the `CP` form). Finding fingerprints,
> baseline files and the report schemas (including their `kind` values such
> as `commitrift.discovery`) are unchanged. Legacy `commitrift` and
> `crashproof` executables (same behavior, plus a deprecation note on
> stderr), the `commitrift.json` and `crashproof.json` configuration files
> (read only when `midwrite.json` does not exist; files are never merged),
> `commitrift-contracts.json` (read only when `midwrite-contracts.json` does
> not exist), the `commitrift.contracts` and `commitrift.source-harness`
> kinds, and the `COMMITRIFT_THREADS` and `CRASHPROOF_THREADS` variables
> still work. Other `COMMITRIFT_*` environment variables are now
> `MIDWRITE_*`. The historical records under `docs/phaseN/` and `reports/`
> and the frozen evaluation sets keep the old names.

### Across modules

The operations of imported functions are analyzed in their callers
(reproducible with `fixtures/projects/named_import`):

```ts
// services/order.ts
export async function markPaid(id: string) {
  await db.update(orders).set({ status: "paid" });
}

// actions/checkout.ts
import { markPaid } from "../services/order";

export async function checkout(id: string) {
  await markPaid(id);
  await queue.publish("order.paid", { id });
}
```

```text
$ cd fixtures/projects && midwrite check named_import --config ../midwrite.json
warning[MW1001]: database commit followed by an independent external effect
  --> named_import/actions/checkout.ts:9:9 (in `checkout`, confidence: high)
  = `db.update(orders)` (via `markPaid(id)`) commits before `queue.publish("order.paid", { id })` (message-publish); a failure in between leaves the database changed without the external effect
  failure sequence:
    1. `db.update(orders)` (named_import/services/order.ts:7:9, in `markPaid`, reached through `markPaid(id)` at 7:9) commits immediately: it is not inside a recognized transaction.
    ...
```

A helper that receives the transaction handle joins the caller's
transaction (`await saveOrder(tx, id)`), and a helper that receives the root
handle inside a transaction is reported as bypassing it. When the argument is
not a recognized handle, the helper's writes stay unknown instead of being
guessed.

## What Midwrite does

1. Parses `.ts`/`.tsx`/`.mts`/`.cts` files with [Oxc](https://oxc.rs) and
   resolves identifiers with Oxc's semantic analysis.
2. Recognizes **database operations** (Drizzle `insert`/`update`/`delete`,
   reads, `db.transaction(...)`) and **configured external effects**
   (`queue.publish`, `mailer.send`, `stripe.charges.create`, ...).
3. Lowers each function to a small, inspectable **Effect IR** that keeps
   execution order, branches, loops, `try`, early exits and transaction scopes
   (`midwrite ir <file>` prints it). Awaited calls of functions declared in
   the same file, or imported from another analyzed file (relative imports,
   `tsconfig` `paths`, workspace packages), are replaced by the callee's
   **summary**, so the callee's commits and external effects are ordered with
   the caller's. Database handles passed as arguments are bound to the
   callee's parameters.
4. Enumerates paths through each function, discarding paths whose simple
   branch conditions contradict each other (`if (a) ...; if (!a) ...`), and
   finds the moments when state becomes **durable** (a commit) or **visible**
   (an external effect).
5. Reports every place where a failure between two such moments leaves partial
   state, with the concrete failure sequence, the evidence for each recognized
   operation, a confidence level, a mitigation and explicit limitations.
6. On request, describes the path behind each finding as a **failure trace**
   (`midwrite trace`): the ordered operations, transaction boundaries, the
   failure-injection boundary and what the transaction model predicts survives
   a crash there. A trace is a *static candidate*, never runtime evidence.
7. Only when explicitly asked to (`midwrite replay --allow-execution`), runs
   a project-owned, instrumented **replay harness** against a disposable
   PostgreSQL database, kills or fails it at a named checkpoint, observes the
   surviving state and checks it against invariants the harness declares
   (`RUNTIME_REPRODUCED`, `RUNTIME_NOT_REPRODUCED` or `INCONCLUSIVE`).

## From a finding to runtime evidence (Node.js + Drizzle)

The fixture [`fixtures/replay/drizzle-node`](fixtures/replay/drizzle-node)
is an order service on Node.js, `drizzle-orm` 0.45.4 and `pg` 8.23.1. The
whole workflow, with the steps that are automated and the one that is not,
is scripted in [`scripts/demo-phase4.sh`](scripts/demo-phase4.sh):

```bash
cd fixtures/replay/drizzle-node && npm ci && npm run build
midwrite check src                                      # MW1001 in completeOrder (and MW1003, MW1002)
midwrite trace src --format json > trace.json           # the failure trace: a static candidate
midwrite init-replay --trace trace.json --finding 4f53a6215ac16913 --output my-harness
# manual: fill in the MIDWRITE-TODO markers (observations, invariant, schema, scenario call);
# replay refuses to run until every marker is gone. A completed example: generated-harness/
midwrite replay generated-harness                       # the plan; nothing runs
midwrite replay generated-harness --allow-execution --case kill-after-db-commit --repeat 5
midwrite replay generated-harness --allow-execution --format json --output v.json
midwrite report v.json -o v.html
```

```text
case kill-after-db-commit: processKill at `after-db-commit`  [RUNTIME_REPRODUCED, as expected]
  static link: Matched — finding 4f53a6215ac16913 (MW1001 in `completeOrder`, src/service.ts:15); trace tr-2a5439a0b830cdc6 (OnTraceBoundary)
  run 1: RUNTIME_REPRODUCED — declared invariant violated: `application-contract` at after: VIOLATED: queuePublishEvents >= 1 (observed 0)
    injected=true outcome=killed exit=signaled (signal 9)
    before injecting: ordersState = "paid" -> ok (observed "paid")
    after:         ordersState="paid" queuePublishEvents=0
```

Checkpoints come from an adapter around the `pg` pool
([`runtime/node/pg.js`](runtime/node/pg.js)), so the application code is
not instrumented, and a commit is only claimed when PostgreSQL acknowledged
it. The same fixture shows a Drizzle transaction rolling back, an event that
escapes a transaction whose COMMIT PostgreSQL rejects, and a transactional
outbox that never loses the intent (but delivers at least once). Details:
[`docs/phase4/NODE-DRIZZLE.md`](docs/phase4/NODE-DRIZZLE.md),
[`docs/phase4/HARNESS-GENERATION.md`](docs/phase4/HARNESS-GENERATION.md).

A runtime result describes that harness under the recorded conditions; it
never changes a finding's static confidence and does not show that other
code has the same bug.

## Concurrency model checking (opt-in)

`midwrite model-check scenario.json` explores bounded concurrent schedules
of the actors a scenario declares — functions extracted from your
TypeScript (or programs declared in the scenario), their arguments, the
schema with its constraints, the PostgreSQL isolation level, crashes and
retries — and reports a counterexample to a declared invariant. With
`--validate-postgres` the counterexample is replayed in lockstep on a
private PostgreSQL. `midwrite check` never runs it.

```bash
midwrite model-check fixtures/concurrency/scenarios/inventory-oversell.json
midwrite model-check fixtures/concurrency/scenarios/inventory-oversell.json --validate-postgres --format html -o oversell.html
midwrite explain MW7001
```

```text
# excerpt of the text output
    1  B        SELECT products WHERE (@id = "product-1") → {id="product-1", stock=1}    [src/purchase.ts:8:25]
    2  A        SELECT products WHERE (@id = "product-1") → {id="product-1", stock=1}    [src/purchase.ts:8:25]
    3  A        UPDATE products SET stock = 0 WHERE (@id = "product-1") → 1 row    [src/purchase.ts:16:9]
    4  A        INSERT orders {id="order-A", product="product-1"} → 1 row    [src/purchase.ts:21:9]
    5  B        UPDATE products SET stock = 0 WHERE (@id = "product-1") → 1 row    [src/purchase.ts:16:9]
    6  B        INSERT orders {id="order-B", product="product-1"} → 1 row    [src/purchase.ts:21:9]

Invariant `successful-purchases-within-initial-stock` (final state):
  purchase:ok <= initial(products[id="product-1"].stock)
    purchase:ok = 2
    initial(products[id="product-1"].stock) = 1

Evidence: PostgresConfirmedSchedule — the schedule was replayed on PostgreSQL: all 6 database outcomes matched the model's predictions and the final committed rows are identical; …
```

Results are `COUNTEREXAMPLE_FOUND`, `NO_COUNTEREXAMPLE_WITHIN_BOUNDS` (only
within the declared bounds — not a proof of race freedom), `INCONCLUSIVE`
or `UNSUPPORTED`, with diagnostics `MW7001` (invariant violated), `MW7002`
(inconclusive), `MW7003` (unsupported behavior), `MW7101` (PostgreSQL did not
reproduce), `MW7102` (validation unavailable) and `MW7103` (the original
code did not reproduce the counterexample, Phase 7). Semantics, supported
subset, validation against PostgreSQL and limitations:
[`docs/phase6/`](docs/phase6/FINAL-REPORT.md) and
[`docs/phase7/`](docs/phase7/FINAL-REPORT.md).

## Original-code verification (Phase 7, opt-in, executes code)

A scenario can select an actor's entry function by a **source selector** —
including the anonymous callback of a wrapped Server Action
(`src/app/actions.ts#inviteTeamMember/arg[1]`). The extractor reads the
wrapper's source to learn how it invokes the callback and requires the
scenario to declare any precondition it cannot prove. Joins, projections,
`RETURNING`, generated keys, defaults, `try`/`catch`/`finally`, `switch` and
loops over literal arrays are now modeled.

With an authorized source harness, the counterexample is then run against
the **original application code**: one worker per actor, every SQL
statement Drizzle sends held and released in the counterexample's order,
PostgreSQL's answers and lock waits observed, and the invariant evaluated
on the rows the run left.

```bash
midwrite model-check fixtures/phase7/scenarios/team-app-invite-double-submit.json \
  --verify-source --allow-execution --source-repeat 5
```

```text
Evidence: OriginalCodeReproduced — the actors' original application code ran under this schedule against PostgreSQL in 5 of 5 run(s) ...
  = #7  Submit1  model: INSERT invitations {...} [src/app/actions.ts:80:11]
           sql:   insert into "invitations" ("id", "team_id", ...) values (default, $1, ...) [src/app/actions.ts:80:34]
```

`--validate-postgres` still replays only the model's SQL; running code
needs `--allow-execution`. Details:
[`docs/phase7/FINAL-REPORT.md`](docs/phase7/FINAL-REPORT.md).

## Scenario discovery (Phase 11, static)

`midwrite discover <project>` finds concurrency and failure candidates in
the original TypeScript without a hand-written scenario: check-then-act,
status transitions, read-modify-write, write skew, duplicate processing,
external-effect gaps, ambiguous commits and lock order. Each candidate
names the entries, the conflicting operations, the shared resource, and a
property whose origin is always stated (`SCHEMA_ENFORCED`,
`USER_DECLARED`, `FRAMEWORK_CONTRACT`, or `HEURISTIC_HYPOTHESIS` — a
hypothesis is never reported as a confirmed bug). It never executes
application code.

```bash
midwrite discover . --model-check --emit-scenarios ./midwrite-scenarios
midwrite model-check ./midwrite-scenarios/<id>-actual.json --verify-source --allow-execution
```

`--model-check` explores the synthesized scenarios within budgets;
`--emit-scenarios` writes them in the `model-check` format. What source
cannot say (external effects, delivery, retries, business invariants,
harness inputs) goes in an optional `midwrite-contracts.json`. Details:
[`docs/phase11/FINAL-REPORT.md`](docs/phase11/FINAL-REPORT.md).

Phase 12 extends what discovery and extraction understand: generated keys
(`uuid().defaultRandom()`, `$defaultFn` with a recognized generator such as
`crypto.randomUUID()`) are modeled as values whose identity is the
generation event, carried through `returning({ id: t.id })` into foreign
keys; schemas split over several modules (scenarios may list several
`extraction.schema` modules and name tables `module#binding`); barrel
re-exports, repository objects and classes, and `getDb()`-style database
factories are resolved; one shared project context serves every extraction.
Details: [`docs/phase12/FINAL-REPORT.md`](docs/phase12/FINAL-REPORT.md).

Phase 13 (uncommitted) adds, each opt-in per scenario so that earlier
scenarios keep their state spaces: PostgreSQL row-lock modes and
foreign-key checks (`features: ["foreign-keys"]`; `NO ACTION`/`RESTRICT`,
immediate constraints), invariants over what callers are told
(`"returned"`), a response step that a crash can precede
(`"response": true`), read-replica handles (`withReplicas` from
`drizzle-orm/pg-core`, or a declared project factory) with replica staleness as a
model-only result (`features: ["replicas"]`), `INSERT … ON CONFLICT DO
UPDATE` and the `.for("share" | "key share" | "no key update")` strengths.
Details: [`docs/phase13/`](docs/phase13/ARCHITECTURE.md) and
[`docs/reference/limitations.md`](docs/reference/limitations.md).

## What Midwrite does not do

* It does **not** prove business consistency. It knows nothing about your
  invariants; every static finding is a *potential* hazard
  (`"kind": "potentialHazard"`), never a confirmed bug, and never marked as
  reproduced (`"reproduced": false`). Runtime evidence exists only in a
  separate verification report produced by an authorized replay harness,
  against invariants the harness author declares.
* It does **not** replace database constraints, integration tests, or
  fault-injection / chaos testing. Use it to find where those are needed.
* It is not a type checker. `check`, `trace`, `ir` and `report` never run your
  code. Recognition is syntactic plus provenance (imports, `const` aliases,
  `drizzle(...)` constructors, transaction callback parameters) and the
  configuration. `replay` runs only a harness directory you name, only with
  `--allow-execution`, and never discovers or runs other code.
* It follows calls it can resolve (functions of the same file or imported
  from analyzed files, methods of receivers whose class is known, callbacks
  passed to analyzed functions, immediately invoked functions) through
  depth-bounded summaries, and resolves calls of functions it proves inert.
  Packages in `node_modules`, methods on values of unknown class (parameters,
  SDK clients) and dynamic imports are not followed. It is not a model of
  JavaScript: no types, no prototypes beyond class declarations, no getters.
* It does not solve conditions. Only simple tests of values that never
  change (`flag`, `!flag`, `x === "literal"`, `switch` cases, properties of
  unescaped objects such as a helper's `{ ok: true }` result) are correlated;
  everything else is assumed satisfiable.
* It does not guess. A call it does not understand is an *unknown effect*; it
  never becomes a claimed hazard, only an informational `MW9001` note when it
  sits next to recognized effects.

## Supported patterns

| Pattern | Support |
| --- | --- |
| Drizzle writes: `db.insert/update/delete(...)...` | Executed when awaited, returned from an `async` function, run by `Promise.all`, or ended with `.execute()`/`.then()`. A builder that is only constructed is not a write. |
| Drizzle reads: `db.select(...)`, `db.query.<t>.findFirst/findMany`, `db.$count` | Recognized; never a commit. |
| `db.transaction(async (tx) => { ... })` | Writes on `tx` commit when the callback completes. Nested `tx.transaction(...)` (savepoints) commit with the outer transaction. Writes on `db` inside the callback are **not** part of the transaction. `tx.rollback()` is an exit. |
| Builders stored in a `const` and awaited later | `const q = db.update(t).set(v); await q;` |
| Receivers | Configured identifiers (`db`, `queue`, ...), renamed imports (`import { db as database }`), `const` aliases, `importedFrom` module checks, handles created by `drizzle(...)` from `drizzle-orm`, and handles returned by a configured call without arguments (`"getDb()"`: `getDb().insert(...)`, `const db = getDb()`, `save(getDb())`). |
| Member receivers | Configured member paths such as `this.db`, `ctx.db` or `this.queue`, written out at the call (`this.db.update(...)`). Reported as a weaker, `member-path` match. Unconfigured lookalikes (`this.repo.db`) are reported as `MW9001`. |
| External effects | Configured `receiver.method(...)` (member paths like `stripe.charges` or `this.queue` allowed) or plain `function(...)`, optionally followed by `.then/.catch/.finally`. |
| Control flow | Sequences, `if`/`else`, `?:`, `&&`/`||`/`??`, `switch` with fall-through, `return`, `throw`, `try`/`catch`/`finally`, loops (effects zero or one iteration; values iterated to a fixpoint), `Promise.all/allSettled/race/any` over array literals (resolved calls in them are summarized as concurrent), `Promise.all(xs.map(cb))` and `xs.forEach(cb)` (callback loops, concurrent / fire-and-forget). |
| Functions | Declarations, expressions, arrows, class and object methods, `export default`, Next.js route handlers and server actions. Nested functions are analyzed as separate units. |
| Same-file helpers | `await helper(...)` and `return helper(...)` (in an `async` function) where `helper` is a function declaration or a `const` arrow/function expression of the same file: the helper's summarized operations are analyzed in the caller, up to 3 calls deep (`--max-call-depth`), never recursively. Diagnostics point at the call; operations the helper performs only on some paths get at most `medium` confidence. |
| Object services | `const orders = { async markPaid() {...}, close: closeOrder }` declared in the same file or imported: `await orders.markPaid(id)` is summarized like a function call. Objects whose properties are assigned or deleted in their file are not followed. |
| Classes (Phase 5) | `new C().m()`, `const x = new C(); x.m()` (exact dispatch), `this.m()` when no analyzed subclass overrides `m`, static methods, `super(...)`/`super.m()`, inheritance across modules. Dynamic reassignment or several possible targets → not resolved. An unanalyzed (package) superclass → resolved under a recorded assumption, confidence ≤ medium. See [`docs/phase5/CLASS-RESOLUTION.md`](docs/phase5/CLASS-RESOLUTION.md). |
| Callbacks and wrappers (Phase 5) | `await execute(async () => ...)` when `execute` awaits its parameter; `withTimeout(op(), ms)`-style wrappers proven inert, with the awaited argument kept as the operation; IIFEs; `setTimeout` callbacks are deferred, never serialized. See [`docs/phase5/ASYNC-SEMANTICS.md`](docs/phase5/ASYNC-SEMANTICS.md). |
| Inert helpers (Phase 5) | A call of a function that performs no write, transaction, external effect or unknown call on any path (transitively, recursion included, builtins modeled) is resolved instead of reported as unknown. |
| Value relationships (Phase 5) | Diagnostics and traces carry `valueRelation` (`provenShared`, `possiblyRelated`, `noRelationEstablished`, `unknown`) from argument provenance across calls. Descriptive only. |
| Imported functions | The same, for functions imported from other analyzed files: named, aliased, default and namespace imports; `export { f } from`, `export *` barrels; relative specifiers, `tsconfig.json` `paths`/`baseUrl` (with relative `extends`), and workspace packages (`package.json` `name` + `exports`). Ambiguous modules or exports are not chosen; type-only imports are never calls. See [`docs/phase2/CROSS-MODULE-ANALYSIS.md`](docs/phase2/CROSS-MODULE-ANALYSIS.md). |
| Handle arguments | A direct identifier argument that is a transaction handle, a configured receiver (including a configured call such as `getDb()`) or a `drizzle(...)` instance is bound to the callee's parameter: `await save(tx, id)` puts `save`'s writes in the caller's transaction. Any other argument (`getDb()`, a computed value) leaves the callee's writes unknown. |
| Branch correlation | Paths whose decisions contradict each other on a variable that is never reassigned (`flag` / `!flag`, `x === "a"` / `x === "b"`, `switch` cases, `const` literals) are discarded. A literal or object returned by an inline transaction callback is known for the `const` holding the transaction's result (`const r = await db.transaction(...)`; `if (r === "forbidden")`). Phase 5: the same for a summarized helper's returned literals and object-literal properties (`const r = await save(); if (r.ok)`), and for property tests of unescaped objects. |

## Supported platforms

| Platform | `check`, `trace`, `report`, `init-replay` | Isolated `replay` | Verified how |
| --- | --- | --- | --- |
| macOS arm64 | yes | yes (`sandbox-exec`) | full test suite with required replay, this repository's development machine |
| Linux aarch64 (Debian 12) | yes | yes (bubblewrap, unprivileged user namespaces) | full test suite with required replay in an unprivileged container |
| Linux x86_64, macOS x86_64 | expected (portable Rust) | expected on Linux with bubblewrap | **not yet run**; CI and release workflows exist but have not run |
| Windows | not supported | no | — |

Replay additionally needs PostgreSQL server binaries and the harness runtime
(Node.js ≥ 22 or Bun ≥ 1.2). Where isolation is unavailable, replay is
blocked (exit 3) instead of running unisolated.

## Installation

Midwrite is a Rust workspace (Rust 1.97 or newer, stable). There are no
published binaries or packages yet; build from source:

```bash
cargo install --path crates/midwrite-cli
```

Or build without installing; the binary is `target/release/midwrite`:

```bash
cargo build --release
```

`scripts/package-release.sh` builds a local release archive (binary,
licenses, the replay client in `runtime/node`, JSON Schemas) with a SHA-256
checksum.

## Usage

```bash
midwrite check .
```

```bash
midwrite check ./src --format json
```

```bash
midwrite explain MW1001
```

| Command | Purpose |
| --- | --- |
| `midwrite check [PATHS]...` | Analyze files/directories (default `.`). Directories are searched recursively for `.ts`, `.tsx`, `.mts`, `.cts`, skipping `node_modules`, `.git`, `dist`, `build`, `out`, `.next`, `.nuxt`, `.output`, `.turbo`, `.vercel`, `.svelte-kit`, `coverage`, and `.d.ts` files. Explicitly named files are always analyzed. |
| `--format text\|json\|sarif` | Human-readable (default), JSON ([schema](docs/DIAGNOSTICS.md#json-output)), or SARIF 2.1.0. |
| `--config <FILE>` | Configuration file. Default: `./midwrite.json` if present, else the legacy `./crashproof.json`, else built-in defaults. |
| `--fail-on error\|warning\|info\|never` | Exit with status 1 when a diagnostic at or above this severity exists (default `warning`). |
| `--max-paths <N>` | Paths explored per function (default 512); functions with more paths get `MW9001`. |
| `--max-call-depth <N>` | Levels of calls inlined from summaries (default 3; `0` disables summaries). |
| `--policy recommended\|strict\|audit` | Which findings are warnings (default `recommended`, see [Policies](#policies)). |
| `--baseline <FILE>` | Findings recorded in the baseline are still reported but do not count toward `--fail-on`. |
| `--write-baseline <FILE>` | Record every current finding (the run then exits 0 unless files could not be analyzed). |
| `-v, --verbose` | Text output: also print evidence, path certainty, evidence strength and general limitations. |
| `midwrite explain [CODE]` | Documentation for a code, or the list of codes. |
| `midwrite ir [PATHS]... [--format text\|json]` | Print the Effect IR. |
| `midwrite trace [PATHS]... [--format text\|json] [--finding ID]` | Print the failure trace behind each finding ([format](docs/phase3/TRACE-FORMAT.md)). Static only. |
| `midwrite replay <HARNESS> [--case ID\|CODE] [--finding ID] [--repeat N] [--allow-execution]` | Run an authorized replay harness ([protocol](docs/phase3/REPLAY-PROTOCOL.md), [Node.js](docs/phase4/NODE-DRIZZLE.md), [security](docs/phase4/SECURITY-REVIEW.md)). Without `--allow-execution` only the plan is printed. Needs PostgreSQL server binaries, Node.js ≥ 22 or Bun ≥ 1.2, and OS isolation (macOS `sandbox-exec`, Linux bubblewrap). |
| `midwrite replay … --baseline OLD.json [--regression-output R.json]` | Compare every case with an earlier verification report: still / no longer / newly reproduced, inconclusive, not executed. Exit 1 on a change, 3 when a case is inconclusive or was not executed. |
| `midwrite init-replay --trace T.json --finding ID --output DIR [--runtime node\|bun]` | Generate an editable replay harness skeleton for one finding ([guide](docs/phase4/HARNESS-GENERATION.md)). Writes only into `DIR` (inside the project); executes and installs nothing. |
| `midwrite report <JSON> [-o FILE]` | Render a trace or verification report as a self-contained, script-free HTML page. |
| `midwrite model-check <SCENARIO>` | Bounded model checking of a scenario; `--validate-postgres`, `--verify-source --allow-execution` ([guide](docs/user-guide/model-check.md)). |
| `midwrite discover <ROOT>` | Candidates, properties and synthesized scenarios from source; `--model-check`, `--emit-scenarios` ([guide](docs/user-guide/discover.md)). Never executes application code. |
| `midwrite schema <ROOT> --file <SCHEMA>` | Schema facts read statically from a Drizzle schema module; `--migrations … --allow-execution` compares them with a disposable database. |

Every flag of the current binary: [`docs/reference/cli.md`](docs/reference/cli.md).

Exit status: `0` clean, `1` diagnostics at or above `--fail-on`, `2` usage,
configuration or input error, `3` some files could not be read or parsed (and
no diagnostic reached the threshold). Parse failures are listed separately from
diagnostics; a file that does not parse is never analyzed. A run analyzes all
given files as one project, so imports between them are followed.

### Adopting it in CI

```bash
midwrite check . --write-baseline midwrite-baseline.json   # once; commit the file
midwrite check . --baseline midwrite-baseline.json          # in CI: fails only on new findings
```

Baseline entries are matched by a fingerprint of the code, file, function and
the operations' source labels (no line numbers), so moving code within a file
keeps the match; a finding that moved to another file is still matched when
the match is unambiguous. Parse failures are never suppressed (exit status 3).
SARIF output carries the same fingerprint (`partialFingerprints`).

### Policies

| Policy | Warnings | Info |
| --- | --- | --- |
| `recommended` (default) | MW1001, MW1002, MW1003; MW1004 when a write bypasses its enclosing transaction | other MW1004 and MW1005 (review candidates), MW9001; unrecognized calls after a commit are grouped into one MW9001 per function |
| `strict` | every hazard (Phase 1 behavior) | MW9001, one per call |
| `audit` | as `strict` | as `strict`, plus unknown calls before the first recognized operation (possible unconfigured effects) |

The codes and their meaning are the same under every policy; only the
severity (and so the CI outcome) changes.

## Configuration

`midwrite.json` (every field optional; a missing field keeps its default):

```json
{
  "databaseReceivers": ["db", "this.db", "ctx.db", { "name": "database", "importedFrom": "@/server/db" }],
  "transactionMethods": ["transaction"],
  "externalEffects": [
    { "receiver": "queue", "method": "publish", "category": "message-publish" },
    { "receiver": "this.queue", "method": "publish", "category": "message-publish" },
    { "receiver": "mailer", "method": "send", "category": "email" },
    { "receiver": "stripe.charges", "method": "create", "category": "payment" },
    { "function": "fetch", "category": "http-request" },
    { "function": "sendEmail", "category": "email", "importedFrom": "@/lib/email" }
  ]
}
```

An external-effect rule may be narrowed to calls whose first argument is a
string literal, and marked best-effort when the application accepts that the
effect is sometimes not performed (rules are tried in order):

```json
{ "function": "triggerJob", "firstArgument": "notification", "category": "message-publish", "bestEffort": true },
{ "function": "triggerJob", "category": "message-publish" }
```

Under the `recommended` policy, a MW1001 or MW1005 whose effect that may
not happen is best-effort is `info` (with a note); MW1002 and MW1003, where
the effect already happened, are unchanged.

A rule may also declare a **recovery property** (`"recovery"`), which is an
assertion you make, never inferred from names and never verified:
`"bestEffort"` (same as `"bestEffort": true`), `"reconciled"` (a process
outside the analyzed code re-derives this effect from committed database
state when it is missing; MW1001/MW1005 about it become `info` under
`recommended`, the window is still reported) and `"idempotentConsumer"`
(receivers deduplicate repeats; a note only, severity unchanged). Diagnostics
list applied declarations in `recovery` with `"source": "configuration"` and
`"verified": false`.

**Recovery contracts** describe a mechanism *outside* a function that
re-executes it after a failure (a job runner's retries) or repairs its effects
(a reconciliation process). They must name the function exactly, list the
codes they cover and cite their evidence; they are never inferred:

```json
"recoveryContracts": [{
  "function": "ProcessDocumentProcessor.process",
  "file": "apps/worker/src/processors/documents/process-document.ts",
  "strategy": "retried",
  "covers": ["MW1001", "MW1002"],
  "evidence": "queues/documents.config.ts:17 attempts 3; stalled jobs are re-run once",
  "limits": "gives up after 3 attempts"
}]
```

A covered finding is still reported with the same fingerprint; under
`recommended` a warning becomes `info`, with a `recovery` note
(`"source": "configuration"`, `"verified": false`) and the cited evidence.
`reconciled` contracts cannot cover MW1002/MW1003. See
[`docs/phase4/RECOVERY-SEMANTICS.md`](docs/phase4/RECOVERY-SEMANTICS.md).

Defaults: `databaseReceivers: ["db"]`, `transactionMethods: ["transaction"]`,
and one external effect, `queue.publish` (`message-publish`). Categories:
`message-publish`, `http-request`, `email`, `payment`, `file-write`, `other`.
Unknown fields are rejected.

`"exclude": ["tests/**", "**/*.test.ts", "scripts/**"]` skips files when a
directory is scanned (a pattern with `/` is matched against the path relative
to the scanned directory, one without against each file and directory name).
In the Phase 2 evaluation, test and seed files produced most of the
`MW1004`/`MW9001` volume of one project.

**Trust model.** Configuration entries are *assertions you make*. A bare name
(`"db"`) matches any binding of that name unless the file shows otherwise (for
example `const db = new Map()` disables it). `importedFrom` requires the
binding to be imported from that exact module specifier, which is stronger
evidence; prefer it in larger codebases. Matching is on the root identifier of
the call (after following `const` aliases and renamed imports). A receiver may
also be a **member path** (`this.db`, `ctx.db`, `this.queue`): the root is
resolved like a bare name (`this` is not resolved at all) and the properties
are trusted as written, so it is the weakest kind of match. A member receiver
whose path is not configured (`this.repo.db`) is reported as `MW9001` with
the path to add, instead of being guessed. Every diagnostic lists which kind
of match produced it (`name-based`, `member-path`, `import-based`,
`drizzle(...)` provenance, or transaction handle).

## Diagnostics

| Code | Severity | Meaning |
| --- | --- | --- |
| `MW1001` | warning | Database commit followed by an independent external effect |
| `MW1002` | warning | External effect followed by an independent database commit |
| `MW1003` | warning | External effect executed inside a database transaction before commit |
| `MW1004` | warning (`strict`); info unless a write bypasses its transaction (`recommended`) | Independent database commits (no shared transaction, or `db` used inside a `tx` callback) |
| `MW1005` | warning (`strict`); info (`recommended`) | Independent external effects |
| `MW9001` | info | Analysis incomplete: unknown or unsupported semantics (not a finding) |

Confidence combines two separately reported dimensions:
`pathCertainty` (`straightLine`, `conditional`, `approximated`, `truncated`,
`unknown`) and `evidenceStrength`, the weakest recognition among the
operations (`provenance`, `configuredImport`, `configuredName`, `memberPath`,
`unknown`). It is `high` only on a straight-line path with no member-path
match; `medium` when the sequence depends on branch decisions, loops,
unawaited calls, an operation a callee performs only on some of its paths, or
a member-path match; `low` for approximations (concurrent operations,
catch-handler entry) and `MW9001`. When an operation comes from a callee's
summary, the diagnostic points at the call, lists the operation at its own
location (in its own file), and says so in `evidence`. See
[`docs/DIAGNOSTICS.md`](docs/DIAGNOSTICS.md).

## Transaction semantics

Midwrite does not treat every awaited write as committed:

* A Drizzle write outside a recognized transaction commits when it executes
  (PostgreSQL auto-commit).
* A write on the handle of `db.transaction(async (tx) => ...)` is *staged*; it
  becomes durable only when the callback completes normally (including an
  early `return`). An exception, `tx.rollback()` or a `throw` rolls it back.
* A nested `tx.transaction(...)` is a savepoint: its writes commit with the
  outermost transaction, and a rolled-back savepoint discards only its own writes.
* A write on `db` inside a transaction callback runs on a separate connection
  and commits on its own (reported as `MW1004`). A `db.transaction(...)`
  started inside another callback is an independent transaction.
* An external effect inside a transaction with writes is visible before the
  commit (`MW1003`). Only that the writes share a transaction is established;
  Midwrite does not check that an outbox table is actually relayed.

## False positives and false negatives

Known **false positives** (reported, but not a real problem):

* A binding that merely shares a configured name (for example a parameter
  `db: Map<...>`) is trusted. Use `importedFrom` to prevent this.
* Conditions on members, calls or reassigned variables are not correlated
  (`if (input.a) write(); if (!input.a) publish();` is still reported).
* Idempotent consumers, job retries, webhook redelivery and reconciliation
  jobs elsewhere are invisible unless declared (`recovery`,
  `recoveryContracts`); in the Phase 2 review, most structurally
  correct findings that were not bugs were best-effort notifications or
  operations repaired by job retries (see
  [`docs/phase2/CORPUS-EVALUATION.md`](docs/phase2/CORPUS-EVALUATION.md)).
* Two adjacent operations are reported even when the second is not related
  to the first (a notification followed by an unrelated update).

Known **false negatives** (real hazards that are not reported):

* Effects inside methods on receivers whose class is not known
  (`service.save()` with `service` a parameter or a factory result; methods
  of known classes are followed since Phase 5), callbacks run by
  unanalyzed code, packages outside the analyzed files, and functions
  whose module or export cannot be resolved. A call after a recognized commit becomes `MW9001`; one
  before the first commit is not reported (except under `--policy audit`).
  Calls are followed only when awaited or returned from an `async` function,
  up to `--max-call-depth` calls deep and never recursively.
* Database handles obtained from a call are recognized only when the call
  is configured (`"getDb()"`) and has no arguments.
* Unconfigured external effects (`fetch`, SDK clients) before the first
  recognized commit, for example charging a card through an unconfigured client
  and then writing to the database.
* Member receivers whose path is not configured, or that are reached through
  aliases or destructuring (`const { db } = ctx`), are reported as `MW9001`
  or not at all, but not analyzed.
* Second and later loop iterations, top-level module code, callbacks run by
  other functions (`array.forEach(async ...)`), and raw SQL.

## Security considerations

* Midwrite only reads the files you point it at and the configuration file.
  It never executes, imports or evaluates project code, makes no network
  requests, has no telemetry, and needs no account or service.
* Output includes short source excerpts (labels and conditions). Treat JSON and
  SARIF reports like source code when sharing them.
* It is a static aid, not a security or correctness guarantee. A clean run does
  not mean the code is crash-consistent.
* `midwrite replay` executes code: the harness directory you name, and only
  with `--allow-execution`. It gives the harness a private, disposable
  PostgreSQL cluster (Unix socket only) and clears the environment. On macOS
  `sandbox-exec` denies IP networking, other Unix sockets and file writes
  outside its temporary directory; on Linux bubblewrap runs it in new user,
  network (loopback only) and PID namespaces with a minimal read-only
  filesystem view and only the temporary directory writable. Without such
  isolation it refuses to run unless both the manifest and the command line
  allow the network. External database URLs and credential-like environment
  variables in a manifest are refused. This protects against accidents in a
  *trusted* harness; it is **not** a sandbox for malicious code. See
  [`docs/phase4/SECURITY-REVIEW.md`](docs/phase4/SECURITY-REVIEW.md) and
  [`docs/phase4/LINUX-ISOLATION.md`](docs/phase4/LINUX-ISOLATION.md).
* Reports are written only where you ask. Nothing is uploaded anywhere.

## Project layout

```text
crates/midwrite-core       Effect IR, failure-path analysis, diagnostics, config (no parser dependency)
crates/midwrite-analyzer   TypeScript frontend: Oxc parsing, provenance, recognition, lowering, module resolution, project analysis; concurrency extractor, schema facts
crates/midwrite-discover   Candidate discovery and scenario synthesis (`midwrite discover`)
crates/midwrite-model      Bounded explicit-state model checker, PostgreSQL model, lockstep oracle, original-code differential
crates/midwrite-cli        The `midwrite` (and legacy `crashproof`) binary: text, JSON, SARIF and HTML output, baselines, every command
crates/midwrite-replay     Opt-in runtime execution: manifest validation, disposable PostgreSQL, isolation, replay supervisor, source coordinator, fault proxy
crates/midwrite-corpus     Developer tool (not published): corpus runs, run comparison, reviewed precision, phase memory profile
runtime/node/                Dependency-free replay client (Node.js and Bun), pg adapter, event-log transport (workspace package @midwrite/replay, unpublished)
runtime/typescript/          Phase 3 import path for Bun harnesses (re-exports runtime/node)
ci/, .github/workflows/      Linux test image; CI and release workflows (not yet run on GitHub)
fixtures/                    vulnerable/, safe/, unsupported/ single-file examples, projects/ multi-file examples, all with `// @expect` annotations; replay/ runnable replay harnesses
corpora/                     Evaluation corpus manifests, per-project configurations and review labels
docs/                        README.md (index of the current documentation), getting-started/, user-guide/, concepts/, architecture/, reference/, development/, research/; ARCHITECTURE.md, DIAGNOSTICS.md, LIMITATIONS.md; phase2/ … phase13/ (historical records)
reports/                     Raw evidence of each phase (reports/phaseN/)
```

## Contributing

Contributions are welcome. See [`CONTRIBUTING.md`](CONTRIBUTING.md) and
[`docs/development/testing.md`](docs/development/testing.md). Before
sending a change, run:

```bash
cargo fmt --all -- --check
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo test --workspace
```

New behavior should come with a fixture under `fixtures/` whose
`// @expect` annotations state the exact diagnostics (or `none`); behavior
that spans modules goes in a directory under `fixtures/projects/`.

The runtime replay tests (`crates/midwrite-cli/tests/replay.rs`) need Bun,
Node.js ≥ 22, the PostgreSQL server binaries, the isolation backend (macOS
`sandbox-exec` or Linux bubblewrap) and the Drizzle fixture's dependencies
(`npm ci` in `fixtures/replay/drizzle-node`); without them they print
`SKIPPED` and pass, which is **not** runtime verification. Set
`MIDWRITE_REQUIRE_REPLAY=1` to make a missing dependency fail instead (the
`replay-linux` CI job does). `scripts/linux-replay-docker.sh` runs the whole
suite on Linux in a container.

To evaluate a change on the corpus (external projects are cloned at pinned
revisions; nothing is executed or modified):

```bash
corpora/fetch.sh
cargo run --release -p midwrite-corpus -- run --manifest corpora/manifest.json --output reports/after.json
cargo run --release -p midwrite-corpus -- compare reports/phase3/corpus-phase3.json reports/after.json
cargo run --release -p midwrite-corpus -- precision --report reports/after.json --labels corpora/labels
```

## Funding

Midwrite is free for individuals and businesses, with no telemetry and no
hosted service. Voluntary sponsorship is welcome in principle, but no
funding destination has been set up or verified yet, so none is listed.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in this
project shall be dual licensed as above, without any additional terms or
conditions.

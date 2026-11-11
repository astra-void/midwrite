# Phase 16 evaluation dataset

## What it is, and what it is not

* A **new** application, `fixtures/phase16/eval-app`, with **40 labeled
  entries** in `fixtures/phase16/eval-labels.json`: **18 positive, 19
  negative, 3 unsupported**. It is static TypeScript (never executed, no
  `node_modules`): drizzle-orm 0.45.4 on node-postgres (`pg` 8.23.1),
  **zod 4.6.5** (`import { z } from "zod"`, zod 4 API: `z.strictObject`,
  `z.uuid()`, `z.email()`, `z.treeifyError`), nanoid 5.1.5; a Drizzle
  `pg-core` schema of 41 tables in six modules behind a barrel; a primary
  `db`, a Drizzle replica set `replicatedDb = withReplicas(db, [replica])`
  and a project-defined replica set `readScaled = createReplicaSet(db,
  [replica])` (`src/lib/replicas.ts`, declared in
  `commitrift-contracts.json` → `replicaFactories`); 20 service modules,
  two Next.js-style route handlers (`src/app/api/*/route.ts#POST`), helpers
  for actors (`src/lib/actor.ts`), PostgreSQL error codes and retries
  (`src/lib/pg-errors.ts`) and generated ids (`src/lib/ids.ts`).
* **Theme: realistic application surfaces around a write.** zod 4
  validation (strict objects, `.optional()` / `.nullable()` / `.default()`,
  validated actors), bounded retry loops and SQLSTATE 40001 retries, JSON
  request bodies, hoisted and conditional Drizzle predicates, multi-row
  locks (`inArray` + `FOR UPDATE`, `SKIP LOCKED`, `NOWAIT`), `NULLS NOT
  DISTINCT`, application-generated identifiers and replica reads. Every
  positive has a safe counterpart in the same family (`counterpart`) that
  differs minimally and is safe in PostgreSQL; `inputSensitivity` names the
  input or initial-state relationship the label depends on.
* **Every decided property is declared.** All 37 positive/negative cases
  name a property declared in `eval-app/commitrift-contracts.json` (21
  `atMostOne`, 2 `nonNegative`, 2 read-your-writes `formula` rules) and
  carry `requiresDeclaration: true`, `declaredInContracts: true` and the
  rule in `contractsUsed`. The set never relies on an undeclared business
  invariant. In the four declared keys over a nullable column
  (`labels.groupId`, `collections.parentId`, `mute_rules.channelId`,
  `notification_rules.channelId`) **NULL equals NULL for the rule** (one
  "ungrouped" group, one root, one "every channel"); each rule's
  description says so. A tool that reads a declared key with SQL's
  NULL-distinct equality would not see E03, E21, E28 as violations: that is
  a gap in the tool, not in the labels.
* **Author: an AI agent (Claude), not an independent human reviewer.** No
  independent human review was performed. Frozen before any Phase 16
  scoring and before any CommitRift command was run on it (see
  `FROZEN.json`).

## How independence was maintained

Read (and only these, besides the files written):

* `docs/reference/supported-patterns.md`, `docs/reference/configuration.md`,
  `docs/reference/limitations.md`;
* `docs/user-guide/discover.md`, `docs/user-guide/contracts.md`;
* `fixtures/phase15/EVALUATION-DATASET.md`, `fixtures/phase15/eval-labels.json`,
  `fixtures/phase15/FROZEN.json`, and parts of `fixtures/phase15/eval-app`
  (`package.json`, `commitrift-contracts.json`, `src/db.ts`, `src/lib/*`,
  `src/schema/index.ts`, `content.ts`, `accounts.ts`, `billing.ts`, and the
  services `profile.ts`, `documents.ts`, `wallets.ts`, `grants.ts`,
  `loyalty.ts`) as a format reference;
* `scripts/phase15-eval.py`, to learn which label keys the scorer reads.

Not read: anything under `crates/`, `runtime/`, `.trail/`, `docs/phase16/`,
or any `docs/phase1*/` design document; `docs/concepts/` was not opened
either. No `cargo`, build, `commitrift`, `tsc` or Node.js command was run;
the application was not type-checked or executed. Labels come from
PostgreSQL semantics (READ COMMITTED unless a case says otherwise),
JavaScript semantics and zod 4 parsing semantics, not from what any version
of the tool does.

**Not blind.** The user documentation lists gaps relevant to this set (as of
Phase 15: zod `strictObject`, `nullable`/`optional`/`default`, `for`/`while`
loops, `req.json()`, predicates hoisted into locals, multi-row
`FOR UPDATE`, properties over generated identifiers, `NULLS NOT DISTINCT`
on nullable columns). The families were requested by the caller and
overlap that list on purpose; the author knew it. The set is therefore
fresh but targeted.

## Format

The Phase 15 label format, unchanged (`schemaVersion` 1; top-level
`$comment`, `schemaVersion`, `project`, `families`, `expectedSchema`,
`cases`, `corrections`; per case `id`, `family`, `entry`, `class`, `label`,
`tables`, `property`, `environment`, `inputSensitivity`, `counterpart`,
`why`, `isolation`, `effectful`, and where relevant `observable`,
`replicas`, `requiresDeclaration`, `declaredInContracts`, `contractsUsed`).
`scripts/phase15-eval.py` reads exactly these keys; it can score this set by
pointing `BASE` at `fixtures/phase16` (the scoring rule needs no change). No
case uses `pairedWith`: every label is about invocations of the entry
itself — one call, a call whose own loop retries, two overlapping calls, or
two calls separated by replica lag, as `environment` says. Ids are
`P16-E01` … `P16-E40`.

* **Inputs** are the values the entry's zod schema admits (entries taking
  `raw: unknown` or a JSON body) or its TypeScript signature admits
  (`parentId: number | null` includes `null`). A JavaScript caller that
  ignores the types is out of scope.
* **A valid initial state** satisfies the schema's constraints, the
  declared invariants and the case's environment; several negatives
  further restrict it to states the entry's own writers can produce (for
  example "a queued job has no run row"), as their `environment` says.
* Every entry is labeled `effectful: true`.

## Families

| family | what the pair exercises | + | − | ? |
| --- | --- | ---: | ---: | ---: |
| zod-strict | `z.strictObject` / `.strict()` gate the write; the protection is the unique key, not the schema | 1 | 1 | |
| optional-nullable-default | `.optional()` → NULL under NULLS DISTINCT vs `.default(0)`; `.nullable()` selecting an unprotected branch vs a non-nullable field | 2 | 2 | |
| validated-authorization | strict actor schema + role guard, then an unconditional vs a conditional status transition | 1 | 1 | |
| bounded-retry-loop | `for (attempt < N)` re-running a plain INSERT vs an upsert; `while (attempt < MAX)` re-running a committed transaction vs a claimed request id | 2 | 2 | |
| sqlstate-retry | 40001 retry at READ COMMITTED vs SERIALIZABLE; an outbox write on `db` vs `tx` inside a retried SERIALIZABLE transaction | 2 | 2 | |
| request-json-body | `POST(request)` with `await request.json()` + `safeParse` + `Response.json`; no unique key vs UNIQUE + ON CONFLICT | 1 | 1 | |
| hoisted-predicate | `const claimable = and(...)` in the SELECT vs in the UPDATE; a hoisted `isNull`/`eq` ternary with and without a serializing row lock | 2 | 2 | |
| multi-row-lock | `inArray` read without a lock vs sorted `FOR UPDATE`; a batch claim without a lock vs `SKIP LOCKED` and `NOWAIT` | 2 | 3 | |
| nulls-not-distinct | the same check-then-act on UNIQUE NULLS DISTINCT vs `.nullsNotDistinct()` | 1 | 1 | |
| generated-identifier | nanoid / randomUUID ids: which generated id is stored and which one the caller is handed (payment intents, refresh-token rotation) | 2 | 2 | |
| replica-read | declared project replica factory: read-back on the replica vs in a primary transaction; `withReplicas` stale check vs a locked primary read | 2 | 2 | |
| unsupported | computed-name dispatch over a runtime registry, `new Function` over stored source, unbounded recursion | | | 3 |

Classes: check-then-act 16, status-transition 13, ambiguous-commit 6,
return-value 2, unknown 3. Isolation: SERIALIZABLE for E14, E15, E16 (set
by the code); READ COMMITTED otherwise.

## The entries

Paths are relative to `eval-app/src/services/` (route handlers: to
`eval-app/src/`). `↔` names the counterpart.

| id | entry | label | family | class | depends on (short) |
| --- | --- | --- | --- | --- | --- |
| P16-E01 | projects.ts#createProject | + | zod-strict | check-then-act | z.strictObject input; SELECT then INSERT, (workspace, slug) not unique ↔ E02 |
| P16-E02 | projects.ts#createBoard | − | zod-strict | check-then-act | z.object().strict(); UNIQUE(workspace, slug) + ON CONFLICT DO NOTHING ↔ E01 |
| P16-E03 | labels.ts#createLabel | + | optional-nullable-default | check-then-act | groupId `.optional()` → NULL; UNIQUE NULLS DISTINCT does not cover it ↔ E04 |
| P16-E04 | labels.ts#createTag | − | optional-nullable-default | check-then-act | groupId `.default(0)`; key never NULL, unique covers every input ↔ E03 |
| P16-E05 | invites.ts#inviteMember | + | optional-nullable-default | check-then-act | projectId `.nullable()`: null takes the unprotected workspace branch ↔ E06 |
| P16-E06 | invites.ts#inviteProjectMember | − | optional-nullable-default | check-then-act | projectId non-nullable: only the primary key + ON CONFLICT branch ↔ E05 |
| P16-E07 | expenses.ts#approveExpense | + | validated-authorization | status-transition | strict actor, role guard, read status, unconditional UPDATE + reimbursement ↔ E08 |
| P16-E08 | expenses.ts#approveExpenseOnce | − | validated-authorization | status-transition | same guard; UPDATE … WHERE status = 'submitted' RETURNING in the transaction ↔ E07 |
| P16-E09 | payments.ts#recordPayment | + | bounded-retry-loop | ambiguous-commit | `for` retry loop re-runs a plain INSERT after a lost reply ↔ E10 |
| P16-E10 | payments.ts#recordPaymentIdempotent | − | bounded-retry-loop | ambiguous-commit | same loop; ON CONFLICT (request_id) DO UPDATE upsert ↔ E09 |
| P16-E11 | credits.ts#grantCredits | + | bounded-retry-loop | ambiguous-commit | `while` retry loop re-runs a committed transaction (lost COMMIT reply) ↔ E12 |
| P16-E12 | credits.ts#grantCreditsOnce | − | bounded-retry-loop | ambiguous-commit | same loop; the transaction first claims the request id (PK, ON CONFLICT) ↔ E11 |
| P16-E13 | inventory.ts#reserveStock | + | sqlstate-retry | check-then-act | 40001 retry loop but READ COMMITTED: oversell to −1 ↔ E14 |
| P16-E14 | inventory.ts#reserveStockSerializable | − | sqlstate-retry | check-then-act | same loop at SERIALIZABLE ↔ E13 |
| P16-E15 | withdrawals.ts#requestWithdrawal | + | sqlstate-retry | ambiguous-commit | SERIALIZABLE + 40001 helper; outbox INSERT on `db` survives the rolled-back attempt ↔ E16 |
| P16-E16 | withdrawals.ts#requestWithdrawalTx | − | sqlstate-retry | ambiguous-commit | same; outbox INSERT on `tx` ↔ E15 |
| P16-E17 | app/api/waitlist/route.ts#POST | + | request-json-body | check-then-act | `POST(request)` + request.json() + safeParse; SELECT then INSERT, no unique ↔ E18 |
| P16-E18 | app/api/rsvp/route.ts#POST | − | request-json-body | check-then-act | same handler shape; UNIQUE(event, email) + ON CONFLICT DO NOTHING ↔ E17 |
| P16-E19 | jobs.ts#claimJob | + | hoisted-predicate | status-transition | hoisted `claimable` used only in the SELECT; UPDATE by id ↔ E20 |
| P16-E20 | jobs.ts#claimJobAtomic | − | hoisted-predicate | status-transition | hoisted `claimable` is the UPDATE's WHERE ↔ E19 |
| P16-E21 | collections.ts#createCollection | + | hoisted-predicate | check-then-act | hoisted ternary isNull/eq predicate; NULL parent duplicates (NULLS DISTINCT) ↔ E22 |
| P16-E22 | collections.ts#createCollectionLocked | − | hoisted-predicate | check-then-act | same, serialized by the workspace row FOR UPDATE ↔ E21 |
| P16-E23 | ledger.ts#transferFundsUnlocked | + | multi-row-lock | check-then-act | inArray read of both accounts without a lock; overdraft ↔ E24 |
| P16-E24 | ledger.ts#transferFunds | − | multi-row-lock | check-then-act | inArray + ORDER BY id + FOR UPDATE (sorted multi-row lock) ↔ E23 |
| P16-E25 | tasks.ts#claimBatchUnlocked | + | multi-row-lock | status-transition | batch read without a lock; UPDATE by inArray(ids); double claims ↔ E26, E27 |
| P16-E26 | tasks.ts#claimBatch | − | multi-row-lock | status-transition | FOR UPDATE SKIP LOCKED batch ↔ E25 |
| P16-E27 | tasks.ts#claimBatchNoWait | − | multi-row-lock | status-transition | FOR UPDATE NOWAIT batch (55P03 is a failure, not a duplicate) ↔ E25 |
| P16-E28 | notifications.ts#muteEvent | + | nulls-not-distinct | check-then-act | check-then-act with channelId null on UNIQUE … NULLS DISTINCT ↔ E29 |
| P16-E29 | notifications.ts#addNotificationRule | − | nulls-not-distinct | check-then-act | same on UNIQUE … NULLS NOT DISTINCT (PostgreSQL 15+) ↔ E28 |
| P16-E30 | checkout.ts#ensurePaymentIntent | + | generated-identifier | status-transition | generated `pi_` + nanoid id; two intents, the order keeps the last ↔ E31 |
| P16-E31 | checkout.ts#ensurePaymentIntentLocked | − | generated-identifier | status-transition | same, order row FOR UPDATE: every caller gets the stored id ↔ E30 |
| P16-E32 | sessions.ts#refreshSession | + | generated-identifier | status-transition | refresh-token rotation with randomUUID; UPDATE by id; token accepted twice ↔ E33 |
| P16-E33 | sessions.ts#refreshSessionChecked | − | generated-identifier | status-transition | UPDATE … WHERE refresh_token = presented ↔ E32 |
| P16-E34 | digests.ts#saveDigestFrequency | + | replica-read | return-value | declared replica factory; write on primary, read-back on lagging replica returns false ↔ E35 |
| P16-E35 | digests.ts#saveDigestFrequencyInTransaction | − | replica-read | return-value | same factory; write and read-back in one primary transaction ↔ E34 |
| P16-E36 | coupons.ts#redeemCoupon | + | replica-read | status-transition | withReplicas: redeemed flag read from the replica; second redemption ↔ E37 |
| P16-E37 | coupons.ts#redeemCouponLocked | − | replica-read | status-transition | replicatedDb.transaction + FOR UPDATE on the primary ↔ E36 |
| P16-E38 | bulk.ts#runBulkAction | unsupported | unsupported | unknown | `` repositories[entity][`${verb}Many`] `` over a runtime-extensible registry |
| P16-E39 | automations.ts#runAutomation | unsupported | unsupported | unknown | `new Function(...)` over source text stored in the database |
| P16-E40 | tree.ts#archiveCollectionTree | unsupported | unsupported | unknown | unbounded recursion over the collection tree |

The full property, environment, input sensitivity and violating schedule (or
the reason no schedule violates the property) of every case are in the
labels file.

## Caveats a scorer must keep

* **Environment-dependent labels.** E29 needs PostgreSQL 15 or newer
  (`NULLS NOT DISTINCT`). E34 and E36 (sequential variant) assume an
  asynchronous replica that has not replayed the write (with synchronous
  `remote_apply` replication E34 would still be positive under concurrency,
  E36 under overlap). E09 and E11 need an ambiguous failure: the server
  commits and the client receives a connection error the loop retries.
  E15 needs a concurrent withdrawal on the same wallet to provoke 40001.
* **Negatives where a call still fails.** E02 (SlugTakenError), E21 with a
  non-null parent (23505 — E21 is positive only through the NULL parent),
  E24 (InsufficientFundsError), E14 (OutOfStockError or contention after 5
  attempts), E27 (55P03 under NOWAIT): failures are outcomes, not violations
  of the labeled property.
* **Reachable initial states.** E08, E12, E20, E26, E27, E31, E33, E37
  restrict the initial state to what their writers produce (a queued job
  has no run, a ready task no claim, an unredeemed coupon no redemption, an
  order without an intent pointer no intent row, …). A state that already
  violates the property is not a valid initial state.
* **Generated values** are assumed distinct (two `randomUUID()` / nanoid
  results never collide); no label depends on a collision.
* **E15/E16 are labeled `ambiguous-commit`**: the hazard is an autocommit
  write inside the `try` that the `catch`-driven loop repeats (on `db`
  instead of `tx`), not a lost COMMIT reply.
* **Unsupported entries** (E38–E40) carry no property; a tool should
  report them as unknown/unsupported, and a "safe" claim on them is listed
  in the scorer's `unsupported` stage (`claimedSafe`).
* **Contract shapes.** `atMostOne {table, key}`, `nonNegative {table,
  column}` and the read-your-writes `formula` follow the shapes of
  `fixtures/phase15/eval-app/commitrift-contracts.json`;
  `replicaFactories` follows `docs/user-guide/contracts.md`. Contracts reject
  unknown fields; if any shape is rejected, that must be recorded as a
  correction, not fixed silently.
* Labels are never edited. A correction goes into the labels file's
  `corrections` array with a reason.

## Freeze

`FROZEN.json` records the SHA-256 of every file written under
`fixtures/phase16` except `FROZEN.json` itself — the application, the
labels and this document — and a manifest digest: the SHA-256 of the UTF-8
text made of one line `<sha256>  <path>\n` (two spaces, LF) per file, paths
relative to `fixtures/phase16`, sorted by path in byte order (the same rule
as Phase 15 and as `scripts/phase15-eval.py`'s `verify_frozen`). The
digests were computed with a Python script run from a scratch directory
outside the repository; no script was left in the repository.

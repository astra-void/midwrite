# Phase 14 evaluation dataset

## What it is, and what it is not

* A **new** application, `fixtures/phase14/eval-app`, with **38 labeled
  entries** in `fixtures/phase14/eval-labels.json`: **18 positive, 18
  negative, 2 unsupported**. It is static TypeScript (never executed, no
  `node_modules`): drizzle-orm 0.45.4 on node-postgres (`pg` 8.23.1), zod
  3.24.2, a Drizzle `pg-core` schema of 28 tables in five modules behind a
  barrel, a primary `db` and a `withReplicas(db, [replica])` replica set,
  services, a worker, Next.js-style `"use server"` actions and route
  handlers (`export async function POST(req)` and `export const POST =
  withAuth(async (ctx) => …)`), an application `withRetry` wrapper, and
  `commitrift-contracts.json`.
* **Frozen before any Phase 14 scoring and before any CommitRift command was
  run on it** (no `cargo` and no `commitrift` invocation of any kind by the
  author). `fixtures/phase14/FROZEN.json` records the SHA-256 (`shasum -a
  256`) of all 38 application files and of the labels: manifest digest
  **`b44c7d184aad052270845ba8b6017bff0460b3e066e9342f9391999273977798`**,
  recorded 2026-10-27T23:56:04Z (2026-10-28 08:56 local, UTC+9).
* **Author: an AI assistant (Claude).** It did not read `crates/`,
  `runtime/` or `docs/phase14/`. It read the Phase 2–13 documents, the
  Phase 12 fixtures, the Phase 13 evaluation set and scorer, and
  `corpora/contracts`. Labels come from PostgreSQL semantics (READ
  COMMITTED unless a case says otherwise) and JavaScript semantics, not
  from what any version of the tool does. The author knew the Phase 13
  supported subset and its documented gaps, so the set is fresh but **not
  blind**. **No independent human review performed.**
* The application was type-checked in a scratch copy against the drizzle-orm
  0.45.4 / pg 8.23.1 typings with a zod typing stub, and the SQL of the
  label-critical statements was printed with Drizzle's `toSQL()` there (no
  database). Neither artifact is part of the fixture.
* Labels are never edited. A correction goes into the labels file's
  `corrections` array with a reason. This document and
  `scripts/phase14-eval.py` are not part of the frozen set.

## Format

The Phase 13 label format (`schemaVersion` 1; `id`, `entry`, `class`,
`label`, `effectful`, `tables`, `property`, `environment`, `why`,
`pairedWith`, `observable`, `replicas`; `why` is the reason), plus:

* `family`: the source construct the case exercises (below).
* `isolation`: the isolation level of the labeled environment
  (`read committed`, except P14-E33 `repeatable read`, set by the code).
* `requiresDeclaration` / `declaredInContracts` (return-value cases only):
  the property is about what callers are told, cannot be read from the
  source or schema, and is or is not declared in
  `eval-app/commitrift-contracts.json`.
* `contractsUsed`: the contract items the environment or property relies on.
* top-level `families`, and `expectedSchema.nonUniqueIndexes`,
  `jsonColumns`, `timestampColumns` (informational).

Every entry is labeled `effectful: true` (each has a reachable write).

`commitrift-contracts.json` declares: `retry: on-crash` for the two
project-setup entries (E08, E09); `concurrent: false` for the invoice
finalizer (E16, E17); three `atMostOne` business rules (`credits`,
`stages`, `priorities`); and three `formula` invariants over return values
for the **declared** return-value variants (E18, E19, E21), in the Phase 13
`actors … returned` form (`RETURN-VALUES-AND-RESPONSES.md` §2). The
undeclared variants (E20, E22) are deliberately absent.

## Families

| family | construct | + | − |
| --- | --- | ---: | ---: |
| arrays | for-of over a literal array; `.map` into `values([...])`; `.includes` guard; `inArray(literal)` in an UPDATE's WHERE | 2 | 2 |
| strings | template-literal key; concatenated key with `trim/toLowerCase`; `split` of a known constant guarding a branch | 1 | 2 |
| multi-row-insert | `values([{…},{…}])` (one atomic statement) resubmitted after a lost response | 1 | 1 |
| zod | `parse` / `safeParse` with a `trim().toLowerCase()` transform feeding a check-then-act | 1 | 1 |
| higher-order | `withRetry(async () => …)` around a non-idempotent / idempotent write; `withAuth(async (ctx) => …)` routes | 2 | 2 |
| replica | check on the replica, write on the primary vs the check via `$primary` | 1 | 1 |
| return-value | at most one caller told success; read-your-writes; declared and undeclared | 4 | 1 |
| fk-parent-child | parent DELETE × child INSERT (FK NO ACTION); soft-deleted parent × child INSERT, unlocked / `FOR SHARE` | 1 | 2 |
| timestamp | lease with expiry vs a fixed `now` argument; `defaultNow` / `$onUpdate` columns | 1 | 1 |
| json | `settings.plan === "pro"` guard; read-merge-write of a jsonb document vs `jsonb \|\|` | 2 | 1 |
| atomic-update | read-modify-write in a RC transaction vs `col + n` vs REPEATABLE READ | 1 | 2 |
| unique | check-then-insert without a unique key / with a unique constraint / with a partial unique index | 1 | 2 |
| unsupported | raw SQL with a runtime text or identifier | – | – (2) |

Classes: check-then-act 16, status-transition 7, read-modify-write 5,
stale-read 4, resubmitted-write 2, ambiguous-commit 2, unknown 2 (the Phase
13 class names).

## The entries

Paths are relative to `eval-app/`. A paired case (`∥`) is labeled for the
entry running concurrently with the named partner.

| id | entry | label | family | class | reason (short) |
| --- | --- | --- | --- | --- | --- |
| P14-E01 | services/access.ts#grantDefaultRoles | + | arrays | check-then-act | for-of over `DEFAULT_ROLES`: SELECT then INSERT per role, no unique key: both callers insert |
| P14-E02 | services/access.ts#grantDefaultScopes | − | arrays | check-then-act | `.map` into one INSERT … ON CONFLICT DO NOTHING; (account, scope) is the primary key |
| P14-E03 | services/tickets.ts#claimTicket | + | arrays | status-transition | `CLAIMABLE.includes(status)` on a plain read; unconditional UPDATE: two 'assigned' events |
| P14-E04 | services/tickets.ts#claimTicketAtomic | − | arrays | status-transition | `status IN (literal list)` in the UPDATE's WHERE; the loser re-evaluates and matches 0 rows |
| P14-E05 | services/invoice-runs.ts#scheduleInvoiceRun | + | strings | check-then-act | template-literal run key; non-unique index: both insert |
| P14-E06 | services/exports.ts#queueExport | − | strings | check-then-act | concatenated normalized key is the PK; ON CONFLICT DO NOTHING |
| P14-E07 | services/beta.ts#joinBetaProgram | − | strings | check-then-act | `"stable/2026.10".split("/")[0]` ≠ "beta": the racy branch is dead; reachable write is PK + ON CONFLICT |
| P14-E08 | services/project-setup.ts#createDefaultStages | + | multi-row-insert | resubmitted-write | crash after commit, client resubmits: three more rows, no unique key |
| P14-E09 | services/project-setup.ts#createDefaultPriorities | − | multi-row-insert | resubmitted-write | UNIQUE(project, name) + ON CONFLICT DO NOTHING: resubmission inserts nothing |
| P14-E10 | app/actions/tags.ts#createTag | + | zod | check-then-act | `parse` trims/lowercases; SELECT then INSERT, no unique key |
| P14-E11 | app/actions/categories.ts#createCategory | − | zod | check-then-act | `safeParse` output normalized; UNIQUE(account, name) + ON CONFLICT DO NOTHING |
| P14-E12 | services/credits.ts#grantSignupCredit | + | higher-order | ambiguous-commit | INSERT committed, reply lost (ECONNRESET), `withRetry` inserts again; grant_key NULL never conflicts |
| P14-E13 | services/credits.ts#grantSignupCreditOnce | − | higher-order | ambiguous-commit | unique grant_key + ON CONFLICT (grant_key) DO NOTHING |
| P14-E14 | app/api/trials/route.ts#POST/arg[0] | + | higher-order | check-then-act | `withAuth` callback: SELECT trial then INSERT, no unique key |
| P14-E15 | app/api/v2/trials/route.ts#POST/arg[0] | − | higher-order | check-then-act | account row FOR UPDATE in a transaction; RC re-check sees the first trial (positive at RR, not labeled) |
| P14-E16 | services/invoices.ts#finalizeInvoice | + | replica | stale-read | consecutive non-overlapping runs (declared); the second reads 'draft' from the lagging replica and posts again |
| P14-E17 | services/invoices.ts#finalizeInvoicePrimary | − | replica | stale-read | the check reads `$primary`; runs never overlap (declared); posting and status commit together |
| P14-E18 | services/slots.ts#reserveSlot | + | return-value | status-transition | both callers told `true` (declared formula); the table looks legal |
| P14-E19 | services/slots.ts#reserveSlotAtomic | − | return-value | status-transition | `UPDATE … WHERE reserved_by IS NULL RETURNING` (declared formula) |
| P14-E20 | services/shifts.ts#claimShift | + | return-value | status-transition | as E18, **not declared** (`requiresDeclaration`) |
| P14-E21 | services/profile.ts#saveDisplayName | + | return-value | stale-read | read-back on the lagging replica returns `false` (declared formula) |
| P14-E22 | services/profile.ts#saveTimezone | + | return-value | stale-read | read-back on the lagging replica returns the old value, **not declared** |
| P14-E23 | services/projects.ts#addTask ∥ purgeProject | − | fk-parent-child | check-then-act | FK NO ACTION: one side fails with 23503 (handled by addTask); no orphan |
| P14-E24 | services/projects.ts#addTaskIfActive ∥ archiveProject | + | fk-parent-child | check-then-act | soft delete never fires the FK; FOR KEY SHARE ≠ FOR NO KEY UPDATE: open task in an archived project |
| P14-E25 | services/projects.ts#addTaskIfActiveLocked ∥ archiveProject | − | fk-parent-child | check-then-act | project FOR SHARE conflicts with archive's UPDATE: either archive cancels the task or the check sees archived_at |
| P14-E26 | workers/jobs.ts#claimJob | + | timestamp | status-transition | `lockedUntil > now` on a plain read; unconditional UPDATE: two runs in one lease |
| P14-E27 | workers/jobs.ts#claimJobAtomic | − | timestamp | status-transition | lease test in the UPDATE's WHERE; the loser re-evaluates `locked_until <= now` on now + 60 s |
| P14-E28 | services/onboarding.ts#requestOnboardingCall | + | json | check-then-act | `settings.plan === "pro"` guard; SELECT then INSERT, no unique key |
| P14-E29 | services/settings.ts#updateSettings | + | json | read-modify-write | `{ ...settings, ...patch }` written back: one patch lost |
| P14-E30 | services/settings.ts#updateSettingsAtomic | − | json | read-modify-write | `settings \|\| $1::jsonb` recomputed on the locked row version |
| P14-E31 | services/usage.ts#recordUsage | + | atomic-update | read-modify-write | RC transaction, no lock: lost increment |
| P14-E32 | services/usage.ts#recordUsageAtomic | − | atomic-update | read-modify-write | `SET used = used + $1` |
| P14-E33 | services/usage.ts#recordUsageRepeatable | − | atomic-update | read-modify-write | REPEATABLE READ: the loser fails with 40001 and does not count |
| P14-E34 | app/api/devices/route.ts#POST | + | unique | check-then-act | route handler; non-unique (account, fingerprint) index: both insert |
| P14-E35 | services/api-clients.ts#registerApiClient | − | unique | check-then-act | UNIQUE(account, name): the loser gets 23505 |
| P14-E36 | services/subscriptions.ts#activateSubscription | − | unique | check-then-act | partial unique index `WHERE status = 'active'` covers every inserted row: 23505 |
| P14-E37 | services/admin.ts#runReport | unsupported | unsupported | unknown | `db.execute(sql.raw(statement))` |
| P14-E38 | services/admin.ts#bumpCounter | unsupported | unsupported | unknown | `sql.identifier(table)` from a runtime argument |

The full property, environment and violating schedule (or the reason no
schedule violates the property) of every case are in the labels file.

## Caveats a scorer must keep

* **Contract shape of the declared return-value properties.** E18, E19 and
  E21 are declared as `{"template": "formula", "assert": <formula>}`. The
  `formula` template and the `actors … returned` term are documented
  (`docs/phase11/CONTRACTS.md`, `docs/phase13/RETURN-VALUES-AND-RESPONSES.md`),
  but no readable document shows the exact field that carries the formula
  inside a contract `property`; `assert` mirrors the scenario invariant
  (`docs/phase6/INVARIANTS.md`). Contracts reject unknown fields, so if this
  guess is wrong the contracts file fails to load. That must be reported as
  a correction, not fixed silently.
* **Undeclared properties.** E20 and E22 are positive by semantics but
  their property is not declared; a tool that infers no return-value
  property cannot find them. `scoring.excludingUndeclared` reports the
  score without them; `scoring.all` keeps them.
* **Environment-dependent negatives.** E15 is negative only at READ
  COMMITTED. E16/E17 depend on the declared `concurrent: false` (with
  overlapping runs E17's primary check would race). E26/E27 assume the two
  claims share the same `now`. E21/E22 assume a lagging asynchronous
  replica (with `remote_apply` they would be negative).
* **Negatives where a call still fails.** E23 (one side gets 23503 — a
  handled constraint error, not a violation of "no orphan"), E33 (40001),
  E35 and E36 (23505). Every-call-succeeds properties are not labeled.
* **E07** is negative because the guarded branch is dead for the constant
  as written; a change of the release constant would change the label.
* **E25** relies on `FOR SHARE` conflicting with `FOR NO KEY UPDATE`; the
  lock is weaker than `FOR UPDATE` but sufficient for the labeled pair.
* **E36** relies on a partial unique index, which is not a table-wide
  constraint but covers every row the entry inserts.

## How to score

```
# once, by the evaluation lead, after building the Phase 14 binary
scripts/phase14-eval.py reports/phase14/eval/first-scoring.json --first
# comparison with an earlier binary (not a first scoring)
COMMITRIFT_BIN=/path/to/phase13/commitrift scripts/phase14-eval.py reports/phase14/eval/phase13-binary.json
```

The script verifies `FROZEN.json` first (changed, missing or added files and
the manifest digest), runs `commitrift discover eval-app --model-check
--labels eval-labels.json --format json --no-timing`, and writes the Phase 13
stages (schema, entries, effects, candidates, the tool's `evaluation`,
validity, unsupported) plus `scoring`: model prediction TP/FP/FN/TN,
precision and recall overall (`all`), without the undeclared return-value
cases (`excludingUndeclared`), and per family (`byFamily`, with each case's
outcome). An unpaired case is predicted positive when the entry's own
candidate has a validated `MODEL_COUNTEREXAMPLE`; a paired case when the
candidate whose actors are exactly the pair has one. If the binary rejects
the Phase 14 label fields, the script retries once with a copy reduced to
the Phase 13 fields and records `labelsPassedToTool`. `--first` refuses to
overwrite an existing output: the first scoring is recorded once, and any
change it prompts is scored and reported separately as post-feedback.

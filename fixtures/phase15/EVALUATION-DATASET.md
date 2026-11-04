# Phase 15 evaluation dataset

## What it is, and what it is not

* A **new** application, `fixtures/phase15/eval-app`, with **30 labeled
  entries** in `fixtures/phase15/eval-labels.json`: **13 positive, 14
  negative, 3 unsupported**. It is static TypeScript (never executed, no
  `node_modules`): drizzle-orm 0.45.4 on node-postgres (`pg` 8.23.1; `zod`
  3.24.2 is pinned but not used), a Drizzle `pg-core` schema of 28 tables in
  five modules behind a barrel, a primary `db` and a `withReplicas(db,
  [replica])` replica set, 14 service modules, two helpers (`withRetry`,
  `requireAdmin`) and `midwrite-contracts.json`.
* **Theme: input-domain and initial-state sensitivity.** Every positive is
  violated only for *some* inputs the TypeScript signature permits and/or
  *some* initial database states the schema and the declared invariants
  permit; the other inputs and states are harmless. Every positive has a safe
  counterpart in the same family (`counterpart`). Each case carries an
  `inputSensitivity` field naming the input or initial-state relationship the
  label depends on (for a negative: why no permitted input or state violates
  the property).
* **Frozen before any Phase 15 scoring and before any Midwrite command was
  run on it** (no `cargo` and no `midwrite` invocation of any kind by the
  author; `scripts/phase15-eval.py` was written but not run).
  `fixtures/phase15/FROZEN.json` records the SHA-256 (`shasum -a 256`) of all
  25 application files and of the labels: manifest digest
  **`b11cc1a112ac4bda46689860c68e3cf8930ce663d59711c96f9724056c53d4ed`**,
  recorded 2026-10-28T09:05:42Z (2026-10-28 18:05 local, UTC+9).
* **Author: an AI assistant (Claude).** It did not read `crates/`,
  `runtime/`, `docs/phase14/` or `docs/phase15/`. It read
  `fixtures/phase14/EVALUATION-DATASET.md`, `fixtures/phase14/eval-labels.json`,
  `fixtures/phase14/eval-app`, `fixtures/phase13/eval-labels.json`,
  `scripts/phase14-eval.py` and `docs/reference`, `docs/concepts`,
  `docs/user-guide`. Labels come from PostgreSQL semantics (READ COMMITTED
  for every case) and JavaScript semantics, not from what any version of the
  tool does. The author knew the documented supported subset and its
  documented gaps — including the Phase 14 notes that synthesis can make an
  initial value coincide with the written value and does not explore
  arguments over `null`, and that `NULLS NOT DISTINCT` on nullable columns is
  not modeled — so the set is fresh but **not blind**. **No independent
  human review performed.**
* The application was type-checked in a scratch copy outside the repository
  against the drizzle-orm 0.45.4 / pg 8.23.1 typings (TypeScript 5.6,
  `strict`, `noUncheckedIndexedAccess`), and the SQL of the label-critical
  statements was printed with Drizzle's `toSQL()` on a mock driver there (no
  database). This confirmed, among others, that a `null` folder id or grant
  key is sent as a parameter, that the conditional folder predicate renders
  as `folder_id is null`, and that `nullsNotDistinct` is recorded on `pages`
  only. Neither artifact is part of the fixture.
* Labels are never edited. A correction goes into the labels file's
  `corrections` array with a reason. This document and
  `scripts/phase15-eval.py` are not part of the frozen set.

## Format

The Phase 14 label format (`schemaVersion` 1; top-level `$comment`,
`schemaVersion`, `project`, `families`, `expectedSchema`, `cases`,
`corrections`; per case `id`, `family`, `entry`, `class`, `label`,
`effectful`, `tables`, `property`, `environment`, `isolation`, `why`, and
`observable`, `replicas`, `requiresDeclaration`, `declaredInContracts`,
`contractsUsed` where relevant), plus:

* `inputSensitivity`: the input or initial-state relationship the label
  depends on.
* `counterpart`: the case's positive/negative twin. It is **not**
  `pairedWith`. No Phase 15 case uses `pairedWith`: every label is about
  invocations of the entry itself (one call, a retried call, a redelivered
  message or two concurrent calls, as the environment says).
* `expectedSchema.nullableColumns` (informational), next to the Phase 14
  `nonUniqueIndexes`.

What counts as an input and a state:

* **Inputs** are the values the TypeScript signature permits (`folderId:
  number | null` includes `null`; `teamId?: number | null` includes
  `undefined` and `null`; `teamId: number` does not include `null`). A
  JavaScript caller that ignores the types is out of scope (see E09).
* **A valid initial state** satisfies the schema's constraints, the declared
  invariants and the case's environment. A state that already violates the
  property (a negative `remaining`) is not a valid initial state.

Every entry is labeled `effectful: true`.

`midwrite-contracts.json` declares: `delivery: at-least-once` for the two
loyalty consumers (E26, E27); two `formula` read-your-writes invariants over
return values, in the Phase 14 `{"template": "formula", "assert": …}` shape
(E01, E02); eight `atMostOne` rules (invitations, carts, wishlists,
environments, channels, routes, aliases, ledger); and two `nonNegative` rules
(`quotas.remaining`, `orgs.vacancies`). Deliberately **not** declared: the
document/page rule (its meaning depends on NULL equality, which a declared
`atMostOne` key cannot state unambiguously), the theme-change and archive
rules (serial-equivalence properties), the admin-count rule (an `atLeast`
with a filter, whose exact JSON shape the readable documents do not show),
the per-call grant rule and the no-failure lock-order rule.

## Families

| family | input / state relationship | + | − |
| --- | --- | ---: | ---: |
| value-distinction | the stored value differs from the written one: replica read-back; a "changed?" check where both actors write the same new value; plus a race made harmless by equal written values | 2 | 3 |
| nullable-argument | `null` / `undefined` arguments: UNIQUE NULLS DISTINCT on a nullable column vs NULLS NOT DISTINCT; a `teamId == null` guard selecting an unprotected branch vs a non-nullable parameter | 2 | 2 |
| actor-keys | crossed actor keys (A.from = B.to, A.to = B.from) vs equal or disjoint keys; lock order vs ordered locks | 1 | 1 |
| boundary | a quota at exactly 1 under concurrency vs `remaining > 0` in the UPDATE; `>=` vs `>` at exactly 0 vacancies under a row lock | 2 | 2 |
| fk-parent-child | a parent (cart) created on demand for a child row: racing only when no parent exists initially; unique parent key + upsert | 1 | 1 |
| authorization | check-then-act behind an admin-membership guard (reachable only when the caller is an admin); guard + admin-count write skew (exactly two admins) vs the same under an org row lock | 2 | 2 |
| composite-unique | a check on (org, slug) with UNIQUE(org, kind, slug): covered only for equal kinds; UNIQUE(org, slug) | 1 | 1 |
| idempotency | an optional idempotency key that becomes NULL on a retried ambiguous commit; an event id checked before the effect vs claimed with it | 2 | 2 |
| unsupported | raw SQL whose text or identifier is a runtime argument | – | – (3) |

Classes: check-then-act 14, status-transition 3, stale-read 2, lock-order 2,
write-skew 2, ambiguous-commit 2, duplicate-processing 2, unknown 3.

## The entries

Paths are relative to `eval-app/src/services/`. `↔` names the counterpart.

| id | entry | label | family | class | depends on (short) |
| --- | --- | --- | --- | --- | --- |
| P15-E01 | profile.ts#saveNickname | + | value-distinction | stale-read | stored nickname ≠ argument; the lagging replica read-back returns false (declared) ↔ E02 |
| P15-E02 | profile.ts#saveNicknameInTransaction | − | value-distinction | stale-read | write and read-back in one primary transaction; any values, any concurrency ↔ E01 |
| P15-E03 | profile.ts#setTheme | + | value-distinction | status-transition | both actors pass the same theme ≠ stored: theme_changes +2, every serial order +1 ↔ E04 |
| P15-E04 | profile.ts#setThemeAtomic | − | value-distinction | status-transition | `WHERE theme <> $t` re-evaluated after the wait ↔ E03 |
| P15-E05 | notes.ts#archiveNote | − | value-distinction | status-transition | racy, but every actor writes the same constant and nothing depends on who did |
| P15-E06 | documents.ts#createDocument | + | nullable-argument | check-then-act | `folderId = null`: UNIQUE … NULLS DISTINCT never conflicts ↔ E07 |
| P15-E07 | documents.ts#createPage | − | nullable-argument | check-then-act | UNIQUE … NULLS NOT DISTINCT (PostgreSQL 15+) ↔ E06 |
| P15-E08 | invitations.ts#inviteMember | + | nullable-argument | check-then-act | `teamId == null` branch: SELECT then INSERT, no unique key (declared) ↔ E09 |
| P15-E09 | invitations.ts#inviteToTeam | − | nullable-argument | check-then-act | `teamId: number` reaches only the PK + ON CONFLICT branch ↔ E08 |
| P15-E10 | wallets.ts#transfer | + | actor-keys | lock-order | crossed wallets: deadlock, one transfer fails with 40P01 ↔ E11 |
| P15-E11 | wallets.ts#transferOrdered | − | actor-keys | lock-order | locks in ascending id order: no wait cycle ↔ E10 |
| P15-E12 | exports.ts#startExport | + | boundary | check-then-act | remaining = 1, two calls: −1 (declared nonNegative) ↔ E13 |
| P15-E13 | exports.ts#startExportAtomic | − | boundary | check-then-act | `remaining > 0` in the UPDATE's WHERE ↔ E12 |
| P15-E14 | seats.ts#addSeat | + | boundary | check-then-act | `vacancies >= 0` admits 0 under the lock: −1 (declared nonNegative) ↔ E15 |
| P15-E15 | seats.ts#addSeatChecked | − | boundary | check-then-act | `vacancies > 0` under the lock ↔ E14 |
| P15-E16 | carts.ts#addToCart | + | fk-parent-child | check-then-act | no cart initially: two carts (declared) ↔ E17 |
| P15-E17 | carts.ts#addToWishlist | − | fk-parent-child | check-then-act | UNIQUE account_id + ON CONFLICT, then re-read ↔ E16 |
| P15-E18 | environments.ts#createEnvironment | + | authorization | check-then-act | callers hold an 'admin' membership; no unique key (declared) ↔ E19 |
| P15-E19 | environments.ts#createReleaseChannel | − | authorization | check-then-act | same guard; UNIQUE(org, name) + ON CONFLICT ↔ E18 |
| P15-E20 | admins.ts#demoteAdmin | + | authorization | write-skew | exactly two admins demoting different admins: none left (not declared) ↔ E21 |
| P15-E21 | admins.ts#demoteAdminLocked | − | authorization | write-skew | org row FOR UPDATE serializes; later statements see the first demotion ↔ E20 |
| P15-E22 | routes.ts#claimRoute | + | composite-unique | check-then-act | different kinds: UNIQUE(org, kind, slug) does not cover (org, slug) (declared) ↔ E23 |
| P15-E23 | routes.ts#claimAlias | − | composite-unique | check-then-act | UNIQUE(org, slug) ↔ E22 |
| P15-E24 | grants.ts#grantCredit | + | idempotency | ambiguous-commit | grantKey omitted → NULL: the `withRetry` rerun inserts again ↔ E25 |
| P15-E25 | grants.ts#grantCreditKeyed | − | idempotency | ambiguous-commit | non-null key: the rerun conflicts and does nothing ↔ E24 |
| P15-E26 | loyalty.ts#applyLoyaltyEvent | + | idempotency | duplicate-processing | event not yet processed, redelivered (declared at-least-once): two ledger rows ↔ E27 |
| P15-E27 | loyalty.ts#applyLoyaltyEventOnce | − | idempotency | duplicate-processing | event id claimed in the same transaction as the ledger row ↔ E26 |
| P15-E28 | ops.ts#runSavedQuery | unsupported | unsupported | unknown | `db.execute(sql.raw(text))` |
| P15-E29 | ops.ts#bumpCounter | unsupported | unsupported | unknown | `sql.identifier(column)` from a runtime argument |
| P15-E30 | ops.ts#countViewUnlessExcluded | unsupported | unsupported | unknown | the write is guarded by the rows of `sql.raw(exclusionQuery)` |

The full property, environment, input sensitivity and violating schedule (or
the reason no schedule violates the property) of every case are in the
labels file.

## Caveats a scorer must keep

* **The positives need specific inputs or states by design.** A tool that
  synthesizes one arbitrary initial state and one argument vector per
  candidate can miss them (stored nickname equal to the argument, a cart
  already present, a non-admin caller, `remaining ≠ 1`, `folderId ≠ null`,
  equal route kinds, crossed vs equal wallets). That is what the set
  measures; it is not a labeling error.
* **Declared vs undeclared properties.** E01/E02 are declared return-value
  properties (`requiresDeclaration`, `declaredInContracts`). E20/E21 are
  write skew, which the documents say must be declared; they are **not**
  declared (`requiresDeclaration` and not `declaredInContracts`), so
  `scoring.excludingUndeclared` reports the score without them and
  `scoring.all` keeps them. E03/E04/E05 (serial equivalence), E06/E07,
  E10/E11 (no failure) and E24/E25 (per call) are not declared and not
  marked `requiresDeclaration`: the documents name synthesized properties of
  those kinds.
* **Contract shapes.** The `formula` and `atMostOne` shapes follow the
  Phase 14 evaluation application. The `nonNegative` shape `{"template":
  "nonNegative", "table", "column"}` follows `docs/user-guide/contracts.md`
  and `docs/concepts/invariants.md` (`nonNegative {table, column}`); the
  columns were chosen as single words (`remaining`, `vacancies`) so that the
  property name and the SQL name coincide, and the declared tables have
  single-word names for the same reason. Contracts reject unknown fields; if
  any shape is wrong the contracts file fails to load, which must be
  reported as a correction, not fixed silently.
* **Environment-dependent labels.** E01 assumes a lagging asynchronous
  replica (with `remote_apply` it would be negative). E07 needs PostgreSQL 15
  or newer. E09 assumes callers respect the `teamId: number` signature. E27
  assumes an initial state these writers can produce (no ledger row without
  its processed event id). E13/E15 assume a valid initial state (a negative
  counter already violates the invariant).
* **Negatives where a call still fails.** E15 (a duplicate seat: 23505), E21
  (forbidden / last-admin errors), E13 (quota exhausted): failures are
  outcomes, not violations of the labeled property. E10's property is
  exactly "no transfer fails", so its 40P01 is the violation.
* **E05** is the deliberate "harmless because the values are equal" case:
  the read-then-write race is real, but every actor writes the same constant
  and no state or return value records which call made the change.
* **E14** is a boundary defect without concurrency (a single call at
  `vacancies = 0`); the row lock makes concurrency irrelevant to it.
* **Unsupported entries** (E28–E30) are reported separately; a tool should
  report them as unknown/unsupported, and a "safe" claim on them is listed in
  the scorer's `unsupported` stage (`claimedSafe`).

## Freeze procedure

1. Write the application and the labels; type-check and print SQL in a
   scratch copy outside the repository; never run `cargo` or `midwrite`.
2. From `fixtures/phase15`:

   ```
   (find eval-app -type f; echo eval-labels.json) | LC_ALL=C sort | xargs shasum -a 256 > manifest.txt
   shasum -a 256 manifest.txt
   ```

   Each line is `<sha256>  <path>` (two spaces, LF), paths relative to
   `fixtures/phase15`, sorted in byte order. The second command's digest is
   `manifestSha256`.
3. Record the per-file digests (`files`), `manifestSha256`, the UTC time and
   the authorship statement in `FROZEN.json`. The manifest text itself is not
   stored; `scripts/phase15-eval.py` rebuilds it from `files` (sorted by path)
   and compares its SHA-256, and also refuses a changed, missing or added
   file under `eval-app/` (`.DS_Store` ignored).
4. The freeze was checked once with an independent re-implementation of that
   verification (no Midwrite, the scorer not run): 0 changed, 0 added,
   manifest matches.

## How to score

```
# once, by the evaluation lead, after building the Phase 15 binary
scripts/phase15-eval.py reports/phase15/eval/first-scoring.json --first
# comparison with an earlier binary (not a first scoring)
MIDWRITE_BIN=/path/to/phase14/midwrite scripts/phase15-eval.py reports/phase15/eval/phase14-binary.json
```

The script is `scripts/phase14-eval.py` on the Phase 15 paths with the same
rule: it verifies `FROZEN.json` first, runs `midwrite discover eval-app
--model-check --labels eval-labels.json --format json --no-timing`, and
writes the stages (schema, entries, effects, candidates, the tool's
`evaluation`, validity, unsupported) plus `scoring`: model prediction
TP/FP/FN/TN, precision and recall overall (`all`), without the undeclared
must-declare cases (`excludingUndeclared`), and per family (`byFamily`). A
case is predicted positive when the entry's own candidate (every actor is the
entry) has a `MODEL_COUNTEREXAMPLE` whose counterexamples are all validated;
the `pairedWith` rule is kept but unused. Added for reporting only:
`inputSensitivity` and `counterpart` on each case row, and
`scoring.counterparts` (each positive with its safe twin, and how many pairs
are both right). If the binary rejects the Phase 15 label fields, the script
retries once with a copy reduced to the Phase 13 fields and records
`labelsPassedToTool`. `--first` refuses to overwrite an existing output.

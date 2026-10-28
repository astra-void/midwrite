# Phase 13 evaluation dataset

(Frozen 2026-10-27, `fixtures/phase13/FROZEN.json`; this title said
"DRAFT" until the Phase 13 closeout. The dataset itself was not changed.)

## What it is, and what it is not

* A **new** application, `fixtures/phase13/eval-app`, with **27 labeled
  entries** in `fixtures/phase13/eval-labels.json`. The app uses a Drizzle
  `pg-core` schema in five modules behind a barrel, a `src/db.ts` with a
  primary and a `withReplicas(primary, [replica])` replica set, object and
  class repositories, a `withAuth` higher-order route wrapper, zod-validated
  inputs, a `withRetry` wrapper and an at-least-once job worker. It is plain
  TypeScript with no `node_modules`. It parses, and it type-checks against
  the Drizzle 0.45.4 typings with a zod stub; that check ran in a scratch
  copy and is not part of the fixture.
* **Frozen before any Phase 13 scoring**, and before any CommitRift command
  was run on the application. `fixtures/phase13/FROZEN.json` records the
  SHA-256 of every application file and of the labels (manifest digest
  `0a00594ded0093858848d6d0c8d593f0bc35a6bb0bef873b3432f61999e58e26`, date
  2026-10-27).
* **Author: an AI assistant (Claude)**, working apart from the Phase 13
  implementation. It read the Phase 12 documents and fixtures, but not the
  in-progress Phase 13 Rust changes. The labels come from PostgreSQL
  semantics (READ COMMITTED unless an entry says otherwise). **No independent
  human review performed.** The author knew the Phase 12 supported subset and
  limitations (for example, return values and a response event were not
  modeled), so this is a fresh set but not a blind one.
* Labels are never edited. A correction goes into the labels file's
  `corrections` array, with a reason. The PostgreSQL lock matrix
  (`LOCK-DIFFERENTIAL.md` §6) ran after the freeze and agrees with every
  label it bears on: **no corrections**.

## Format

The format is the Phase 12 label format (`schemaVersion` 1, with `entry`,
`class`, `label` positive/negative/unsupported, `effectful`, `tables`,
`property`, `environment` and `why`), plus four fields:

* `id` (E01–E27): the same entry can be labeled in two environments (E01 and
  E03).
* `pairedWith`: the label concerns the entry running concurrently with this
  other entry, not with itself (E03, E07, E08).
* `observable`: the property is about what callers are told (the return
  value or the response), not about table contents (E09–E11, E16–E18, E21,
  E23). In E09, E11 and E23 the table ends in a state that looks legal, and
  only the return values reveal the violation.
* `replicas`: the routing of the `withReplicas` handle that the label depends
  on (E16–E19). Under it, `select` reads the replica; `insert`, `update`,
  `transaction` and `$primary` go to the primary. The replica is an
  asynchronous streaming standby that may lag.

`expectedSchema` lists 16 tables, the generated keys, the 12 foreign keys
(all NO ACTION and not deferrable), the unique keys and the replica handle.
`eval-app/commitrift-contracts.json` uses only the Phase 11/12 contract
vocabulary: `retry: on-crash` for checkout, `delivery: at-least-once` for
the welcome worker, and four `atMostOne` business rules. Properties outside
that vocabulary (deadlock freedom, return values, read-your-writes) are
stated in the labels only. A scorer that needs them as declarations must
add them in a separate, post-freeze file and report that it did.

## The entries

15 positive, 11 negative, 1 unsupported.

| id | entry | class | label | property / environment (short) |
| --- | --- | --- | --- | --- |
| E01 | services/teams.ts#createTeam | check-then-act | positive | one team per owner; two concurrent calls |
| E02 | services/teams.ts#createTeamLocked | check-then-act | negative | same, owner row FOR UPDATE, RC (positive at RR) |
| E03 | createTeam ∥ createTeamLocked | check-then-act | positive | Phase 12 FK divergence pattern: the INSERT may wait behind FOR UPDATE, then duplicates |
| E04 | services/teams.ts#createTeamShared | check-then-act | positive | owner row FOR SHARE does not serialize |
| E05 | services/teams.ts#addMember | lock-cycle | positive | insert member (FOR KEY SHARE on team), then team FOR UPDATE: 40P01 |
| E06 | services/teams.ts#addMemberCounted | lock-cycle | negative | insert member, then `member_count + 1` (FOR NO KEY UPDATE): no cycle, RC |
| E07 | posts.ts#addComment ∥ deletePost | check-then-act | negative | no orphan comment: FK, the loser gets 23503 |
| E08 | posts.ts#addBookmark ∥ deletePost | check-then-act | positive | orphan bookmark: no FK |
| E09 | services/seats.ts#holdSeat | status-transition | positive | at most one caller told true (observable) |
| E10 | services/seats.ts#holdSeatAtomic | status-transition | negative | `UPDATE … WHERE holder IS NULL RETURNING` |
| E11 | services/seats.ts#holdSeatDirect | status-transition | positive | unconditional `UPDATE … RETURNING`: both told true |
| E12 | services/orders.ts#placeOrder | resubmitted-write | positive | single INSERT; crash after commit, before the response; client resubmits |
| E13 | services/orders.ts#placeOrderIdempotent | resubmitted-write | negative | client checkout id as PK, ON CONFLICT DO NOTHING |
| E14 | services/orders.ts#settleOrder | ambiguous-commit | positive | COMMIT acknowledgment lost; `withRetry` reruns: second payment |
| E15 | services/orders.ts#settleOrderOnce | ambiguous-commit | negative | the pending→paid conditional UPDATE gates the payment |
| E16 | services/profiles.ts#renameAndRead | stale-read | positive | read-your-writes through the replica (observable) |
| E17 | services/profiles.ts#renameAndReadPrimary | stale-read | negative | read through `$primary` |
| E18 | services/profiles.ts#renameInTransaction | stale-read | negative | `replicatedDb.transaction` runs on the primary |
| E19 | services/labels.ts#addLabel | stale-read | positive | check on the replica, duplicate even for SEQUENTIAL calls |
| E20 | api/teams.ts#createTeamRoute/arg[0] | check-then-act | positive | `withAuth` callback, zod input, repository check-then-insert |
| E21 | api/invites.ts#inviteRoute/arg[0] | check-then-act | negative | `withAuth`, zod-normalized email, UNIQUE + ON CONFLICT DO NOTHING |
| E22 | services/counters.ts#recordView | read-modify-write | negative | `ON CONFLICT DO UPDATE SET hits = hits + 1` |
| E23 | services/counters.ts#reserveHandle | status-transition | positive | `ON CONFLICT DO UPDATE … RETURNING`: both told reserved (observable) |
| E24 | workers/welcome.ts#handleWelcomeJob | duplicate-processing | positive | at-least-once delivery, no dedup |
| E25 | workers/welcome.ts#handleWelcomeJobOnce | duplicate-processing | negative | processed_jobs key in the same transaction |
| E26 | services/folders.ts#createFolder | check-then-act | positive | UNIQUE(owner, parent, name) with parent NULL (NULLS DISTINCT) |
| E27 | services/admin.ts#runMaintenance | unknown | unsupported | `db.execute(sql.raw(statement))` |

Classes: check-then-act 9, status-transition 4, stale-read 4, lock-cycle 2,
resubmitted-write 2, ambiguous-commit 2, duplicate-processing 2,
read-modify-write 1, unknown 1. The new families are lock-cycle, stale-read
and duplicate-processing. They are the label author's names, not
identifiers taken from the implementation.

## Caveats a scorer must keep

* E02 and E06 are negative only at READ COMMITTED, the labeled environment.
  The matrix shows E02 duplicating at REPEATABLE READ, and E06's second
  UPDATE failing with 40001 there.
* E07 is negative for "no orphan"; one of the two calls still fails with
  23503. E15 is negative for "one payment"; its retried call returns false
  although the order was paid. Neither of these other properties is labeled.
* E16–E19 assume a lagging replica. With a synchronous standby
  (`remote_apply`) they would be negative.
* Since Phase 12 did not model return values or a response event, the
  observable entries (E09, E11, E16, E23) and E12 test exactly those gaps.

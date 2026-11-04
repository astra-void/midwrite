# Phase 16 — `UNIQUE ... NULLS NOT DISTINCT`

Track D. Before Phase 16 the model treated NULLs as distinct in every
unique key, and the scenario loader dropped a `NULLS NOT DISTINCT` rule over
a nullable column entirely ("not modeled"), so Phase 15 had to downgrade any
counterexample on such a table to INCONCLUSIVE. The model now enforces the
rule wherever a unique key matters, the lockstep oracle creates it, and the
downgrade applies only to a rule the model still cannot enforce.

## Facts

### PostgreSQL

* `UNIQUE NULLS NOT DISTINCT (cols)` (constraint or unique index, PostgreSQL
  15+): for this rule a NULL key value equals another NULL. Two keys are
  equal when every column is equal, NULL to NULL included. The default,
  `NULLS DISTINCT`, never finds a conflict for a key that has a NULL.
* The rule applies wherever PostgreSQL checks uniqueness: single-row and
  multi-row `INSERT` (a duplicate inside one statement fails), `UPDATE`,
  `ON CONFLICT`. An insert that meets an uncommitted equal key waits for
  that transaction and then fails with 23505 (it committed) or goes on (it
  aborted). This is the same wait as for a non-NULL key.
* `ON CONFLICT (target)`: PostgreSQL uses as arbiters every unique index
  whose key columns are exactly the target's, in any order. Whether the
  index is `NULLS NOT DISTINCT` does not affect which indexes are chosen.
  It does decide whether a NULL key can match: with a `NULLS NOT DISTINCT`
  arbiter a NULL key finds the existing row, with a `NULLS DISTINCT`
  arbiter it never conflicts. On a match, `DO NOTHING` / `DO UPDATE` act as
  for any conflict. Under REPEATABLE READ / SERIALIZABLE, a matching row
  committed after the snapshot raises 40001. All of this was observed on
  PostgreSQL 17.11 (Evidence, below). PostgreSQL 15 and 16 were not
  available here and were not run. The source of
  `check_exclusion_or_unique_constraint` (`execIndexing.c`) skips the check
  for NULL input only when the index is not `NULLS NOT DISTINCT`
  (`ii_NullsNotDistinct`), which is consistent with what we observed. The
  oracle's DDL needs PostgreSQL ≥ 15. An older server rejects it, so the
  comparison never starts and cannot pass.

### Drizzle (pinned 0.45.4)

The fixtures' `drizzle-orm` is 0.45.4. We ran its builders under Node 22
(`getTableConfig`). The script is in the Evidence section.

| spelling | result in 0.45.4 |
| --- | --- |
| `unique("n").on(t.a, t.b).nullsNotDistinct()` | `UniqueConstraint { nullsNotDistinct: true }` |
| `unique("n").on(t.a, t.b)` | `nullsNotDistinct: false` |
| column `.unique("n", { nulls: "not distinct" })` | `isUnique: true, uniqueType: "not distinct"` |
| column `.unique()` | `uniqueType` absent (NULLS DISTINCT) |
| `uniqueIndex("n").on(t.a).nullsNotDistinct()` | **TypeError**: `uniqueIndex(...).on(...).nullsNotDistinct is not a function` |

The migration SQL for a constraint is
`CONSTRAINT "n" UNIQUE NULLS NOT DISTINCT("a","b")`
(`fixtures/phase15/worlds-app/drizzle/0000_init.sql`,
`fixtures/phase16/nulls/drizzle/0000_init.sql`).

The analyzer (`crates/commitrift-analyzer/src/schema.rs`) already read the
first three spellings. It also accepted `nullsNotDistinct()` on
`uniqueIndex()` / `index()`, a method that does not exist in 0.45.4. It now
records that chain as an unsupported schema declaration and does not
extract a rule from it. That is the only analyzer change.

## Representation

`TableSchema` (`crates/commitrift-model/src/db.rs`) gains

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub nulls_not_distinct: Vec<Vec<String>>,
```

This is the list of unique sets declared `NULLS NOT DISTINCT` over a
nullable column. It sits next to `unique`, which keeps its meaning (NULLS
DISTINCT sets). A table without such a rule serializes exactly as before.
The same set can appear in both lists, because PostgreSQL allows both
constraints. `TableSchema::key_rules()` returns every unique rule in
`key_sets()` order, as `KeyRule { name, columns, nulls_not_distinct }`:
first the primary key, then `unique`, then the NULLS NOT DISTINCT sets.
`KeyRule::checks(key)` says whether a key tuple is checked at all:
`nulls_not_distinct || no NULL in key`. Equality is model value equality
(`Value::Null == Value::Null`), the same as for every unique key.
`key_sets()` keeps its signature and now includes the NULLS NOT DISTINCT
sets. Its callers (foreign-key target check, key columns of an UPDATE's
lock mode, `DO NOTHING`'s visibility check) need no change. Default names
follow PostgreSQL: a repeated `<table>_<cols>_key` gets the next numeric
suffix.

A NULLS NOT DISTINCT rule over non-nullable columns behaves exactly like
NULLS DISTINCT. It stays in `unique`, as before.

### Loading

* From the extracted schema (`from`): a non-partial rule with
  `nullsNotDistinct` and a nullable column, all of whose columns are
  modeled, goes to `nulls_not_distinct` with the fact
  `unique <kind> \`<name>\` NULLS NOT DISTINCT on nullable (<cols>): NULLs
  are equal for this rule (PostgreSQL 15+); a second row with the same key,
  NULLs included, conflicts` (origin `extracted-from-source`; it was
  `not-modeled` before). A rule with an unmodeled column stays `not-modeled`
  (`... NULLS NOT DISTINCT: a column is not modeled`).
* A **partial** unique index (`.where(...)`) is unchanged: `not-modeled`,
  never enforced, whatever its NULL mode.
* Declared tables (no `from`): the scenario table accepts
  `"nullsNotDistinct": [["col", ...], ...]`. The columns are validated like
  `unique`'s. With `from`, a non-empty declared list must equal the
  source's, or the scenario is refused.
* `overrides.unique` replaces `unique` only. The NULLS NOT DISTINCT sets
  stay as the source declares them.
* Initial state: two rows whose keys are equal with NULLs under a NULLS NOT
  DISTINCT rule are refused (`duplicate key violates \`<name>\``), as
  PostgreSQL would refuse the seed.
* When a model has such a rule, every report lists this assumption:
  "UNIQUE NULLS NOT DISTINCT (PostgreSQL 15+): for t(cols) a NULL key value
  equals another NULL, …; non-NULL values compare by model value equality
  as for every unique key". The report's `database.tables[].unique` (JSON
  schema v5, `additionalProperties: false`) is not extended. The rule
  appears in the schema facts and in the assumptions.

## Semantics in the engine

All of these paths share one place, `exec.rs::unique_check`, plus the
arbiter lookup:

| path | before | Phase 16 |
| --- | --- | --- |
| single-row INSERT | NULL key skipped | NND rule checked with NULL = NULL: 23505 against a committed or own row; `Block` (wait) on another transaction's uncommitted equal key, re-checked when it ends |
| multi-row INSERT | per row, as single-row | same; a second NULL key in one statement meets the statement's own first row → 23505, `DO NOTHING` skips it |
| UPDATE | NULL key skipped | NND key checked, waits and 23505 as above |
| `ON CONFLICT DO NOTHING` | any violated key | the violated NND rule is found for NULL keys; RR/SER visibility check unchanged (40001 when the match committed after the snapshot) |
| `ON CONFLICT (t) DO UPDATE` | arbiter skipped NULL keys | `TableSchema::arbiter_nulls_not_distinct(t)`: if an NND rule has exactly the target's columns, NULL keys match (row → update / 21000; uncommitted → wait). Conflicts on non-arbiter rules stay 23505 |
| rollback | — | unchanged: an aborted inserter's version disappears and the waiter goes on |
| footprints / POR | `key_overlap` ignored keys with NULL | NND rules compare NULL keys: two inserts of equal NULL keys are dependent |

This is not a final-state check. Every statement checks the rule when it
runs, with the existing wait and abort machinery (`UCheck::Block`).

## Discovery (Phase 15 rule narrowed)

`crates/commitrift-discover/src/pipeline.rs::unenforced_unique` still
names a non-partial NULLS NOT DISTINCT rule over a nullable column, but
only when the model does not enforce it: when the candidate's table is not
modeled or a column of the rule is not modeled (the loader's own
condition). A counterexample on a table whose NND rules are all enforced
is now reported like any other. Partial unique indexes keep their Phase 14
INCONCLUSIVE rule unchanged. The remaining message says why: "which the
model does not enforce (a column of it is not modeled)".

### The one changed expectation: `createLabel`

`fixtures/phase15/worlds-app#createLabel` (UNIQUE(owner, parent, name)
NULLS NOT DISTINCT, check-then-insert with `onConflictDoNothing`):

* Phase 15: INCONCLUSIVE. The model did not load the rule at all, so it
  found a duplicate. On the original code with PostgreSQL that schedule
  was **not reproduced** 0/3 (`docs/phase15/POSTGRESQL-DIFFERENTIAL.md`):
  PostgreSQL rejected the duplicate.
* Phase 16: **NO_VIOLATION_WITHIN_BOUNDS**. Every world was checked to
  completion, the `null:parentId` world included. The lockstep
  differential below shows the model and PostgreSQL 17.11 agree on the
  rule. In addition, the model's race schedule for the `null:parentId`
  world (both SELECTs, then both INSERTs; `sourceVerification.recoveryProbe`
  with `require: [statement:createLabel-1, statement:createLabel-2,
  statement:createLabel-1, statement:createLabel-2]`) ran on the
  **original code**: `invariant-held`, evidence
  `original-code-invariant-held`, 3/3 runs `followed`. Every statement's
  outcome matched the model: the second `INSERT ... ON CONFLICT DO
  NOTHING RETURNING id` returned no row. PostgreSQL's table was the
  model's: one label `(1, NULL, name-1)`. The base world (`parentId = 2`)
  gave the same result, 3/3.

Updated tests (the only expectation changes):
`crates/commitrift-discover/tests/phase15.rs::worlds_app_finds_both_input_dependent_defects_and_keeps_the_safe_counterparts`
(INCONCLUSIVE → NO_VIOLATION_WITHIN_BOUNDS, with the null world complete),
`crates/commitrift-cli/tests/phase15.rs::counterexamples_found_in_other_worlds_come_with_a_reproduction_artifact`
(status; still no counterexample artifact), and
`crates/commitrift-cli/tests/phase15.rs::input_dependent_counterexamples_reproduce_on_the_original_code`
("not-reproduced" of a counterexample that no longer exists → the
original-code recovery probe above, `invariant-held`).

No corpus is affected. midday and saas-starter mention `nullsNotDistinct`
only in drizzle-kit snapshots, as `false`. The private project has no
`nullsNotDistinct()` / `nulls: "not distinct"` declaration and no
snapshot with `true`. `fixtures/phase15/eval-app#createPage` (P15-E07, NND)
produces no candidate (its select predicate is not extracted), before and
after.

## First-party slice

`fixtures/phase16/nulls` (Drizzle 0.45.4, node-postgres): check-then-insert
without `ON CONFLICT`, once per NULL mode and per spelling.

| entry | rule | status | deciding / checked world |
| --- | --- | --- | --- |
| `createTag` | `unique().on(owner, group, name)` NULLS DISTINCT | MODEL_COUNTEREXAMPLE (validated) | `null:groupId` |
| `createBadge` | the same `.nullsNotDistinct()` | NO_VIOLATION_WITHIN_BOUNDS | `null:groupId` complete; 6 schedules, 4 end with an actor failing 23505 |
| `registerDevice` | column `.unique("n")` NULLS DISTINCT | MODEL_COUNTEREXAMPLE (validated) | `null:serialNo` |
| `registerScanner` | column `.unique("n", { nulls: "not distinct" })` | NO_VIOLATION_WITHIN_BOUNDS | `null:serialNo` complete; 6 schedules, 4 end with 23505 |

## Evidence

### Lockstep differential (PostgreSQL 17.11)

`crates/commitrift-model/tests/phase16_nulls.rs`:

```
COMMITRIFT_REQUIRE_REPLAY=1 cargo test -p commitrift-model --test phase16_nulls -- --nocapture
```

There are 15 workloads × {NULLS DISTINCT, NULLS NOT DISTINCT} × {READ
COMMITTED, REPEATABLE READ, SERIALIZABLE}, giving **90 configurations**.
**Every** schedule of each model was replayed in lockstep against a
disposable cluster, **1542 schedule replays** in all (882 NULLS DISTINCT,
660 NULLS NOT DISTINCT), with **0 disagreements**. Each replay compared
statement results, SQLSTATEs, which backend waits for which (via
`pg_blocking_pids`) and the final tables. 291 schedules ended in 23505
under NULLS NOT DISTINCT. None did on a NULL-key workload under NULLS
DISTINCT.

DDL (table `items`: `id text` PK, nullable `owner, k, p, q bigint`,
`name text`, `v bigint`):

```sql
CREATE TABLE "items" (..., PRIMARY KEY ("id"), UNIQUE ("k"));
CREATE TABLE "items" (..., PRIMARY KEY ("id"), UNIQUE NULLS NOT DISTINCT ("k"));
-- likewise ("owner", "p", "name") and ("p", "q")
```

Statements (each in `BEGIN ISOLATION LEVEL <iso>; ... COMMIT;`, or
`ROLLBACK` for the rolling-back actor):

```sql
INSERT INTO "items" ("id","k","name","owner","p","q") VALUES ('a', NULL, 'x', NULL, NULL, NULL) RETURNING 1
INSERT INTO "items" (...) VALUES ('a', NULL, 'x', 1, NULL, NULL) ...          -- composite, one NULL
INSERT INTO "items" (...) VALUES ('a', NULL, 'x', NULL, NULL, NULL), ('a2', NULL, DEFAULT, DEFAULT, DEFAULT, DEFAULT) [ON CONFLICT DO NOTHING] ...
INSERT INTO "items" (...) VALUES (...) ON CONFLICT DO NOTHING ...
INSERT INTO "items" ("id","k","v") VALUES ('a', NULL, 1) ON CONFLICT ("k") DO UPDATE SET "v" = ("items"."v" + 1) ...
UPDATE "items" SET "k" = NULL WHERE ("id" = 'a') ...
```

Schedules per isolation level (RC/RR/SER) and the outcomes observed, all
equal to the model's:

| workload | NULLS DISTINCT | NULLS NOT DISTINCT |
| --- | --- | --- |
| single nullable column, NULL vs NULL | 20/20/20: both insert | 14/14/14: second waits, then 23505 (first committed) |
| distinct non-NULL values | 20/20/20: both insert | 20/20/20: both insert |
| the same non-NULL value | 14/14/14: wait, 23505 | 14/14/14: wait, 23505 |
| committed NULL row, then INSERT and INSERT … DO NOTHING of NULL | 20/20/20: both insert | 10/10/10: INSERT 23505 at once, DO NOTHING 0 rows (no wait) |
| composite, one NULL column, same owner/name | 20/20/20: both insert | 14/14/14: wait, 23505 |
| composite, one NULL column, other owner | 20/20/20 | 20/20/20: both insert |
| composite, both columns NULL | 20/20/20: both insert | 14/14/14: wait, 23505 |
| composite, NULL in different columns | 20/20/20 | 20/20/20: both insert |
| first inserter rolls back | 20/20/20: both insert | 14/14/14: waiter proceeds after the rollback; 23505 when the committing actor went first |
| `ON CONFLICT DO NOTHING`, both NULL | 20/20/20: both insert | 14/14/14: wait, then 0 rows (RC); 40001 under RR/SER |
| `ON CONFLICT (k) DO UPDATE`, both NULL | 20/20/20: both insert | 14/14/14: wait, then the update (RC); 40001 under RR/SER |
| `DO UPDATE` against a committed NULL row | 20/20/20: both insert | 14/14/14: each updates the existing row; wait on the other's update; 40001 under RR/SER |
| UPDATE two rows' key to NULL | 20/20/20: both update | 14/14/14: wait, 23505 |
| multi-row INSERT with two NULL keys + concurrent NULL insert | 20/20/20: 2 rows + 1 row | 10/10/10: the multi-row statement fails 23505 (own duplicate), or waits first |
| the same with `DO NOTHING` | 20/20/20 | 14/14/14: 1 row of 2 (0 when the other actor's NULL row committed first); wait / 40001 under RR/SER as above |

### Model-only tests (same file, no server)

Under NULLS DISTINCT, two concurrent NULL inserts (plain and `DO
NOTHING`) produce a counterexample to "at most one NULL key" at all three
levels. Under NULLS NOT DISTINCT there is none. The rule appears in
`key_rules`, in `arbiter_nulls_not_distinct` and in the DDL. The table's
JSON has no `nullsNotDistinct` key without the rule. An initial state with
two NULL keys is accepted only under NULLS DISTINCT. A declared rule over
an unknown column is refused.

On all 90 models, sleep sets, classic DPOR and source-set DPOR (with and
without the cache) reach the same terminal states and verdicts as the
unreduced search, and `audit_independence` finds no unsound independence.
Mutation check: with `key_overlap` reverted to skip NULL keys under NULLS
NOT DISTINCT too, `audit_independence` reports a violation on the first NND
workload it checks (`rollback-then-insert/nulls-not-distinct/read-committed`),
so the test detects a missing NULL-key dependency.

### Discovery and original code

```
cargo test -p commitrift-discover --test phase16_nulls --test phase15
COMMITRIFT_REQUIRE_REPLAY=1 cargo test -p commitrift-cli --test phase15
```

The CLI test builds `fixtures/phase15/worlds-app` and runs the
`createLabel` null-world recovery probe described above on the original
code (`--verify-source --allow-execution`). The 3-run figures above were
produced with `--source-repeat 3` by hand.

### Drizzle probe

A Node 22 script imported `drizzle-orm/pg-core` from the fixtures'
`node_modules` (0.45.4). It built one table with every spelling above and
printed `getTableConfig(t).uniqueConstraints`, the columns'
`isUnique`/`uniqueName`/`uniqueType`, and the TypeError of
`uniqueIndex().on().nullsNotDistinct()`. The results are the ones in the
table.

## Approximations and limitations

* Verified on PostgreSQL 17.11 (this track) and, at integration, on 15.19
  in the Linux container (the same test with replay required). PostgreSQL
  16 was not run.
  PostgreSQL ≤ 14 has no NULLS NOT DISTINCT; there the application's
  migration fails, and so does the oracle's DDL.
* Non-NULL key values compare by model value equality, as for every unique
  key. Collations and type-specific equality are not modeled; this is
  unchanged.
* Partial unique indexes (with or without NULLS NOT DISTINCT) remain
  unmodeled, and counterexamples on their tables remain INCONCLUSIVE.
* An NND rule with an unmodeled column is not enforced. Discovery keeps
  the INCONCLUSIVE downgrade for it.
* (Resolved at integration.) Discovery's world synthesizer
  (`crates/commitrift-discover/src/synth.rs`, `add`) skipped NULL keys
  when keeping generated rows unique. It now skips a key with a NULL only
  when the key set is not NULLS NOT DISTINCT
  (`crate::detect::nulls_not_distinct_sets`), so it never builds two rows
  with equal NULL keys for an NND table. Had it done so, the loader would
  have refused that world ("duplicate key violates") and reported it as
  unsatisfied, never checked it with an impossible state.
* The model-check report's `database.tables[].unique` does not list NND
  sets (the v5 report schema forbids new keys there). The rule is reported
  in the schema facts and the assumptions.

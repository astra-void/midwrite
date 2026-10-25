# Drizzle extraction and the database model

## Four layers, kept apart

| Layer | Where | Checked by |
| --- | --- | --- |
| 1. source-level query construction | extractor (`select`, `mutation`, `find_first`) | analyzer tests |
| 2. abstract query semantics | model `StmtKind`/`ClosedStmt`, `exec.rs` | model tests, reduction tests |
| 3. SQL a backend runs | oracle `stmt_sql` (model SQL) / **Drizzle's own SQL** observed by the coordinator | PostgreSQL differential; source verification |
| 4. what PostgreSQL did | oracle observations / coordinator observations | same |

A statement is compared on results, not only on final rows: a different
returned row can change later control flow.

## Newly supported

| Construct | Model | Notes |
| --- | --- | --- |
| `.innerJoin(t, eq(a.x, t.y))`, `.leftJoin(...)` | `Select { joins }` with qualified columns `table.column` | equality of two columns only; several joins; a table at most once (no aliases); no row locks; result rows nested by table name (`{ users: {...}, team_members: null }`) as Drizzle returns them |
| `select({ user: users, teamId: teamMembers.teamId })` | `projection` | fields are whole tables (nested, `null` for an unmatched left join) or columns; computed fields are `Unknown` (the Phase 6 behavior "projection ignored, full row read" is gone) |
| `const rows = await …limit(1)` | row array of ≤ 1 row | `rows.length`, `rows[0]` exact, `rows[n>0]` is `undefined`; an array is truthy |
| `insert(...).values(...).returning()` | `Insert { returning }` | the inserted row (generated and default values included) or `null` for `ON CONFLICT DO NOTHING`; `returning({...})` is `Unknown` |
| omitted columns | `ColumnDefault::Value` / `ColumnDefault::Sequence` | constants; sequences (`serial`, identity) |
| `values(newActivity)` with `const newActivity = {...}` | object binding | properties evaluated at the declaration |
| `ActivityType.INVITE_TEAM_MEMBER` | literal | TypeScript `enum` (string and numeric) or `const` object of literals, local or imported from an analyzed file |

## Generated values

A sequence column takes the next value of its sequence when an insert
omits it. As in PostgreSQL, allocation happens before constraint checks and
is **not transactional**: a rolled-back or conflicting insert consumes its
value (`sequence_values_are_not_given_back_by_a_rollback`). Values are
concrete only because the disposable database starts the sequence at a
known value (declared `start`, else one more than the largest initial
value); the coordinator sets the application database's sequences to the
same value (`setval`), and the report states this assumption. Allocations
on the same sequence are dependent transitions for partial-order reduction
(`Access::Seq`).

## Footprints and SERIALIZABLE

A join reads every participating table as a whole: its footprint conflicts
with any write to them (conservative, sound for reduction), and under
SERIALIZABLE it records whole-table reads — which can make the model abort
more than PostgreSQL with index scans. SERIALIZABLE + join is therefore
validated only on the small workloads below; the existing one-way caveat
([`../phase6/LIMITATIONS.md`](../phase6/LIMITATIONS.md)) applies.

## Validation

* PostgreSQL differential (`crates/commitrift-model/tests/phase7.rs`): every
  schedule of left join vs concurrent membership, projected join, sequence +
  rollback, unique constraint + generated ids + `ON CONFLICT DO NOTHING …
  RETURNING` + rollback after `RETURNING`, inner join vs delete — each under
  READ COMMITTED, REPEATABLE READ and SERIALIZABLE: **102 schedules, 15
  configurations, 0 disagreements** on PostgreSQL 17.11. Reduction keeps
  every reachable state on these workloads.
* Real Drizzle 0.45.4 SQL: the source-verification runs compare the model's
  join/projection/insert results cell by cell with the rows Drizzle's own
  statements returned (`table.column` keys from PostgreSQL's column
  provenance) — [`DIFFERENTIAL-VERIFICATION.md`](DIFFERENTIAL-VERIFICATION.md).

## Schema facts

The scenario declares tables; `sqlName` maps a model column (Drizzle
property `teamId`) to the application's column (`team_id`), and the
scenario's table name is the SQL table name. Schema facts are **not yet
derived from Drizzle `pgTable` definitions**; unknown constraints are not
assumed. Columns a scenario does not model (timestamps with `defaultNow()`)
keep their database defaults at runtime and are not compared; reading or
writing them in extracted code is `Unknown`.

## Not supported

`ON CONFLICT DO UPDATE`, aggregates other than `count`, `orderBy`
semantics (primary-key order is assumed and reported), `limit` other than
1 for bound arrays, multi-row `values`, subqueries, raw `sql` beyond
arithmetic in `set`, `RIGHT`/`FULL`/`CROSS` joins, triggers, foreign-key
checks in the model (the application database enforces them; the seed must
satisfy them), `undefined` vs `null` in inserted values (Drizzle uses the
column default for `undefined`; the model has one `null`).

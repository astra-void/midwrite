# PostgreSQL model

`crates/commitrift-model/src/{db,exec,state,value}.rs`. The model is a
small, explicitly bounded subset of PostgreSQL. **It does not model every
PostgreSQL feature**; what it models was checked against PostgreSQL 17.11
(macOS) and 15.19 (Linux container) by lockstep replay, and since Phase 13
also against an independently produced ground truth.

## Modeled

| area | behavior | since | document |
| --- | --- | --- | --- |
| storage | multi-version rows, snapshots, visibility of committed vs uncommitted versions | 6 | [Phase 6 database model](../phase6/DATABASE-MODEL.md) |
| statements | point/predicate `SELECT … LIMIT 1` (primary-key order, an assumption the report lists), `count(*)`, `UPDATE`, single-row `INSERT`, `DELETE`; inner/left joins with column equality and projections (7); `RETURNING` (7, 12); `ON CONFLICT DO NOTHING` (6); `ON CONFLICT (target) DO UPDATE SET … WHERE …` with `excluded` (13) | 6–13 | [Phase 7 Drizzle extraction](../phase7/DRIZZLE-EXTRACTION.md), [Phase 13 advanced Drizzle](../phase13/ADVANCED-DRIZZLE.md) |
| READ COMMITTED, REPEATABLE READ | PostgreSQL snapshot rules, EvalPlanQual re-evaluation on the newest version, unique-key waits, 40001 at REPEATABLE READ | 6 | [Phase 6 PostgreSQL isolation](../phase6/POSTGRES-ISOLATION.md) |
| SERIALIZABLE | a conservative serialization-graph abstraction (every history whose logical serialization graph is acyclic), **not SSI** | 6 | same |
| constraints | primary keys, unique sets (no partial or expression indexes), NOT NULL, simple CHECK | 6 | [Phase 6 limitations](../phase6/LIMITATIONS.md#sql-and-database-subset) |
| row locks | `FOR UPDATE`, `FOR NO KEY UPDATE`, `FOR SHARE`, `FOR KEY SHARE` with PostgreSQL's compatibility table; implicit modes of `UPDATE` (key vs non-key change, by value) and `DELETE`; holders in acquisition order, waiting on the first conflicting holder (multixact order); `SKIP LOCKED`/`NOWAIT` with `FOR UPDATE` | 6; modes 13 | [Phase 13 FK locks § 1](../phase13/POSTGRESQL-FK-LOCKS.md#1-lock-modes) |
| foreign keys | `RI_FKey_check` on the referencing side (snapshot rules per level, `FOR KEY SHARE` on the parent, wait, 23503/40001), `NO ACTION`/`RESTRICT` on the referenced side, immediate constraints, `MATCH SIMPLE` — with the scenario feature `foreign-keys` | 13 | [Phase 13 FK locks § 2–3](../phase13/POSTGRESQL-FK-LOCKS.md#2-referencing-side-ri_fkey_check-insert-update-of-the-foreign-key) |
| deadlocks | detected on the wait-for graph; any member of a cycle may be the victim (over-approximation of PostgreSQL's timer race) | 6 | [Phase 6 PostgreSQL isolation](../phase6/POSTGRES-ISOLATION.md) |
| sequences, defaults | non-transactional sequence allocation (`serial`, identity), constant defaults; values concrete relative to a declared start | 7 | [Phase 7 Drizzle extraction](../phase7/DRIZZLE-EXTRACTION.md#generated-values) |
| generated values | application- and database-generated keys (`gen_random_uuid()`, `$defaultFn` with recognized generators) as values whose identity is the generation event; assumption `generated-values-distinct` (no collision mode) | 12 | [Phase 12 generated values](../phase12/GENERATED-VALUE-SEMANTICS.md) |
| read replica | one asynchronous hot standby replaying the primary's commits in order with unbounded lag; replica reads take no locks and never wait — with the scenario feature `replicas`; **model only** | 13 | [Phase 13 replica consistency](../phase13/REPLICA-CONSISTENCY.md) |
| in-flight operations | a COMMIT or autocommit write whose acknowledgment is lost may be completed by the server later (`ServerComplete`) | 9 | [Phase 9 ambiguous commit](../phase9/AMBIGUOUS-COMMIT.md) |
| connection pools | bounded FIFO leases | 9 | [Phase 9 connection pool](../phase9/CONNECTION-POOL.md) |

Statements run atomically unless they wait. A conflicting statement that
waits is resumed when what it waits for ends, and re-checks.

## Isolation

* READ COMMITTED and REPEATABLE READ: validated for the Phase 6 matrix
  (1 695 schedules, 0 disagreements, on 17.11 and 15.19) and every later
  matrix ([evidence index § 4](../research/evidence-index.md#4-postgresql-differential-fidelity)).
* SERIALIZABLE: PostgreSQL may abort more than the model (measured: 60 of
  70 schedules of a sequential-scan workload were aborted by PostgreSQL and
  allowed by the model, never the reverse). A SERIALIZABLE counterexample
  may therefore not be realizable; a SERIALIZABLE "no counterexample" holds
  for the model's histories, which include PostgreSQL's — argued and
  consistent with every measurement, not proven. Mixed isolation levels in
  one scenario were not differentially tested.

## Refused or not modeled

Refused (the scenario does not load): `ON DELETE`/`ON UPDATE` `CASCADE`,
`SET NULL`, `SET DEFAULT` when a statement would run them; `DEFERRABLE
INITIALLY DEFERRED` foreign keys when the referencing table is written;
replica reads without the `replicas` feature; writes or row locks on a
replica.

Not modeled: subqueries, aggregates other than `count`, `ORDER BY` other
than the key, `LIMIT` other than 1, multi-row `INSERT … VALUES`, row values
of `UPDATE`/`DELETE … RETURNING`, partial-index upsert arbiters
(`targetWhere`), `SET CONSTRAINTS`, `MATCH FULL`, triggers, row-level
security, database functions, `NOW()`, savepoints and nested transactions,
`lock_timeout`/`statement_timeout`, advisory locks, `LISTEN`/`NOTIFY`,
timestamp/JSON/array column values, decimals, string concatenation,
several replicas with different lags, synchronous replication, hot-standby
query cancellation, server crashes that lose committed data. Complete,
deduplicated list: [limitations § unsupported PostgreSQL behavior](../reference/limitations.md#unsupported-postgresql-behavior).

## The lockstep oracle

`model-check --validate-postgres` (and the `pg_matrix` tests) start a
private cluster (Unix socket only), open one session per actor, and replay
a model schedule statement by statement: blocking is observed from the
server (`pg_stat_activity`, `pg_blocking_pids`), crashes are realized by
killing sessions, deadlock victims are steered by timing relative to
`deadlock_timeout` (the only timing-based coordination). The oracle sends
the **model's** SQL; it does not run the application or Drizzle. It refuses
replica models (one server). [Phase 6 PostgreSQL validation](../phase6/POSTGRES-VALIDATION.md).

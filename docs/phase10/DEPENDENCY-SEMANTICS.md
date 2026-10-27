# Dependency semantics: footprints and conflicts

Source: `crates/commitrift-model/src/footprint.rs` (relation) and
`engine.rs` (where each access is recorded). Phase 10 changed no conflict
rule; it added a test-only way to delete a kind of access
(`footprint::mutation`, feature `mutation-testing`) and an audit
(`audit::audit`) that checks the relation against the transition system.

## Access kinds

The conceptual categories of the Phase 10 brief map onto the existing kinds
as follows. A kind is *conservative* when it can only make more pairs
dependent.

| category | access | conflicts with |
| --- | --- | --- |
| process, task | `Actor(a)` | `Actor(a)`: all transitions of one actor (shared locals, one connection per transaction; **tasks of one actor are therefore dependent here, while program order is tracked per task**) |
| database row | `RowRead(row)`, `RowWrite{row, old, new}` | read/write and write/write on the same row; two writes also conflict when an old or new image shares a non-null value of a unique or primary key (a unique-constraint race), even for different rows |
| database predicate | `Pred{table, pred}` | a `RowWrite` of the same table whose old or new image matches the predicate (an insert that would appear in a scan, a delete that would vanish); an evaluation error counts as a match |
| transaction snapshot | `Snapshot` | `CommitWrite` (a snapshot that outlives the statement reads the commit stream) |
| transaction commit | `CommitWrite` | `Snapshot` |
| lock | `LockCheck(row)`, `LockChange(row)` | change/change and change/check on the same row |
| wait-for graph | `Wait` | `Wait` (blocking, deadlock victim choice) |
| SERIALIZABLE bookkeeping | `Ser` | `Ser` |
| connection pool, lease | `Pool(process)` | `Pool(process)`: arrival, release and the claim of a handed-over connection are mutually dependent (who gets the connection) |
| sequence | `Seq(i)` | `Seq(i)` (the id an insert gets depends on allocation order; sequences are not transactional) |
| external effect | `Ext(channel)` | `Ext(channel)` of the same channel |
| in-process variable | `App(process, name)` | the same variable |
| fault budget | `Budget` | `Budget` (taking an in-flight fault can disable another) |
| process, retry, promise, pending server operation | `Global` for crashes, retries, deadlock victims, unsupported behavior, a blocked step; the actor plus the rows it commits for `ServerComplete`; the actor plus `Budget` for a lost acknowledgment | `Global` conflicts with everything |
| unknown | `Global` | everything |

Not separate kinds: *retry budget* (`Retry` is `Global`), *promise settlement*
(tasks of an actor are dependent through `Actor`), *queue redelivery* (a
delivery is a `Retry` or another actor's start, `Global` or `Actor`),
*knowledge observation* (carried by the actor's state: `Actor`, plus
`ServerComplete` for what a recovery read can see).

## Read/write distinctions and PostgreSQL

* Read/read does not conflict. A row read conflicts only with writes of the
  same row; a predicate read conflicts with exactly the writes that could
  change its result, which is how an insert that matches `WHERE status =
  'pending'` is dependent with a scan that found no such row, even though the
  scan names no row.
* Visibility is not part of the relation: a snapshot read is `Snapshot`, and
  isolation levels change *which* accesses a statement records (READ
  COMMITTED re-reads the latest committed version, REPEATABLE READ and
  SERIALIZABLE use the transaction snapshot, SERIALIZABLE adds `Ser` and
  predicate reads of the transaction), not how two accesses conflict. The
  isolation-specific behavior is the engine's (`docs/phase6/`); the relation
  only has to be a superset of it, which the audit checks on workloads at all
  three levels.
* Unknown aliasing and unsupported SQL stay dependent: a statement the model
  cannot evaluate is recorded as `Global`.

## What is checked, and how

| property | check |
| --- | --- |
| independent pairs commute and stay enabled | `audit::audit`, `tests/por_audit.rs` (clean on the corpus) |
| footprints do not drift under commutation | same (`footprint_drift` is 0) |
| enabling pairs conflict | same; the search additionally supplies the dependency itself (`ENABLING-RELATIONS.md`) |
| every kind of access is needed by some workload | `tests/por_mutation.rs`: kinds whose deletion no workload notices are listed, not hidden |
| reductions are sound | `tests/sdpor.rs`, `tests/por_adversarial.rs`, `tests/por_regressions.rs` against the unreduced search |

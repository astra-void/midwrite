# Phase 13: replica-aware handle provenance (analyzer)

Analyzer side of Track C (written as a draft in the analyzer worktree,
finalized at the Phase 13 closeout: see §4 for what was merged). The model side of read replicas (snapshots,
replication progress, the scenario `features` flag) is documented with the
model checker's changes; this page covers what the extractor emits and why.

## 1. drizzle-orm `withReplicas` routing (verified)

Read from `drizzle-orm/pg-core/db.js` of **drizzle-orm 0.45.4**, the only
version installed in the repository's fixtures (10 installs; the
`withReplicas` function text is identical in all of them), lines 293–326,
and its declaration in `pg-core/db.d.ts`:

| member of `withReplicas(primary, replicas, getReplica?)` | goes to |
| --- | --- |
| `select`, `selectDistinct`, `selectDistinctOn`, `$count`, `with`, `$with` | `getReplica(replicas)` (default: a uniformly random replica, chosen per call) |
| `query` (a **getter**: `getReplica(replicas).query`, chosen per access) | a replica |
| `insert`, `update`, `delete`, `execute`, `transaction`, `refreshMaterializedView` | `primary` |
| `$primary` | `primary` itself |
| `$replicas` | the replica array |
| anything else | `...primary` (own enumerable properties of the primary object) |

The type is `PgWithReplicas<Q> = Q & { $primary: Q; $replicas: Q[] }`, so a
replicated database is **assignable to its primary's type** — a parameter
typed `NodePgDatabase<...>` may hold one. drizzle-orm has no
`usePrimaryOnly`. A row lock (`.for("update")`) through `select` runs on a
replica; PostgreSQL rejects row locks on a hot standby.

midday (51587319) uses its **own** `withReplicas`
(`packages/db/src/replicas.ts`): the same routing through getters, plus
`usePrimaryOnly()` (a database whose reads also use the primary), `$primary`,
`executeOnReplica` and `transactionOnReplica` (a transaction on the
**replica**), no `$replicas`, and `getReplica = (replicas) => replicas[0]!`.

## 2. Handle kinds and provenance (`crates/commitrift-analyzer/src/handles.rs`)

| kind | binding | how it is known |
| --- | --- | --- |
| primary | `Binding::Db` | `drizzle(...)` imported from `drizzle-orm/<driver>`; `$primary` / `usePrimaryOnly()` of a replicated database; a declared drizzle `...Database` type (Phase 12) in a project that builds no replicated database |
| replicated | `Binding::Replicated(Routing)` | `withReplicas` imported from `drizzle-orm/pg-core` whose primary argument is a recognized primary (built-in, verified routing); a call of a **declared** replica factory (`ReplicaFactoryBinding`, `module#export`, resolved through imports and re-exports to its declaration) whose `primary_argument` is a recognized primary; the type `PgWithReplicas` of `drizzle-orm/pg-core` |
| ambiguous (primary or replicated) | `Routing { ambiguous: true }` | a handle known only by a plain drizzle type in a project that builds a replicated database |
| transaction | `Binding::Tx` | the transaction callback's parameter (unchanged) |
| unknown | `Binding::UnknownDb(why)` or `param_types` | everything else, with the reason |

Provenance follows, without executing anything: module-level `const`s
(through imports and re-exports), `await`, conditional expressions whose
branches agree, calls of project functions whose declared return type is
known or whose body is exactly `return <expr>` (`connectDb = async () => {
return db; }`; a body doing anything before the return is refused, since
replacing the call by its result would skip those effects), function-local
`const`s, and types: `Awaited<...>`, `Promise<...>`, `ReturnType<typeof
f>`, `typeof x`, non-generic type aliases of the module or of an imported
project module (following `export { } from` and `export *`), and `T & {
...type literal }`. Parameters are never substituted, so a wrapper that
returns its argument stays unknown. A union (`Database | Transaction`) stays
unknown as an entry parameter (Phase 12 behavior); such a helper inlined with
a known handle uses that handle.

"The project builds a replicated database" means: a replica factory is
declared, or some module imports a binding named `withReplicas` from any
specifier (one scan of the project's files, text first). Then a configured
receiver name (`db`) whose value is not recognized is **not** the primary
(`UnknownDb`; Phase 7–12 bound it by name), and a plain drizzle type is
ambiguous. Projects without a `withReplicas` keep the Phase 12 bindings
(test `projects_without_replicas_keep_phase_12_bindings`).

A value is never taken to be replicated or primary because of a name: a
`withReplicas` from another package, from another drizzle dialect, or a
module-level function that shadows the name, is unknown (tests
`look_alike_shadowed_and_wrapped_factories_are_neither_replicated_nor_primary`,
`with_replicas_of_another_drizzle_dialect_is_not_the_pg_core_contract`).

## 3. Lowering through a replicated database (`concurrency.rs`, `database`/`route`)

| operation | emitted |
| --- | --- |
| `insert` / `update` / `delete` (and `execute`, `refreshMaterializedView`, unsupported as before) | `Conn::Pool` (primary autocommit) |
| `transaction(cb)` | a primary transaction (`Begin` … `Conn::Tx` … `Commit`) |
| `$primary.<...>`, `usePrimaryOnly().<...>` (declared names) | the primary, reads included |
| `select…`, `$count`, `with`, `$with`, `query.<t>.findFirst/findMany` | `Conn::Replica` **only when `ExtractRequest.replicas` is true**; otherwise `Op::Unknown` "read through a read replica (replica modeling not enabled): `db.select(...)` of a replicated database (<origin>)" |
| a read through an ambiguous handle | `Op::Unknown` "read through a read replica or the primary (the handle's routing is ambiguous)" — always |
| `select … .for("update")` routed to a replica | `Op::Unknown` "read through a read replica with a row lock" |
| any other member (`executeOnReplica`, `transactionOnReplica`, `$replicas`, …) | `Op::Unknown` "database handle of a replicated database: the routing of `<m>` is not part of the … contract" |

A replica read is never bound to the primary: test
`no_replica_read_is_ever_bound_to_the_primary` extracts every fixture entry
with and without replica modeling and with and without the declaration.
Every binding prints its origin and contract as an assumption (built-in:
"verified in drizzle-orm 0.45.4 pg-core/db.js"; declared: "an assumption;
its body is not interpreted"), and a parameter bound through its type prints
"type provenance; casts and other values of the same type are not tracked".

## 4. Plumbing (`commitrift-discover`)

* `commitrift-contracts.json` accepts `replicaFactories`
  (`ReplicaFactoryBinding`: `factory` `module#export` (required),
  `primaryArgument` (default 0), `primaryOnlyMethods`, `primaryProperties`,
  `reason` (required)). A declaration that does not resolve is reported as a
  note and as an assumption, and is not applied.
* `ProjectIndex::index` computes `replicated` (`Provenance::replicating`):
  `Bindings { replicas: replicated.is_some(), replica_factories:
  contracts.replica_factories }`; `extract_entry` copies both into every
  `ExtractRequest`. The memo key and the disk-cache key include both;
  `CACHE_VERSION` is `commitrift-discover-cache-3`.
* Notes: "the project builds a replicated database (...)", and a hint to
  declare a project-defined `withReplicas`.
* `corpora/contracts/midday.json` declares midday's factory
  (`packages/db/src/replicas.ts#withReplicas`, `usePrimaryOnly`,
  `$primary`) with the reason read from its source; it is passed with
  `commitrift discover --contracts`.
* Unknown categories: `read replica (modeling not enabled)`, `read replica
  or primary (ambiguous handle)`, `row lock through a read replica`,
  `replicated database: routing unknown`.

**Model / synthesis side (closeout correction).** When this page was
written in the analyzer worktree, the scenario loader built its
`ExtractRequest` with `replicas: false` and no factories, and midday's
`toggleCustomerPortal` candidate was UNSUPPORTED for that reason. The merged
Phase 13 code does carry both: a synthesized scenario gets
`extraction.replicaFactories` (`discover/src/synth.rs`) and the `replicas`
feature when an actor reads through a replica, and the loader passes
`replicas: features.replicas` and `extraction.replica_factories` into the
request (`model/src/scenario.rs`). With the final Phase 13 binary
(`reports/phase13/final/`), `toggleCustomerPortal` loads and is UNSUPPORTED
for a different, later reason: the call `nanoid(21)` (an unanalyzed package)
on its path (`MIDDAY-COVERAGE.md`, closeout note). Phase 14 adds an explicit
discovery-vs-loader round-trip check (`docs/phase14/REPLICA-INTEGRATION.md`).

## 5. Tests

`crates/commitrift-analyzer/tests/phase13.rs` (13 tests) on
`fixtures/phase13/replica-app` and temporary projects; discovery plumbing in
`crates/commitrift-discover/tests/phase13.rs` (3 tests) and the contracts
parser's unit test.

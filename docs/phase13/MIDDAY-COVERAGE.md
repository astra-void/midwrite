# Phase 13: midday coverage (replica-aware analysis)

midday at its pinned revision `51587319` (public, `.corpora/midday`); static
discovery with bounded model checking (`commitrift discover --model-check`);
nothing was executed. Both binaries ran on the same machine and input; wall
is the median of 3 runs, RSS the maximum. Raw data:
`reports/phase13/analyzer/corpus/` (`scripts/phase13-analyzer-corpus.py`).
Statement counts are read from the extraction cache of the same run
configuration.

| | Phase 12 | Phase 13, no declaration | Phase 13, declared factory |
| --- | ---: | ---: | ---: |
| entries extracted | 1 168 | 1 168 | 1 168 |
| effectful entries | 0 | 0 | **47** |
| read-only entries | 2 | 0 | **64** |
| resources (index) | 0 | 0 | 52 |
| replica reads (`Conn::Replica` SELECT) | 0 | 0 | **86** (73 entries) |
| primary reads (`Conn::Pool` SELECT) | **3** (2 entries) | 0 | 0 |
| primary autocommit writes | 0 | 0 | 54 (33 DELETE, 5 INSERT, 16 UPDATE; 45 entries) |
| primary transactions / statements in them | 0 / 0 | 0 / 0 | 16 / 10 (6 SELECT, 3 UPDATE, 1 DELETE; 5 entries) |
| "database handle of an unresolved type" | 752 | 771 | **62** |
| "replicated database: routing unknown" | 0 | 0 | 15 |
| unknown operations (total) | 17 603 | 17 696 | 17 496 |
| candidates | 0 | 0 | 3 |
| model-checkable / counterexamples | 0 / 0 | 0 / 0 | 0 / 0 |
| wall | 1.05 s | 1.22 s | 1.29 s |
| peak RSS | 111 MiB | 113 MiB | 114 MiB |

"Declared factory" is `corpora/contracts/midday.json`: midday's own
`withReplicas` with `usePrimaryOnly` and `$primary`, and the reason read from
`packages/db/src/replicas.ts`. No midday function name appears in Rust code.

## What changed and why

* **Phase 12 bound replica reads to the primary.** Its 2 read-only entries
  (`apps/api/src/bot/platform-resolver.ts#resolvePlatformLinkCode`,
  `apps/api/src/bot/thread-helpers.ts#hasCurrentTeamAccess`) import `db`
  from `@midday/db/client` — the replicated database — and their 3 SELECTs
  were bound as primary reads by the receiver-name rule. Phase 13 does not:
  without the declaration they are "database handle of an unresolved type"
  (the project builds a replicated database, so a receiver name alone is not
  the primary); with it they are replica reads.
* **The 752 operations behind `db: Database`.** `Database =
  Awaited<ReturnType<typeof connectDb>>` resolves through `connectDb = async
  () => { return db; }` to `db = withReplicas(primaryDb, [replicaDb], ...)`,
  a call of the declared factory whose argument 0 is `drizzle(...)` of
  `drizzle-orm/node-postgres`. Without the declaration the factory's body is
  not interpreted (it does more than return a value), so the type stays
  unresolved: 690 operations behind `db: Database`, 61 behind a union, 1
  behind a non-database parameter (the same 752 as Phase 12), plus 19
  operations on the directly imported `db` that Phase 12 bound by its name
  (its 3 SELECTs above; the rest had failed later as unsupported query
  shapes or unmodeled columns). With it, 62 remain: 61 are
  parameters typed `DatabaseOrTransaction = Database | TransactionClient`
  (a union: the connection kind depends on the caller; bound when such a
  helper is inlined), 1 is a parameter that is not a database.
* **15 `executeOnReplica` calls** are not part of the declared contract
  (raw SQL on a replica) and stay unknown; `transactionOnReplica` likewise.
* **Newly exposed behavior.** Bound handles reach code that was not reached
  before: "column not modeled (schema)" 6 → 179 (timestamp/JSON columns in
  `where`/`set`), "unsupported query shape" 3 → 112, "unsupported transaction
  shape" 434 → 532, "unresolved method call" 6 053 → 5 578.

## Candidates (declared factory)

| candidate | class | status | reason |
| --- | --- | --- | --- |
| `packages/db/src/queries/customers.ts#toggleCustomerPortal` | read-modify-write | UNSUPPORTED | the synthesized scenario is re-extracted by the scenario loader, which in this branch builds its request with `replicas: false` and no replica factories: the loader sees `db: Database` as an unresolved type. Needs the model side's scenario `features`/factory plumbing |
| `packages/db/src/queries/teams.ts#leaveTeam` | write-skew | BLOCKED | no declared invariant (CommitRift does not infer a multi-row invariant) |
| `packages/db/src/queries/teams.ts#deleteTeamMember` | write-skew | BLOCKED | no declared invariant |

**Closeout note (2026-10-28).** The `toggleCustomerPortal` reason above was
measured with the analyzer worktree's binary before the merge. With the final
Phase 13 binary (sha256 `97e96ac0…`, `reports/phase13/final/`) on the same
input and declaration, the counts are unchanged (47 effectful, 64 read-only,
3 candidates, 0 model-checkable), the scenario loads with the declared
factory and the `replicas` feature, and the candidate is UNSUPPORTED because
its path reaches `nanoid(21)` (`packages/db/src/queries/customers.ts:708`,
"call to `nanoid`: imported from the package `nanoid`, which is not
analyzed"). The two write-skew candidates stay BLOCKED (no declared
invariant). Wall 1.68 s and peak RSS 122 MiB for one run (the table above is
a median of 3 runs of the earlier binary).

No candidate was model-checked; no counterexample is claimed. A replica
read followed by a primary write in a candidate (`toggleCustomerPortal`
reads the customer through a replica and updates it on the primary) is
exactly the case whose verdict depends on replica lag, which the model side
represents.

## Performance

The project-wide `withReplicas` scan reads every module's text once
(2 022 modules); provenance facts are memoized in the shared project
context. Wall time rose from 1.05 s to 1.22 s without the declaration and
to 1.29 s with it (more entries bind and lower their queries). Peak RSS
rose by at most 3 MiB.

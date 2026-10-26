# Phase 8 architecture

What Phase 8 changed, and how the pieces talk to each other. Phase 1–7
components were extended, not rewritten; every change to an existing path
was checked byte for byte against the Phase 7 binary
([`DIFFERENTIAL-VERIFICATION.md`](DIFFERENTIAL-VERIFICATION.md)).

## Dependency map

```
                      TypeScript / TSX (Oxc parser + semantic)
                                   |
                     analyzer::semantic  (NEW: shared facts)
          unwind / method_calls / property_key / member_path / string_value
             |                        |                          |
   recognize.rs, lower.rs     concurrency.rs              schema.rs (NEW)
   (Effect IR: static)        (model IR: concurrent)      (Drizzle schema facts,
             |                        |                     no execution)
   core::analysis::summary           |                          |
   (IR inliner: memo by              |                 model::schema_facts (NEW)
    depth-canonical key, NEW)        |                          |
             |                        |                 model::scenario  ── from/overrides
   core::analysis::paths              |                 (table facts + origins, NEW)
   (legacy / compact / SHARED         |                          |
    path summaries, NEW)              v                          v
             |                model::engine / explore  <── model::Model
   detectors, traces          (COW locals and row chains, NEW)
             |                        |
     check / trace output     counterexample / recovery probe (NEW: model::probe)
                                      |
                       model::source_verify  ── plan: Release / Complete /
                       (prediction, comparison)    Crash / Restart (NEW)
                                      |
                       replay::source (coordinator, Rust)
                       ├── worker lifecycle: slot per execution (attempt),
                       │   SIGKILL at checkpoint, restart, rlimits, run-token sweep (NEW)
                       ├── channel test doubles for external effects (NEW)
                       ├── pg_stat_activity: lock waits, session termination (NEW)
                       └── replay::catalog: PostgreSQL catalog (NEW)
                                      |
                 runtime/node/source.js (worker client, protocol 3)
                 ├── node-postgres adapter (Phase 7)
                 ├── postgres-js adapter (NEW, version-pinned)
                 ├── Drizzle call-site attribution (+ async-chain restoration, NEW)
                 └── sourceEffect: effects through the coordinator (NEW)
                                      |
                       disposable PostgreSQL cluster (Unix socket only)
```

## Communication

| from → to | channel | what |
| --- | --- | --- |
| model → replay | `RunRequest` (Rust) | actors, plan steps, seed SQL, final queries, channel contracts, `read_catalog` |
| replay → worker | env built from scratch + Unix socket (JSON lines) | `COMMITRIFT_SOURCE_*`, `COMMITRIFT_SOURCE_ATTEMPT`, `COMMITRIFT_RUN_TOKEN`; `go` / `stop` / `go`+`effect` receipt |
| worker → replay | same socket | `hello` (attempt, pid), `query`, `effect`, `result`/`error`, `driver-internal`, `unsupported`, `done` |
| replay → PostgreSQL | `psql` (read-only sessions) | `pg_stat_activity` (waits, sessions by `application_name`), catalog JSON, final rows |
| analyzer → model | `ActorProvider` trait | `extract` (Phase 6/7), `extract_schema` (Phase 8) |

## Duplication found before implementing (Milestone 0)

| duplicated logic | where | Phase 8 |
| --- | --- | --- |
| call/member chain decomposition | `recognize::unwind`, `concurrency::flatten` | one implementation in `semantic.rs`; `flatten` keeps its stricter acceptance on top of it |
| property-key names | `concurrency::static_key`, `identity::key_name` | `semantic::property_key` (the concurrency extractor uses it) |
| dotted member paths | `concurrency::callee_text` (and a stricter local variant in `lower.rs`) | `semantic::member_path` for the extractors; `lower.rs`'s variant has different semantics (no parentheses) and stays |
| table/column facts | hand-written in every scenario | read from Drizzle source (`schema.rs`), compared with scenarios and with the database |
| per-instance path summaries | `paths::call_compact` keyed by `CallScope` address | instantiation across corresponding instances |
| per-depth inliner memo | `summary::Inliner::memo` keyed by remaining depth | depth-canonical key with exact counter replay |

## Files

New: `analyzer/src/{semantic,schema}.rs`, `core/src/fasthash.rs`,
`model/src/{probe,schema_facts,schema_fidelity,fasthash}.rs`,
`replay/src/catalog.rs`, `cli/src/schema.rs`, `fixtures/phase8/*`,
`runtime/node/test/postgres-js.test.js`, tests `*/tests/phase8.rs`,
`replay/tests/source_lifecycle.rs`, scripts `scripts/phase8-*`.

Changed: `replay/src/source.rs` (crash/restart/effects/limits/catalog),
`model/src/source_verify.rs` (plan and comparison for crashes, restarts,
effects, probes), `model/src/{engine,state,db,explore}.rs` (copy-on-write
state, recorded effects), `core/src/analysis/{summary,paths}.rs`
(summaries), `core/src/ir.rs` (shared label strings), `runtime/node/source.js`.

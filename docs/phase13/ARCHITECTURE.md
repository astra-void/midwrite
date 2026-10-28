# Phase 13 architecture and implementation plan

Consolidates the plan's `ARCHITECTURE` and `IMPLEMENTATION-PLAN`. Nothing
replaces an existing component: the model checker, the scenario loader, the
extractor, discovery and the runtime coordinator are extended in place.

## 1. How the work was organized

| track | owner | files |
| --- | --- | --- |
| A — PostgreSQL FK and row-lock semantics | lead | `model/src/{db,exec,state,model,scenario,oracle}.rs`, `analyzer/src/schema.rs` |
| B — return values, response step, retry | lead | `model/src/{ir,engine,invariant,dsl,scenario,source_verify}.rs`, `replay/src/source.rs`, `runtime/node/source.js` |
| C (model) — replica state and reads | lead | `model/src/{engine,exec,state}.rs` |
| C (analyzer) — handle provenance, `withReplicas` | agent, isolated worktree | `analyzer/src/{handles,concurrency,context}.rs`, `discover/src/{project,extract,contracts,pipeline}.rs` |
| D — source resolution, package contracts | same agent | `analyzer/src/{concurrency,packages}.rs` |
| E — upserts, row-lock strengths in Drizzle | lead | `model/src/{exec,ir,dsl,oracle}.rs`, `analyzer/src/concurrency.rs` |
| F — frozen evaluation set, PostgreSQL lock ground truth | agent, no access to `crates/` | `fixtures/phase13/eval-app`, `fixtures/phase13/lock-matrix`, `scripts/phase13-lock-matrix.py` |
| G — integration, compatibility, reports | lead | everything else |

**Checkpoint 1 (shared interfaces)** was fixed before any parallel work: a
patch (`Conn::Replica`, `Op::ReturnValue`, `Op::Respond`,
`ExtractRequest.{replicas, replica_factories}`, `ReplicaFactoryBinding`)
was applied verbatim in the main checkout and as the first step of the
analyzer agent's worktree, so the two sides merged with identical hunks.
The agent's worktree was merged with per-file three-way merges (`git
merge-file`); its edits to the model crate were verified to be exactly the
interface patch. Nothing was committed.

## 2. New model state (Checkpoint 1 representations)

| state | representation | in the canonical encoding |
| --- | --- | --- |
| row locks | `Store.locks: RowId → [(TxnId, RowLock)]`, acquisition order (PostgreSQL's multixact member order), strongest mode per transaction | modes and holder lists only when `Model::uses_lock_modes()` (a foreign key or a `FOR [KEY] SHARE`); otherwise the Phase 6–12 bytes (one holder) |
| pending foreign-key checks | `Pending.fk: [FkCheck]`, `Pending.deferred: Option<StmtResult>` | only while non-empty (marker `0xF4`) |
| task outcomes | `ActorState.result: Option<ReturnSlot>` (current attempt), `ActorState.returned: [ReturnSlot]` (one per completion) | only when recorded, i.e. when an invariant reads return values (marker `0xD3`) |
| response step | `Op::Respond` in the main code (a visible step) | it is code: no new state |
| read replica | `State.replica_seq` (the replayed prefix of the commit sequence), `TKind::Replicate` | only with the `replicas` feature: the replica is one more snapshot holder, and every committed version carries its distance from the replica's prefix (the replay order of the commits the replica has not replayed) |
| upsert | `StmtKind::Insert.upsert: Option<Upsert>` (`target`, `set`, `filter`) | statement text |

## 3. Versioned semantics (Track G2)

New semantics never change an earlier scenario silently:

* **`features`** (scenario field, additive): `foreign-keys` enforces the
  source schema's foreign keys; `replicas` routes replica reads. A table
  declared in a scenario may list `foreignKeys` itself (an explicit opt-in).
  Unknown feature names are refused.
* **`response: true`** (actor field) inserts the response step.
* **Return values** are recorded only when an invariant reads them
  (`"returned": v` in an `actors` term); otherwise `Op::ReturnValue` is not
  even extracted (`ExtractRequest.return_values`), so extracted code, state
  spaces and report bytes are those of Phase 12.

Compatibility measured on the Phase 6–9 model scenarios and the static
corpus: `COMPATIBILITY` in `FINAL-REPORT.md`.

Discovery opts into the new semantics where they apply: a synthesized
scenario gets `foreign-keys` when its tables carry foreign keys (the table
closure always includes the referenced tables), `replicas` when an actor
reads through a replica, and `response: true` for actors whose caller
retries after a crash. The Phase 12 `foreign-key-locks` limitation note is
no longer emitted.

## 4. Where each feature lives

```text
analyzer   schema.rs ............ FK referential actions (onDelete/onUpdate)
           concurrency.rs ....... return values, upserts, .for(strength), replica routing (agent)
           handles.rs ........... database-handle provenance (agent)
           packages.rs .......... package contracts (agent)
model      db.rs ................ RowLock, ForeignKeySchema, lock holders
           exec.rs .............. lock modes, RI checks, upserts, replica reads
           engine.rs ............ ReturnValue/Respond/Replicate, replica statements
           invariant.rs ......... `returned`
           scenario.rs .......... features, FK linking/refusals, responses, replica checks
           oracle.rs ............ FK DDL, upsert SQL, parent-first seeding
           source_verify.rs ..... response checkpoint, runtime return values, crash-first Gen matching
replay     source.rs ............ SqlKind::Respond, COMMITRIFT_SOURCE_RESPONSE
runtime    node/source.js ....... held response (protocol 4 + `respond`)
discover   synth.rs ............. features, response, replicaFactories
           pipeline/project/... . replica plumbing (agent)
```

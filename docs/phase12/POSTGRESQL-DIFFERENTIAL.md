# PostgreSQL and Drizzle differential, source-to-model fidelity

Consolidates the plan's `POSTGRESQL-DIFFERENTIAL` and `SOURCE-MODEL-FIDELITY`
documents (Milestones 4, 11, 13.4). Environment: drizzle-orm 0.45.4 and
node-postgres 8.23.1 (pinned in the fixtures), PostgreSQL server 17.11, Node
22.23.2, macOS arm64.

## 1. Drizzle probes (`fixtures/phase12/defaults-probe`)

First-party probe functions run by `verify/probe.js` on a fresh schema of a
disposable cluster; the runner records every statement Drizzle sends with its
parameters (Drizzle's logger), how often the application callbacks ran, and
the final rows. Raw outputs: `reports/phase12/differential/*.json`.

| probe | what Drizzle sent | what PostgreSQL stored | finding |
| --- | --- | --- | --- |
| `twoNotes` | two INSERTs, the key a **parameter** (`$1`) | two rows, distinct UUIDs | one evaluation per statement |
| `personWithProfile` | `values (default, $1) returning "id"`, then the profile with the returned id as `$1` | the profile's `person_id` = the person's generated `id` | database generation; RETURNING exposes it; the FK carries it |
| `explicitNote` | the given id as `$1` | the given id | an explicit value suppresses the callback for that column |
| `duplicatePerson` | three INSERTs (the second fails with 23505, caught) | two people | a failed insert leaves nothing |
| `rollbackThenInsert` | `begin`, two INSERTs, `rollback`, two INSERTs | `counters.id = 2`; one note with a new id | **a sequence is not rolled back**; an application key of a rolled-back transaction is not reused |
| `identityAndConstant` | `default` for the identity and the constant | ids 1, 2; `state = 'new'` | identity and constant defaults |
| `multiRow` | one INSERT, `($1,$2,$3), ($4,$5,$6)` | two rows | **one evaluation per row** (callback count 2) |
| `sameBuilderTwice` | the same builder awaited twice: two INSERTs with **different** keys | two rows | **re-executing a builder re-evaluates** `$defaultFn`; nothing is reused |
| `sqlDefault` | `values (default, $1)` | one row | `sql\`default\`` is the database default |
| `failingInsert` | an INSERT with `null` for a NOT NULL column (fails with 23502) | nothing | **the callback still ran** (count 1); its value is discarded |

These are the facts the model's `Generated` semantics rest on
(`GENERATED-VALUE-SEMANTICS.md` §2). No probe contradicted the model; no
mismatch reproduction had to be saved.

## 2. Model vs PostgreSQL on the same source

`drizzle_generated_defaults_match_the_model` (CLI `tests/phase12.rs`) runs
the six probes the model represents **twice**: through pinned Drizzle on a
disposable PostgreSQL, and by extracting the same TypeScript functions
(`src/probes.ts`, a two-module schema behind a barrel) and executing them in
the model engine. The final rows must be equal **column by column**, with
generated values matched one-to-one (the same generated value is always the
same UUID; two generated values are never one UUID), integers (sequences,
identity) exactly. Result: **6 / 6 agree** — `twoNotes`, `personWithProfile`
(FK = returned key), `explicitNote`, `duplicatePerson` (23505 path),
`rollbackThenInsert` (sequence gap 2, rolled-back key not reused),
`identityAndConstant`. The `notes.stamp` column (an unrecognized callback with
a side effect) is not modeled and is not compared.

`the_model_does_not_represent_shapes_it_has_not_validated`: the probes whose
runtime behavior is observed but not modeled (`multiRow`, `sameBuilderTwice`,
`sqlDefault`) extract to an unknown operation, never to a guessed one.

## 3. Lockstep oracle with generated keys

`postgres_agrees_with_schedules_over_generated_keys` (model
`tests/phase12.rs`): two actors register the same email (database UUID key,
UNIQUE email) and create a profile from the returned key (application UUID
key); the model's witness schedule is replayed statement by statement on
PostgreSQL by the Phase 6 oracle: **agree** (one user, one profile; the
losing actor fails with 23505 in both).

## 4. Original-code verification of generated-ID scenarios (Milestone 11)

`fixtures/phase12/workspace-app`, discovered automatically (no hand-written
scenario), replayed with `model-check --verify-source --allow-execution`
(`generated_id_counterexamples_reproduce_on_the_original_code`, and the
evidence runs in `reports/phase12/evidence/`):

| candidate | path | evidence | runs |
| --- | --- | --- | --- |
| check-then-act, `createWorkspace` × 2 | Server Action wrapper (other candidate) / service → `usersRepository` (object) → `workspacesRepository.create` (INSERT, `$defaultFn` key, `RETURNING id`) → `membershipRepository.add` (class, `this.database`, FK = returned key) | `original-code-reproduced` | 3 / 3 |
| resubmitted-write, `addWorkspace`, crash after the workspace INSERT, declared resubmission | object repository → INSERT RETURNING → crash before the membership INSERT → new process, same input | `original-code-crash-reproduced` | 3 / 3 |

Two more counterexamples reproduce the same way (the Server Action pair,
and the Server Action × service pair: `original-code-reproduced`, 3 / 3
each) — 4 of the 6 counterexample scenarios with a harness mapping in all;
the two pairs with the locked variant diverge (§5).

In the crash run the runtime tables show two workspaces with **different**
ids and one membership pointing at the second: the resubmitted request
generated a new key, exactly as the model's per-execution generation
predicts. The idempotent counterpart (`addWorkspaceIdempotent`, the request id
as the key) is `NO_VIOLATION_WITHIN_BOUNDS` in the model (its resubmission
fails with 23505); the stable-id assumption is never made without the
application supplying the id.

The comparison matches generated values bijectively per run: statement
results (only the columns `RETURNING` lists), final tables as multisets, and
the committed rows at the crash checkpoint.

## 5. Mismatches

**One model limitation (kept, stated, pinned).** Two discovered
counterexamples pair the unlocked `createWorkspace` (or its Server Action)
with `createWorkspaceLocked`; their original-code runs **diverge** (0 / 3
each, `reports/phase12/evidence/runtime-cr-synth-95906b-actual.json`,
`…-6c9b41-actual.json`): *"the model predicts the statement completes but
PostgreSQL reported a lock wait"*. Minimal trigger:

```sql
-- T2                                   -- T1 (autocommit)
BEGIN;
SELECT * FROM users WHERE email = $1 FOR UPDATE;
                                        INSERT INTO workspaces (id, name, owner_id) VALUES (…);  -- waits
```

PostgreSQL's foreign-key check on the INSERT locks the referenced `users`
row `FOR KEY SHARE`, which conflicts with `FOR UPDATE`; the model has no
foreign keys and therefore no such lock. Cause: **the model** (not the
extractor, not the harness). The invariant was still violated in every run,
along a different schedule. Fixing it needs lock modes in the store (every
earlier state space would change), so it is marked instead: such candidates
carry the assumption `foreign-key-locks` (model-limitation), and
`a_foreign_key_lock_the_model_lacks_is_stated_and_detected_at_runtime`
pins the runtime divergence (it fails, and must be revisited, the day the
model gains foreign-key locks). No expectation was changed to make a run
"reproduce".

**No mismatch with the generated-value semantics.** Harness-side and
synthesis mismatches found while building this, all fixed in CommitRift
(not by changing expectations):

1. The comparison asserted every column of an INSERT's returned row while
   Drizzle's `RETURNING` listed one → the IR now carries the RETURNING
   columns and the oracle/verifier assert exactly those.
2. Final tables and crash checkpoints compared generated values by text →
   `GenMatch` (bijection per run).
3. Synthesized arguments `"ownerId-1"` for a `uuid` column were rejected by
   PostgreSQL → `uuid` parameters get valid UUIDs.
4. Synthesis seeded a parent row with the actor's own generated (or
   request-supplied) key, which made the actor's insert collide → parents
   created by the execution are not initial data.

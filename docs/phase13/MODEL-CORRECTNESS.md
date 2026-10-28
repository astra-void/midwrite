# Phase 13 model correctness and source–runtime fidelity

Consolidates the plan's `MODEL-CORRECTNESS` and `SOURCE-RUNTIME-FIDELITY`.

## 1. Model vs PostgreSQL (semantics)

| check | scope | result |
| --- | --- | --- |
| lockstep oracle, every schedule (`pg_matrix::foreign_key_lock_workloads`) | 20 foreign-key / lock-mode workload pairs × 3 isolation levels | **543 schedules, 0 disagreements** |
| lockstep oracle, every schedule (`pg_matrix::upsert_workloads`) | 6 upsert pairs × with/without an existing row × 3 levels | **396 schedules, 0 disagreements** |
| independent ground truth replayed in the model (`lock_ground_truth`) | 707 of 837 cells of `scripts/phase13-lock-matrix.py` | **2 726 labeled steps, 0 disagreements** (waiting and outcome); sensitivity: a mutated compatibility table gives 109 |
| Phase 6–12 lockstep matrices | unchanged workloads | pass (full suite) |

Model mismatches found on the way and fixed (`LOCK-DIFFERENTIAL.md` §7):
the parent-side `NO ACTION` check under REPEATABLE READ (23503, not 40001)
and child inserts after a committed update of another unique column (40001
at RR, not success).

## 2. Partial-order reduction (Track G3)

Every new state-changing operation records a footprint: foreign-key checks
(`Pred` on the referenced key, `RowRead`, `LockCheck`/`LockChange`, `Wait`),
lock-mode grants (`LockChange`), upserts (the insert intent, `RowRead`,
locks, `RowWrite` of the updated row), replica reads (`Snapshot`) and replica
progress (`CommitWrite`), the response step (its actor). On

* the 20 FK workloads × 3 levels, a three-actor FK workload × 3 levels,
* claim / atomic-claim (return values) × 3 levels, signup with the response
  step (unsafe and idempotent),
* 6 upsert workloads × with/without a row × 2 levels,
* two writers and a replica reader,

the independence audit (`explore::audit_independence`) finds no violation,
and the sleep-set search, classic DPOR and Source-DPOR (stateful+sleep,
stateless+sleep) give the same terminal states, verdicts, counterexample
existence, stuck states and unsupported behavior as the unreduced search
(`model/tests/phase13.rs`). The Phase 10 POR suites (differential,
adversarial, mutation, regression seeds) pass unchanged.

## 3. Source → model → original code (fidelity)

| slice | source | model | original code (PostgreSQL 17.11) |
| --- | --- | --- | --- |
| 1 foreign-key lock | `workspace-app` `createWorkspace × createWorkspaceLocked`, and via the Server Action (discovered) | counterexample with the unlocked INSERT waiting on the lock | **3/3 and 3/3**: statements, the lock wait, results, outcomes and rows agree; Phase 12: 0/3 (diverged) |
| 2 return values | `request-app` `claimTask × claimTask` | both callers told `true` | **3/3**, evaluated on the returned values |
| 3 last-statement crash | `request-app` `recordSignup` (crash retry, response step) | INSERT, crash before the response, retry, INSERT | **3/3** (`original-code-crash-reproduced`) |
| E upsert | `request-app` `reserveHandle × reserveHandle` | both told they reserved it | **3/3** |
| 4 replica staleness | `fixtures/phase13/model/replica-read-your-writes.json` | stale read counterexample | **model only** (no replication run) |

Safe counterparts hold in the model: `claim-atomic`, `signup-idempotent`,
`reserve-if-free`, `count-views`, and the workspace-app locked/unique/
idempotent variants. No model/runtime divergence remains in Phase 13's
first-party evidence; the Phase 12 divergences are resolved (§3 slice 1).

**Runtime harness defects found and fixed:** generated values first observed
at a crash checkpoint were matched against the final rows first (an
intermittent crash-check disagreement); the CLI's scenario provider did not
resolve cross-module references (foreign keys to tables of another schema
module were not linked). Both had been latent since Phase 12.

## 4. Evidence statuses

The Phase 12 statuses are kept (`model-counterexample`,
`original-code-reproduced`, `original-code-crash-reproduced`, `diverged`,
`inconclusive`, `unsupported`, …). A runtime schedule that violates the
invariant differently from the model is reported as diverged, never as a
reproduction; a replica counterexample cannot be promoted beyond
`model-counterexample` (its schedule is refused for original-code runs).

# Phase 13 final report: semantic fidelity, distributed-database modeling, real-world verification

Phase 13 targets the gap between what a TypeScript/Drizzle application,
Drizzle and PostgreSQL actually do and what the model represents: PostgreSQL
row-lock modes and foreign-key locks, what callers are told (return values),
the response boundary, read replicas, upserts, and the source-resolution
blockers of the real corpus. Every number below comes from a file under
`reports/phase13/`.

**Snapshot.** Base commit `32efbca` (Phase 12 committed, clean tree).
Phase 13 changes are **uncommitted**. macOS: Darwin 27.0.0 arm64, rustc
1.98.1, Node 22.23.2, PostgreSQL server 17.11. Nothing was committed, pushed
or published. This report was completed at the Phase 13 closeout
(2026-10-28): the earlier version ended with a placeholder after the
document list, and several numbers it promised (final test count, Linux
gate, evidence) had not been recorded. They are recorded here, from runs on
one frozen source tree (§3).

Documents (the plan's 27 consolidated into 17): `BASELINE.md`,
`ARCHITECTURE.md` (+ implementation plan), `POSTGRESQL-FK-LOCKS.md` (+ row-lock
compatibility), `LOCK-DIFFERENTIAL.md`, `RETURN-VALUES-AND-RESPONSES.md`
(task return values, response lifecycle, response-loss verification),
`REPLICA-AWARE-ANALYSIS.md`, `REPLICA-CONSISTENCY.md`, `MIDDAY-COVERAGE.md`,
`SOURCE-RESOLUTION.md`, `PACKAGE-CONTRACTS.md`, `SCHOOL-LEDGER-COVERAGE.md`,
`ADVANCED-DRIZZLE.md` (+ RETURNING and upsert), `MODEL-CORRECTNESS.md`
(+ source-runtime fidelity), `EVALUATION-DATASET.md`, `EVALUATION.md`
(precision/recall), `SECURITY.md`, `LIMITATIONS.md`, this report. The
earlier list also named `REAL-WORLD-EVALUATION.md` and `PERFORMANCE.md`;
those were never written as separate files: the real-world results are §6
and performance is §8 of this report.

## 1. Implemented

* **Row-lock modes and foreign-key locks** (`POSTGRESQL-FK-LOCKS.md`): row
  locks are `Store.locks: RowId → [(TxnId, RowLock)]` with PostgreSQL's
  four strengths and its compatibility table; `NO ACTION`/`RESTRICT`
  foreign keys run their RI checks (parent `FOR KEY SHARE`, the referencing
  probe on delete/key update) with PostgreSQL's REPEATABLE READ cross-check
  behavior. `CASCADE`/`SET NULL`/`SET DEFAULT` and deferred constraints are
  refused when a statement would run them.
* **Return values and the response step** (`RETURN-VALUES-AND-RESPONSES.md`):
  `Op::ReturnValue` (recorded only when an invariant reads it), the
  `returned` filter of `actors` terms, `Op::Respond` for request handlers
  whose caller retries after a crash, and the runtime's held response.
* **Read replicas** (`REPLICA-AWARE-ANALYSIS.md`, `REPLICA-CONSISTENCY.md`):
  handle provenance for drizzle-orm `withReplicas` and declared project
  factories; `Conn::Replica` reads on a replayed prefix of the commit
  sequence; the `replicas` scenario feature. Model-only (no replication was
  run).
* **Upserts and lock strengths in Drizzle** (`ADVANCED-DRIZZLE.md`):
  `ON CONFLICT (target) DO UPDATE [WHERE]`, `.for("share" | "key share" |
  "no key update")`.
* **Source resolution** (`SOURCE-RESOLUTION.md`, `PACKAGE-CONTRACTS.md`):
  exported `const` function aliases, React `cache` functions, named
  package contracts (bcryptjs, zod) whose results stay unknown.
* **Versioned semantics**: the scenario `features` list (`foreign-keys`,
  `replicas`), `response: true`, and return values only on demand; earlier
  scenarios keep their meaning and bytes (§5).

## 2. Known limitations

`LIMITATIONS.md` (Phase 13 view) and the current consolidated list
`docs/reference/limitations.md`. In short: cascading and deferred foreign
keys are refused; one replica with unbounded lag, model-only; return-value
properties are never inferred; error responses are not a separate step;
multi-row `values([...])`, arrays, string building, `Map`/`Set`, timestamp
columns and higher-order callback wrappers are not modeled (all four are
Phase 14 work); `undefined` and `null` are one value; no independent human
review.

## 3. Quality gates (final)

Run on a frozen copy of the working tree in a private scratch directory with
its own `CARGO_TARGET_DIR` (raw logs: `reports/phase13/final/`).

| | |
| --- | --- |
| source digest (`scripts/phase8-digest.py`, 780 files) | `dc3161e13092b5494af52fe312bc76f5364705d5885662303534c40fef2cd260` |
| release binary | sha256 `97e96ac08f2b51d1c4d2fcbbb00d483e3b99e3026e5e9e620e64b8b46737afe7` |

| gate | result |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace --no-fail-fast`, run 1 | **652 passed, 1 failed, 3 ignored** (64 test targets) — exit 101 |
| the same, run 2 (same tree, same binary; `test-rerun.txt`) | **653 passed, 0 failed, 3 ignored** (64 test targets) — exit 0 |
| `cargo build --release` | exit 0 |

**The failure** (run 1): `crates/commitrift-cli/tests/phase12.rs`
`a_foreign_key_lock_is_modeled_and_the_schedule_reproduces_at_runtime`.
The original-code run reported `diverged` with "plan step 9 (transition 10):
createWorkspace-1 has no waiting statement to resume": every statement's
result agreed with the model, but the coordinator did not record the
unlocked INSERT as waiting for the `FOR UPDATE` lock. The test passes alone
(3/3), and fails in 1 of 6 copies run in parallel on the same machine
(`reports/phase13/final/fk-lock-stress.txt`, which states the binary used):
it is a **load-dependent coordination failure
of the runtime harness**, not a model disagreement (no run produced a
different result, row or SQLSTATE). It is not fixed in the Phase 13 binary. **Cause, found during
Phase 14:** the coordinator clears a task's "running" statement when its
answer arrives; the unblocked INSERT's answer can arrive while the
coordinator still waits for the lock holder's COMMIT, so the following
"resume the waiting statement" step found nothing and declared a divergence
although every result agreed. The Phase 14 tree accepts a waiting statement
that already completed (`replay/src/source.rs`, `run_plan`); with that fix
the test passed 12/12 in two rounds of 6 parallel copies and 12/12 in one
round of 12 (`fk-lock-stress.txt`). The Phase 13 claim "3/3 runs" for this slice stands for runs
in isolation.

**Linux.** Not run. `scripts/phase13-linux-docker.sh` exists, but the local
Docker daemon answered every API request with HTTP 500 during the closeout
(an environmental blocker: no container could be started). No Linux result
is claimed for Phase 13.

## 4. PostgreSQL differential results

| check | result |
| --- | --- |
| lockstep oracle, every model schedule, 20 FK / lock-mode workload pairs × 3 isolation levels | **543 schedules, 0 disagreements** |
| lockstep oracle, 6 upsert pairs × with/without a row × 3 levels | **396 schedules, 0 disagreements** |
| independent PostgreSQL 17.11 ground truth replayed in the model (`LOCK-DIFFERENTIAL.md` §7) | **707 of 837 cells, 2 726 labeled steps, 0 disagreements**; a mutated compatibility table gives 109 |
| Phase 6–12 lockstep matrices | pass (in the full suite) |

Two model mismatches were found and fixed before scoring
(`LOCK-DIFFERENTIAL.md` §7).

## 5. Compatibility (final binary)

| check | result |
| --- | --- |
| Phase 6–9 model scenarios, Phase 12 binary vs final Phase 13 binary, JSON field by field (`reports/phase13/final/model-identity*.txt`) | 57 scenarios: 48 byte-identical; 9 differ **only** in `/exploration/metrics/visitedBytes` (the memory estimate grew with the return-value fields of `ActorState`); text reports, verdicts, state and transition counts identical |
| static `check` (json, sarif, text) and `trace` (json), 4 corpus projects × depths 3/6/12 × 3 path modes (`reports/phase13/final/corpus-identity.txt`) | **144/144 byte-identical** |
| Phase 13 frozen evaluation set, final binary (`reports/phase13/eval/final-binary.json`) | identical to the recorded post-feedback scoring |
| discovered workspace-app scenarios, `foreign-keys` on vs off | 579 vs 571 states (1.01×; two scenarios 1.19×) (`reports/phase13/compat/state-space-overhead.md`) |

## 6. Real-world corpus (final binary, `reports/phase13/final/corpus/`)

Static discovery with bounded model checking; nothing executed; private
projects from read-only copies without `node_modules`, aggregate counts only.

| project | effectful | read-only | unknown ops | candidates | model-checkable | statuses |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| saas-starter | 8 | 4 | 284 | 2 | 1 | 1 MODEL_COUNTEREXAMPLE, 1 UNSUPPORTED |
| midday, no declaration | 0 | 0 | 17 696 | 0 | 0 | — |
| midday, declared replica factory | 47 | 64 | 17 496 | 3 | 0 | 1 UNSUPPORTED, 2 BLOCKED |
| school-ledger | 21 | 30 | 14 615 | 9 | 0 | 7 INCONCLUSIVE, 2 UNSUPPORTED |
| unnamed-ai-project | 3 | 6 | 1 009 | 0 | 0 | — |

The final binary reproduces every count recorded during Phase 13. One
recorded *reason* was stale: midday's `toggleCustomerPortal` is not blocked
by the scenario loader (that was the analyzer worktree's binary); with the
final binary it loads with the declared factory and the `replicas` feature
and is UNSUPPORTED because its path calls `nanoid(21)`
(`MIDDAY-COVERAGE.md`, closeout note). No corpus finding is claimed.

## 7. Precision and recall (`EVALUATION.md`)

Frozen set (27 labeled entries; AI-authored, frozen before scoring; **no
independent human review**):

| | TP | FP | FN | TN | precision | recall |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Phase 12 binary | 2 | 0 | 13 | 11 | 1.000 | 0.133 |
| Phase 13, **first scoring** | 3 | 0 | 12 | 11 | 1.000 | 0.200 |
| Phase 13 final, post-feedback (not held out) | 4 | 0 | 11 | 11 | 1.000 | 0.267 |

Earlier benchmarks under the Phase 13 binary: Phase 11 first-party and
held-out unchanged; Phase 12 set TP 6 FP 0 FN 1 TN 6
(`reports/phase13/regression/earlier-benchmarks.md`).

## 8. Performance

Discovery (`reports/phase13/final/corpus/SUMMARY.md`, median of 3, final
binary): midday with the declared factory 1.32 s / 121 MiB (Phase 12 binary
on the same input: 1.16 s / 112 MiB); school-ledger 0.70 s / 100 MiB
(0.56 s / 81 MiB). The cost is the project-wide `withReplicas` scan and more
inlining (aliases, React `cache`). Model checking: foreign-key semantics add
1 % states over the discovered workspace-app scenarios (§5).

## 9. Model and original-code evidence

`scripts/phase13-evidence.py --repeat 3` with the final binary
(`reports/phase13/evidence/SUMMARY.md`; raw reports beside it). The script
had never completed during Phase 13: a selector containing `/`
(`createWorkspaceAction/arg[0]`) was used as a file name. Fixed at the
closeout (the only change to the tree between the gates and this run:
digest of the tree it ran on `12453f6a…`, `reports/phase13/final/digest-evidence.txt`).
Original code runs only for the first-party fixtures, on disposable
PostgreSQL 17.11 clusters.

| slice | scenario | model | evidence | original code |
| --- | --- | --- | --- | --- |
| 2 return value | `claim-race` | counterexample | `original-code-reproduced` | 3/3 |
| 2 (safe) | `claim-atomic` | no counterexample | — | — |
| 3 last-statement crash | `signup-crash` | counterexample | `original-code-crash-reproduced` | 3/3 |
| 3 (safe) | `signup-idempotent` | no counterexample | — | — |
| E upsert | `reserve-race` | counterexample | `original-code-reproduced` | 3/3 |
| E (safe) | `reserve-if-free`, `count-views` | no counterexample | — | — |
| 1 foreign-key lock | `createWorkspace × createWorkspaceLocked` (discovered) | counterexample | `original-code-reproduced` | 3/3 |
| 1 foreign-key lock | `createWorkspaceAction/arg[0] × createWorkspaceLocked` (discovered) | counterexample | `model-counterexample` (runs unstable) | **2/3** — run 2 diverged |
| 4 replica staleness | `replica-read-your-writes` | counterexample | `model-counterexample` | not run (model-only) |

The one non-reproduced run is the §3 coordination failure again: this
evidence run overlapped with other PostgreSQL-backed test runs on the same
machine. The script therefore exits 1. In Phase 13's own runs (and in
isolation) the pair reproduced 3/3; this report does not replace that claim
with a better one: under load the original-code verification of the
foreign-key-lock slice is **not stable**.

The Phase 12 evidence script also passes with the final Phase 13 binary
(`reports/phase13/final/phase12-evidence.txt`, exit 0). Every model-check
report in the evidence validates against its schema
(`phase13-schemas.txt`; the one "error" is the discovery report, for which
no JSON schema exists).

Model-only vs original code: the replica slice is a model result about the
stated replication model, never an observation of a replica; the other
counterexamples above are model results that the original application code
followed on PostgreSQL with the outcomes listed.

## 10. Remaining blockers

* The load-dependent runtime coordination failures: the unlocked INSERT's
  lock wait not attributed under parallel load (§3, §9), and, seen once
  during the Phase 14 work under the same conditions, an upsert worker
  that "finished without sending the expected INSERT" (1 of 3 runs). Both
  are runtime-harness nondeterminism; neither produced a result that
  disagrees with the model. The first has a cause and a fix in the Phase 14
  tree (§3); the second did not recur in 6 stressed runs and has no
  established cause.
* Linux not run (Docker daemon unavailable during the closeout).
* Replica staleness is model-only; no streaming replication was run.
* Return-value properties need a declaration; the frozen set's four
  observable positives stay false negatives.
* The evaluation is first-party and AI-authored; no independent human
  review.

## 11. Commit status

Nothing committed, pushed or published. All Phase 13 work is uncommitted in
the working tree on branch `phase-8` (base `32efbca`).

# PostgreSQL test reliability

Rules the Phase 9 tests follow, and the flakes that led to them.

## Rules

1. **No fixed sleeps as synchronization.** Tests wait for an observable
   condition with a deadline: a proxy event (`OperationHeld`,
   `OperationCompleted`), a `pg_stat_activity` state, a worker checkpoint, or
   a row in a table. The deadline only turns a hang into an INCONCLUSIVE
   result; it is never the thing a test relies on to be ordered.
2. **Each test owns its cluster and its directory.** `initdb` in a private
   temporary directory, trust auth, Unix socket only, `listen_addresses=''`;
   removed on every exit path. Nothing is shared between tests or runs.
3. **Scenario files that tests generate live outside the repository**
   (temporary directory), so a test can never leave a modified fixture behind.
4. **Tests that spawn workers and PostgreSQL are serialized** where they
   compare wall-clock-sensitive observations (`SERIAL` mutex in
   `crates/commitrift-cli/tests/phase9.rs`), so a loaded machine does not
   change which of two valid server behaviors is observed.
5. **A blocked precondition is reported, not skipped.** Without PostgreSQL
   binaries or Node the runtime tests fail under
   `COMMITRIFT_REQUIRE_REPLAY=1`; without it they print which test was skipped
   and why. The Phase 9 gate runs with the variable set.
6. **Failure classification.** A runtime result is one of reproduced,
   not reproduced, diverged, unsupported, inconclusive, blocked. Timeouts,
   proxy errors and worker deaths are *inconclusive*; they are never counted as
   agreement and never upgraded.
7. **Expected-result lists are not widened to fit a flake.** An expected
   verdict is changed only when the semantics change, with the reason in the
   commit.

## Flakes seen and what was done

| symptom | cause | fix | status |
| --- | --- | --- | --- |
| deadlock victim differs from the model's prediction (`pg_matrix`) | which backend's `deadlock_timeout` fires first decides the victim; under load the other member can be chosen — a valid PostgreSQL behavior | only that observation (`(deadlock detection)` as the sole disagreement) is retried, at most 3 attempts, every retry counted and printed; any other disagreement, or a wrong victim after the last attempt, fails the test | in place; retries are counted in the test output |
| `claim-skip-locked-2/serializable` once predicted `ok`, PostgreSQL returned 40001 (SSI) | suspected: autovacuum's ANALYZE changing a plan (index scan ↔ sequential scan) between replays, which changes predicate-lock granularity | oracle cluster starts with `autovacuum=off` | **hypothesis, not proven**: the failure was seen once under load, passed 3/3 in isolation and in the later full runs; the mitigation addresses a plausible cause, the cause was not reproduced on demand |
| a worker task's INSERT attributed to the wrong task (plan diverged) in the pool scenario | `pg` calls the `pool.connect` callback from the releaser's async context | the harness captures the caller's context at `pool.connect` and restores it in the callback (`coordinatePool`) | fixed, deterministic |
| an adjacent `ServerComplete` missing from crash predictions of other actors in a group kill | predictions were taken before the contiguous completion run | predictions are computed after it | fixed, deterministic |

The only nondeterminism the tests deliberately tolerate is the deadlock victim
(bounded and counted). Everything else must agree on the first attempt.

## `claim-skip-locked-2/serializable`: finalization attempt

`constraint_delete_and_locking_workloads` (the `pg_matrix` test that contains
the case) was run 10 more times in a row from the debug test binary
(`reports/phase9/final/flake-claim-skip-locked.txt`), runs 1–5 while the
700-seed release DPOR test loaded the CPU, runs 6–10 without it: **10/10
passed**, `claim-skip-locked-2/serializable` replayed 70 of 70 schedules
with 0 mismatches and 0 deadlock-victim retries every time (62–70 s per
run). The failure was **not reproduced**; it stays an unresolved
intermittent failure with an unconfirmed hypothesis. The assertion was not
changed.

What would test the hypothesis, not done: start the oracle cluster with
autovacuum on and run `ANALYZE` on the claimed table between replays (or
`SET enable_indexscan = off` for half of them) and compare the 40001 rate;
the diagnostic already available is the matrix's per-schedule mismatch
output, which names the schedule and the SQLSTATE.

## Not done

* The SSI hypothesis was not tested by forcing a plan change; the flake was
  not reproduced, so there is no before/after rate.
* Tests are not run under artificial CPU starvation as a gate. Repeated
  full-suite runs and isolated reruns are the evidence (counts in
  `FINAL-REPORT.md`).
* Two Phase 7/8 HTML escaping tests read committed reports
  (`reports/phase7/model-check/team-app-invite-double-submit.json`,
  `reports/phase8/model-check/checkout-crash-after-charge.json`) as input:
  git-tracked, but generated. Rerunning an evidence script into those
  directories changes the tests' input. The Phase 9 escaping tests generate
  their reports in the test (`every_string_of_a_fresh_phase9_report_is_escaped`).

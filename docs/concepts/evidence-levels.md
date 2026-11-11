# Evidence levels

Every Midwrite result states how it was established. Levels are never
merged: a stronger level is reached only by running the step that produces
it, and a failed step never upgrades or downgrades another step's result.

## The ladder

| # | level | produced by | what was actually run |
| --- | --- | --- | --- |
| 0 | **static candidate** (`STATIC_CANDIDATE`, `"kind": "potentialHazard"`, `"reproduced": false`; a trace's crash simulation is `SIMULATED`) | `check`, `trace`, `discover` without `--model-check` | nothing; source was parsed |
| 1 | **model counterexample** (`model-counterexample`, `MODEL_COUNTEREXAMPLE`) | `model-check`, `discover --model-check` | the model, in memory |
| 2 | **PostgreSQL-confirmed schedule** (`postgres-confirmed-schedule`) | `model-check --validate-postgres` | the model's SQL on a disposable PostgreSQL, in lockstep |
| 3 | **original code reproduced** (`original-code-reproduced`, `original-code-crash-reproduced`, `original-code-in-flight-failure-reproduced`, `original-code-connection-loss-reproduced`) | `model-check --verify-source --allow-execution` | the application's original code through an authorized harness, on a disposable PostgreSQL, under the counterexample's schedule |
| — | **runtime replay** (`RUNTIME_REPRODUCED`, `RUNTIME_NOT_REPRODUCED`, `INCONCLUSIVE`) | `replay --allow-execution` | one request of an authorized harness with an injected failure |

Level 3 is about one schedule, one input, pinned driver versions and one
PostgreSQL version on a disposable database. No level says anything about
production frequency.

## Negative and bounded results

| result | meaning |
| --- | --- |
| `NO_COUNTEREXAMPLE_WITHIN_BOUNDS` / `NO_VIOLATION_WITHIN_BOUNDS` (`none`) | no violation in the explored bounded behaviors; not a proof |
| `original-code-recovery-reproduced` | no counterexample within bounds; a recovery schedule with a real crash and retry ran on the original code and the invariant held |
| `original-code-invariant-held` | no counterexample within bounds; a probe schedule without crash ran on the original code and the invariant held |
| `postgres-did-not-reproduce` (`MW7101`) | PostgreSQL disagreed with the model on that schedule; the first disagreement is reported; do not call it a race |
| `runtime-inconclusive` (`MW7102`) | validation was requested but could not be carried out |
| source status `diverged` / `not-reproduced` / `inconclusive` / `unstable` (`MW7103`) | the original code did not reproduce; the model counterexample keeps its level |
| `INCONCLUSIVE` | a bound or budget ended the search, or no valid initial state was found |
| `UNSUPPORTED` | a reachable operation is not modeled; the result is withheld, not approximated |

A run that violates the property along a **different** schedule than the
model's is `diverged`, never a reproduction. Example: before foreign-key
locks were modeled, a discovered check-then-act on the first-party
`fixtures/apps/workspace-app` diverged in every run — PostgreSQL's
foreign-key check made the unlocked actor's `INSERT` wait behind the other
actor's `FOR UPDATE`, and the property was violated along a different
schedule ([PostgreSQL model](../architecture/postgres-model.md)). A replica
counterexample cannot be promoted beyond `model-counterexample`: its
schedule is refused for original-code runs.

## Property origin is a separate axis

A model counterexample against a `HEURISTIC_HYPOTHESIS` is reported as
such at every level: the evidence level says the schedule is real for the
model, the database or the code; the origin says whether the violated
property is a requirement ([invariants](invariants.md#property-origins)).

## Static analysis kinds of evidence

Within a static finding, `evidenceStrength` (`provenance`,
`configuredImport`, `configuredName`, `memberPath`, `unknown`) and
`pathCertainty` (`straightLine`, `conditional`, `approximated`,
`truncated`, `unknown`) explain the confidence. Configured assumptions
(`recovery`, `recoveryContracts`, `bestEffort`) are listed with
`"source": "configuration"` and `"verified": false`. Table:
[`docs/LIMITATIONS.md` § kinds of evidence](../LIMITATIONS.md#kinds-of-evidence).

## Where the levels are defined

* Model checker levels: `EvidenceLevel` in
  `crates/midwrite-model/src/report.rs`; how each level is produced:
  [model checker](../architecture/model-checker.md),
  [runtime verification](../architecture/runtime-verification.md).
* Static and replay levels: `crates/midwrite-replay` and
  [runtime verification](../architecture/runtime-verification.md).
* How the ladder grew: [project history](../HISTORY.md).

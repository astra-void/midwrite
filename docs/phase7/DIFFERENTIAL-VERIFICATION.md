# End-to-end differential verification

`crates/commitrift-model/src/source_verify.rs`;
`commitrift model-check scenario.json --verify-source --allow-execution [--harness M] [--source-repeat N]`.

## Pipeline

| Stage | Evidence recorded |
| --- | --- |
| 1. analyze the original source | `extraction[]`: entry identity, wrapper facts, declared contract |
| 2. extract the concurrency model | operations with source spans; `sourceCoverage` |
| 3. validate completeness | `Unknown` reached → `UNSUPPORTED`; eligibility checks |
| 4–5. explore, counterexample | `exploration`, `counterexample` (minimized, replayed) |
| 6–8. run original code, observe Drizzle SQL, coordinate actors | `sourceVerification.statements[].sql`, `runtimeSource`, `outsideModel` |
| 9. observe PostgreSQL | results, row counts, SQLSTATEs, lock waits; final rows |
| 10. compare | per statement: kind, table, source region, rows/cells, affected rows, SQLSTATE, waits; final rows; outcomes |
| 11. invariant | evaluated **on the rows the run left** (not inferred from the model) |
| 12. report | `sourceVerification`, evidence level, CP7103 if not reproduced |

## Comparisons, kept separate

```
Abstract model vs PostgreSQL         --validate-postgres   report.runtime            (model SQL)
Drizzle SQL vs abstract model        --verify-source       sourceVerification.statements
Original application vs model        --verify-source       sourceVerification.runs (rows, outcomes, invariant)
```

Passing the first does not count for the others. `--validate-postgres`
never runs application code.

## Evidence levels

| Level | Meaning |
| --- | --- |
| `model-counterexample` | MODEL_COUNTEREXAMPLE: found by the model checker only |
| `postgres-confirmed-schedule` | POSTGRESQL_MODEL_REPLAY_CONFIRMED: model SQL replayed, every outcome matched |
| `original-code-reproduced` | ORIGINAL_CODE_RUNTIME_REPRODUCED: in every run, the coordinator followed the plan, every statement agreed, final modeled rows and actor outcomes agreed, nothing unexpected was sent, and the invariant is violated on the observed rows |
| `sourceVerification.status` `diverged` / `inconclusive` / `not-reproduced` / `unstable` | INCONCLUSIVE at the source level (the model counterexample keeps its own level; CP7103) |
| `unsupported` / `blocked` | UNSUPPORTED / not run (with the reason) |

A modeled SQL replay alone never reaches `original-code-reproduced`.
`unstable` means some runs reproduced and others did not.

## Results (macOS arm64, PostgreSQL 17.11, Node.js 22.23.2, Drizzle 0.45.4)

| Scenario | Counterexample | Original code | Runs |
| --- | --- | --- | --- |
| team-app `inviteTeamMember`, two concurrent submits (wrapped Server Action, LEFT JOIN membership check, pending-invitation check, insert with generated id, activity log) | 10 transitions | **reproduced**: 10/10 statements agree, attributed to `src/…` lines inside each operation's span; 2 wrapper `getUser()` reads outside the model; final rows equal; invariant violated on observed rows (2 pending invitations) | 5/5 |
| team-app `acceptInvitation` twice (Drizzle transaction, READ COMMITTED) | 10 transitions | **reproduced**, BEGIN/COMMIT attributed to the `db.transaction` call | 5/5 |
| same, a schedule with a lock wait (test `a_lock_wait_in_an_original_transaction_matches_the_model`) | unminimized | **reproduced**: the second UPDATE is reported waiting by PostgreSQL exactly where the model predicts, and resumes after the first COMMIT | 1/1 per test run |
| harness input contradicting the scenario (other email) | — | `not-reproduced`: statements agree, final rows differ | test |

Reports: `reports/phase7/model-check/`. No disagreement between model and
original code was found in these runs; no model correction was needed for
them. The fixes Phase 7 made to the model came from extraction tests and
the PostgreSQL differential (see [`FINAL-REPORT.md`](FINAL-REPORT.md)).

## Witness mapping

Each statement row shows: the model operation and its source span; the SQL
the application sent and its innermost application frame; the predicted and
observed outcome; every difference. Statements outside the model and
unexpected statements are listed separately; nothing is dropped because the
final rows happen to match.

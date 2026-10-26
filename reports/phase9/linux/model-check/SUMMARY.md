# Phase 9 original-code evidence (3 runs per scenario)

| scenario | model verdict | evidence | runs (followed / matched) | in-flight: operation → fate (model = run) | server answer observed | backend before kill | session ended (ms) | stuck / pool | tasks killed | s |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| ledger-autocommit-blind-in-flight | COUNTEREXAMPLE_FOUND | ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED | 3/3 / 3/3 | INSERT PAYMENTS → answered-and-withheld = answered-and-withheld | CommandComplete INSERT 0 1 | idle | 6 | — | — | 4.3 |
| ledger-blind-commit-in-flight | COUNTEREXAMPLE_FOUND | ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED | 3/3 / 3/3 | COMMIT → answered-and-withheld = answered-and-withheld | CommandComplete COMMIT | idle | 6 | — | — | 4.3 |
| ledger-idempotent-commit-answered | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | ORIGINAL_CODE_RECOVERY_REPRODUCED | 3/3 / 3/3 | COMMIT → answered-and-withheld = answered-and-withheld | CommandComplete COMMIT | idle | 5 | — | — | 4.3 |
| ledger-idempotent-commit-held | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | ORIGINAL_CODE_RECOVERY_REPRODUCED | 3/3 / 3/3 | COMMIT → held-for-later-delivery = held-for-later-delivery | — | idle in transaction | — | — | — | 4.5 |
| ledger-idempotent-commit-not-reached | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | ORIGINAL_CODE_RECOVERY_REPRODUCED | 3/3 / 3/3 | COMMIT → not-reached = not-reached | — | idle in transaction | 21 | — | — | 4.3 |
| ledger-pool-of-two | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | no counterexample | 0/0 / 0/0 | — | — | — | — | — | — | 0.0 |
| ledger-pool-starvation | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | — | — | — | — | pool-starvation: quiet 809 ms, pool [1, 0, 1], lock waits 0 | 1 | 5.6 |
| ledger-reconciled-commit-in-flight | COUNTEREXAMPLE_FOUND | ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED | 3/3 / 3/3 | COMMIT → held-for-later-delivery = held-for-later-delivery | — | idle in transaction | — | — | — | 4.6 |
| ledger-settle-update-answered | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | ORIGINAL_CODE_RECOVERY_REPRODUCED | 3/3 / 3/3 | UPDATE ORDERS → answered-and-withheld = answered-and-withheld | CommandComplete UPDATE 1 | idle | 21 | — | 1 | 3.0 |
| ledger-settle-update-not-reached | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | ORIGINAL_CODE_RECOVERY_REPRODUCED | 3/3 / 3/3 | UPDATE ORDERS → not-reached = not-reached | — | idle | 5 | — | 1 | 3.1 |
| ledger-shared-process-crash | COUNTEREXAMPLE_FOUND | ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED | 3/3 / 3/3 | INSERT PAYMENTS → answered-and-withheld = answered-and-withheld | CommandComplete INSERT 0 1 | idle | 6; 6; 6 | — | 3 | 3.1 |

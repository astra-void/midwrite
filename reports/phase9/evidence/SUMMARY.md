# Phase 9 original-code evidence (5 runs per scenario)

| scenario | model verdict | evidence | runs (followed / matched) | in-flight: operation → fate (model = run) | server answer observed | backend before kill | session ended (ms) | stuck / pool | tasks killed | s |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| ledger-autocommit-blind-in-flight | COUNTEREXAMPLE_FOUND | ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED | 5/5 / 5/5 | INSERT PAYMENTS → answered-and-withheld = answered-and-withheld | CommandComplete INSERT 0 1 | idle | 17 | — | — | 10.5 |
| ledger-blind-commit-in-flight | COUNTEREXAMPLE_FOUND | ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED | 5/5 / 5/5 | COMMIT → answered-and-withheld = answered-and-withheld | CommandComplete COMMIT | idle | 17 | — | — | 10.4 |
| ledger-idempotent-commit-answered | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | ORIGINAL_CODE_RECOVERY_REPRODUCED | 5/5 / 5/5 | COMMIT → answered-and-withheld = answered-and-withheld | CommandComplete COMMIT | idle | 15 | — | — | 8.8 |
| ledger-idempotent-commit-held | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | ORIGINAL_CODE_RECOVERY_REPRODUCED | 5/5 / 5/5 | COMMIT → held-for-later-delivery = held-for-later-delivery | — | idle in transaction | — | — | — | 9.4 |
| ledger-idempotent-commit-not-reached | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | ORIGINAL_CODE_RECOVERY_REPRODUCED | 5/5 / 5/5 | COMMIT → not-reached = not-reached | — | idle in transaction | 16 | — | — | 9.3 |
| ledger-pool-of-two | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | no counterexample | 0/0 / 0/0 | — | — | — | — | — | — | 0.0 |
| ledger-pool-starvation | COUNTEREXAMPLE_FOUND | original-code-reproduced | 5/5 / 5/5 | — | — | — | — | pool-starvation: quiet 803 ms, pool [1, 0, 1], lock waits 0 | 1 | 11.0 |
| ledger-reconciled-commit-in-flight | COUNTEREXAMPLE_FOUND | ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED | 5/5 / 5/5 | COMMIT → held-for-later-delivery = held-for-later-delivery | — | idle in transaction | — | — | — | 9.7 |
| ledger-settle-update-answered | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | ORIGINAL_CODE_RECOVERY_REPRODUCED | 5/5 / 5/5 | UPDATE ORDERS → answered-and-withheld = answered-and-withheld | CommandComplete UPDATE 1 | idle | 15 | — | 1 | 7.4 |
| ledger-settle-update-not-reached | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | ORIGINAL_CODE_RECOVERY_REPRODUCED | 5/5 / 5/5 | UPDATE ORDERS → not-reached = not-reached | — | idle | 14 | — | 1 | 7.2 |
| ledger-shared-process-crash | COUNTEREXAMPLE_FOUND | ORIGINAL_CODE_IN_FLIGHT_FAILURE_REPRODUCED | 5/5 / 5/5 | INSERT PAYMENTS → answered-and-withheld = answered-and-withheld | CommandComplete INSERT 0 1 | idle | 15; 15; 15 | — | 3 | 7.0 |

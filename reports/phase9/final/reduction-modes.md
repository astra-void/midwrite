| scenario | extra | mode | verdict | complete | states | transitions | counterexample step kinds |
| --- | --- | --- | --- | --- | --- | --- | --- |
| ambiguous-commit-blind-retry | — | full | COUNTEREXAMPLE_FOUND | True | 10 | 9 | step step ack-lost-committed server-complete retry step step step |
| ambiguous-commit-blind-retry | — | sleep sets | COUNTEREXAMPLE_FOUND | True | 10 | 9 | step step ack-lost-committed server-complete retry step step step |
| ambiguous-commit-blind-retry | — | DPOR | COUNTEREXAMPLE_FOUND | True | 10 | 9 | step step ack-lost-committed server-complete retry step step step |
| ambiguous-commit-idempotency-key | — | full | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 19 | 22 | — |
| ambiguous-commit-idempotency-key | — | sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 19 | 22 | — |
| ambiguous-commit-idempotency-key | — | DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 28 | 27 | — |
| ambiguous-commit-lookup-then-insert-concurrent | — | full | COUNTEREXAMPLE_FOUND | True | 11 | 10 | step step step step step step step step |
| ambiguous-commit-lookup-then-insert-concurrent | — | sleep sets | COUNTEREXAMPLE_FOUND | True | 11 | 10 | step step step step step step step step |
| ambiguous-commit-lookup-then-insert-concurrent | — | DPOR | COUNTEREXAMPLE_FOUND | True | 11 | 10 | step step step step step step step step |
| ambiguous-commit-lookup-then-insert | — | full | COUNTEREXAMPLE_FOUND | True | 12 | 11 | step step step ack-lost-committed retry step server-complete step step step |
| ambiguous-commit-lookup-then-insert | — | sleep sets | COUNTEREXAMPLE_FOUND | True | 12 | 11 | step step step ack-lost-committed retry step server-complete step step step |
| ambiguous-commit-lookup-then-insert | — | DPOR | COUNTEREXAMPLE_FOUND | True | 12 | 11 | step step step ack-lost-committed retry step server-complete step step step |
| ambiguous-commit-lookup-unique | — | full | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 305 | 612 | — |
| ambiguous-commit-lookup-unique | — | sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 305 | 551 | — |
| ambiguous-commit-lookup-unique | — | DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 9252 | 9251 | — |
| shipping-outbox-no-loss | --max-in-flight 1 | full | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 72 | 83 | — |
| shipping-outbox-no-loss | --max-in-flight 1 | sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 72 | 80 | — |
| shipping-outbox-no-loss | --max-in-flight 1 | DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 115 | 114 | — |
| shipping-outbox-relay-duplicate | --max-in-flight 1 | full | COUNTEREXAMPLE_FOUND | True | 16 | 15 | step step step step step step crash-in-flight retry step step step |
| shipping-outbox-relay-duplicate | --max-in-flight 1 | sleep sets | COUNTEREXAMPLE_FOUND | True | 16 | 15 | step step step step step step crash-in-flight retry step step step |
| shipping-outbox-relay-duplicate | --max-in-flight 1 | DPOR | COUNTEREXAMPLE_FOUND | True | 16 | 15 | step step step step step step crash-in-flight retry step step step |
| job-claimed-effect-redelivery | --max-in-flight 1 | full | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 90 | 105 | — |
| job-claimed-effect-redelivery | --max-in-flight 1 | sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 90 | 105 | — |
| job-claimed-effect-redelivery | --max-in-flight 1 | DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 119 | 118 | — |
| checkout-idempotent-crash-after-charge | --max-in-flight 1 | full | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 83 | 95 | — |
| checkout-idempotent-crash-after-charge | --max-in-flight 1 | sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 83 | 95 | — |
| checkout-idempotent-crash-after-charge | --max-in-flight 1 | DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 119 | 118 | — |
| checkout-crash-after-charge | --max-in-flight 1 | full | COUNTEREXAMPLE_FOUND | True | 14 | 13 | step step step crash-in-flight retry step step step step |
| checkout-crash-after-charge | --max-in-flight 1 | sleep sets | COUNTEREXAMPLE_FOUND | True | 14 | 13 | step step step crash-in-flight retry step step step step |
| checkout-crash-after-charge | --max-in-flight 1 | DPOR | COUNTEREXAMPLE_FOUND | True | 14 | 13 | step step step crash-in-flight retry step step step step |
| ledger-autocommit-blind-in-flight | — | full | COUNTEREXAMPLE_FOUND | True | 9 | 8 | crash-in-flight server-complete retry step |
| ledger-autocommit-blind-in-flight | — | sleep sets | COUNTEREXAMPLE_FOUND | True | 9 | 8 | crash-in-flight server-complete retry step |
| ledger-autocommit-blind-in-flight | — | DPOR | COUNTEREXAMPLE_FOUND | True | 9 | 8 | crash-in-flight server-complete retry step |
| ledger-blind-commit-in-flight | — | full | COUNTEREXAMPLE_FOUND | True | 21 | 20 | step step crash-in-flight server-complete retry step step step step |
| ledger-blind-commit-in-flight | — | sleep sets | COUNTEREXAMPLE_FOUND | True | 21 | 20 | step step crash-in-flight server-complete retry step step step step |
| ledger-blind-commit-in-flight | — | DPOR | COUNTEREXAMPLE_FOUND | True | 21 | 20 | step step crash-in-flight server-complete retry step step step step |
| ledger-idempotent-commit-answered | — | full | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 25 | 29 | — |
| ledger-idempotent-commit-answered | — | sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 25 | 29 | — |
| ledger-idempotent-commit-answered | — | DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 37 | 36 | — |
| ledger-idempotent-commit-held | — | full | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 25 | 29 | — |
| ledger-idempotent-commit-held | — | sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 25 | 29 | — |
| ledger-idempotent-commit-held | — | DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 37 | 36 | — |
| ledger-idempotent-commit-not-reached | — | full | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 25 | 29 | — |
| ledger-idempotent-commit-not-reached | — | sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 25 | 29 | — |
| ledger-idempotent-commit-not-reached | — | DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 37 | 36 | — |
| ledger-pool-of-two | — | full | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 5 | 4 | — |
| ledger-pool-of-two | — | sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 5 | 4 | — |
| ledger-pool-of-two | — | DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 5 | 4 | — |
| ledger-pool-starvation | — | full | COUNTEREXAMPLE_FOUND | False | 3 | 2 | step step |
| ledger-pool-starvation | — | sleep sets | COUNTEREXAMPLE_FOUND | False | 3 | 2 | step step |
| ledger-pool-starvation | — | DPOR | COUNTEREXAMPLE_FOUND | False | 3 | 2 | step step |
| ledger-reconciled-commit-in-flight | — | full | COUNTEREXAMPLE_FOUND | True | 21 | 20 | step step step crash-in-flight retry step server-complete step step step |
| ledger-reconciled-commit-in-flight | — | sleep sets | COUNTEREXAMPLE_FOUND | True | 21 | 20 | step step step crash-in-flight retry step server-complete step step step |
| ledger-reconciled-commit-in-flight | — | DPOR | COUNTEREXAMPLE_FOUND | True | 21 | 20 | step step step crash-in-flight retry step server-complete step step step |
| ledger-settle-update-answered | — | full | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 14 | 13 | — |
| ledger-settle-update-answered | — | sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 14 | 13 | — |
| ledger-settle-update-answered | — | DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 14 | 13 | — |
| ledger-settle-update-not-reached | — | full | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 14 | 13 | — |
| ledger-settle-update-not-reached | — | sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 14 | 13 | — |
| ledger-settle-update-not-reached | — | DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | True | 14 | 13 | — |
| ledger-shared-process-crash | — | full | COUNTEREXAMPLE_FOUND | True | 18 | 21 | step step step crash-in-flight server-complete |
| ledger-shared-process-crash | — | sleep sets | COUNTEREXAMPLE_FOUND | True | 18 | 17 | step step step crash-in-flight server-complete |
| ledger-shared-process-crash | — | DPOR | COUNTEREXAMPLE_FOUND | True | 14 | 13 | step step step crash-in-flight server-complete |

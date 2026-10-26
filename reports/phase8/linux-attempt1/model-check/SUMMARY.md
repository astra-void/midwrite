# Phase 8 original-code evidence (3 runs per scenario)

| scenario | model verdict | evidence | runs (followed / matched) | invariant on observed state | crash checks | restarts | effects (model = runtime) | PostgreSQL | s |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| accept-crash-lock-holder | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | none | 0/0 / 0/0 |  | — | — | — |  | 1.4 |
| accept-crash-while-waiting | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | none | 0/0 / 0/0 |  | — | — | — |  | 1.1 |
| checkout-crash-after-charge | COUNTEREXAMPLE_FOUND | model-counterexample | 0/0 / 0/0 |  | — | — | — |  | 1.2 |
| checkout-idempotent-crash-after-charge | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | none | 0/0 / 0/0 |  | — | — | — |  | 1.8 |
| checkout-idempotent-two-requests | COUNTEREXAMPLE_FOUND | model-counterexample | 0/0 / 0/0 |  | — | — | — |  | 1.1 |
| inventory-notify-after-commit | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | none | 0/0 / 0/0 |  | — | — | — |  | 1.1 |
| inventory-retry-duplicate-notice | COUNTEREXAMPLE_FOUND | model-counterexample | 0/0 / 0/0 |  | — | — | — |  | 1.1 |
| inventory-retry-exhausted | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | none | 0/0 / 0/0 |  | — | — | — |  | 1.1 |
| job-claimed-effect-concurrent | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | none | 0/0 / 0/0 |  | — | — | — |  | 1.6 |
| job-claimed-effect-redelivery | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | none | 0/0 / 0/0 |  | — | — | — |  | 1.5 |
| job-concurrent-claims-duplicate | COUNTEREXAMPLE_FOUND | model-counterexample | 0/0 / 0/0 |  | — | — | — |  | 1.1 |
| job-redelivery-duplicate | COUNTEREXAMPLE_FOUND | model-counterexample | 0/0 / 0/0 |  | — | — | — |  | 1.1 |
| pgjs-accept-crash-lock-holder | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | none | 0/0 / 0/0 |  | — | — | — |  | 1.1 |
| pgjs-accept-crash-while-waiting | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | none | 0/0 / 0/0 |  | — | — | — |  | 1.0 |
| pgjs-accept-invitation-twice | COUNTEREXAMPLE_FOUND | model-counterexample | 0/0 / 0/0 |  | — | — | — |  | 1.0 |
| pgjs-checkout-crash-after-charge | COUNTEREXAMPLE_FOUND | model-counterexample | 0/0 / 0/0 |  | — | — | — |  | 1.0 |
| pgjs-inventory-retry-duplicate-notice | COUNTEREXAMPLE_FOUND | model-counterexample | 0/0 / 0/0 |  | — | — | — |  | 1.0 |
| pgjs-invite-double-submit | COUNTEREXAMPLE_FOUND | model-counterexample | 0/0 / 0/0 |  | — | — | — |  | 1.0 |
| shipping-dual-write-crash | COUNTEREXAMPLE_FOUND | model-counterexample | 0/0 / 0/0 |  | — | — | — |  | 1.0 |
| shipping-outbox-no-loss | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | none | 0/0 / 0/0 |  | — | — | — |  | 1.0 |
| shipping-outbox-relay-duplicate | COUNTEREXAMPLE_FOUND | model-counterexample | 0/0 / 0/0 |  | — | — | — |  | 1.0 |

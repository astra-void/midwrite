# Phase 9 model-checker measurements

macOS-27.0.1-arm64-arm-64bit-Mach-O; Phase 8 binary = the saved release build of commit 4dc01af; median of 3 runs unless noted; `wall ms` is the exploration time the report states.

## 1. Phase 6-8 fixture scenarios (identical verdicts and counts)

| scenario | verdict | states | transitions | Phase 8 wall ms | Phase 9 wall ms |
| --- | --- | --- | --- | --- | --- |
| fixtures/concurrency/corpus/saas-starter-invite-double-submit.json | COUNTEREXAMPLE_FOUND | 7 | 6 | 0 | 0 |
| fixtures/concurrency/corpus/saas-starter-invite-unique-mitigation.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 12 | 11 | 0 | 0 |
| fixtures/concurrency/scenarios/doctors-write-skew-read-committed.json | COUNTEREXAMPLE_FOUND | 12 | 11 | 0 | 0 |
| fixtures/concurrency/scenarios/doctors-write-skew-repeatable-read.json | COUNTEREXAMPLE_FOUND | 12 | 11 | 0 | 0 |
| fixtures/concurrency/scenarios/doctors-write-skew-serializable.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 30 | 33 | 0 | 0 |
| fixtures/concurrency/scenarios/inventory-atomic-update.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 9 | 8 | 0 | 0 |
| fixtures/concurrency/scenarios/inventory-oversell-in-transaction.json | COUNTEREXAMPLE_FOUND | 14 | 13 | 0 | 0 |
| fixtures/concurrency/scenarios/inventory-oversell-repeatable-read.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 37 | 41 | 0 | 0 |
| fixtures/concurrency/scenarios/inventory-oversell.json | COUNTEREXAMPLE_FOUND | 11 | 10 | 0 | 0 |
| fixtures/concurrency/scenarios/inventory-row-lock.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 30 | 34 | 0 | 0 |
| fixtures/concurrency/scenarios/job-double-processing.json | COUNTEREXAMPLE_FOUND | 28 | 33 | 0 | 0 |
| fixtures/concurrency/scenarios/payment-crash-retry-provider-dedup.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 7 | 6 | 0 | 0 |
| fixtures/concurrency/scenarios/payment-crash-retry.json | COUNTEREXAMPLE_FOUND | 7 | 6 | 0 | 0 |
| fixtures/concurrency/scenarios/redeem-atomic.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 5 | 4 | 0 | 0 |
| fixtures/concurrency/scenarios/redeem-check-then-act-unique.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 12 | 11 | 0 | 0 |
| fixtures/concurrency/scenarios/redeem-check-then-act.json | COUNTEREXAMPLE_FOUND | 7 | 6 | 0 | 0 |
| fixtures/phase7/scenarios/saas-starter-original-invite.json | COUNTEREXAMPLE_FOUND | 19 | 18 | 0 | 0 |
| fixtures/phase7/scenarios/team-app-accept-invitation-twice.json | COUNTEREXAMPLE_FOUND | 15 | 14 | 0 | 0 |
| fixtures/phase7/scenarios/team-app-invite-double-submit.json | COUNTEREXAMPLE_FOUND | 19 | 18 | 0 | 0 |
| fixtures/phase8/scenarios/accept-crash-lock-holder.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 121 | 197 | 1 | 1 |
| fixtures/phase8/scenarios/accept-crash-while-waiting.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 121 | 197 | 1 | 1 |
| fixtures/phase8/scenarios/checkout-crash-after-charge.json | COUNTEREXAMPLE_FOUND | 11 | 10 | 0 | 0 |
| fixtures/phase8/scenarios/checkout-idempotent-crash-after-charge.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 30 | 29 | 0 | 0 |
| fixtures/phase8/scenarios/checkout-idempotent-two-requests.json | COUNTEREXAMPLE_FOUND | 15 | 14 | 0 | 0 |
| fixtures/phase8/scenarios/inventory-notify-after-commit.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 93 | 112 | 0 | 0 |
| fixtures/phase8/scenarios/inventory-retry-duplicate-notice.json | COUNTEREXAMPLE_FOUND | 24 | 23 | 0 | 0 |
| fixtures/phase8/scenarios/inventory-retry-exhausted.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 43 | 47 | 0 | 0 |
| fixtures/phase8/scenarios/job-claimed-effect-concurrent.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 24 | 23 | 0 | 0 |
| fixtures/phase8/scenarios/job-claimed-effect-redelivery.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 21 | 20 | 0 | 0 |
| fixtures/phase8/scenarios/job-concurrent-claims-duplicate.json | COUNTEREXAMPLE_FOUND | 9 | 8 | 0 | 0 |
| fixtures/phase8/scenarios/job-redelivery-duplicate.json | COUNTEREXAMPLE_FOUND | 11 | 10 | 0 | 0 |
| fixtures/phase8/scenarios/pgjs-accept-crash-lock-holder.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 121 | 197 | 1 | 1 |
| fixtures/phase8/scenarios/pgjs-accept-crash-while-waiting.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 121 | 197 | 1 | 1 |
| fixtures/phase8/scenarios/pgjs-accept-invitation-twice.json | COUNTEREXAMPLE_FOUND | 15 | 14 | 0 | 0 |
| fixtures/phase8/scenarios/pgjs-checkout-crash-after-charge.json | COUNTEREXAMPLE_FOUND | 11 | 10 | 0 | 0 |
| fixtures/phase8/scenarios/pgjs-inventory-retry-duplicate-notice.json | COUNTEREXAMPLE_FOUND | 24 | 23 | 0 | 0 |
| fixtures/phase8/scenarios/pgjs-invite-double-submit.json | COUNTEREXAMPLE_FOUND | 19 | 18 | 0 | 0 |
| fixtures/phase8/scenarios/shipping-dual-write-crash.json | COUNTEREXAMPLE_FOUND | 4 | 3 | 0 | 0 |
| fixtures/phase8/scenarios/shipping-outbox-no-loss.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 29 | 32 | 0 | 0 |
| fixtures/phase8/scenarios/shipping-outbox-relay-duplicate.json | COUNTEREXAMPLE_FOUND | 13 | 12 | 0 | 0 |

## 2. Actor scaling: team-app action, hypothetical UNIQUE(teamId, email)

| actors | binary | states | transitions | wall ms | peak RSS MiB | visited bytes | bytes / state | MiB / million states |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 2 | Phase 8 | 40 | 39 | 0 | 8.2 | 75804 | 1895 | 205859 |
| 2 | Phase 9 | 40 | 39 | 0 | 8.3 | 15809 | 395 | 208594 |
| 3 | Phase 8 | 280 | 294 | 4 | 9.5 | 738920 | 2639 | 33984 |
| 3 | Phase 9 | 280 | 294 | 4 | 8.7 | 50697 | 181 | 30971 |
| 4 | Phase 8 | 1984 | 2291 | 40 | 17.0 | 6669280 | 3362 | 8545 |
| 4 | Phase 9 | 1984 | 2291 | 40 | 9.4 | 272453 | 137 | 4733 |
| 5 | Phase 8 | 13984 | 18042 | 360 | 90.9 | 56858880 | 4066 | 6501 |
| 5 | Phase 9 | 13984 | 18042 | 342 | 12.1 | 1680001 | 120 | 866 |
| 6 | Phase 8 | 97408 | 140975 | 3222 | 742.9 | 463456448 | 4758 | 7627 |
| 6 | Phase 9 | 97408 | 140975 | 2997 | 26.7 | 14034161 | 144 | 274 |
| 7 | Phase 8 | 669568 | 1084010 | 28979 | 2934.5 | 3643533952 | 5442 | 4383 |
| 7 | Phase 9 | 669568 | 1084010 | 25240 | 118.5 | 113256237 | 169 | 177 |
| 8 | Phase 8 | not run | | | | | | extrapolated ≈ 23581 MiB of visited keys alone |
| 8 | Phase 9 | 4544512 | 8183475 | 207387 | 532.4 | 880814441 | 194 | 117 |

## 3. Full search, sleep sets, DPOR (this build)

| workload | mode | verdict | states stored | transitions executed | wall ms | peak RSS MiB |
| --- | --- | --- | --- | --- | --- | --- |
| team-app, 3 actors (conflicting inserts) | full (no reduction) | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 280 | 588 | 5 | 8.7 |
| team-app, 3 actors (conflicting inserts) | sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 280 | 294 | 4 | 8.6 |
| team-app, 3 actors (conflicting inserts) | DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 6626 | 6625 | 55 | 8.5 |
| team-app, 4 actors (conflicting inserts) | full (no reduction) | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 1984 | 5344 | 50 | 9.5 |
| team-app, 4 actors (conflicting inserts) | sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 1984 | 2291 | 35 | 9.4 |
| team-app, 4 actors (conflicting inserts) | DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 3476615 | 3476614 | 30362 | 8.9 |
| independent actors, 2 (disjoint tables) | full (no reduction) | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 16 | 24 | 0 | 4.7 |
| independent actors, 2 (disjoint tables) | sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 16 | 15 | 0 | 4.7 |
| independent actors, 2 (disjoint tables) | DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 7 | 6 | 0 | 4.6 |
| independent actors, 3 (disjoint tables) | full (no reduction) | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 64 | 144 | 0 | 4.9 |
| independent actors, 3 (disjoint tables) | sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 64 | 63 | 0 | 4.9 |
| independent actors, 3 (disjoint tables) | DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 10 | 9 | 0 | 4.7 |
| independent actors, 4 (disjoint tables) | full (no reduction) | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 256 | 768 | 3 | 5.0 |
| independent actors, 4 (disjoint tables) | sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 256 | 255 | 2 | 5.1 |
| independent actors, 4 (disjoint tables) | DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 13 | 12 | 0 | 4.8 |
| 2 clients, crash + retry + in-flight COMMIT (idempotent) | full (no reduction) | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 792 | 1552 | 4 | 5.2 |
| 2 clients, crash + retry + in-flight COMMIT (idempotent) | sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 792 | 1336 | 3 | 5.2 |
| 2 clients, crash + retry + in-flight COMMIT (idempotent) | DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 21898 | 21897 | 62 | 5.1 |
| 2 tasks of one process, pool of 1 | full (no reduction) | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 7 | 6 | 0 | 4.5 |
| 2 tasks of one process, pool of 1 | sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 7 | 6 | 0 | 4.5 |
| 2 tasks of one process, pool of 1 | DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 7 | 6 | 0 | 4.4 |

## 4. In-flight faults on the Phase 8 delivery scenarios (this build)

| scenario | maxInFlight | verdict | states | transitions |
| --- | --- | --- | --- | --- |
| shipping-dual-write-crash | 0 | COUNTEREXAMPLE_FOUND | 4 | 3 |
| shipping-dual-write-crash | 1 | COUNTEREXAMPLE_FOUND | 4 | 3 |
| shipping-outbox-no-loss | 0 | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 29 | 32 |
| shipping-outbox-no-loss | 1 | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 72 | 80 |
| shipping-outbox-relay-duplicate | 0 | COUNTEREXAMPLE_FOUND | 13 | 12 |
| shipping-outbox-relay-duplicate | 1 | COUNTEREXAMPLE_FOUND | 16 | 15 |
| job-redelivery-duplicate | 0 | COUNTEREXAMPLE_FOUND | 11 | 10 |
| job-redelivery-duplicate | 1 | COUNTEREXAMPLE_FOUND | 12 | 11 |
| job-claimed-effect-redelivery | 0 | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 21 | 20 |
| job-claimed-effect-redelivery | 1 | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 90 | 105 |
| checkout-idempotent-crash-after-charge | 0 | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 30 | 29 |
| checkout-idempotent-crash-after-charge | 1 | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 83 | 95 |
| checkout-crash-after-charge | 0 | COUNTEREXAMPLE_FOUND | 11 | 10 |
| checkout-crash-after-charge | 1 | COUNTEREXAMPLE_FOUND | 14 | 13 |

## 5. Verification cost (process wall time, ms; median of 3)

| scenario | model only | --validate-postgres | --verify-source (1 run) |
| --- | --- | --- | --- |
| checkout-crash-after-charge.json | 9 | 947 | 1787 |
| ledger-blind-commit-in-flight.json | 10 | 902 | 1683 |
| ledger-reconciled-commit-in-flight.json | 10 | 966 | 1877 |
| ledger-shared-process-crash.json | 11 | 948 | 1410 |
| ledger-pool-starvation.json | 9 | 922 | 2173 |

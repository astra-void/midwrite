# Phase 8 model-checker measurements

macOS-27.0.1-arm64-arm-64bit-Mach-O; median of 5 runs unless noted; `wall ms` is the exploration time the report states.

## 1. Phase 6 fixture scenarios

| scenario | verdict | states | transitions | Phase 7 wall ms | Phase 8 wall ms |
| --- | --- | --- | --- | --- | --- |
| saas-starter-invite-double-submit.json | COUNTEREXAMPLE_FOUND | 7 | 6 | 0 | 0 |
| saas-starter-invite-unique-mitigation.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 12 | 11 | 0 | 0 |
| doctors-write-skew-read-committed.json | COUNTEREXAMPLE_FOUND | 12 | 11 | 0 | 0 |
| doctors-write-skew-repeatable-read.json | COUNTEREXAMPLE_FOUND | 12 | 11 | 0 | 0 |
| doctors-write-skew-serializable.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 30 | 33 | 0 | 0 |
| inventory-atomic-update.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 9 | 8 | 0 | 0 |
| inventory-oversell-in-transaction.json | COUNTEREXAMPLE_FOUND | 14 | 13 | 0 | 0 |
| inventory-oversell-repeatable-read.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 37 | 41 | 0 | 0 |
| inventory-oversell.json | COUNTEREXAMPLE_FOUND | 11 | 10 | 0 | 0 |
| inventory-row-lock.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 30 | 34 | 0 | 0 |
| job-double-processing.json | COUNTEREXAMPLE_FOUND | 28 | 33 | 0 | 0 |
| payment-crash-retry-provider-dedup.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 7 | 6 | 0 | 0 |
| payment-crash-retry.json | COUNTEREXAMPLE_FOUND | 7 | 6 | 0 | 0 |
| redeem-atomic.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 5 | 4 | 0 | 0 |
| redeem-check-then-act-unique.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 12 | 11 | 0 | 0 |
| redeem-check-then-act.json | COUNTEREXAMPLE_FOUND | 7 | 6 | 0 | 0 |

## 2. Exhaustive exploration, extracted team-app action, hypothetical UNIQUE(teamId, email)

| actors | POR | states | transitions | Phase 7 wall ms | Phase 8 wall ms | Phase 7 peak RSS MiB | Phase 8 peak RSS MiB |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 2 | on | 40 | 39 | 1 | 0 | 8.1 | 8.3 |
| 2 | off | 40 | 60 | 1 | 0 | 8.1 | 8.2 |
| 3 | on | 280 | 294 | 9 | 5 | 9.6 | 9.6 |
| 3 | off | 280 | 588 | 11 | 6 | 9.4 | 9.4 |
| 4 | on | 1984 | 2291 | 97 | 48 | 17.6 | 17.1 |
| 4 | off | 1984 | 5344 | 124 | 65 | 17.2 | 16.9 |
| 5 | on | 13984 | 18042 | 936 | 427 | 92.3 | 91.2 |
| 5 | off | 13984 | 46160 | 1200 | 604 | 91.8 | 91.0 |
| 6 | on | 97408 | 140975 | 8968 | 3756 | 740.9 | 744.4 |

## 3. Crash and retry dimensions (extracted idempotent checkout, exhaustive)

| clients | maxCrashes | maxRetries | verdict | states | transitions | Phase 8 wall ms |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 0 | 0 | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 6 | 5 | 0 |
| 1 | 1 | 1 | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 30 | 29 | 0 |
| 1 | 2 | 2 | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 96 | 96 | 0 |
| 2 | 0 | 0 | COUNTEREXAMPLE_FOUND | 15 | 14 | 0 |
| 2 | 1 | 1 | COUNTEREXAMPLE_FOUND | 21 | 23 | 0 |
| 2 | 2 | 2 | COUNTEREXAMPLE_FOUND | 21 | 23 | 0 |
| 3 | 0 | 0 | COUNTEREXAMPLE_FOUND | 20 | 19 | 0 |
| 3 | 1 | 1 | COUNTEREXAMPLE_FOUND | 32 | 43 | 0 |
| 3 | 2 | 2 | COUNTEREXAMPLE_FOUND | 32 | 43 | 0 |

## 4. Verification cost (process wall time, ms; median of 3)

| scenario | model only | --validate-postgres | --verify-source (1 run) |
| --- | --- | --- | --- |
| team-app-invite-double-submit.json | 15 | 1127 | 2001 |
| pgjs-invite-double-submit.json | 16 | 1120 | 2015 |
| checkout-crash-after-charge.json | 12 | 1126 | 2139 |
| pgjs-checkout-crash-after-charge.json | 12 | 1141 | 2085 |
| accept-crash-while-waiting.json | 14 | 14 | 2358 |

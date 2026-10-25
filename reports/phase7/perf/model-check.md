# Phase 7 model-checker measurements

Median of 5 runs; `wallMs` is the exploration time the report states.

## Phase 6 vs Phase 7 binary, Phase 6 fixture scenarios

| scenario | verdict | states | transitions | Phase 6 wall ms | Phase 7 wall ms |
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

## Extracted team-app invite scenario, more submitters

| actors | verdict | states | transitions | wall ms (exploration) |
| --- | --- | --- | --- | --- |
| 2 | COUNTEREXAMPLE_FOUND | 19 | 18 | 0 |
| 3 | COUNTEREXAMPLE_FOUND | 46 | 45 | 1 |
| 4 | COUNTEREXAMPLE_FOUND | 145 | 144 | 6 |

## Runtime verification cost (process wall time, ms)

| scenario | model only | --validate-postgres | --verify-source (1 run) | --verify-source --source-repeat 5 |
| --- | --- | --- | --- | --- |
| team-app-invite-double-submit.json | 14 | 1081 | 1914 | 9395 |
| team-app-accept-invitation-twice.json | 11 | 1050 | 1893 | 9411 |

## Exhaustive exploration: hypothetical UNIQUE(teamId, email) on invitations (model only; the application's schema has no such constraint)

| actors | POR | verdict | states | transitions | wall ms |
| --- | --- | --- | --- | --- | --- |
| 2 | on | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 40 | 39 | 0 |
| 2 | off | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 40 | 60 | 1 |
| 3 | on | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 280 | 294 | 7 |
| 3 | off | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 280 | 588 | 10 |
| 4 | on | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 1984 | 2291 | 83 |
| 4 | off | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 1984 | 5344 | 112 |

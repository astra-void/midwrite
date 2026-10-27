# Phase 11 evidence

Binary: `commitrift 0.1.0`, sha256 `6141d12d689ca316…`.

## First-party benchmark (fixtures/phase11/labels.json)

| entry | label | outcome | own candidates |
| --- | --- | --- | --- |
| `src/app/invitations.ts#inviteMember/arg[1]` | positive | TP | check-then-act:MODEL_COUNTEREXAMPLE |
| `src/app/invitations.ts#inviteSeat/arg[1]` | negative | TN | check-then-act:NO_VIOLATION_WITHIN_BOUNDS |
| `src/app/inventory.ts#reserveItem` | positive | TP | read-modify-write:MODEL_COUNTEREXAMPLE |
| `src/app/inventory.ts#reserveItemAtomic` | negative | TN | — |
| `src/app/inventory.ts#reserveItemLocked` | negative | TN | read-modify-write:NO_VIOLATION_WITHIN_BOUNDS |
| `src/services/orders.ts#claimTask` | positive | TP | status-transition:MODEL_COUNTEREXAMPLE |
| `src/services/orders.ts#claimTaskConditional` | negative | TN | — |
| `src/services/orders.ts#endShift` | positive | TP | write-skew:MODEL_COUNTEREXAMPLE |
| `src/services/orders.ts#endShiftSerializable` | negative | TN | write-skew:NO_VIOLATION_WITHIN_BOUNDS |
| `src/services/jobs.ts#processRefund` | positive | TP | duplicate-processing:MODEL_COUNTEREXAMPLE, status-transition:MODEL_COUNTEREXAMPLE |
| `src/services/jobs.ts#processRefundOnce` | negative | TN | duplicate-processing:NO_VIOLATION_WITHIN_BOUNDS, status-transition:NO_VIOLATION_WITHIN_BOUNDS |
| `src/app/checkout.ts#checkout` | positive | TP | external-effect-gap:MODEL_COUNTEREXAMPLE, status-transition:MODEL_COUNTEREXAMPLE |
| `src/app/checkout.ts#checkoutIdempotent` | negative | FP | status-transition:MODEL_COUNTEREXAMPLE, external-effect-gap:NO_VIOLATION_WITHIN_BOUNDS |
| `src/services/orders.ts#recordPayment` | positive | TP | ambiguous-commit:MODEL_COUNTEREXAMPLE |
| `src/services/orders.ts#recordPaymentIdempotent` | negative | TN | ambiguous-commit:NO_VIOLATION_WITHIN_BOUNDS |
| `src/app/inventory.ts#transferStock` | positive | TP | lock-order:MODEL_COUNTEREXAMPLE |
| `src/app/inventory.ts#transferStockOrdered` | negative | TN | lock-order:NO_VIOLATION_WITHIN_BOUNDS |
| `src/services/reports.ts#stockLevel` | negative | TN | — |
| `src/services/reports.ts#archiveOrder` | unsupported | reported | — |
| `src/services/reports.ts#notifyCustomer` | unsupported | reported | — |
| `src/app/api/checkout/route.ts#POST` | unsupported | reported | — |

staticHazardPrediction: TP 8, FP 2, FN 0, TN 8, precision 0.8, recall 1.0, F1 0.889

modelPrediction: TP 8, FP 1, FN 0, TN 9, precision 0.889, recall 1.0, F1 0.941

class agreement: {'agrees': 7, 'differs': 2}; unsupported reported 3, missed 0; candidate-to-model 20/20

Summary: `{"byClass": {"ambiguous-commit": 2, "check-then-act": 2, "duplicate-processing": 2, "external-effect-gap": 2, "lock-order": 2, "read-modify-write": 5, "status-transition": 5, "write-skew": 2}, "byPropertyOrigin": {"FRAMEWORK_CONTRACT": 2, "HEURISTIC_HYPOTHESIS": 16, "SCHEMA_ENFORCED": 2, "USER_DECLARED": 2}, "byStatus": {"MODEL_COUNTEREXAMPLE": 12, "NO_VIOLATION_WITHIN_BOUNDS": 8, "UNSUPPORTED": 2}, "candidates": 22, "counterexamplesAgainstHypotheses": 10, "deduplicatedFindings": 11, "modelCheckable": 20, "modelCounterexamples": 12, "requiringAssumptions": 22, "unknownOperationBreakdown": {"other": 3, "unresolved identifier": 3, "unresolved method call": 8, "unsupported query (raw SQL)": 1}, "unsupportedBreakdown": {"unsupported operation": 2}, "withDeclaredProperty": 2}`

## Held-out labels (Phase 7/8 applications)

### shop-app

| entry | label | outcome | own candidates |
| --- | --- | --- | --- |
| `src/app/checkout.ts#payOrder` | positive | TP | status-transition:MODEL_COUNTEREXAMPLE, external-effect-gap:MODEL_COUNTEREXAMPLE |
| `src/app/checkout.ts#payOrderIdempotent` | positive | TP | status-transition:MODEL_COUNTEREXAMPLE, external-effect-gap:NO_VIOLATION_WITHIN_BOUNDS |
| `src/app/inventory.ts#reserveStock` | positive | TP | external-effect-gap:MODEL_COUNTEREXAMPLE, read-modify-write:NO_VIOLATION_WITHIN_BOUNDS |
| `src/app/inventory.ts#reserveStockNotifyAfterCommit` | negative | TN | read-modify-write:NO_VIOLATION_WITHIN_BOUNDS |
| `src/app/inventory.ts#reserveStockOnce` | negative | TN | read-modify-write:NO_VIOLATION_WITHIN_BOUNDS |
| `src/app/jobs.ts#sendReceipt` | positive | TP | duplicate-processing:MODEL_COUNTEREXAMPLE, status-transition:MODEL_COUNTEREXAMPLE |
| `src/app/jobs.ts#sendReceiptOnce` | negative | TN | duplicate-processing:NO_VIOLATION_WITHIN_BOUNDS, status-transition:NO_VIOLATION_WITHIN_BOUNDS |
| `src/app/shipping.ts#shipOrderDualWrite` | positive | FN | — |
| `src/app/shipping.ts#relayOutbox` | positive | TP | status-transition:MODEL_COUNTEREXAMPLE, external-effect-gap:MODEL_COUNTEREXAMPLE |
| `src/app/shipping.ts#shipOrderWithOutbox` | negative | TN | — |

model prediction: TP 5, FP 0, FN 1, TN 4, precision 1.0, recall 0.833

### team-app

| entry | label | outcome | own candidates |
| --- | --- | --- | --- |
| `src/app/actions.ts#inviteTeamMember/arg[1]` | positive | TP | check-then-act:MODEL_COUNTEREXAMPLE |
| `src/app/actions.ts#acceptInvitation/arg[1]` | positive | TP | status-transition:MODEL_COUNTEREXAMPLE |

model prediction: TP 2, FP 0, FN 0, TN 0, precision 1.0, recall 1.0

## The same application without commitrift-contracts.json

`{"byClass": {"ambiguous-commit": 2, "check-then-act": 2, "lock-order": 2, "read-modify-write": 5, "status-transition": 4, "write-skew": 2}, "byPropertyOrigin": {"HEURISTIC_HYPOTHESIS": 13, "SCHEMA_ENFORCED": 2}, "byStatus": {"BLOCKED": 2, "MODEL_COUNTEREXAMPLE": 6, "NO_VIOLATION_WITHIN_BOUNDS": 4, "UNSUPPORTED": 5}, "candidates": 17, "counterexamplesAgainstHypotheses": 6, "deduplicatedFindings": 5, "modelCheckable": 10, "modelCounterexamples": 6, "requiringAssumptions": 17, "unknownOperationBreakdown": {"other": 2, "unresolved identifier": 2, "unresolved method call": 11, "unsupported query (raw SQL)": 1}, "unsupportedBreakdown": {"missing invariant": 2, "unknown external effect": 3, "unsupported operation": 2}, "withDeclaredProperty": 0}`

## Generated counterexamples on the original code

Each scenario 3 times, `commitrift model-check <file> --verify-source --allow-execution --source-repeat 3`.

| scenario | candidate | model | evidence | runs reproduced |
| --- | --- | --- | --- | --- |
| `cr-synth-bedc65-actual.json` | CR-SYNTH-BEDC65 (duplicate-processing) | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 |
| `cr-synth-1c8df2-actual.json` | CR-SYNTH-1C8DF2 (status-transition) | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 |
| `cr-synth-1c8df2-distinct-request-key.json` | CR-SYNTH-1C8DF2 (status-transition) | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 |
| `cr-synth-3e97d8-actual.json` | CR-SYNTH-3E97D8 (external-effect-gap) | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 |
| `cr-synth-5e3d04-actual.json` | CR-SYNTH-5E3D04 (status-transition) | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 |
| `cr-synth-eef9ea-actual.json` | CR-SYNTH-EEF9EA (status-transition) | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 |
| `cr-synth-199193-actual.json` | CR-SYNTH-199193 (check-then-act) | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 |
| `cr-synth-05e13a-actual.json` | CR-SYNTH-05E13A (read-modify-write) | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 |
| `cr-synth-2551df-actual.json` | CR-SYNTH-2551DF (status-transition) | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 |
| `cr-synth-9336e1-actual.json` | CR-SYNTH-9336E1 (read-modify-write) | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 |

## Corpus

| project | modules | entry candidates | extracted | effectful | unknown ops | tables modeled / refused | candidates | counterexamples | wall | peak RSS |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| saas-starter | 41 | 44 | 37 | 8 | 274 | 5 / 0 | 2 | 1 | 0.07 s | 10.8 MiB |
| midday | 2022 | 2864 | 1168 | 0 | 15382 | 8 / 42 | 0 | 0 | 28.58 s | 72.3 MiB |
| school-ledger | 461 | 880 | 546 | 0 | 9029 | 9 / 21 | 0 | 0 | 3.51 s | 49.3 MiB |
| unnamed-ai-project | 104 | 74 | 33 | 1 | 1009 | 7 / 2 | 0 | 0 | 0.08 s | 11.7 MiB |

Unknown operations by category:

| category | saas-starter | midday | school-ledger | unnamed-ai-project |
| --- | --- | --- | --- | --- |
| call into an unanalyzed package | 55 | 4136 | 521 | 67 |
| column not modeled (schema) | 12 | 0 | 0 | 6 |
| inline depth limit | 0 | 25 | 321 | 20 |
| loop over a collection of unknown length | 0 | 133 | 305 | 54 |
| other | 12 | 588 | 423 | 30 |
| re-export not followed | 0 | 82 | 294 | 0 |
| table not modeled (schema) | 0 | 0 | 0 | 43 |
| unresolved call | 4 | 538 | 328 | 0 |
| unresolved identifier | 0 | 659 | 517 | 7 |
| unresolved import | 0 | 1132 | 343 | 0 |
| unresolved method call | 123 | 5433 | 4149 | 625 |
| unsupported binding pattern | 4 | 706 | 269 | 16 |
| unsupported expression | 47 | 1631 | 1455 | 139 |
| unsupported query shape | 17 | 1 | 0 | 2 |
| unsupported transaction shape | 0 | 318 | 104 | 0 |

## Cost centers and timings

Median of 3 runs; peak RSS is the maximum.

| what | wall | peak RSS |
| --- | --- | --- |
| commerce-app: discovery only | 0.06 s | 10.1 MiB |
| commerce-app: discovery + model checking | 0.08 s | 10.5 MiB |
| commerce-app: discovery + model checking, source-DPOR | 0.08 s | 10.5 MiB |
| commerce-app: commitrift check (static analysis) | 0.01 s | 7.3 MiB |
| commerce-app: commitrift check, Phase 10 binary | 0.01 s | 7.3 MiB |
| midday: commitrift check | 0.22 s | 94.9 MiB |
| midday: commitrift check, Phase 10 binary | 0.21 s | 91.7 MiB |
| midday: discovery only | 30.03 s | 73.1 MiB |

commerce-app phase timings (one run): `{"detectionMs": 0, "extractionMs": 22, "indexMs": 5, "modelCheckingMs": 8, "synthesisMs": 9, "totalMs": 77}`

Extraction cache on midday (discovery only):

* cold: 36.60 s, 1168 misses
* warm: 7.71 s, 1168 hits, 0 misses
* reports equal apart from timings and cache counters: True


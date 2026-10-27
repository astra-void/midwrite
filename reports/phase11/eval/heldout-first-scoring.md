# Held-out labels: first scoring (before any change prompted by them)

Recorded 2026-10-27 from the first run of `commitrift discover --model-check
--labels` on the Phase 7/8 applications with
`fixtures/phase11/heldout/*.contracts.json`. After this scoring the engine was
changed because of what it showed (the `rows.length` idiom, joined tables,
join-aware seeding, read-derived keys, state tests in the read's WHERE
clause, serial outcomes with failed actors, effects inside retried
transactions). The later scores on the same labels are therefore NOT
held-out results; both are reported in docs/phase11/PRECISION-RECALL.md.

## shop-app (model prediction)

| entry | outcome | own candidates |
| --- | --- | --- |
| payOrder | TP | status-transition, external-effect-gap: counterexample |
| payOrderIdempotent | FN | status-transition, external-effect-gap: no violation |
| reserveStock | TP | read-modify-write: counterexample |
| reserveStockNotifyAfterCommit | FP | read-modify-write: counterexample |
| reserveStockOnce | FP | read-modify-write: counterexample |
| sendReceipt | TP | duplicate-processing, status-transition: counterexample |
| sendReceiptOnce | TN | duplicate-processing, status-transition: no violation |
| shipOrderDualWrite | FN | none |
| relayOutbox | TP | external-effect-gap: counterexample |
| shipOrderWithOutbox | TN | none |

TP 4, FP 2, FN 2, TN 2: precision 0.667, recall 0.667, F1 0.667.

## team-app (model prediction)

| entry | outcome | own candidates |
| --- | --- | --- |
| inviteTeamMember | FN | none |
| acceptInvitation | FN | none |

TP 0, FN 2: recall 0.0.

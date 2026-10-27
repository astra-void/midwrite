# Phase 11 benchmarks under the Phase 12 binary (regression check, not a new estimate)



first-party (commerce-app): model {'truePositives': 8, 'falsePositives': 1, 'falseNegatives': 0, 'trueNegatives': 9, 'precision': 0.889, 'recall': 1.0, 'f1': 0.941}; static {'truePositives': 8, 'falsePositives': 2, 'falseNegatives': 0, 'trueNegatives': 8, 'precision': 0.8, 'recall': 1.0, 'f1': 0.889}; unsupported reported 3, missed 0

held-out shop-app: {'truePositives': 5, 'falsePositives': 0, 'falseNegatives': 1, 'trueNegatives': 4, 'precision': 1.0, 'recall': 0.833, 'f1': 0.909}; outcomes [('payOrder', 'TP'), ('payOrderIdempotent', 'TP'), ('reserveStock', 'TP'), ('reserveStockNotifyAfterCommit', 'TN'), ('reserveStockOnce', 'TN'), ('sendReceipt', 'TP'), ('sendReceiptOnce', 'TN'), ('shipOrderDualWrite', 'FN'), ('relayOutbox', 'TP'), ('shipOrderWithOutbox', 'TN')]

held-out team-app: {'truePositives': 2, 'falsePositives': 0, 'falseNegatives': 0, 'trueNegatives': 0, 'precision': 1.0, 'recall': 1.0, 'f1': 1.0}; outcomes [('inviteTeamMember/arg[1]', 'TP'), ('acceptInvitation/arg[1]', 'TP')]

held-out total: TP 7 FP 0 FN 1 TN 4; precision 1.000; recall 0.875

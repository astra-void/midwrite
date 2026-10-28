# Earlier benchmarks under the Phase 14 binary (regression check, not a new estimate)

Binary: `target/release/commitrift` of the Phase 14 working tree (2026-10-28, after the post-feedback-2 changes). Model prediction as each phase's scorer defines it.

| set | Phase 13 binary (recorded, `reports/phase13/regression/earlier-benchmarks.md`) | Phase 14 binary |
| --- | --- | --- |
| Phase 11 first-party (commerce-app) | TP 8 FP 1 FN 0 TN 9 | TP 8 FP 1 FN 0 TN 9 (unchanged; `checkoutIdempotent` still FP) |
| Phase 11 held-out shop-app | TP 5 FP 0 FN 1 TN 4 | TP 5 FP 0 FN 1 TN 4 (unchanged) |
| Phase 11 held-out team-app | TP 2 FP 0 FN 0 TN 0 | TP 2 FP 0 FN 0 TN 0 (unchanged) |
| Phase 12 evaluation set | TP 6 FP 0 FN 1 TN 6; unsupported reported 2, missed 0 | TP 6 FP 0 FN 1 TN 6 (`reports/phase14/eval/phase12-set.json`); unsupported: `importTasks` is now **missed** by the scorer's rule, because its loop over an input array is modeled (a bounded synthesized array of at most 3 titles) and no unknown operation remains — the label's premise ("a loop over an input array of unknown length") no longer holds; the frozen label was not edited |
| Phase 13 frozen set | TP 4 FP 0 FN 11 TN 11 (post-feedback) | TP 7 FP 0 FN 8 TN 11 (`reports/phase14/eval/phase13-frozen-set.json`; post-feedback, not held out) |

# Phase 16 performance: Phase 15 binary vs Phase 16 binary (cold / warm cache)

Median of 3 runs (wall), peak RSS the maximum, `discover --model-check`; timings from the report. `warm`: `--cache` populated by one earlier run.

| app | binary | candidates | modelCheckable | counterexamples | worldsBuilt | scenariosModelChecked | statesExplored | synthesisMs | modelCheckingMs | totalMs | wallSecondsMedian | peakRssMiB |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| fixtures/phase13/eval-app | phase15 | 20 | 18 | 10 | 32 | 32 | 1056 | 5 | 6 | 58 | 0.066 | 12.2 |
| fixtures/phase13/eval-app | phase16 cold | 20 | 18 | 10 | 32 | 32 | 1056 | 0 | 6 | 50 | 0.059 | 12.7 |
| fixtures/phase13/eval-app | phase16 warm | 20 | 18 | 10 | 32 | 32 | 1056 | 1 | 6 | 50 | 0.059 | 12.8 |
| fixtures/phase14/eval-app | phase15 | 20 | 18 | 8 | 30 | 30 | 558 | 1 | 0 | 45 | 0.053 | 12.3 |
| fixtures/phase14/eval-app | phase16 cold | 24 | 22 | 10 | 37 | 37 | 621 | 2 | 1 | 56 | 0.065 | 13.4 |
| fixtures/phase14/eval-app | phase16 warm | 24 | 22 | 10 | 37 | 37 | 621 | 3 | 1 | 54 | 0.062 | 13.4 |
| fixtures/phase15/eval-app | phase15 | 19 | 13 | 8 | 28 | 28 | 803 | 5 | 6 | 53 | 0.063 | 12.1 |
| fixtures/phase15/eval-app | phase16 cold | 23 | 19 | 10 | 39 | 39 | 954 | 5 | 7 | 67 | 0.076 | 13.1 |
| fixtures/phase15/eval-app | phase16 warm | 23 | 19 | 10 | 39 | 39 | 954 | 6 | 7 | 66 | 0.074 | 13.3 |
| fixtures/phase15/worlds-app | phase15 | 3 | 3 | 2 | 8 | 8 | 105 | 0 | 1 | 9 | 0.016 | 9.8 |
| fixtures/phase15/worlds-app | phase16 cold | 3 | 3 | 2 | 9 | 9 | 122 | 0 | 1 | 9 | 0.016 | 10.1 |
| fixtures/phase15/worlds-app | phase16 warm | 3 | 3 | 2 | 9 | 9 | 122 | 0 | 1 | 10 | 0.017 | 10.3 |
| fixtures/phase11/commerce-app | phase15 | 22 | 20 | 12 | 32 | 32 | 836 | 0 | 8 | 52 | 0.062 | 12.1 |
| fixtures/phase11/commerce-app | phase16 cold | 23 | 21 | 13 | 34 | 34 | 889 | 0 | 9 | 57 | 0.066 | 12.5 |
| fixtures/phase11/commerce-app | phase16 warm | 23 | 21 | 13 | 34 | 34 | 889 | 2 | 11 | 59 | 0.067 | 12.8 |
| fixtures/phase16/eval-app | phase15 | 4 | 2 | 1 | 3 | 3 | 59 | 0 | 0 | 16 | 0.024 | 12.6 |
| fixtures/phase16/eval-app | phase16 cold | 26 | 12 | 6 | 24 | 24 | 286 | 27 | 4 | 92 | 0.101 | 17.2 |
| fixtures/phase16/eval-app | phase16 warm | 26 | 12 | 6 | 24 | 24 | 286 | 27 | 4 | 88 | 0.098 | 17.1 |
| fixtures/phase16/retry | phase15 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0.008 | 7.9 |
| fixtures/phase16/retry | phase16 cold | 6 | 6 | 2 | 11 | 11 | 298 | 0 | 0 | 14 | 0.022 | 11.0 |
| fixtures/phase16/retry | phase16 warm | 6 | 6 | 2 | 11 | 11 | 298 | 0 | 0 | 15 | 0.022 | 11.3 |
| fixtures/phase16/locks | phase15 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 0.009 | 9.1 |
| fixtures/phase16/locks | phase16 cold | 6 | 6 | 3 | 12 | 12 | 295 | 0 | 2 | 15 | 0.022 | 10.4 |
| fixtures/phase16/locks | phase16 warm | 6 | 6 | 3 | 12 | 12 | 295 | 1 | 2 | 15 | 0.022 | 10.6 |
| fixtures/phase16/nulls | phase15 | 4 | 4 | 2 | 8 | 8 | 72 | 0 | 0 | 7 | 0.014 | 9.6 |
| fixtures/phase16/nulls | phase16 cold | 4 | 4 | 2 | 10 | 10 | 106 | 0 | 0 | 8 | 0.014 | 9.9 |
| fixtures/phase16/nulls | phase16 warm | 4 | 4 | 2 | 10 | 10 | 106 | 0 | 0 | 8 | 0.015 | 10.1 |
| fixtures/phase16/genid | phase15 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| fixtures/phase16/genid | phase16 cold | 14 | 12 | 6 | 16 | 16 | 202 | 0 | 0 | 13 | 0.021 | 10.8 |
| fixtures/phase16/genid | phase16 warm | 14 | 12 | 6 | 16 | 16 | 202 | 0 | 0 | 14 | 0.022 | 10.9 |
| fixtures/phase16/zod/app | phase15 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0.007 | 8.0 |
| fixtures/phase16/zod/app | phase16 cold | 2 | 2 | 1 | 7 | 7 | 75 | 0 | 1 | 10 | 0.017 | 10.3 |
| fixtures/phase16/zod/app | phase16 warm | 2 | 2 | 1 | 7 | 7 | 75 | 1 | 1 | 11 | 0.017 | 10.4 |

* fixtures/phase16/genid with phase15: not run (error: `/Users/returnf4lse/Desktop/Workspace/rust/crash-proof/fixtures/phase16/genid/commitrift-contracts.json`: unknown variant `forEachActor`, expected one of)

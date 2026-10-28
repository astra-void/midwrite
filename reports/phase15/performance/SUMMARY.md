# Phase 15 performance: world search off (1) vs default (8)

Median of 3 runs (wall), peak RSS the maximum, `discover --model-check`; timings from the report.

| app | maxWorlds | candidates | counterexamples | counterexamplesInNonBaseWorlds | worldsBuilt | scenariosModelChecked | statesExplored | synthesisMs | modelCheckingMs | msPerCandidate | wallSecondsMedian | peakRssMiB |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| fixtures/phase13/eval-app | 1 | 20 | 9 | 0 | 30 | 30 | 1015 | 7 | 6 | 2.8 | 0.068 | 12.2 |
| fixtures/phase13/eval-app | 8 | 20 | 10 | 1 | 32 | 32 | 1056 | 16 | 10 | 4.0 | 0.082 | 12.3 |
| fixtures/phase14/eval-app | 1 | 20 | 7 | 0 | 28 | 28 | 520 | 6 | 1 | 2.5 | 0.061 | 12.3 |
| fixtures/phase14/eval-app | 8 | 20 | 8 | 1 | 30 | 30 | 558 | 10 | 1 | 3.0 | 0.073 | 12.5 |
| fixtures/phase15/eval-app | 1 | 19 | 4 | 0 | 20 | 20 | 685 | 8 | 5 | 2.6 | 0.059 | 12.1 |
| fixtures/phase15/eval-app | 8 | 19 | 8 | 4 | 28 | 28 | 803 | 12 | 6 | 3.6 | 0.075 | 12.2 |
| fixtures/phase15/worlds-app | 1 | 3 | 0 | 0 | 6 | 6 | 87 | 0 | 0 | 2.7 | 0.018 | 9.9 |
| fixtures/phase15/worlds-app | 8 | 3 | 2 | 2 | 8 | 8 | 105 | 0 | 1 | 4.0 | 0.021 | 10.0 |
| fixtures/phase11/commerce-app | 1 | 22 | 12 | 0 | 31 | 31 | 800 | 10 | 11 | 3.3 | 0.076 | 12.2 |
| fixtures/phase11/commerce-app | 8 | 22 | 12 | 0 | 32 | 32 | 836 | 7 | 10 | 3.0 | 0.075 | 12.2 |

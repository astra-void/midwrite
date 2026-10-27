# Phase 12 corpus: Phase 11 binary vs Phase 12

Median of 3 runs (wall), peak RSS the maximum; discovery with bounded model checking; no project executed.

| project | phase | modules | entry cand. | extracted | effectful | tables modeled / refused | generated cols | unknown ops | candidates | model-checkable | counterexamples | wall | peak RSS | ms / extracted entry |
| --- | --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| saas-starter | phase11 | 41 | 44 | 37 | 8 | 5 / 0 | 0 | 274 | 2 | 1 | 1 | 0.06 s | 11 MiB | 1.62 |
| saas-starter | phase12 | 41 | 44 | 37 | 8 | 5 / 0 | 0 | 284 | 2 | 1 | 1 | 0.03 s | 11 MiB | 0.81 |
| midday | phase11 | 2022 | 2864 | 1168 | 0 | 8 / 42 | 0 | 15382 | 0 | 0 | 0 | 28.07 s | 73 MiB | 24.03 |
| midday | phase12 | 2022 | 2864 | 1168 | 0 | 50 / 0 | 48 | 17603 | 0 | 0 | 0 | 1.13 s | 112 MiB | 0.97 |
| school-ledger | phase11 | 461 | 880 | 546 | 0 | 9 / 21 | 0 | 9029 | 0 | 0 | 0 | 3.36 s | 50 MiB | 6.15 |
| school-ledger | phase12 | 461 | 880 | 546 | 21 | 30 / 0 | 21 | 12894 | 9 | 0 | 0 | 0.53 s | 81 MiB | 0.97 |
| unnamed-ai-project | phase11 | 104 | 74 | 33 | 1 | 7 / 2 | 0 | 1009 | 0 | 0 | 0 | 0.07 s | 11 MiB | 2.12 |
| unnamed-ai-project | phase12 | 104 | 74 | 33 | 3 | 9 / 0 | 2 | 1009 | 0 | 0 | 0 | 0.03 s | 12 MiB | 0.91 |

## Candidate statuses

* saas-starter phase11: {'MODEL_COUNTEREXAMPLE': 1, 'INCONCLUSIVE': 1}
* saas-starter phase12: {'MODEL_COUNTEREXAMPLE': 1, 'UNSUPPORTED': 1}
* midday phase11: {}
* midday phase12: {}
* school-ledger phase11: {}
* school-ledger phase12: {'UNSUPPORTED': 9}
* unnamed-ai-project phase11: {}
* unnamed-ai-project phase12: {}

## Unknown operations by category (Phase 12)

| category | saas-starter | midday | school-ledger | unnamed-ai-project |
| --- | ---: | ---: | ---: | ---: |
| call into an unanalyzed package | 55 | 4335 | 614 | 67 |
| column not modeled (schema) | 12 | 6 | 157 | 9 |
| database handle of an unresolved type | 0 | 752 | 173 | 0 |
| inline depth limit | 0 | 179 | 871 | 20 |
| loop over a collection of unknown length | 0 | 167 | 413 | 54 |
| other | 17 | 1984 | 2555 | 77 |
| table not modeled (schema) | 0 | 0 | 22 | 13 |
| unresolved call | 4 | 576 | 606 | 0 |
| unresolved identifier | 0 | 717 | 646 | 7 |
| unresolved method call | 133 | 6053 | 4603 | 625 |
| unsupported binding pattern | 4 | 718 | 321 | 16 |
| unsupported expression | 42 | 1679 | 1706 | 119 |
| unsupported query (raw SQL) | 0 | 0 | 60 | 0 |
| unsupported query shape | 17 | 3 | 20 | 2 |
| unsupported transaction shape | 0 | 434 | 127 | 0 |

## midday extraction cache (cold, then warm)

| phase | cold wall | cold RSS | warm wall | warm RSS | warm hits | same candidates | extracted | effectful |
| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: |
| phase11 | 34.39 s | 100 MiB | 7.21 s | 92 MiB | 1168 | True | 1168 | 0 |
| phase12 | 1.54 s | 161 MiB | 1.06 s | 124 MiB | 1168 | True | 1168 | 0 |

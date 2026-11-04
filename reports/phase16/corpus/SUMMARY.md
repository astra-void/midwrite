# Phase 16 corpus: Phase 15 binary vs Phase 16 binary

Median of 3 runs (wall), peak RSS the maximum; `commitrift discover --model-check`; no project executed. `phase15-declared`/`phase16-declared`: with `corpora/contracts/midday.json` (the declared replica factory).

| project | config | extracted | effectful | read-only | resources | unknown ops | candidates | model-checkable | counterexamples | statuses | wall | peak RSS |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: |
| saas-starter | phase15 | 37 | 9 | 11 | 23 | 237 | 2 | 1 | 1 | {'MODEL_COUNTEREXAMPLE': 1, 'UNSUPPORTED': 1} | 0.03 s | 13 MiB |
| saas-starter | phase16 | 37 | 9 | 11 | 23 | 236 | 2 | 1 | 1 | {'MODEL_COUNTEREXAMPLE': 1, 'UNSUPPORTED': 1} | 0.03 s | 13 MiB |
| midday | phase15 | 1168 | 0 | 0 | 0 | 18954 | 0 | 0 | 0 | {} | 1.19 s | 144 MiB |
| midday | phase16 | 1168 | 0 | 0 | 0 | 34333 | 0 | 0 | 0 | {} | 1.57 s | 217 MiB |
| midday | phase15-declared | 1168 | 81 | 63 | 123 | 18861 | 4 | 0 | 0 | {'INCONCLUSIVE': 1, 'BLOCKED': 3} | 1.23 s | 149 MiB |
| midday | phase16-declared | 1168 | 86 | 84 | 123 | 35302 | 4 | 1 | 0 | {'NO_VIOLATION_WITHIN_BOUNDS': 1, 'UNSUPPORTED': 1, 'BLOCKED': 2} | 1.77 s | 230 MiB |
| school-ledger | phase15 | 546 | 70 | 27 | 68 | 13396 | 9 | 0 | 0 | {'UNSUPPORTED': 9} | 0.72 s | 121 MiB |
| school-ledger | phase16 | 546 | 70 | 38 | 68 | 14061 | 9 | 0 | 0 | {'UNSUPPORTED': 9} | 0.81 s | 131 MiB |
| unnamed-ai-project | phase15 | 33 | 3 | 7 | 3 | 881 | 0 | 0 | 0 | {} | 0.03 s | 14 MiB |
| unnamed-ai-project | phase16 | 33 | 3 | 8 | 3 | 844 | 0 | 0 | 0 | {} | 0.03 s | 15 MiB |

## Database statements in extracted entries (from the extraction cache)

| project | config | statements |
| --- | --- | --- |
| saas-starter | phase15 | {'pool delete': 2, 'pool insert': 13, 'pool select': 25, 'pool update': 4} |
| saas-starter | phase16 | {'pool delete': 2, 'pool insert': 13, 'pool select': 25, 'pool update': 4} |
| midday | phase15 | {} |
| midday | phase16 | {} |
| midday | phase15-declared | {'pool delete': 42, 'pool insert': 26, 'pool update': 30, 'replica select': 99, 'replica selectCount': 2, 'transactions (begin)': 19, 'tx delete': 1, 'tx insert': 5, 'tx select': 8, 'tx update': 7} |
| midday | phase16-declared | {'pool delete': 42, 'pool insert': 26, 'pool update': 78, 'replica select': 225, 'replica selectCount': 2, 'transactions (begin)': 67, 'tx delete': 1, 'tx insert': 13, 'tx select': 16, 'tx update': 15} |
| school-ledger | phase15 | {'pool insert': 68, 'pool select': 14, 'pool selectCount': 2, 'pool update': 5, 'transactions (begin)': 150, 'tx delete': 15, 'tx insert': 41, 'tx select': 66, 'tx selectCount': 3, 'tx update': 73} |
| school-ledger | phase16 | {'pool insert': 68, 'pool select': 21, 'pool selectCount': 2, 'pool update': 5, 'transactions (begin)': 150, 'tx delete': 15, 'tx insert': 41, 'tx select': 121, 'tx selectCount': 3, 'tx update': 73} |
| unnamed-ai-project | phase15 | {'pool delete': 4, 'pool select': 12} |
| unnamed-ai-project | phase16 | {'pool delete': 4, 'pool select': 14} |

## Unknown operations by category (discovery report)

| category | saas-starter phase15 | saas-starter phase16 | midday phase15 | midday phase16 | midday phase15-declared | midday phase16-declared | school-ledger phase15 | school-ledger phase16 | unnamed-ai-project phase15 | unnamed-ai-project phase16 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| call into an unanalyzed package | 49 | 49 | 5911 | 9259 | 5983 | 9587 | 955 | 897 | 97 | 97 |
| column not modeled (schema) | 0 | 0 | 0 | 0 | 250 | 314 | 119 | 124 | 8 | 8 |
| database handle of an unresolved type | 0 | 0 | 787 | 819 | 65 | 81 | 175 | 175 | 0 | 0 |
| inline depth limit | 0 | 0 | 365 | 1373 | 365 | 1389 | 1514 | 1514 | 20 | 20 |
| other | 18 | 23 | 2925 | 4467 | 3612 | 5699 | 4969 | 4926 | 383 | 411 |
| package call with a contract (result not modeled) | 12 | 12 | 2 | 2 | 2 | 2 | 8 | 8 | 62 | 42 |
| replicated database: routing unknown | 0 | 0 | 0 | 0 | 15 | 31 | 0 | 0 | 0 | 0 |
| table not modeled (schema) | 0 | 0 | 0 | 0 | 2 | 2 | 30 | 30 | 13 | 13 |
| unresolved call | 4 | 4 | 620 | 1004 | 628 | 1028 | 549 | 837 | 0 | 0 |
| unresolved identifier | 0 | 0 | 632 | 1086 | 624 | 1190 | 275 | 275 | 5 | 5 |
| unresolved method call | 129 | 123 | 5625 | 12060 | 4987 | 11182 | 3476 | 3568 | 224 | 176 |
| unsupported binding pattern | 1 | 1 | 226 | 298 | 226 | 298 | 68 | 68 | 2 | 2 |
| unsupported expression | 18 | 18 | 1025 | 1227 | 1078 | 1306 | 1085 | 1414 | 65 | 68 |
| unsupported query (raw SQL) | 0 | 0 | 0 | 0 | 8 | 16 | 63 | 63 | 0 | 0 |
| unsupported query shape | 6 | 6 | 1 | 1 | 83 | 134 | 29 | 81 | 2 | 2 |
| unsupported transaction shape | 0 | 0 | 835 | 2737 | 933 | 3043 | 81 | 81 | 0 | 0 |

## Candidate stages (status / property origin / first-blocker category)

| project | config | stages |
| --- | --- | --- |
| saas-starter | phase15 | {'MODEL_COUNTEREXAMPLE / HEURISTIC_HYPOTHESIS / none': 1, 'UNSUPPORTED / SCHEMA_ENFORCED / other': 1} |
| saas-starter | phase16 | {'MODEL_COUNTEREXAMPLE / HEURISTIC_HYPOTHESIS / none': 1, 'UNSUPPORTED / SCHEMA_ENFORCED / other': 1} |
| midday | phase15 | {} |
| midday | phase16 | {} |
| midday | phase15-declared | {'INCONCLUSIVE / HEURISTIC_HYPOTHESIS / inconclusive': 1, 'BLOCKED / none / blocked': 2, 'BLOCKED / none / other': 1} |
| midday | phase16-declared | {'NO_VIOLATION_WITHIN_BOUNDS / HEURISTIC_HYPOTHESIS / none': 1, 'UNSUPPORTED / SCHEMA_ENFORCED / other': 1, 'BLOCKED / none / blocked': 2} |
| school-ledger | phase15 | {'UNSUPPORTED / HEURISTIC_HYPOTHESIS / other': 9} |
| school-ledger | phase16 | {'UNSUPPORTED / HEURISTIC_HYPOTHESIS / other': 9} |
| unnamed-ai-project | phase15 | {} |
| unnamed-ai-project | phase16 | {} |

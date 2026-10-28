# Phase 15 corpus: Phase 14 binary vs Phase 15 binary

Median of 3 runs (wall), peak RSS the maximum; `commitrift discover --model-check`; no project executed. `phase14-declared`/`phase15-declared`: with `corpora/contracts/midday.json` (the declared replica factory).

| project | config | extracted | effectful | read-only | resources | unknown ops | candidates | model-checkable | counterexamples | statuses | wall | peak RSS |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: |
| saas-starter | phase14 | 37 | 9 | 11 | 23 | 237 | 2 | 1 | 1 | {'MODEL_COUNTEREXAMPLE': 1, 'UNSUPPORTED': 1} | 0.04 s | 13 MiB |
| saas-starter | phase15 | 37 | 9 | 11 | 23 | 237 | 2 | 1 | 1 | {'MODEL_COUNTEREXAMPLE': 1, 'UNSUPPORTED': 1} | 0.04 s | 13 MiB |
| midday | phase14 | 1168 | 0 | 0 | 0 | 18954 | 0 | 0 | 0 | {} | 1.41 s | 144 MiB |
| midday | phase15 | 1168 | 0 | 0 | 0 | 18954 | 0 | 0 | 0 | {} | 1.31 s | 144 MiB |
| midday | phase14-declared | 1168 | 81 | 63 | 123 | 18861 | 4 | 0 | 0 | {'INCONCLUSIVE': 1, 'BLOCKED': 3} | 1.36 s | 144 MiB |
| midday | phase15-declared | 1168 | 81 | 63 | 123 | 18861 | 4 | 0 | 0 | {'INCONCLUSIVE': 1, 'BLOCKED': 3} | 1.32 s | 148 MiB |
| school-ledger | phase14 | 546 | 70 | 27 | 68 | 13396 | 9 | 0 | 0 | {'INCONCLUSIVE': 7, 'UNSUPPORTED': 2} | 0.71 s | 120 MiB |
| school-ledger | phase15 | 546 | 70 | 27 | 68 | 13396 | 9 | 0 | 0 | {'UNSUPPORTED': 9} | 0.76 s | 121 MiB |

## Database statements in extracted entries (from the extraction cache)

| project | config | statements |
| --- | --- | --- |
| saas-starter | phase14 | {'pool delete': 2, 'pool insert': 13, 'pool select': 25, 'pool update': 4} |
| saas-starter | phase15 | {'pool delete': 2, 'pool insert': 13, 'pool select': 25, 'pool update': 4} |
| midday | phase14 | {} |
| midday | phase15 | {} |
| midday | phase14-declared | {'pool delete': 42, 'pool insert': 26, 'pool update': 30, 'replica select': 99, 'replica selectCount': 2, 'transactions (begin)': 19, 'tx delete': 1, 'tx insert': 5, 'tx select': 8, 'tx update': 7} |
| midday | phase15-declared | {'pool delete': 42, 'pool insert': 26, 'pool update': 30, 'replica select': 99, 'replica selectCount': 2, 'transactions (begin)': 19, 'tx delete': 1, 'tx insert': 5, 'tx select': 8, 'tx update': 7} |
| school-ledger | phase14 | {'pool insert': 68, 'pool select': 14, 'pool selectCount': 2, 'pool update': 5, 'transactions (begin)': 150, 'tx delete': 15, 'tx insert': 41, 'tx select': 66, 'tx selectCount': 3, 'tx update': 73} |
| school-ledger | phase15 | {'pool insert': 68, 'pool select': 14, 'pool selectCount': 2, 'pool update': 5, 'transactions (begin)': 150, 'tx delete': 15, 'tx insert': 41, 'tx select': 66, 'tx selectCount': 3, 'tx update': 73} |

## Unknown operations by category (discovery report)

| category | saas-starter phase14 | saas-starter phase15 | midday phase14 | midday phase15 | midday phase14-declared | midday phase15-declared | school-ledger phase14 | school-ledger phase15 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| call into an unanalyzed package | 49 | 49 | 5911 | 5911 | 5983 | 5983 | 955 | 955 |
| column not modeled (schema) | 0 | 0 | 0 | 0 | 250 | 250 | 119 | 119 |
| database handle of an unresolved type | 0 | 0 | 787 | 787 | 65 | 65 | 175 | 175 |
| inline depth limit | 0 | 0 | 365 | 365 | 365 | 365 | 1514 | 1514 |
| other | 18 | 18 | 2925 | 2925 | 3612 | 3612 | 4969 | 4969 |
| package call with a contract (result not modeled) | 12 | 12 | 2 | 2 | 2 | 2 | 8 | 8 |
| replicated database: routing unknown | 0 | 0 | 0 | 0 | 15 | 15 | 0 | 0 |
| table not modeled (schema) | 0 | 0 | 0 | 0 | 2 | 2 | 30 | 30 |
| unresolved call | 4 | 4 | 620 | 620 | 628 | 628 | 549 | 549 |
| unresolved identifier | 0 | 0 | 632 | 632 | 624 | 624 | 275 | 275 |
| unresolved method call | 129 | 129 | 5625 | 5625 | 4987 | 4987 | 3476 | 3476 |
| unsupported binding pattern | 1 | 1 | 226 | 226 | 226 | 226 | 68 | 68 |
| unsupported expression | 18 | 18 | 1025 | 1025 | 1078 | 1078 | 1085 | 1085 |
| unsupported query (raw SQL) | 0 | 0 | 0 | 0 | 8 | 8 | 63 | 63 |
| unsupported query shape | 6 | 6 | 1 | 1 | 83 | 83 | 29 | 29 |
| unsupported transaction shape | 0 | 0 | 835 | 835 | 933 | 933 | 81 | 81 |

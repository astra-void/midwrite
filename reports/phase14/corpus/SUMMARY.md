# Phase 14 corpus: Phase 13 binary vs Phase 14 binary

Median of 3 runs (wall), peak RSS the maximum; `commitrift discover --model-check`; no project executed. `phase13-declared`: with `corpora/contracts/midday.json` (the declared replica factory).

| project | config | extracted | effectful | read-only | resources | unknown ops | candidates | model-checkable | counterexamples | statuses | wall | peak RSS |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: |
| saas-starter | phase13 | 37 | 8 | 4 | 17 | 284 | 2 | 1 | 1 | {'MODEL_COUNTEREXAMPLE': 1, 'UNSUPPORTED': 1} | 0.03 s | 12 MiB |
| saas-starter | phase14 | 37 | 9 | 11 | 23 | 237 | 2 | 1 | 1 | {'MODEL_COUNTEREXAMPLE': 1, 'UNSUPPORTED': 1} | 0.04 s | 13 MiB |
| midday | phase13 | 1168 | 0 | 0 | 0 | 17696 | 0 | 0 | 0 | {} | 1.07 s | 119 MiB |
| midday | phase14 | 1168 | 0 | 0 | 0 | 18954 | 0 | 0 | 0 | {} | 1.21 s | 143 MiB |
| midday | phase13-declared | 1168 | 47 | 64 | 52 | 17496 | 3 | 0 | 0 | {'UNSUPPORTED': 1, 'BLOCKED': 2} | 1.12 s | 121 MiB |
| midday | phase14-declared | 1168 | 81 | 63 | 123 | 18861 | 4 | 0 | 0 | {'INCONCLUSIVE': 1, 'BLOCKED': 3} | 1.24 s | 148 MiB |
| school-ledger | phase13 | 546 | 21 | 30 | 18 | 14615 | 9 | 0 | 0 | {'INCONCLUSIVE': 7, 'UNSUPPORTED': 2} | 0.56 s | 98 MiB |
| school-ledger | phase14 | 546 | 70 | 27 | 68 | 13396 | 9 | 0 | 0 | {'INCONCLUSIVE': 7, 'UNSUPPORTED': 2} | 0.70 s | 122 MiB |
| unnamed-ai-project | phase13 | 33 | 3 | 6 | 3 | 1009 | 0 | 0 | 0 | {} | 0.03 s | 13 MiB |
| unnamed-ai-project | phase14 | 33 | 3 | 7 | 3 | 881 | 0 | 0 | 0 | {} | 0.04 s | 15 MiB |

## Database statements in extracted entries (from the extraction cache)

| project | config | statements |
| --- | --- | --- |
| saas-starter | phase13 | {'pool delete': 2, 'pool insert': 13, 'pool select': 14, 'pool update': 3} |
| saas-starter | phase14 | {'pool delete': 2, 'pool insert': 13, 'pool select': 25, 'pool update': 4} |
| midday | phase13 | {} |
| midday | phase14 | {} |
| midday | phase13-declared | {'pool delete': 33, 'pool insert': 5, 'pool update': 16, 'replica select': 86, 'transactions (begin)': 16, 'tx delete': 1, 'tx select': 6, 'tx update': 3} |
| midday | phase14-declared | {'pool delete': 42, 'pool insert': 26, 'pool update': 30, 'replica select': 99, 'replica selectCount': 2, 'transactions (begin)': 19, 'tx delete': 1, 'tx insert': 5, 'tx select': 8, 'tx update': 7} |
| school-ledger | phase13 | {'pool insert': 17, 'pool select': 13, 'pool selectCount': 2, 'pool update': 5, 'transactions (begin)': 148, 'tx delete': 5, 'tx insert': 8, 'tx select': 53, 'tx update': 6} |
| school-ledger | phase14 | {'pool insert': 68, 'pool select': 14, 'pool selectCount': 2, 'pool update': 5, 'transactions (begin)': 150, 'tx delete': 15, 'tx insert': 41, 'tx select': 66, 'tx selectCount': 3, 'tx update': 73} |
| unnamed-ai-project | phase13 | {'pool delete': 4, 'pool select': 11} |
| unnamed-ai-project | phase14 | {'pool delete': 4, 'pool select': 12} |

## Unknown operations by category (discovery report)

| category | saas-starter phase13 | saas-starter phase14 | midday phase13 | midday phase14 | midday phase13-declared | midday phase14-declared | school-ledger phase13 | school-ledger phase14 | unnamed-ai-project phase13 | unnamed-ai-project phase14 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| call into an unanalyzed package | 43 | 49 | 4347 | 5911 | 4347 | 5983 | 614 | 955 | 67 | 97 |
| column not modeled (schema) | 12 | 0 | 0 | 0 | 201 | 250 | 157 | 119 | 9 | 8 |
| database handle of an unresolved type | 0 | 0 | 771 | 787 | 62 | 65 | 173 | 175 | 0 | 0 |
| inline depth limit | 0 | 0 | 183 | 365 | 175 | 365 | 1137 | 1514 | 20 | 20 |
| loop over a collection of unknown length | 0 | 0 | 167 | 0 | 173 | 0 | 413 | 0 | 54 | 0 |
| other | 17 | 18 | 1976 | 2925 | 2593 | 3612 | 3311 | 4969 | 77 | 383 |
| package call with a contract (result not modeled) | 12 | 12 | 2 | 2 | 2 | 2 | 8 | 8 | 62 | 62 |
| replicated database: routing unknown | 0 | 0 | 0 | 0 | 15 | 15 | 0 | 0 | 0 | 0 |
| table not modeled (schema) | 0 | 0 | 0 | 0 | 2 | 2 | 22 | 30 | 13 | 13 |
| unresolved call | 4 | 4 | 576 | 620 | 578 | 628 | 484 | 549 | 0 | 0 |
| unresolved identifier | 0 | 0 | 744 | 632 | 714 | 624 | 594 | 275 | 7 | 5 |
| unresolved method call | 133 | 129 | 6092 | 5625 | 5578 | 4987 | 5326 | 3476 | 563 | 224 |
| unsupported binding pattern | 4 | 1 | 724 | 226 | 724 | 226 | 321 | 68 | 16 | 2 |
| unsupported expression | 42 | 18 | 1679 | 1025 | 1721 | 1078 | 1848 | 1085 | 119 | 65 |
| unsupported query (raw SQL) | 0 | 0 | 0 | 0 | 7 | 8 | 60 | 63 | 0 | 0 |
| unsupported query shape | 17 | 6 | 1 | 1 | 72 | 83 | 20 | 29 | 2 | 2 |
| unsupported transaction shape | 0 | 0 | 434 | 835 | 532 | 933 | 127 | 81 | 0 | 0 |

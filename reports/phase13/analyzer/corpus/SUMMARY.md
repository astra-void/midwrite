# Phase 13 analyzer corpus: Phase 12 binary vs Phase 13 binary

Median of 3 runs (wall), peak RSS the maximum; `commitrift discover --model-check`; no project executed. `phase13-declared`: with `corpora/contracts/midday.json` (the declared replica factory).

| project | config | extracted | effectful | read-only | resources | unknown ops | candidates | model-checkable | counterexamples | statuses | wall | peak RSS |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: |
| saas-starter | phase12 | 37 | 8 | 4 | 17 | 284 | 2 | 1 | 1 | {'MODEL_COUNTEREXAMPLE': 1, 'UNSUPPORTED': 1} | 0.03 s | 11 MiB |
| saas-starter | phase13 | 37 | 8 | 4 | 17 | 284 | 2 | 1 | 1 | {'MODEL_COUNTEREXAMPLE': 1, 'UNSUPPORTED': 1} | 0.03 s | 11 MiB |
| midday | phase12 | 1168 | 0 | 2 | 0 | 17603 | 0 | 0 | 0 | {} | 1.05 s | 111 MiB |
| midday | phase13 | 1168 | 0 | 0 | 0 | 17696 | 0 | 0 | 0 | {} | 1.22 s | 113 MiB |
| midday | phase13-declared | 1168 | 47 | 64 | 52 | 17496 | 3 | 0 | 0 | {'UNSUPPORTED': 1, 'BLOCKED': 2} | 1.29 s | 114 MiB |
| school-ledger | phase12 | 546 | 21 | 29 | 18 | 12894 | 9 | 0 | 0 | {'UNSUPPORTED': 9} | 0.54 s | 80 MiB |
| school-ledger | phase13 | 546 | 21 | 30 | 18 | 14615 | 9 | 0 | 0 | {'INCONCLUSIVE': 7, 'UNSUPPORTED': 2} | 0.64 s | 91 MiB |
| unnamed-ai-project | phase12 | 33 | 3 | 6 | 3 | 1009 | 0 | 0 | 0 | {} | 0.04 s | 13 MiB |
| unnamed-ai-project | phase13 | 33 | 3 | 6 | 3 | 1009 | 0 | 0 | 0 | {} | 0.04 s | 13 MiB |

## Database statements in extracted entries (from the extraction cache)

| project | config | statements |
| --- | --- | --- |
| saas-starter | phase12 | {'pool delete': 2, 'pool insert': 13, 'pool select': 14, 'pool update': 3} |
| saas-starter | phase13 | {'pool delete': 2, 'pool insert': 13, 'pool select': 14, 'pool update': 3} |
| midday | phase12 | {'pool select': 3} |
| midday | phase13 | {} |
| midday | phase13-declared | {'pool delete': 33, 'pool insert': 5, 'pool update': 16, 'replica select': 86, 'transactions (begin)': 16, 'tx delete': 1, 'tx select': 6, 'tx update': 3} |
| school-ledger | phase12 | {'pool insert': 17, 'pool select': 12, 'pool selectCount': 2, 'pool update': 5, 'transactions (begin)': 148, 'tx delete': 5, 'tx insert': 8, 'tx select': 53, 'tx update': 6} |
| school-ledger | phase13 | {'pool insert': 17, 'pool select': 13, 'pool selectCount': 2, 'pool update': 5, 'transactions (begin)': 148, 'tx delete': 5, 'tx insert': 8, 'tx select': 53, 'tx update': 6} |
| unnamed-ai-project | phase12 | {'pool delete': 4, 'pool select': 11} |
| unnamed-ai-project | phase13 | {'pool delete': 4, 'pool select': 11} |

## Unknown operations by category (discovery report)

| category | saas-starter phase12 | saas-starter phase13 | midday phase12 | midday phase13 | midday phase13-declared | school-ledger phase12 | school-ledger phase13 | unnamed-ai-project phase12 | unnamed-ai-project phase13 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| call into an unanalyzed package | 55 | 43 | 4335 | 4347 | 4347 | 614 | 614 | 67 | 67 |
| column not modeled (schema) | 12 | 12 | 6 | 0 | 179 | 157 | 157 | 9 | 9 |
| database handle of an unresolved type | 0 | 0 | 752 | 771 | 62 | 173 | 173 | 0 | 0 |
| inline depth limit | 0 | 0 | 179 | 183 | 175 | 871 | 1137 | 20 | 20 |
| loop over a collection of unknown length | 0 | 0 | 167 | 167 | 173 | 413 | 413 | 54 | 54 |
| other | 17 | 17 | 1984 | 1976 | 2585 | 2555 | 3311 | 77 | 77 |
| package call with a contract (result not modeled) | 0 | 12 | 0 | 2 | 2 | 0 | 8 | 0 | 62 |
| replicated database: routing unknown | 0 | 0 | 0 | 0 | 15 | 0 | 0 | 0 | 0 |
| table not modeled (schema) | 0 | 0 | 0 | 0 | 2 | 22 | 22 | 13 | 13 |
| unresolved call | 4 | 4 | 576 | 576 | 578 | 606 | 484 | 0 | 0 |
| unresolved identifier | 0 | 0 | 717 | 744 | 714 | 646 | 594 | 7 | 7 |
| unresolved method call | 133 | 133 | 6053 | 6092 | 5578 | 4603 | 5326 | 625 | 563 |
| unsupported binding pattern | 4 | 4 | 718 | 724 | 724 | 321 | 321 | 16 | 16 |
| unsupported expression | 42 | 42 | 1679 | 1679 | 1711 | 1706 | 1848 | 119 | 119 |
| unsupported query (raw SQL) | 0 | 0 | 0 | 0 | 7 | 60 | 60 | 0 | 0 |
| unsupported query shape | 17 | 17 | 3 | 1 | 112 | 20 | 20 | 2 | 2 |
| unsupported transaction shape | 0 | 0 | 434 | 434 | 532 | 127 | 127 | 0 | 0 |

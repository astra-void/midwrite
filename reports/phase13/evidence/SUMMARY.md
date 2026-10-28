# Phase 13 evidence: vertical slices

Binary `../commitrift-phase13`; `--source-repeat 3`; original code runs on disposable PostgreSQL (first-party fixtures only).

| slice | scenario | model | states | evidence | reproduced | expectation |
| --- | --- | --- | ---: | --- | --- | --- |
| 2 return value | `claim-race` | COUNTEREXAMPLE_FOUND | 7 | original-code-reproduced | 3/3 | ok |
| 2 return value (safe) | `claim-atomic` | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 5 | none | - | ok |
| 3 last-statement crash | `signup-crash` | COUNTEREXAMPLE_FOUND | 7 | original-code-crash-reproduced | 3/3 | ok |
| 3 last-statement crash (safe) | `signup-idempotent` | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 7 | none | - | ok |
| E upsert | `reserve-race` | COUNTEREXAMPLE_FOUND | 3 | original-code-reproduced | 3/3 | ok |
| E upsert (safe) | `reserve-if-free` | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 5 | none | - | ok |
| E upsert (safe) | `count-views` | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 5 | none | - | ok |
| 1 foreign-key lock | `CR-SYNTH-95906B createWorkspace × createWorkspaceLocked` | COUNTEREXAMPLE_FOUND | 25 | original-code-reproduced | 3/3 | ok |
| 1 foreign-key lock | `CR-SYNTH-6C9B41 createWorkspaceAction/arg[0] × createWorkspaceLocked` | COUNTEREXAMPLE_FOUND | 25 | model-counterexample | 2/3 | FAILED |
| 4 replica staleness (model only) | `replica-read-your-writes` | COUNTEREXAMPLE_FOUND | 3 | model-counterexample | - | ok |

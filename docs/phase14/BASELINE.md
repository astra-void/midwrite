# Phase 14 baseline

What Phase 14 starts from: the final Phase 13 tree and binary. Raw output:
`reports/phase13/final/` (gates, corpus, compatibility) and
`reports/phase14/eval/phase13-binary.json` (the new evaluation set scored
with the Phase 13 binary).

## Revision

| | |
| --- | --- |
| branch | `phase-8` |
| commit | `32efbca` (Phase 12 committed); Phase 13 changes **uncommitted** in the working tree |
| source digest (`scripts/phase8-digest.py`, 780 files) | `dc3161e13092b5494af52fe312bc76f5364705d5885662303534c40fef2cd260` (the tree the Phase 13 gates ran on; a frozen copy in a private scratch directory with its own `CARGO_TARGET_DIR`) |
| release binary | sha256 `97e96ac08f2b51d1c4d2fcbbb00d483e3b99e3026e5e9e620e64b8b46737afe7` (kept outside the repository) |
| platform | Darwin 27.0.0 arm64, rustc 1.98.1, Node 22.23.2, PostgreSQL 17.11 (Homebrew) |

The Phase 13 gate results on this tree are in `docs/phase13/FINAL-REPORT.md`
§3.

## The frozen Phase 14 evaluation set, scored with the Phase 13 binary

`fixtures/phase14/eval-app` (38 labeled entries: 18 positive, 18 negative,
2 unsupported, 12 construct families), frozen in `fixtures/phase14/FROZEN.json`
(sha256 of `FROZEN.json` `778117d2…`) before any Phase 14 scoring and before
any CommitRift command was run on it. Written by an AI agent that did not
read `crates/`; **no independent human review performed**.

| binary | TP | FP | FN | TN | precision | recall |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Phase 13 final (`97e96ac0…`) | 2 | 1 | 16 | 17 | 0.667 | 0.111 |
| same, without the two undeclared return-value cases (E20, E22) | 2 | 1 | 14 | 17 | 0.667 | 0.125 |

Schema: 28/28 tables modeled. Entries: 38/38 extracted; 26 of the 38
labeled entries have a bound database effect. Only these aggregates were
read before the first Phase 14 scoring; the per-case results were not used
to steer the implementation (`EVALUATION.md`).

## The Phase 13 frozen set (regression baseline)

The final Phase 13 binary reproduces the recorded post-feedback scores
exactly (`reports/phase13/eval/final-binary.json`): TP 4, FP 0, FN 11, TN 11
(precision 1.000, recall 0.267); static hazard prediction 0.833 / 0.333.

## Corpus (final Phase 13 binary, `reports/phase13/final/corpus/`)

| project | effectful | read-only | unknown ops | candidates | model-checkable | statuses |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| saas-starter | 8 | 4 | 284 | 2 | 1 | 1 MODEL_COUNTEREXAMPLE, 1 UNSUPPORTED |
| midday (declared replica factory) | 47 | 64 | 17 496 | 3 | 0 | 1 UNSUPPORTED, 2 BLOCKED |
| school-ledger (private, aggregate only) | 21 | 30 | 14 615 | 9 | 0 | 7 INCONCLUSIVE, 2 UNSUPPORTED |
| unnamed-ai-project (private, aggregate only) | 3 | 6 | 1 009 | 0 | 0 | — |

midday's UNSUPPORTED candidate (`toggleCustomerPortal`) loads with the
declared replica factory; its first blocker is `nanoid(21)`, a call into an
unanalyzed package (the Phase 13 document named an earlier, already fixed
loader blocker: `docs/phase13/MIDDAY-COVERAGE.md`, closeout note).

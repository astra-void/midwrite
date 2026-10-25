# Recovery-aware analysis

Phase 4 milestone 5. Code: `crates/commitrift-core/src/recovery.rs`;
tests: `crates/commitrift-analyzer/tests/phase4.rs`,
`crates/commitrift-core/src/recovery.rs` (unit), `verification.rs` (unit).

## Starting point

Phase 3 accepted three per-effect declarations
(`externalEffects[].recovery`: `bestEffort`, `reconciled`,
`idempotentConsumer`). The Phase 3 blind review then re-classified 4 of 5
sampled midday "true positives" as intended risks, and the most frequent
reason it cited was a **mechanism outside the function**: BullMQ retries and
stalled-job recovery of the job processor (review items 2, 10, 12, 17).
Per-effect declarations cannot express "this whole function is executed
again".

## Function-level recovery contracts

```json
"recoveryContracts": [{
  "function": "ProcessDocumentProcessor.process",
  "file": "apps/worker/src/processors/documents/process-document.ts",
  "strategy": "retried",
  "covers": ["CP1001", "CP1002", "CP1004", "CP1005"],
  "evidence": "apps/worker/src/queues/documents.config.ts:16-20 attempts 3; :47 stalledInterval with BullMQ's default maxStalledCount 1",
  "limits": "Bounded: at most 3 attempts and one stalled re-run, then the job fails"
}]
```

| Rule | Why |
| --- | --- |
| `function` is matched **exactly** as diagnostics name it; no patterns (`*`, `?`) | no blanket suppression |
| `file` optional, recommended | disambiguates equal names |
| `covers` must list codes explicitly | a contract never applies to codes it was not written for |
| `strategy: "retried"` may cover CP1001–CP1005; `"reconciled"` only CP1001, CP1004, CP1005 | reconciliation from database state cannot undo an external effect whose database change never committed (CP1002, CP1003); refused at load time |
| CP9001 cannot be covered | it is an analysis limitation, not a failure window |
| `evidence` is required (≥ 10 characters) | a contract must say where the mechanism is |
| duplicate contracts for one function/file are refused | one source of truth |

Effect on a covered finding:

* it is **still reported**, with the same fingerprint, location, failure
  sequence and trace (`traces_are_unchanged_by_contracts`);
* under `recommended` a covered **warning becomes `info`**; under `strict`
  and `audit` the severity is unchanged;
* a `recovery` note is attached: `declaration: "retried" | "reconciled"`,
  `scope: "function"`, `source: "configuration"`, `verified: false`,
  `effectOnFinding: "informational" | "noteOnly"`, the cited `evidence` and
  `limits`;
* a limitation says what the contract asserts, that CommitRift does not
  verify it, and — for `retried` — that the effect completed before the
  failure is performed again (it must be idempotent or its duplicates
  acceptable).

Text output prints a `recovery:` line, SARIF carries the notes under
`properties.recovery`, JSON under `recovery`.

**Never inferred from names.** A function named `retryFailedJobs` with the
same shape as a covered one stays a warning
(`recovery_is_never_inferred_from_names`,
`contracts_apply_only_to_the_named_function_file_and_codes`; Phase 3's
`recovery_is_never_inferred_from_names` still passes).

## Provenance of recovery properties

| `source` | Meaning | Where it can appear |
| --- | --- | --- |
| `sourceCode` | established by supported analysis (e.g. a transactional outbox: no finding at all) | implicit: such windows are not reported |
| `configuration` | asserted by the user (`externalEffects[].recovery`, `recoveryContracts`), `verified: false` | static reports |
| `runtimeObservation` | observed by executing a harness's recovery scenarios | verification reports only (`cases[].recoveryObservation`) |
| `unknown` | everything else | nothing is attached |

A configuration declaration is never displayed as runtime proof. Runtime
observation (verification schema 2): for a case with `recovery` scenarios,
`recoveryObservation` counts the runs in which every declared invariant
held after recovery and in which an invariant had been violated before
recovery. It **never changes the case's level**: a window that was
reproduced stays `RUNTIME_REPRODUCED` even when the recovery repaired the
state afterwards. Example (`order-outbox`, `kill-after-tx-commit`):
"after relay every declared invariant held in 1 of 1 run(s); in 0 of them an
invariant had been violated before recovery".

## Severity policy, summarized

| Situation | How it is presented |
| --- | --- |
| Potential structural failure | warning (or info for CP1004/CP1005 under `recommended`), `STATIC_CANDIDATE` |
| Intended best effort (`bestEffort`) | info under `recommended`, note `bestEffort`, `configuration`, unverified |
| Failure handled by documented recovery (contract or `reconciled`) | info under `recommended`, note with cited evidence, unverified |
| Unverified recovery assumption | every configuration note says `verified: false` and the limitation says so |
| Runtime-reproduced invariant violation | only in a verification report (`RUNTIME_REPRODUCED`), never merged into the static finding |

## Evaluation on real code (demonstration, not precision)

`corpora/configs/midday.phase4-contracts.json` is `midday.json` plus seven
`retried` contracts for the BullMQ processors of midday's `documents` and
`inbox` queues, each citing the queue configuration at the pinned revision
(attempts, stalled-job settings) and the limit that callers can override
`attempts` per job (`packages/job-client/src/index.ts:46`). They were written
by the CommitRift maintainers from reading the source; nobody from midday
reviewed them and nothing was executed.

Result (`reports/phase4/corpus/midday-contracts-vs-phase4.txt`): 0 added,
0 removed, **8 of 49 warnings → info**. Existing labels of those 8: Phase 2
intended-risk 5, false-positive 2, unclear 1 — none labeled true positive;
the Phase 3 automated review classified the 3 it sampled as intended risk.
These labels are one reviewer's judgement plus an automated second opinion,
so this is consistency, not proof. Numbers from this variant must not be
reported as precision.

## Investigated and not changed

* **Effects concerning unrelated state** (Phase 2's main false-positive
  class): needs value flow through returns and database state; deferred, as
  in Phase 3. The descriptive `relationship` field is unchanged.
* **Infeasible paths**: no new evidence of a sound improvement.
* **Best-effort notifications in `try/catch`**: still declarable per effect
  (`bestEffort` with `firstArgument`); not inferred from `try/catch`,
  because real bugs sit inside `try/catch` too (Phase 2 learning).

The default corpus configurations contain no contracts, and the default
corpus reports are byte-identical to Phase 3 in all three configurations.

# Precision: static regression, recovery declarations, effect relationships, second review

## 1. Static regression, Phase 2 → Phase 3

Same pinned corpus, same configurations (`reports/phase3/phase2-vs-phase3.txt`):

| Comparison | Added | Removed | Changed |
| --- | --- | --- | --- |
| `recommended`, original configurations (vs `reports/phase2/corpus-phase2.json`) | 0 | 0 | 0 |
| `recommended`, post-review configurations (vs `reports/phase2/corpus-next.json`) | 0 | 0 | 0 |
| `strict`, original configurations (vs `reports/phase2/corpus-phase2-strict.json`) | 0 | 0 | 0 |

Diagnostic identities (fingerprints), warning counts, CP9001 counts, truncated
functions (midday 10, school-ledger 12), cross-module resolution and summary
counters are identical to the final Phase 2 binary run in the same session
(`reports/phase3/baseline/`). The reviewed-precision table
(`reports/phase3/precision-phase3.txt`) is identical to Phase 2's. The
recorded `reports/phase2/corpus-phase2.json` differs from both only in
counters changed by the documented Phase 2 follow-up (midday cross-file calls
resolved 1262 → 1297 from object services; school-ledger pruned/merged paths
from transaction-result facts).

No diagnostic changed, so there is nothing to explain per diagnostic. Fewer
warnings were not a goal: Phase 3's precision work is descriptive (below)
and does not remove findings.

Message texts changed only by the rename (`CrashProof` → `CommitRift` in
explanations and limitations). Fingerprints do not include message text,
except for CP9001 notes without related operations, whose texts contain no
product name.

## 2. Recovery declarations

`externalEffects[].recovery`: `bestEffort`, `reconciled`,
`idempotentConsumer` (see the README). Precisely:

| Property | Where it comes from | Effect |
| --- | --- | --- |
| Transactional outbox | **Proven** by supported analysis: the state change and the outbox insert are writes on one transaction handle | No finding at all (shown at runtime by `order-outbox`) |
| `bestEffort`, `reconciled` | **Asserted** in configuration, never verified | CP1001/CP1005 whose missing effect carries it: `info` under `recommended`; the window is still reported; `recovery[]` says `source: configuration, verified: false` |
| `idempotentConsumer` | Asserted | A note on CP1002/CP1003/CP1005 where the effect is already performed; severity unchanged (idempotency does not undo an effect) |
| Duplicate delivery, retry repairing the state | **Observed** by a replay harness | Only in the verification report |
| Anything else (retries, cron jobs, webhook redelivery) | Unknown | Not inferred: names such as `retry`, `outbox`, `idempotencyKey` change nothing (`recovery_is_never_inferred_from_names`) |

Tests: `crates/commitrift-analyzer/tests/phase3.rs` (positive, negative,
strict policy, fingerprint stability) and the config tests (conflicting and
unknown declarations rejected). No corpus configuration declares `reconciled`
or `idempotentConsumer`: we do not know which of midday's effects are
reconciled, and inventing declarations would make the numbers meaningless.

## 3. Effect relationships (investigated; descriptive only)

The main Phase 2 false-positive class was "two adjacent operations that are
not related". Phase 3 records, for every operation, the variables its
arguments refer to (by name, in the function that contains the call; table
names excluded; no value flow), and reports for each two-operation finding
whether the two sides share one (`relationship.status`: `sharedArguments`,
`noSharedArgument`, or `notComparable` when an operation comes from a
summarized call or has no variable argument).

Against the Phase 2 labels (`reports/phase3/relationship-vs-labels.txt`):

| Label | sharedArguments | noSharedArgument | notComparable |
| --- | --- | --- | --- |
| true positive | 4 | 3 | 20 |
| intended risk | 8 | 1 | 56 |
| false positive | 1 | 0 | 4 |
| unclear | 0 | 0 | 6 |

It does not separate the classes. Most findings involve operations inside
summarized helpers, where names belong to another scope; the one comparable
false positive shares only a tenant id (`teamId`); three true positives share
no argument because they are related through database state and return
values (a user row created with `returning()` and later used under another
name). Using this signal to demote findings would have hidden real bugs, so
it is **not** used for severity and appears only in JSON as a review hint.
Relating operations soundly would need value flow through returns and
database state, which is out of scope.

## 4. Second review of Phase 2 labels

No human reviewer was available. Instead, a **separate model session (same
model family as the implementer) re-reviewed a sample blind**: it received
18 findings with their locations and the source trees, was instructed not to
open the label files, reports or docs, and classified each finding from the
source. **This is an automated second opinion, not independent human
verification.** Sample: deterministic, stratified by the Phase 2 label (the
first findings by fingerprint in each class): midday 5 true positives, 5
intended risks, 4 false positives (all of them); saas-starter 2 true
positives, 2 intended risks. Files: `reports/phase3/blind-review.txt`,
`blind-review-reasons.json`, `blind-review-sample.txt`.

| | Result |
| --- | --- |
| Exact agreement | 11 / 18 (Cohen's κ = 0.38) |
| Agreement on "likely bug" vs not | 14 / 18 |
| Phase 2 true positives confirmed | 3 / 7 (saas-starter 2/2, midday 1/5) |
| Phase 2 true positives re-classified | 3 → intended risk (job retries, documented best-effort order, dashboard repair action), 1 → false positive (the write and the job concern disjoint sets of records) |
| Phase 2 false positives | 1 / 4 confirmed; 3 → intended risk (the second reviewer considered the window real but harmless; Phase 2 had called the operations unrelated) |
| Intended risks | 7 / 7 confirmed |

Interpretation: the structural claim (a window exists) is broadly
confirmed, but **Phase 2's "likely bug" labels on midday look optimistic**:
the second reviewer found retry, redelivery or user-visible repair paths for
4 of 5 sampled midday "true positives". Phase 2's bug precision for midday
(0.35 for CP1001) should be read as an upper estimate from one reviewer.
The Phase 2 label files are left unchanged as the historical record; the
disagreements are recorded here. Two reviewers from the same model family
can share blind spots, so even agreement is weak evidence; a human
maintainer review remains open.

## 5. Remaining false positives and false negatives

Unchanged from Phase 2 (`docs/LIMITATIONS.md`): unrelated adjacent effects
(now with a descriptive relationship field), retries and reconciliation
outside the analyzed code (now declarable, not inferable), class methods and
`this.method()` (not implemented in Phase 3: following them soundly needs
receiver typing and override rules, and runtime verification had priority),
callbacks, raw SQL, truncation at 512 paths.

Trace-level limitation: a complete trace can still be infeasible at runtime
(branch conditions are not solved), and an `incomplete` trace (unknown calls
inside the window) is common in large projects: 47/180 traces in midday,
93/123 in school-ledger.

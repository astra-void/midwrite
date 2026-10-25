# Real-world evaluation (Phase 4)

Phase 4 milestone 7. Raw data: `reports/phase4/corpus/`,
`reports/phase4/review/`. External repositories were analyzed statically
only — never modified, built or executed.

## 1. Corpus regression (pinned Phase 3 corpus, same machine)

| Configuration | Phase 3 → Phase 4 |
| --- | --- |
| `recommended`, original configurations (`corpus-phase4.json`) | 0 added, 0 removed, 0 changed; report **byte-identical** to `reports/phase3/corpus-phase3.json` |
| `recommended`, post-review configurations (`corpus-phase4-next.json`) | 0 / 0 / 0; byte-identical |
| `strict`, original configurations (`corpus-phase4-strict.json`) | 0 / 0 / 0; byte-identical |

There is therefore no diagnostic change to explain. `commitrift check` JSON
output of the Phase 3 binary (rebuilt from the digest) and the Phase 4 binary
is byte-identical on all four projects as well (`PERFORMANCE.md`).

Tracked quantities (original configurations, unchanged from Phase 3):

| | saas-starter | midday | school-ledger | unnamed-ai-project |
| --- | --- | --- | --- | --- |
| Revision | `6e33e58b` | `51587319` | `4b97f34` + 2 local changes | `39fcfc7` + 2 local changes |
| Diagnostics by code | CP1001 1, CP1004 11, CP1005 3, CP9001 6 | CP1001 40, CP1002 9, CP1004 116, CP1005 15, CP9001 89 | CP1004 123, CP9001 194 | CP1004 8, CP9001 9 |
| Warnings / info | 1 / 20 | 49 / 220 | 0 / 317 | 0 / 17 |
| Recovery-informational (contracts) | — | — (8 in the contract variant, §3) | — | — |
| Cross-module calls resolved / unresolved | 33 / 0 | 1297 / 71 | 3203 / 5 | 11 / 1 |
| Functions truncated (path budget) | 0 | 10 | 12 | 0 |
| Traces: complete / approximated / incomplete | 14 / 1 / 0 | 126 / 7 / 47 | 30 / 0 / 93 | 8 / 0 / 0 |
| Findings without a trace (CP9001) | 6 | 89 | 194 | 9 |
| Traces failing model validation | 0 | 0 | 0 | 0 |
| Runtime-verified cases | 0 | 0 (one isolated pattern, Phase 3) | 0 | 0 |

## 2. Incomplete traces: same number, now explained

`commitrift-corpus traces` (new) validates every trace against the
analysis model and classifies incompleteness
(`reports/phase4/corpus/trace-coverage.txt`). **All 140 incomplete traces
have one cause: an operation with unknown semantics inside the failure
window.** None comes from unsupported control flow. On midday the most
frequent unknown calls are calls through an expression (10), 
`supabase.storage.from(…)` (8, an external effect the configuration does not
declare), `withTimeout(…)` (8, a wrapper around an awaited promise),
`getTeamCalibration(…)` (7), `this.updateProgress(…)` (5, a class method —
unsupported) and `removeProcessingReaction(…)` (5). School-ledger has 75
distinct ones (private project; labels not published).

The number did not shrink: reducing it would mean declaring more effects in
the corpus configurations (which changes the evaluated configuration) or
analyzing class methods (deferred). The trace's `incomplete` status is
honest: a reproduction plan should not be built on an unknown call in the
window, and `init-replay` refuses such traces by default.

## 3. Recovery contracts on real code (a demonstration, not precision)

See [`RECOVERY-SEMANTICS.md`](RECOVERY-SEMANTICS.md#evaluation-on-real-code-demonstration-not-precision):
seven evidence-cited `retried` contracts for midday's BullMQ processors turn
8 of 49 warnings into info (0 added, 0 removed); none of the 8 carries a
"true positive" label.

## 4. Review workflow for independent human adjudication

The Phase 3 blind review was automated and agreed only partially with the
Phase 2 labels, so neither set of labels is ground truth. Phase 4 adds a
workflow a human can use without being primed:

```bash
commitrift-corpus review-packet --manifest corpora/manifest.json --project midday \
  --sample 12 --output review-midday            # or --finding <id> (repeatable)
# reviewer fills review-midday/reviews.json from the packets only
commitrift-corpus review-check review-midday/reviews.json --labels corpora/labels/midday.json
```

* A packet per finding: the diagnostic, its failure sequence, its trace,
  the source (±12 lines) around the reported location and every related
  operation, and three questions. **No label, no earlier verdict.**
* `reviews.json`: one entry per finding with `classification`
  (`confirmed-bug`, `intended-risk`, `false-positive`, `unclear`,
  `not-reviewed`), `reviewer`, `role`, `date` (YYYY-MM-DD), `notes`,
  `evidence` (file:line references). `review-check` rejects a reviewed entry
  without reviewer, role, date or notes, and compares with labels only on
  request, after validation.
* Prepared: `reports/phase4/review/midday-sample/` (12 midday warnings, the
  first 12 by fingerprint; midday is public).

**Reviews completed: 0.** No human reviewer was available in this phase and
no review was invented. The form in the repository records all 12 as
`not-reviewed`.

## 5. Real project reproduction

**Not attempted on an original application.** Candidates with strong static
evidence and clear boundaries exist (midday's invoice flow, labeled true
positive in Phase 2), but running midday requires Supabase, Redis/BullMQ,
external APIs and secrets, and executing an external repository was not
authorized. The Phase 3 *isolated reproduction of the suspected pattern*
(`fixtures/replay/invoice-pattern`) still behaves identically (regression
against the Phase 3 report: 3 still reproduced) and now reports the runtime
recovery observation: the client retry restores the invariant in 1 of 1 run
after the window was reproduced. It remains a re-write, not midday; no
maintainer was contacted and no issue was filed.

**Externally reviewable case with complete evidence:** the first-party
Drizzle fixture (`fixtures/replay/drizzle-node`): source, static finding,
trace, harness, manifest with declared invariants, verification reports on
macOS and Linux, HTML. Anyone with Node.js 22, PostgreSQL and the
isolation backend can re-run it. It is a fixture, so it demonstrates the
method on real Drizzle/PostgreSQL semantics, not a bug in someone's
application.

## 6. Goals versus outcomes

| Target | Outcome |
| --- | --- |
| At least one real Drizzle-based runtime reproduction | **Met** (fixture, 5 reproduced cases, 5/5 repeat, macOS and Linux) |
| At least one Linux replay environment with isolation enabled | **Met** locally (Debian container, unprivileged, bubblewrap); GitHub runner unverified |
| A generated harness completing a real demonstration | **Met** (Drizzle fixture, linked by exact fingerprint) |
| No regressions in previously validated fixtures | **Met** (Phase 3 harnesses: regression reports unchanged; corpus byte-identical) |
| No unsupported confidence upgrades | **Met**: confidence never changes from runtime results; contracts only lower severity and are labeled unverified |
| A smaller or better-explained set of incomplete traces | **Better explained** (single cause, top contributors); not smaller |
| At least one externally reviewable case with complete evidence | **Met** for a first-party fixture; **not** for an external application |
| Human review of corpus findings | **Not met** (workflow ready, 0 reviews) |

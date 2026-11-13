# Independent human review (DEV-162)

Every label behind Midwrite's precision and recall numbers, and each of the
four upstream reports, was written by an AI agent. Agreement between two AI
agents is not independent validation. This page describes how a person
reviews a fixed sample of those labels and reports, and how the result
becomes an adjudicated label set without any label changing silently.

**Status (2026-11-13): no person has reviewed anything.** On 2026-11-13
the project owner changed the scope of DEV-162: human review is not
required, and the tier-1 sample was re-reviewed by independent blind AI
reviewers instead (see [Blind AI review](#blind-ai-review-2026-11-13)).
Every published number is still AI-labeled: none is human-validated
precision. A later human review can use the same sample and tooling.

## What gets reviewed

[`corpora/review/sample.json`](../../corpora/review/sample.json) is drawn by
`scripts/human-review.py sample` with a fixed rule. Within each stratum the
items are ordered by `sha256("DEV-162/<project>/<id>")`, which does not
depend on any label, so the sample cannot be steered toward agreeable
cases. The file also records the digest of every label file it was drawn
from.

| tier | stratum | rule | items |
| --- | --- | --- | ---: |
| 1 | `held-out-finding` | every finding on a held-out project | 32 |
| 1 | `held-out-known-positive` | every audited window on a held-out project | 18 |
| 1 | `dev-false-positive` | every dev finding labeled false-positive | 41 |
| 1 | `dev-unclear` | every dev finding labeled unclear | 9 |
| 1 | `corrected` | every label an earlier correction changed, including the removed dittofeed KP04 | 5 |
| 1 | `upstream-report` | the four reported upstream issues with their findings | 4 |
| 2 | `dev-true-positive` | 10% of each dev project's true positives, at least 2 | 22 |
| 2 | `dev-intended-risk` | 10% of each dev project's intended risks, at least 2 | 26 |
| 2 | `dev-known-positive` | 3 audited windows per dev project | 15 |

Tier 1 has 109 items and must be finished before any human-validated number
is published. It covers every positive and negative held-out label, as
DEV-162 asks for a new held-out set. Tier 2 has 63 items: a representative
sample of the dev positives, which estimates how often the AI labels hold
up there. Findings labeled after analyzer tuning carry
`relabeledAfterTuning: true` so their agreement can be read separately.

Do not redraw the sample after a record has been started. Records are bound
to the sample's digest. If labels change while a review is under way,
`check` notes it, and the review continues against the sampled revision.

## Reviewing

```bash
python3 scripts/eval.py fetch --set dev
```

```bash
python3 scripts/eval.py fetch --set held-out
```

```bash
python3 scripts/human-review.py packet
```

```bash
python3 scripts/human-review.py template --reviewer "Your Name"
```

1. Fetch the corpora at their pinned revisions. The upstream sources are
   fetched and digest-checked by `scripts/upstream-review.py`.
2. Generate the packet: `reports/review/packet/tier1.blind.md` and
   `tier2.blind.md` show each item's location and source excerpt, without
   the AI label. The packet holds third-party source and is not committed.
3. Create your record under `corpora/review/records/`. For every item,
   read the code at the pinned revision, the whole function and the code
   that calls it, before you open the revealed packet. Then write:

   | field | values | meaning |
   | --- | --- | --- |
   | `hazard` | `real`, `not-real`, `unclear` | does the claimed partial state exist: can one effect commit and the next one fail or never run? |
   | `actionable` | `unhandled-bug`, `accepted-or-mitigated`, `not-applicable`, `unclear` | separately, is it a bug? `accepted-or-mitigated` covers retries, reconciliation, idempotent consumers, best-effort side effects and documented intent. Use `not-applicable` when the hazard is not real |
   | `blind` | `true`, `false` | `true` if you wrote the verdict before reading the AI label |
   | `rationale` | text | why, in your own words |
   | `codeRefs` | `["path:line", …]` | the lines your verdict rests on: the effects, guards, retries or compensation you checked |
   | `revision` | pre-filled | must stay the sampled revision |

   Keeping `hazard` and `actionable` apart is the point of the record. A
   window can be real and still not be a bug, and hazard precision and bug
   precision are scored from different answers. An audited window (known
   positive) is kept if `hazard` is `real`. For an upstream report, also
   check the external effects and any documented compensation or retry
   path, and say in `rationale` whether the report's failure sequence holds.
4. Fill in `reviewer.independence`, your relation to Midwrite and to the
   labels (for example "did not write Midwrite, its rules or any label"),
   and `reviewer.toolsUsed`. Set `attestation` to `true` only if every
   verdict is your own: you may use tools to read code, not to produce
   verdicts. Set `completed` when you are done.
5. Run `check`. It refuses a record with a non-human reviewer, a missing
   attestation, a missing rationale or code reference, or a revision other
   than the sampled one.

```bash
python3 scripts/human-review.py check corpora/review/records/your-name.json --complete
```

A second reviewer works the same way, in a separate record and without
reading the first one.

## Summary and adjudication

```bash
python3 scripts/human-review.py summarize corpora/review/records/*.json -o reports/review/YYYY-MM-DD-summary.json
```

The summary records:

* the number reviewed in each stratum;
* agreement with the AI labels, both full-label and hazard-only, with
  Wilson 95% intervals;
* a confusion matrix from AI label to human verdict;
* every disagreement with its rationale and code references;
* disagreements between reviewers.

It computes human-validated held-out hazard and bug precision only when
every held-out finding has been reviewed, and it prints the AI-label
precision for the same items next to it.

```bash
python3 scripts/human-review.py adjudicate corpora/review/records/*.json
```

`adjudicate` is a dry run unless `--apply` is given. Reviewer disagreements
must first be settled in a `--resolutions` file, which maps an item to its
`classification`, the person who settled it (`by`) and a `rationale`.

With `--apply`:

* A changed label gets a `human-adjudication` correction holding the
  previous label, the reviewers, the rationale, the code references and the
  record path.
* Labels the reviewer confirmed are listed in one `human-confirmed`
  correction.
* `labeling.humanReview` states how many items were reviewed. It covers the
  sample, not every label.
* A held-out label file that changed must be refrozen with the printed
  `scripts/eval.py freeze` command. The reason is the human adjudication,
  never a score.
* Upstream issue files get a `humanReview` field.

Commit the record, the summary and the adjudicated labels together, then
rescore with `scripts/eval.py`.

## Upstream reports

[`corpora/review/upstream-status.json`](../../corpora/review/upstream-status.json)
holds the public state of the four reports, from
`scripts/human-review.py upstream-status` (read-only `gh`). On 2026-11-13
all four were open with no maintainer reply: **every upstream report is
unconfirmed.** `maintainerConfirmed` is set by hand, with a link, only when
a maintainer says whether the failure is real. A human reviewer's verdict on
a report is separate from maintainer confirmation, and both are recorded.

## What may be claimed

| evidence | allowed wording |
| --- | --- |
| AI labels only (today) | "AI-labeled precision", "agreement between separate AI agents" |
| a completed tier-1 record by one person | "human-reviewed held-out precision (one reviewer)", with the reviewer's stated independence |
| two records, disagreements settled | "independently human-validated precision (two reviewers)" |
| an upstream issue a maintainer confirmed | "confirmed upstream bug" |
| an upstream issue that is open or unanswered | "reported, unconfirmed" |

Never present the AI-label agreement rate as human-validated precision, and
never present a reviewer's agreement with the AI labels as a precision
figure: report human precision from the human verdicts.

## Blind AI review (2026-11-13)

Records:

* [`corpora/review/records/ai-pass-1.json`](../../corpora/review/records/ai-pass-1.json)
  and [`ai-pass-2.json`](../../corpora/review/records/ai-pass-2.json);
* summary: [`reports/review/2026-11-13-ai-review-summary.json`](../../reports/review/2026-11-13-ai-review-summary.json);
* rescoring: [`reports/eval/2026-11-13-ai-review-rescore.json`](../../reports/eval/2026-11-13-ai-review-rescore.json).

Both records carry `reviewer.kind: "ai"` and no attestation. `check`
accepts them. They are summarized under `aiReview…` keys, never
`human…`, and they are applied only through `adjudicate --ai`.

**Procedure, fixed before any verdict was seen.**

1. **Pass 1.** Six separate AI agents reviewed one batch each, covering
   all 109 tier-1 items. Each agent saw only the pinned source and each
   item's operations and path. For a finding no longer reported, it saw
   the location instead. It did not see the existing labels, the reports,
   the review files or the other agents' verdicts.
2. **Pass 2.** The 26 items where pass 1 disagreed with the existing label
   went to two different agents, under the same blinding.
3. **Adjudication.** A label changed only when both reviewers agreed with
   each other and disagreed with it. Each change is an `ai-adjudication`
   correction that keeps the previous label, with `humanReviewed: false`.
   Confirmed labels are listed in `ai-confirmed`. The three held-out files
   that changed were refrozen with the reason.

**Results.**

| | items |
| --- | ---: |
| pass 1 agrees with the existing label | 83 of 109 |
| changed (both passes agree against the label) | 23 |
| kept (pass 2 disagreed with pass 1) | 3 |

* **The 23 changes.** Fourteen midday dev labels: ten false positives were
  real windows, accepted by retries, compensation or best-effort design,
  and several `unclear` labels were decided. In kan, one false positive
  is a real non-idempotent webhook window. In dittofeed, three `unclear`
  labels were decided. On the held-out sets:
  * nextjs-billing: one true-positive became intended-risk;
  * petdex: one false-positive became intended-risk, and two true
    positives became `unclear`, because the function has no caller in the
    checkout;
  * next-saas-stripe-starter: one intended-risk became true-positive.
* **The 3 kept** are midday `f67d93b946c14519`, kan `5b00a075f33349a8`
  and nextjs-billing `b6cf38aa870df5cb`.
* **Audited windows.** Pass 1 confirmed all 18 held-out audited windows
  (and the dev ones in the sample).
* **Upstream reports.** It also found that all four upstream reports'
  failure sequences hold as written, recorded as `aiReview` in each
  issue file. Maintainers have not confirmed any of them.

| measure | before | after |
| --- | ---: | ---: |
| dev hazard precision | 413/436 | 421/443 |
| dev bug precision | 180/436 | 181/443 |
| dev unclear | 8 | 1 |
| held-out hazard precision | 30/32 | 28/30 (2 now `unclear`), gate PASS |
| held-out bug precision | 3/32 | 1/30 |
| held-out recall | 8/19 | 7/18 |

These are AI judgments checked by more AI judgments. They are more
independent than single labels, but they are not human validation.

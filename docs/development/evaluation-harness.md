# Evaluation harness: dev and held-out sets

`scripts/eval.py` measures the precision and recall of `midwrite check` on
real projects (Linear DEV-147). It replaces "one number from the project the
rules were built against" with two numbers: one on a **dev set** you may tune
against, and one on a **held-out set** you never tune against.

The discovery scores (for example precision 1.000 (6/6), recall 0.333
(6/18) on the newest frozen set) come from synthetic frozen applications
under `fixtures/eval/p11`–`p16`, which score `discover --model-check`
([evaluation datasets](../research/evaluation-datasets.md)). They are
still scored by `scripts/eval-sets.py`. This harness scores static
`check` findings on third-party code.

## Sets

`corpora/eval/sets.json`:

| project | set | revision | why |
| --- | --- | --- | --- |
| midday | dev | `51587319` | the project the rules were developed against |
| kan (kanbn/kan) | held-out | `1cc2f332` | tRPC + repositories, email, webhooks, Stripe, S3 |
| dittofeed | held-out | `91277939` | Postgres through `db()`, Temporal, ClickHouse, Kafka |
| zero (Mail-0/Zero) | held-out | `36132735` | Workers + Durable Objects, Gmail APIs, queues, R2/KV |

Reserve: vercel/chatbot. Every project uses Drizzle + PostgreSQL. The first
proposed candidates (cal.com, dub, documenso, formbricks) use Prisma, which
Midwrite does not analyze.

## Rules

* **Never change rules, defaults or code because of a held-out result.**
  When a held-out project drives a fix, move it to `dev` in `sets.json`,
  record the move in `history`, and promote a project from `reserve` (pin
  its revision, write its configuration from source, audit it, label it,
  freeze it).
* Held-out configurations are written from the project's source, before its
  findings are looked at.
* Held-out labels, configuration and revision are frozen
  (`scripts/eval.py freeze`, `corpora/eval/FROZEN.json`). `run` verifies
  them; a held-out project whose frozen files changed is reported and fails
  the gate. Refreezing needs `--reason` and keeps the previous checksums in
  `history`. It is for label corrections, never for raising a score.
* Report scores with their counts, split by set:
  `precision 0.92 (23/25), recall 0.40 (23/58)`.
* Never execute or modify the projects. `check` is static.

## Labels

`corpora/eval/labels/<project>.json` (dev midday reuses the first corpus review's list in
`corpora/labels/midday.json`):

* `knownPositives`: unhandled half-written-state windows found by an
  **audit of the source that did not look at Midwrite's output**. Each entry
  has the function's line span and a pattern: A commit then effect, B
  effect then commit, C effect inside a transaction, D independent commits,
  E independent effects. They form the recall denominator.
* `findings`: every reported hazard (MW1001–MW1005), keyed by fingerprint,
  classified as `true-positive` (window exists, unhandled), `intended-risk`
  (window exists, accepted on purpose), `false-positive` (the partial state
  is not real) or `unclear`. These are the corpus review classes
  ([evaluation methodology](../research/evaluation-methodology.md#static-finding-review)).

## Scores

* **hazard precision** = (TP + intended) / (TP + intended + FP): the
  reported window exists. **Stage-1 gate: ≥ 0.9 on the held-out set**, with
  every held-out finding labeled and every held-out project frozen.
* **bug precision** = TP / (TP + intended + FP): the window is an unhandled
  bug. It is printed next to the gate number and is always lower.
* **recall** = found / positives. Positives are the known positives plus
  true-positive findings outside the audit. A known positive is found when
  a hazard of its pattern's code lies inside its function in the same file
  and is not labeled false-positive. Recall is not computed for a project
  without audited positives: true-positive findings alone would make it 1
  by construction.

`unclear` findings are left out of both precision denominators and are
printed. MW9001 is an analysis limitation, not a claim, and is not scored.

## Commands

```bash
cargo build --release
python3 scripts/eval.py fetch                  # clone pinned revisions into .corpora/
python3 scripts/eval.py run -o eval.json       # all sets; table, per-set lines, gate
python3 scripts/eval.py run --set held-out --gate   # exit 1 unless the gate passes
python3 scripts/eval.py freeze --project kan    # after labeling a held-out project
```

## CI precision gate

The `held-out precision >= 0.9` job in `.github/workflows/ci.yml` runs on
every push and pull request (Linear DEV-149). It builds the static analyzer
in release mode, fetches only the held-out projects at their pinned
revisions with `python3 scripts/eval.py fetch --set held-out`, and runs:

```bash
python3 scripts/eval.py run --set held-out --gate -o eval.json
```

The job fails when aggregate held-out hazard precision is below 0.9,
without rounding the score. Exactly 0.9 passes. It also fails for unlabeled
held-out findings, missing or changed frozen inputs, or no decided held-out
labels. Fetch or analyzer errors fail the job as well. Scores, counts and
gate failure reasons appear in the job log; `eval.json` stays in the job's
workspace. Recall and dev-set precision do not determine this gate.

The first recorded baseline below already fails the threshold. Enabling
the job does not lower the threshold or change the frozen labels to make
that baseline pass. Follow the set rotation rules above when a held-out
result drives an analyzer change.

## Caveats

* Every label so far was written by AI agents (Claude): the audits that
  produced the known positives, and the classification of findings. The
  agents doing each job were separate, and the auditors never saw
  Midwrite's output. That is not independent human review. Treat the
  numbers as provisional until a person has reviewed the labels; record
  disagreements in the label file's `corrections`.
* The audits were capped at 30 entries, highest confidence first. kan
  reached the cap and reported more windows of the same kind, so its recall
  denominator is a sample.

## First scoring (2026-11-05)

Record: [`reports/eval/2026-11-05-first-scoring.json`](../../reports/eval/2026-11-05-first-scoring.json)
(release build of commit `171d197`, held-out labels frozen before scoring).

| set | precision (hazard) | bug precision | recall |
| --- | --- | --- | --- |
| dev (midday) | 0.94 (59/63) | 0.33 (21/63) | n/a (no audit; 123 of 192 findings unlabeled) |
| **held-out** | **0.89 (122/137)** | 0.36 (50/137) | **0.69 (44/64)** |
| kan | 0.96 (87/91) | 0.47 (43/91) | 0.94 (31/33) |
| dittofeed | 0.95 (20/21) | 0.24 (5/21) | 0.48 (10/21) |
| zero | 0.60 (15/25) | 0.08 (2/25) | 0.30 (3/10) |

**Stage-1 gate: FAIL** (0.89 < 0.9).

* All 10 Zero false positives come from one configuration rule, `fetch` as
  an `http-request` effect. It also matches read-only GETs: existence
  probes, `getIamPolicy`, an OAuth token exchange, DNS-over-HTTPS. Fixing
  this, for example by telling read-only requests apart, would be a change
  driven by a held-out result. Under the rules above, Zero then moves to dev
  and a reserve project replaces it.
* Recall misses are mostly windows Midwrite cannot see. In dittofeed these
  are ClickHouse and Temporal effects reached through helpers. In Zero they
  are writes through the database Durable Object stub (`getZeroDB(userId)`),
  which is neither a Drizzle chain nor a zero-argument receiver, plus
  Gmail-label/KV pairs.
* `reviewQueue` in each label file lists 10 cases where the auditor and the
  finding labeler disagree. The auditor calls the window unhandled, and the
  labeler classified the matching finding as intended-risk. Examples: an
  audit-only activity row, a log-only Stripe seat decrement, Temporal
  activities whose retry converges. Neither label was changed; a person
  should decide them.
* The issue that asked for this harness quoted "precision 1.0, recall 0.33
  from Midday". Those are the newest synthetic frozen set's (`fixtures/eval/p16`)
  `discover --model-check` scores, not Midday's. Midday has never produced a
  model counterexample.

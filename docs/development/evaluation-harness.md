# Evaluation harness: dev and held-out sets

`scripts/eval.py` measures the precision and recall of `midwrite check` on
real projects (Linear DEV-147). It replaces "one number from the project the
rules were built against" with two numbers: one on a **dev set** you may tune
against, and one on a **held-out set** you never tune against.

The scores in the phase reports (for example Phase 16's precision 1.000
(6/6), recall 0.333 (6/18)) come from synthetic frozen applications under
`fixtures/phase13`–`phase16`, which score `discover --model-check`. They are
still scored by `scripts/phase1N-eval.py`. This harness scores static
`check` findings on third-party code.

## Sets

`corpora/eval/sets.json`:

| project | set | revision | why |
| --- | --- | --- | --- |
| midday | dev | `51587319` | the project the rules were developed against (Phases 2–16) |
| kan (kanbn/kan) | held-out | `1cc2f332` | tRPC + repositories, email, webhooks, Stripe, S3 |
| dittofeed | held-out | `91277939` | Postgres through `db()`, Temporal, ClickHouse, Kafka |
| zero (Mail-0/Zero) | dev (held-out until 2026-11-06) | `36132735` | Workers + Durable Objects, Gmail APIs, queues, R2/KV |
| vercel-chatbot (vercel/chatbot) | held-out (since 2026-11-06) | `a8989b52` | Next.js AI chat: queries in `lib/db/queries.ts`, Vercel Blob, Redis streams and rate limit |

Reserve: empty; add a project before the next held-out project moves to
dev. Every project uses Drizzle + PostgreSQL. The first
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

`corpora/eval/labels/<project>.json` (dev midday reuses the Phase 2 list in
`corpora/labels/midday.json`):

* `knownPositives`: unhandled half-written-state windows found by an
  **audit of the source that did not look at Midwrite's output**. Each entry
  has the function's line span and a pattern: A commit then effect, B
  effect then commit, C effect inside a transaction, D independent commits,
  E independent effects. They form the recall denominator.
* `findings`: every reported hazard (MW1001–MW1005), keyed by fingerprint,
  classified as `true-positive` (window exists, unhandled), `intended-risk`
  (window exists, accepted on purpose), `false-positive` (the partial state
  is not real) or `unclear`. These are the Phase 2 classes.

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
  from Midday". Those are the Phase 16 synthetic frozen set's
  `discover --model-check` scores, not Midday's. Midday has never produced a
  model counterexample.

## Second scoring (2026-11-06)

Record: [`reports/eval/2026-11-06-second-scoring.json`](../../reports/eval/2026-11-06-second-scoring.json)
(release build of commit `87186c0`; the analyzer is unchanged since the first
scoring).

Changes to the sets, following the rules above (`sets.json` `history`):

* Zero moved to dev, so its `fetch` false positives can be fixed. Its frozen
  entry was removed from `FROZEN.json`.
* vercel/chatbot was promoted from reserve, pinned at `a8989b52`. Its
  configuration was written from source before the analyzer ran on it. The
  8 known positives come from a source-only audit, and a separate agent
  classified the 11 findings without seeing them. 2 auditor/labeler
  disagreements are in `reviewQueue`: the Redis rate-limit increment and the
  resumable-stream row, which the labeler called intended-risk.

| set | precision (hazard) | bug precision | recall |
| --- | --- | --- | --- |
| dev (midday + zero) | 0.84 (74/88) | 0.26 (23/88) | 0.30 (3/10), zero only |
| **held-out** | **0.96 (118/123)** | 0.46 (57/123) | **0.77 (48/62)** |
| kan | 0.96 (87/91) | 0.47 (43/91) | 0.94 (31/33) |
| dittofeed | 0.95 (20/21) | 0.24 (5/21) | 0.48 (10/21) |
| vercel-chatbot | 1.00 (11/11) | 0.82 (9/11) | 0.88 (7/8) |

**Stage-1 gate: PASS** (0.96 ≥ 0.9).

* The pass comes from changing the sets, not from a better analyzer. kan
  and dittofeed alone already score 0.96 (107/112). vercel-chatbot adds
  only 11 findings: a small project whose Drizzle helpers have no
  transactions, so most windows are plain independent-commit chains
  (MW1004).
* The `fetch` rule that produced Zero's false positives is unchanged. It is
  now dev work.
* All labels are still AI-only, with no human review.
* vercel-chatbot's one missed known positive is KP04: per-message
  `updateMessage` / `saveMessages` calls inside `Promise.all` in `onEnd`.

## Missed cases (DEV-148, 2026-11-06)

Fixes are chosen from **dev** misses only (midday, zero). Held-out misses are
listed for the record and were not used to pick or shape a fix. midday now
has known positives: a source-only audit (30, capped; the auditor listed
11 more windows it left out), so dev recall covers midday too.

Dev misses by cause, before the fix below (20 of 52 dev positives):

| cause | dev misses | examples |
| --- | --- | --- |
| external effect not recognized (not in the configuration, no built-in) | 8 | midday: `notifications.create` email (KP02), accounting provider push (KP06, KP07), Supabase auth admin and Resend contacts (KP13), BullMQ `job.remove()` (KP20), Polar SDK (KP22), Supabase storage `remove` (KP24), Redis cache delete (KP28) |
| database writes behind a project RPC stub that Midwrite does not model | 4 | zero: `getZeroDB(userId).deleteConnection/deleteUser` (KP02–KP05); reported as MW9001 |
| writes through a non-Drizzle client (Supabase JS `from(t).upsert/update/insert`) | 3 | midday KP04, KP14, KP30 |
| async callback flows | 3 | midday KP05 (writes in a CSV parser `chunk` callback), zero KP07 (`onSecretRotated` callback), zero KP10 (email inside `Effect.runPromise`) |
| lazy builder returned from a non-async function | 1 | zero KP08: `withAssistantDb((db) => db.update(...))` after an email. **Fixed** |
| class instance created with `new` and a private method | 1 | midday KP29 (`new InboxConnector(...).exchangeCodeForAccount` → `#saveAccount`) |

Held-out misses (not used for fixes): 14. Mostly ClickHouse and Temporal
effects reached through helpers in dittofeed (11), plus kan KP09 and KP21 and
vercel-chatbot KP04.

**What was fixed.** Lazy builders returned from non-async functions now run
where the caller awaits the call (`Execution::ReturnedLazy`). An unawaited
call drops them, because a builder nobody awaits never runs
(`crates/midwrite-analyzer/tests/returned_builders.rs`). This is a small group (1 dev miss),
chosen because it is an analyzer bug. The largest group, unrecognized
external effects, is a configuration-coverage question and is left open:
built-in recognition of common SDKs would change the default rules, and
needs a decision first.

Record: [`reports/eval/2026-11-06-dev148-returned-builders.json`](../../reports/eval/2026-11-06-dev148-returned-builders.json).
Differences from the second scoring:

* Removed findings: none.
* New findings: 3, all labeled intended-risk by a separate agent: midday
  `enrich-transaction.ts:147`, zero `assistant.ts:253` (KP08, auditor and
  labeler disagree → `reviewQueue`), and held-out dittofeed
  `requestContext.ts:353`. dittofeed was refrozen with that reason; no
  existing label changed.

| set | precision (hazard) | recall |
| --- | --- | --- |
| dev (midday + zero) | 0.84 (76/90) | 0.63 (33/52); was 0.62 (32/52) |
| held-out | 0.96 (119/124); was 0.96 (118/123) | 0.77 (48/62), unchanged |

Stage-1 gate: still PASS.

### Built-in SDK effects (largest group)

The largest dev group, unrecognized external effects, is addressed by
built-in recognition of common SDK clients (`builtinEffects`, on by default;
[configuration](../reference/configuration.md#built-in-sdk-effects)). Clients
are matched by provenance: constructed or typed in the file, or imported from
a project module that constructs or returns one. A name alone never matches.

Record: [`reports/eval/2026-11-06-dev148-sdk-builtins.json`](../../reports/eval/2026-11-06-dev148-sdk-builtins.json).

* midday known positives found: 17/30 → 20/30. KP13 (Supabase auth admin and
  Resend contacts), KP20 (BullMQ `job.remove()` through `getQueue()` and
  `getJob()`) and KP22 (Polar) are found now.
* Still missed from that group: KP02, KP06 and KP07 (effects inside
  project classes reached through dynamic providers), KP24 (the storage
  helper's client parameter has a local structural type, `StorageClient`,
  not the SDK's client type, so nothing proves it is a Supabase client), and
  KP28 (a Redis client held in a class field, `this.redis`, behind an object
  service).
* New findings: midday 32 (dev) and dittofeed 3 (held-out). Separate agents
  labeled them:
  * midday: 6 true-positive, 20 intended-risk, 6 false-positive. All 6
    false positives pair a real effect with a commit on a path where both
    cannot happen, mostly cleanup on failure (`if (!updated) await
    job.remove()`). The recognition is right; the path reasoning is the
    weak point.
  * dittofeed: 3 intended-risk (a best-effort "view in browser" copy before
    the send). dittofeed was refrozen with that reason; no existing label
    changed.

| set | precision (hazard) | recall |
| --- | --- | --- |
| dev (midday + zero) | 0.84 (102/121); was 0.84 (76/90) | 0.70 (38/54); was 0.63 (33/52) |
| held-out | 0.96 (122/127); was 0.96 (119/124) | 0.77 (48/62), unchanged |

Stage-1 gate: still PASS. Recall denominators grow when a new finding is a
true positive outside the audit (midday 12 → 14).


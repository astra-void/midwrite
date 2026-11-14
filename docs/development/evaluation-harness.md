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
| kan (kanbn/kan) | dev (since 2026-11-12) | `1cc2f332` | tRPC + repositories, email, webhooks, Stripe, S3 |
| dittofeed | dev (since 2026-11-12) | `91277939` | Postgres through `db()`, Temporal, ClickHouse, Kafka |
| zero (Mail-0/Zero) | dev (held-out until 2026-11-06) | `36132735` | Workers + Durable Objects, Gmail APIs, queues, R2/KV |
| vercel-chatbot (vercel/chatbot) | dev (since 2026-11-12) | `a8989b52` | Next.js AI chat: queries in `lib/db/queries.ts`, Vercel Blob, Redis streams and rate limit |
| nextjs-billing (lmsqueezy/nextjs-billing) | **held-out** (since 2026-11-12) | `5ddd7680` | Next.js server actions, Lemon Squeezy subscriptions, webhook route; Drizzle over Neon HTTP, no transactions |
| petdex (crafter-station/petdex) | **held-out** (since 2026-11-12) | `31c0aee7` | Next.js routes and admin flows; Drizzle over Neon HTTP, R2 writes, Resend, Clerk |
| next-saas-stripe-starter (mickasmt/next-saas-stripe-starter) | **held-out** (since 2026-11-12) | `c6df229a` | Better Auth hooks with the Stripe plugin; Drizzle over Neon serverless |
| react-starter-kit (kriasoft/react-starter-kit) | **held-out** (since 2026-11-12) | `dbdb0d49` | Hono + tRPC on Workers; every write happens inside Better Auth |

Reserve: empty. All former held-out projects moved to dev before the
2026-11-12 coverage fixes below. nextjs-billing was added afterwards as the
only held-out project ([held-out scoring](#nextjs-billing-held-out-2026-11-12)),
then three more by a fixed sampling rule
([held-out set restored](#held-out-set-restored-2026-11-12)). Every project uses Drizzle + PostgreSQL. The first
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

The first scoring below failed the threshold (0.89). The gate passes only
since the second scoring's set change (0.96), so the job depends on the
current `sets.json` and `FROZEN.json`. The job was not enabled by lowering
the threshold or editing frozen labels. Follow the set rotation rules above
when a held-out result drives an analyzer change.

## Label rekeys

Labels are matched to findings by fingerprint. A change to how fingerprints
are computed therefore leaves every affected finding unlabeled, and the gate
fails even though the findings themselves are unchanged.

On 2026-11-11, DEV-152 removed positions from anonymous-function names
(`<anonymous@11:18>`), which changed 124 fingerprints: kan 71, midday 46,
zero 5 and dittofeed 2. These labels were rekeyed as follows:

- Each old ID was looked up in the last scoring record
  (`reports/eval/2026-11-06-dev148-sdk-builtins.json`). Midday labels store
  only the ID, so this record is the only source of their locations.
- Each was matched to exactly one new unlabeled finding with the same code,
  file, line and position-free function name.
- Only the ID strings were replaced. No classification changed.

kan and dittofeed were refrozen with that reason, and every score matched
the DEV-148 record (held-out 0.96, 122/127). A label that was already stale
before the change (one in midday) stays stale.

## Caveats

* Every label so far was written by AI agents (Claude): the audits that
  produced the known positives, and the classification of findings. The
  agents doing each job were separate, and the auditors never saw
  Midwrite's output. That is not independent human review. Treat the
  numbers as provisional until a person has reviewed the labels; record
  disagreements in the label file's `corrections`. The fixed review sample,
  record format and adjudication steps are in
  [human review](human-review.md) (DEV-162).
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

## Source-grounded label corrections (2026-11-12)

Record: [`reports/eval/2026-11-12-label-corrections.json`](../../reports/eval/2026-11-12-label-corrections.json).
Scored with a debug build of the current working tree. This corrects the
evaluation labels; it does not improve analyzer coverage. No analyzer,
configuration, finding classification or recall-matching rule changed.

At Dittofeed's pinned revision `91277939`, two label problems were checked
against the source and corrected in `corpora/eval/labels/dittofeed.json`:

* KP01–KP03 pair provider delivery with `submitTrack` / `submitBatch`.
  These helpers reach `insertUserEvents`, which writes to Kafka or
  ClickHouse, rather than a modeled PostgreSQL commit. Their pattern was
  corrected from B (MW1002) to E (MW1005). KP01 now matches the existing
  MW1005 finding; KP02 and KP03 remain missed. The strict pattern/code
  matching rule remains in place so an unrelated hazard in the same
  function cannot satisfy the audit. KP01's auditor/labeler disagreement
  about whether the risk is intended is retained in `reviewQueue`.
* KP04 assumed a crash between changing a journey from Paused to Running
  and starting its restart workflow. `upsertJourney` returns the updated
  row from `UPDATE ... RETURNING` (`journeys.ts:1083–1141`), but the restart
  guard requires that returned row to still be Paused while the requested
  status is Running (`1183–1198`). The restart branch is unreachable, so
  this is not an interrupted partial-write window. KP04 was removed from
  the active known positives; its full original entry and the source
  evidence are preserved in `corrections`. The existing false-positive
  finding on this branch remains false-positive. A possible application
  logic bug in an unreachable branch is outside this evaluation.

Dittofeed was refrozen with this reason, preserving the previous input
hashes. Other projects' frozen inputs were unchanged. Labels and these
corrections remain AI-reviewed, without independent human validation.

| project / set | hazard precision | recall | FP | FN |
| --- | --- | --- | ---: | ---: |
| kan | 87/91 (95.60%) | 31/33 (93.94%) | 4 | 2 |
| dittofeed | 24/25 (96.00%) | 11/20 (55.00%) | 1 | 9 |
| vercel-chatbot | 11/11 (100%) | 7/8 (87.50%) | 0 | 1 |
| **held-out** | **122/127 (96.06%)** | **49/61 (80.33%)** | **5** | **12** |
| dev | 102/121 (84.30%) | 38/54 (70.37%) | 19 | 16 |

Before these label corrections, the same analyzer produced held-out
recall 48/62 (77.42%), FP 5 and FN 14. The two fewer misses come from
recognizing an already-detected window and removing an invalid positive,
not from fixing analyzer misses. Dev still has 121 unlabeled and 6 unclear
findings, excluded from precision. Stage-1 gate: PASS. The evaluation
suite's 12 tests pass, including pattern/code, file/span and false-positive
exclusion checks.

## Effect coverage fixes and remaining false positives (2026-11-12)

Records: [current scoring](../../reports/eval/2026-11-12-effect-coverage.json),
[comparison with the corrected baseline](../../reports/eval/2026-11-12-effect-coverage-comparison.json).
This comparison uses the same pinned Kan, Dittofeed and Chatbot sources.
They were moved to dev **before** these results drove changes. Previous
frozen inputs remain archived under `retired` in `FROZEN.json`.

All 12 original misses are detected. Three original false-positive
diagnostics disappear: Kan's S3 health probes, the invitation's
success/failure branch and its transaction-only board-member helper.
Two original false positives remain. Recognizing more operations also
exposes an additional false-positive diagnostic on the existing unreachable
Dittofeed restart branch; these were not suppressed or relabeled to raise
precision.

| development comparison | hazards | hazard precision | recall | FP | FN | unclear |
| --- | ---: | --- | --- | ---: | ---: | ---: |
| kan | 97 | 96/97 (98.97%) | 33/33 (100%) | 1 | 0 | 0 |
| dittofeed | 61 | 56/58 (96.55%) | 30/30 (100%) | 2 | 0 | 3 |
| vercel-chatbot | 13 | 13/13 (100%) | 8/8 (100%) | 0 | 0 | 0 |
| **same three projects** | **171** | **165/168 (98.21%)** | **71/71 (100%)** | **3** | **0** | **3** |

The before numbers were FP 5, FN 12, precision 122/127 (96.06%) and
recall 49/61 (80.33%). Ten newly exposed true-positive findings outside
the audit increase the recall denominator to 71. All 54 audited positives
are also found (54/54). Fifty new diagnostics were reviewed by Codex from
source, without blind or independent human review; three provider/error
semantics cases remain unclear and are excluded from precision. No finding
is unlabeled on these three projects. This is a tuned development result,
not a held-out generalization score or runtime confirmation.

The implementation changes address these causes:

* Preserve synchronous promise-forwarding calls, and retain the laziness
  of builders forwarded through them. Retain a builder argument when the
  callee demonstrably awaits it, even when the wrapper's own effects are
  unknown because of logging.
* Connect single-use `const` promise collections to `Promise.all`; sample
  a second invocation for branching inline callbacks, with separate call
  frames. This exposes the mixed update/insert message batch.
* Recognize AWS multipart `Upload.done`, ClickHouse `insert`, configured
  local functions and import aliases, and configured zero-argument factory
  receivers. Kan's broad `client.send` rule was removed so S3 command
  provenance distinguishes writes from health probes.
* Resolve workspace emitted exports to analyzed source using declared
  TypeScript `outDir` and `rootDir` (or composite project's default root).
  Missing build output alone no longer hides the template write.
* Model an inline OpenTelemetry `startActiveSpan` callback in its lexical
  frame, preserving captures and local returns. Cache only eight callback
  contexts per callee; analyze excess contexts without caching instead of
  dropping their effects. Specialized callback hazards are reported at the
  caller that supplied the callback, without duplicating intrinsic hazards
  through unrelated outer calls.
* Kan opts into `closedWorld`: all observed calls to the board-member
  helper pass `tx`, and its expanded body writes only through that same
  parameter. Open-world, mixed-handle, escaping or unobserved helpers keep
  their conservative standalone analysis.
* Kan's invitation rule declares `resolvedResult: {"status": true}` from
  the [pinned Better Auth 1.4.6 implementation](https://github.com/better-auth/better-auth/blob/v1.4.6/packages/better-auth/src/plugins/magic-link/index.ts#L217-L227).
  It prunes only the unreachable falsy-status arm after successful
  resolution. The real insert-before-email crash window remains reported.

Remaining false-positive causes:

* Kan `import.ts:260`: token re-encryption and import creation are separate
  real writes, but their partial state is harmless and the tasks are
  semantically independent. The analyzer does not infer this application
  invariant from cryptographic transformations.
* Dittofeed `journeys.ts:1194` and `:1236`: the updated status travels
  through `UPDATE RETURNING`, a result wrapper and result transformations.
  The analyzer still cannot establish that the Running/Paused restart
  guard is contradictory. One operation pair now produces MW1001 and the
  previous pair still produces MW1005. Both remain labeled false-positive.

The CI evaluation job scores development projects while the held-out set
is empty and explicitly says that no held-out gate was measured. If new
held-out projects are added, the unchanged precision threshold and
`--gate` checks apply again. `--gate` itself still fails on an empty set.

## Returned-row facts and taxonomy correction (2026-11-12)

This supersedes the remaining-FP list in the effect-coverage record above.
Records: [scoring](../../reports/eval/2026-11-12-returned-row-facts.json),
[comparison](../../reports/eval/2026-11-12-returned-row-facts-comparison.json).
The same three projects remain development inputs.

Two Dittofeed findings (`ed4a3babab01f114`, `29694cf677dd7e5d`) disappear
because the analyzer now derives the returned row's status through the
actual source helper and Result transformations. It resolves the schema's
namespace/destructuring alias, verifies the PostgreSQL enum column, and
establishes that a compatible defined Running assignment returns Running.
The subsequent Running/Paused restart conjunction is contradictory. Both
the commit-to-restart and restart-to-recompute diagnostics disappear;
the real commit-to-recompute finding `068753260a2b6e61` remains. No finding
IDs, file exclusions or per-project guard rules were added to the detector.
Details and boundaries: [returned-row facts](../architecture/returned-row-facts.md).

Kan `221cf564aa4c6cea` remains reported. Its previous false-positive note
already established harmlessness and semantic independence, not the
absence of a write. Token re-encryption really commits before the import
row insert, preserving the decoded credential with a compare-and-set
storage upgrade. Under the established taxonomy this is `intended-risk`:
a real, accepted window. Its classification was corrected, with the full
previous entry and source evidence retained in `corrections`. This is a
label correction, not an analyzer suppression or a confirmed bug fix.
The independent-maintenance fixture ensures such windows stay reported.

| development comparison | hazards | hazard precision | recall | FP | FN | unclear |
| --- | ---: | --- | --- | ---: | ---: | ---: |
| kan | 97 | 97/97 (100%) | 33/33 (100%) | 0 | 0 | 0 |
| dittofeed | 59 | 56/56 (100%) | 30/30 (100%) | 0 | 0 | 3 |
| vercel-chatbot | 13 | 13/13 (100%) | 8/8 (100%) | 0 | 0 | 0 |
| **same three projects** | **169** | **166/166 (100%)** | **71/71 (100%)** | **0** | **0** | **3** |

All 86 previously true-positive findings remain. No new finding is
unlabeled. The three unclear cases are still excluded from precision;
true-positive plus intended-risk windows form its numerator. Bug precision
remains a separate 86/166 (51.81%). All 54 audited positives are found;
the expanded recall denominator is unchanged at 71. The result is tuned
development performance with AI-only source labels, not an independent
held-out result or runtime confirmation.

The regression suite tests the real post-commit publication alongside the
removed contradiction, previous-row snapshots, undefined assignments,
numeric-to-text coercion, custom codecs, missing schemas, mutations,
escapes, captured writes, mutable input re-reads, repeated helper calls,
unawaited promises, generators, formatting and constant-object aliases.
The refinement uses the source bytes already lowered and cancels the proof
when supported semantics or its fixed budgets are insufficient.

### Reading the 100% figures

The table above is correct arithmetic, but it overstates what changed and
what is known. Quote these figures with it:

* **Analyzer change alone: 165/166 (99.40%).** Of the three removed false
  positives, the analyzer removed two (Dittofeed). The third (Kan
  `221cf564aa4c6cea`) left the false-positive count because its label was
  corrected, not because the output changed. The correction fits the
  taxonomy (MW1004 claims a window between two commits, and both commits
  happen), but it was made the same day by the AI session that tuned the
  analyzer, and was edited in place rather than re-labeled by a separate
  agent through `reviewQueue`. Its evidence also shows the partial state is
  harmless, not that the application accepted it on purpose, which is what
  `intended-risk` means. A separate labeler, given only the diagnostic and
  the pinned source (no labels, reports or prior notes), then chose
  `intended-risk` independently; the agreement is recorded in Kan's
  `corrections` as an `independent-recheck`. Both are AI judgments, so
  report both numbers until a human confirms it.
* **Audited recall: 54/54.** 17 of the 71 recall positives are true-positive
  findings outside the audit (Kan 3 of 33, Dittofeed 14 of 30). They enter
  the denominator only because Midwrite reported them, so they are found
  by construction. Only the 54 audited positives measure misses.
* **Sample size.** Wilson 95% intervals: hazard precision 166/166 is
  [97.7%, 100%] (165/166: [96.7%, 99.9%]), audited recall 54/54 is
  [93.4%, 100%], bug precision 86/166 is [44.3%, 59.3%]. Counting the three
  unclear findings as false positives gives 166/169 (98.22%).
* **Severity does not rank bug likelihood.** Bug precision by code: MW1001
  (warning) 10/27 (37%), MW1002 (warning) 6/13 (46%), MW1004 (info) 60/94
  (64%), MW1005 (info) 10/32 (31%). Warnings as a whole are 16/40 (40%),
  info 70/126 (56%). The intervals overlap, so this is not evidence that
  info is more often a bug, but there is none that warnings are.
* **The "dev" set in the record is three of the five dev projects.** The
  report's `sets.dev` covers only Kan, Dittofeed and vercel-chatbot.
  Scoring all five development projects with the same working tree:

| development set (all five) | hazards | unlabeled | unclear | hazard precision | bug precision | recall |
| --- | ---: | ---: | ---: | --- | --- | --- |
| midday | 232 | 121 | 6 | 94/105 (89.52%) | 29/105 (27.62%) | 36/46 (78.26%) |
| kan | 97 | 0 | 0 | 97/97 (100%) | 51/97 (52.58%) | 33/33 (100%) |
| dittofeed | 59 | 0 | 3 | 56/56 (100%) | 24/56 (42.86%) | 30/30 (100%) |
| zero | 30 | 0 | 0 | 20/30 (66.67%) | 2/30 (6.67%) | 4/10 (40.00%) |
| vercel-chatbot | 13 | 0 | 0 | 13/13 (100%) | 11/13 (84.62%) | 8/8 (100%) |
| **all dev** | **431** | **121** | **9** | **280/301 (93.02%)** | **117/301 (38.87%)** | **111/127 (87.40%)** |

  Record: [full dev scoring](../../reports/eval/2026-11-12-full-dev-labels.json).
  Audited recall across all five is 78/94 (midday 20/30, zero 4/10); the
  other 33 positives are true-positive findings outside the audit. The 121
  unlabeled midday findings are the long-standing unsampled remainder.

  The returned-row change does not alter midday or zero: their finding IDs
  are identical with a build of the previous commit. Since the
  label-corrections record earlier the same day, however, midday went from
  222 to 232 findings and zero from 26 to 30, introduced by the
  effect-coverage commit `b96f446`, the only analyzer change between the
  two records. A separate blind labeler (no labels, reports or known
  positives) reviewed those 14: 2 true-positive, 10 intended-risk and 2
  false-positive. The false positives are midday
  `batch-process-matching.ts:46` deletes that `updateInbox` performs only
  for `status: "deleted"`, which the batch-matching path never passes, so
  `b96f446` added two false positives on midday. The labels and reasons are
  in the midday and zero `corrections`.

## Literal arguments decide callee branches (2026-11-12)

Records: [scoring after](../../reports/eval/2026-11-12-literal-arguments.json),
[finding-ID comparison](../../reports/eval/2026-11-12-literal-arguments-comparison.json).
This is development tuning on dev projects only, with no held-out set. A
separate blind labeler classified midday `3b74ca15611c4770` and
`dc5ad0d481bf0d62` (`batch-process-matching.ts:46`) as false positives.
Both came from the effect-coverage change (`b96f446`). Each pairs an inbox
write with a delete that `updateInbox` runs only under
`if (data.status === "deleted")`, after
`const { id, teamId, ...data } = params`. On that path every call passes
the literal `"analyzing"`, `"pending"` or `"suggested_match"`.

Before this change, literal arguments never pruned callee branches. Now a
fact on a parameter snapshot is an input of the function's summary:
a parameter, a binding destructured from one, or a property of the rest
object of one
([value analysis](../architecture/value-analysis.md#path-facts-and-loops)).
Each call records the literal properties of its object arguments before
they escape. When the summary is instantiated, an arm whose fact
contradicts the literal becomes infeasible for that call. There are no
finding, file or project rules. Regression:
`fixtures/projects/literal_argument_branch`. That fixture covers decided
literals, an unknown status, a missing status, `"deleted"`, a forwarding
helper, a helper that rewrites the status before forwarding and a
parameter that escapes before it is destructured.

Full dev set, rebased onto the returned-row and blind-label commits.
Before is [full dev scoring](../../reports/eval/2026-11-12-full-dev-labels.json),
where the blind labeler had already classified both targets as false
positives; after differs only by this change.

| project | hazards before → after | unlabeled | unclear | hazard precision | bug precision | recall |
| --- | --- | --- | --- | --- | --- | --- |
| midday | 232 → 217 | 121 → 109 | 6 → 5 | 94/105 → 94/103 | 29/105 → 29/103 | 36/46 → 36/46 |
| kan | 97 → 97 | 0 → 0 | 0 → 0 | 97/97 → 97/97 | 51/97 → 51/97 | 33/33 → 33/33 |
| dittofeed | 59 → 59 | 0 → 0 | 3 → 3 | 56/56 → 56/56 | 24/56 → 24/56 | 30/30 → 30/30 |
| zero | 30 → 30 | 0 → 0 | 0 → 0 | 20/30 → 20/30 | 2/30 → 2/30 | 4/10 → 4/10 |
| vercel-chatbot | 13 → 13 | 0 → 0 | 0 → 0 | 13/13 → 13/13 | 11/13 → 11/13 | 8/8 → 8/8 |
| **dev** | **431 → 416** | **121 → 109** | **9 → 8** | **280/301 → 280/299** | **117/301 → 117/299** | **111/127 → 111/127** |

Kan, Dittofeed, Zero and Chatbot have identical finding IDs before and
after. Midday does not: 17 finding IDs disappear and 2 appear, so the
fix is broader than the two targets.

* All 17 removed findings are MW1004 and come from the same mechanism. Sixteen pair
  a write with the `transactionAttachments` or
  `transactionMatchSuggestions` delete in `updateInbox`. They are reached
  through calls with literal statuses (`"processing"`, `"analyzing"`,
  `"pending"`, `"other"`), in batch matching, bidirectional matching, the
  upload processors, bot inbox upload and `calculateInboxSuggestions`.
  The other one, `b900e23c8134a906` (`upcoming-notification.ts:228`),
  pairs with a `platformIdentities` update. Only the `type === "match"` arm of
  `sendToProviders` reaches it, and this call passes
  `"recurring_invoice_upcoming"`. One removed finding,
  `41b76f15d5a434ea-2`, was labeled `unclear`, and the two targets were
  labeled false-positive; none was labeled true positive or intended
  risk. Hazard precision rises only by the two removed false positives
  (280/301 → 280/299); recall does not move.
* Pruning brings `run` (jobs `batch-process-matching.ts:34`) and
  `BatchProcessMatchingProcessor.process` with its callback under the
  512-path limit (four MW9001 notes fewer). The bidirectional-matching `run` stays
  truncated, but a different set of 512 paths is now analyzed. This exposes `f7562c7236a52276` (MW1004, jobs
  `batch-process-matching.ts:34`, inbox update and inbox update; the
  worker twin `a3d512ea943bb6ce` was already reported) and
  `53f4da3bbb577624` (MW1002, jobs
  `match-transactions-bidirectional.ts:104`, the phase-1 match
  notification before a phase-2 inbox update). A separate blind labeler
  (no labels, reports or known positives) classified both as
  `intended-risk`: each pairs operations on different inbox items, and
  retried jobs, best-effort notification handling and the stuck-`analyzing`
  reset accept the window. The labeler noted that false-positive is
  arguable for both, because the two operations belong to different
  records. With these labels, dev is hazard 282/301 (93.69%), bug 117/301
  (38.87%) and recall 111/127, with 107 unlabeled
  ([labeled scoring](../../reports/eval/2026-11-12-literal-arguments-labeled.json)).

## Every dev finding labeled (2026-11-12)

Record: [scoring](../../reports/eval/2026-11-12-midday-full-labels.json).
Midday's precision so far came from a labeled sample; 107 findings were
unlabeled. Four separate blind labelers (no labels, reports or known
positives, source only) classified all of them: 46 true-positive, 54
intended-risk, 7 false-positive. Test seed code and retried or
best-effort work account for most intended risks. Six pairs act on
different records (item A's write and item B's write in one batch); the
window exists, so they were not called false positives for that reason,
and they are marked `crossRecord` in the labels so the rule can be
revisited. Each false positive carries an `fpCause`. No existing label
was changed.

| development set (all five) | hazards | unlabeled | hazard precision | bug precision | recall | audited recall |
| --- | ---: | ---: | --- | --- | --- | --- |
| before | 416 | 107 | 282/301 (93.69%) | 117/301 (38.87%) | 111/127 | 78/94 |
| after labeling | 416 | 0 | 382/408 (93.63%) | 163/408 (39.95%) | 152/168 | 78/94 |

Labeling did not move precision. Recall rises only because the 41 new
true positives outside the audit enter its denominator (found by
construction); audited recall is unchanged at 78/94.

Remaining false positives by cause: zero's 10 are all the generic
`fetch` rule matching read-only GETs and a stateless token exchange.
Midday's 16 are 4 `updateInbox` deletes behind a literal status that
was not decided (fixed below), and twelve single causes (an update that
matched no row, mutually exclusive branches, a private helper that only
ever receives `tx`, notifications the labelers did not see as owed).

Tried and ruled out: adding `httpMethods: ["POST", "PUT", "PATCH",
"DELETE"]` to zero's `fetch` rule. Zero's writes go through
`makeAuthenticatedRequest(url, options)`, which spreads `options` into
`fetch`, so the method is dynamic and the writes stop matching, while the
OAuth token exchange (a literal `POST`) still matches. It removed 9 false
positives and 4 real intended-risk windows, and the 6 findings it exposed
were all false positives on the token exchange. The configuration was
left unchanged.

## Literal entries of object literals in loops (2026-11-12)

Record: [scoring](../../reports/eval/2026-11-12-loop-literal-entries.json).
The four remaining midday `updateInbox` false positives
(`match-transactions-bidirectional.ts:154` and `:171`, jobs and worker
copies) pass the literal `status: "pending"`, but their entry value was
unknown. The call sits in a loop, so the object literal is evaluated
again with the same allocation site; the heap then holds a summary
object for all iterations and cannot export it. The call still receives
the object just created, so its entry value is now read from the literal
itself: literal-valued properties keep their literal, other properties
are unknown, and a spread, computed key or accessor leaves it unknown
([value analysis](../architecture/value-analysis.md#path-facts-and-loops)).
Regression: `markEachPending`, `removeEach` and `markEachWithExtra` in
`fixtures/projects/literal_argument_branch`.

Exactly those four finding IDs disappear; no other finding in the five
dev projects changes.

| development set (all five) | hazards | hazard precision | bug precision | recall | audited recall |
| --- | ---: | --- | --- | --- | --- |
| before | 416 | 382/408 (93.63%) | 163/408 (39.95%) | 152/168 | 78/94 |
| after | 412 | 382/404 (94.55%) | 163/404 (40.35%) | 152/168 | 78/94 |

Wilson 95% intervals after: hazard precision [91.9%, 96.4%], bug
precision [35.7%, 45.2%]. This is development tuning on AI-only labels;
there is no held-out set.

## nextjs-billing held out (2026-11-12)

Record: [`reports/eval/2026-11-12-nextjs-billing-held-out.json`](../../reports/eval/2026-11-12-nextjs-billing-held-out.json)
(release build of `095ce4a`; labels and configuration frozen before scoring).

lmsqueezy/nextjs-billing (MIT) is pinned at `5ddd7680`, the HEAD of `main`
on 2026-11-12. It uses Drizzle over Neon HTTP with no transactions, and the
Lemon Squeezy SDK, which is not a built-in effect. Three separate AI agents
did the steps, following the rules above:

* **Configuration:** written from source by an agent that did not run the
  analyzer. Its four rules are `createCheckout`, `cancelSubscription`,
  `updateSubscription` (payment) and `createWebhook` (http-request), all
  imported from `@lemonsqueezy/lemonsqueezy.js`. Reads such as `getPrice`
  and `listProducts` are left out.
* **Known positives:** a source-only audit that did not see Midwrite
  output found 3, all pattern D:
  * KP01: `syncPlans` upserts one plan per variant between SDK reads that
    can throw, and runs only while the table is empty.
  * KP02: the webhook route stores the event, then processes it.
  * KP03: `processWebhookEvent` applies the subscription upsert, then marks
    the event processed.

  The auditor rejected the four subscription actions (cancel, pause,
  unpause, change plan). In each, the SDK call comes before the row update
  (pattern B), and the `subscription_updated` webhook reconciles the row.
* **Findings:** 7 hazards and 1 MW9001. A third agent, which saw neither
  the audit nor the labels, classified them: 1 true-positive and 6
  intended-risk.
  * The true positive is `pauseUserSubscription` (`actions.ts:377`). The
    webhook upsert always writes `isPaused: false`, so nothing restores the
    flag.
  * The other three MW1002 findings are reconciled by the webhook. The
    three MW1004 findings are repaired by redelivery and the idempotent
    upsert.
  * KP02 and KP03 are in `reviewQueue`. The auditor says the stored event
    row stays unprocessed forever. The labeler says nothing reads
    `processed`.

| set | hazards | hazard precision | bug precision | recall | audited recall |
| --- | ---: | --- | --- | --- | --- |
| **held-out (nextjs-billing)** | 7 | **7/7 (100%)** | 1/7 (14%) | 3/4 | 2/3 |

**Stage-1 gate: PASS.**

* The sample is small. Wilson 95% intervals: hazard precision 7/7 is
  [64.6%, 100%], bug precision 1/7 is [2.6%, 51.3%] and audited recall 2/3
  is [20.8%, 93.9%]. The README keeps the 127-hazard pre-tuning scoring as
  its main estimate and quotes this one beside it.
* The missed known positive, KP01 (`syncPlans`), is a held-out miss. Under
  the rules above it is recorded here, not used to choose a fix. A fix
  driven by it moves nextjs-billing to dev.
* Six of the seven findings depend on Lemon Squeezy sending
  `subscription_updated` and redelivering webhooks that did not get a 200.
  The labeler could not verify that from the source.
* All labels are AI-only, with no human review.

## Request methods decided through helpers (2026-11-12)

Record: [scoring](../../reports/eval/2026-11-12-http-method-helpers.json).
Zero's 10 false positives came from its generic `fetch` rule. The earlier
attempt to add `httpMethods` failed because writes go through
`makeAuthenticatedRequest(url, options = {})`, which calls
`fetch(url, { ...options, headers })`: the method was dynamic. Now a
`fetch` whose method comes from such a parameter (spread last into the
options, or passed as them) is decided at each call of the helper, from
the argument passed, in the way literal arguments decide callee branches
([value analysis](../architecture/value-analysis.md#path-facts-and-loops),
[configuration](../reference/configuration.md#narrowing-http-effects)).
A missing method is a GET when the request target is a URL string (a
literal, `new URL(...)`, or a binding declared `string`/`URL`). A target
that may be a `Request` keeps an absent method unknown. A literal or a
conditional between literals decides it, and a dynamic method stays an
unknown call (MW9001), as in a direct call. Zero's config now restricts
`fetch` to `POST`, `PUT`, `PATCH` and `DELETE`. Regression:
`fixtures/vulnerable/http_method_helper.ts` (implicit GET, literal PUT,
POST-or-PUT conditional, DELETE forwarded through a second helper, and two
dynamic methods), plus direct-call cases in
`crates/midwrite-analyzer/tests/http_methods.rs`.

Decided: the OAuth token exchange (`getAccessToken`, a literal `POST`)
stays reported. A method does not show whether a request changes remote
state, and Midwrite does not guess from URLs or bodies. So once the GET
lookups no longer count, the token exchange they trigger becomes the
completed first effect of the same three windows.

Finding IDs against the
[previous record](../../reports/eval/2026-11-12-loop-literal-entries.json):
midday, kan, dittofeed and vercel-chatbot are unchanged. In zero, 9
findings disappear, all labeled false-positive: the 6 GET-lookup pairs at
`google-subscription.factory.ts:120`, `:157` and `:190`, the token
exchange followed by the now-unknown request inside
`makeAuthenticatedRequest` (`:97`), and the two `bimi.ts` GET pairs. No
true-positive or intended-risk finding is lost. 7 findings are new, and a
separate blind labeler (source only, no labels or reports) classified
them. 6 are false-positive: the token exchange is the completed first
effect at `:120`, `:157` and `:190` (`fpCause` "first effect is a token
fetch / read-only lookup"). 1 is intended-risk: `3a582c35f3710521`,
topic setup then subscription creation in `subscribe` (`:290`), retried
by the scheduled renewal. The remaining false positive from before is
`chat-attachments.ts:77`, a literal `POST` to a stateless conversion API.

| development set (all five) | hazards | hazard precision | bug precision | recall | audited recall |
| --- | ---: | --- | --- | --- | --- |
| before | 412 | 382/404 (94.55%) | 163/404 (40.35%) | 152/168 | 78/94 |
| after | 410 | 383/402 (95.27%) | 163/402 (40.55%) | 152/168 | 78/94 |

Zero alone: 20/30 → 21/28 hazard precision, with 10 → 7 false
positives. Wilson 95% intervals after (all five): hazard precision
[92.7%, 97.0%], bug precision [35.9%, 45.4%]. This is development tuning
on AI-only labels. The nextjs-billing held-out findings are unchanged by
this change: the same finding IDs, gate PASS (hazard precision 7/7).

## Held-out set restored (2026-11-12)

Record: [`reports/eval/2026-11-12-held-out-restored.json`](../../reports/eval/2026-11-12-held-out-restored.json)
(release build of `461c75e`; configurations and labels of every held-out
project frozen before this scoring). Linear DEV-161.

### Sampling rule

To keep the choice from depending on how well Midwrite does, candidates
came from one fixed list, taken in order: GitHub repositories with the
topic `drizzle-orm`, language TypeScript, not archived, not forks, sorted
by stars, queried on 2026-11-12 (`gh search repos --topic drizzle-orm
--language typescript --sort stars`). A candidate was eligible when:

1. it has an OSI license;
2. it is an application or application template (not a library, CLI,
   scaffolder or ORM tool) with commits in the last 12 months;
3. its server code uses Drizzle on the PostgreSQL dialect;
4. its server code imports at least one external-effect SDK (payments,
   email, object storage, queues, caches, webhooks);
5. it never appeared in Midwrite's development (no fixture, corpus,
   manifest or document names it).

The first three eligible projects were taken, at the HEAD of their default
branch: react-starter-kit (2nd by stars), petdex (6th) and
next-saas-stripe-starter (7th). Skipped: supermemory (1st, no Drizzle in
the public repository), kanbn/kan (3rd, already dev),
create-better-t-stack (4th, scaffolder) and skateshop (5th, no commits
since 2025-04). nextjs/saas-starter, cloned in `.corpora/` since early
development, fails rule 5: its invite flow is a fixture.

The rule judges the source only, so it kept react-starter-kit even though
all of its writes happen inside Better Auth. It contributes no findings and
no audited windows. It was not replaced: a replacement chosen after seeing
that would be a choice made on results.

### Procedure

The same three separate AI agents per project as for nextjs-billing:

* **Configuration** written from source by an agent that did not run the
  analyzer or read any finding. petdex adds two wrapper rules for R2 helpers
  that one caller reaches through a dynamic `import()`, and a Clerk
  `users.updateUser` rule. It excludes `scripts/**` (operator scripts).
  The two Better Auth projects need no rule beyond the built-ins.
* **Known positives** from a source-only audit that saw neither the
  configuration nor Midwrite output: petdex 12, next-saas-stripe-starter 3,
  react-starter-kit 0. The auditors also listed the candidates they
  rejected and why (`auditRejected` in each label file).
* **Findings** classified by a third agent that saw neither the audit nor
  the configuration rationale, only the source and the findings.

### Scores

| project | hazards | hazard precision | bug precision | audited recall |
| --- | ---: | --- | --- | --- |
| nextjs-billing | 7 | 7/7 | 1/7 | 2/3 |
| petdex | 24 | 22/24 | 2/24 | 4/12 |
| next-saas-stripe-starter | 1 | 1/1 | 0/1 | 1/3 |
| react-starter-kit | 0 | n/a | n/a | n/a (no audited windows) |
| **held-out (all four)** | **32** | **30/32 (93.75%)** | **3/32 (9.4%)** | **7/18** |

**Stage-1 gate: PASS.** The harness's recall line (8/19) also counts
nextjs-billing's true positive outside the audit.

* Wilson 95% intervals: hazard precision [79.9%, 98.3%], bug precision
  [3.2%, 24.2%], recall 8/19 [23.1%, 63.7%].
* One project carries most of the weight: petdex has 24 of the 32
  hazards. One more false positive there gives 29/32 (0.906), still a
  pass; two more fail the gate.
* petdex's two false positives: `26c8b8e69f66b1c1` pairs an approve-only
  publish with an edit-only republish that never run in one call
  (`submission-decisions.ts:179`, path infeasible), and `5c207dae68e2a103`
  pairs a `finishReview` update with a second one that runs only after the
  first matched no row (`submission-review.ts:249`, first change not
  persisted).
* Recall misses (held-out misses, recorded here and not used to choose a
  fix):
  * petdex KP06, KP07, KP09 and KP12 are operator scripts under
    `scripts/`, which the configuration excludes. Inside the analyzed code
    audited recall is 7/14.
  * petdex KP01 (`takedownPet`): the delete commits through a raw-SQL
    transaction or a neon-http `db.batch`, which Midwrite does not model
    (MW9001), so the later R2 delete has no commit before it.
  * petdex KP08, KP10, KP11 and nextjs-billing KP01 are not reported.
  * next-saas-stripe-starter KP01 and KP03 sit in Better Auth hooks whose
    first commit happens inside the library.
* `reviewQueue` holds four disagreements, where the auditor called a window
  unhandled and the labeler called the matching findings intended-risk:
  petdex KP02, KP03 and KP04 (best-effort emails and R2 cleanup after an
  approval) and next-saas-stripe-starter KP02 (a retry converges). Neither
  label was changed.
* All labels are AI-only, with no human review (DEV-162).

## Module-private transaction helpers (2026-11-12, DEV-166)

Record: [`reports/eval/2026-11-12-private-transaction-helpers.json`](../../reports/eval/2026-11-12-private-transaction-helpers.json).
The DEV-166 targets were taken only from dev-set misses and false
positives, never from held-out ones.

**Fixed.** midday's `createSystemCategoriesForTeam`
(`packages/db/src/queries/teams.ts:227`) is a module-private helper whose
only caller passes the transaction handle. It was reported as two
independent commits because the transaction-only refinement needed
`closedWorld`, which midday does not set. That opt-in guards against
callers outside the project, and no such caller exists for a function
that is not exported and whose binding is only ever called directly in its
own module. Such helpers are now refined without `closedWorld`. The
fixture `fixtures/projects/private_transaction_helper` keeps exported,
escaping, mixed-caller and aliased helpers reported.

| development set (all five) | hazards | hazard precision | bug precision | recall |
| --- | ---: | --- | --- | --- |
| before | 410 | 383/402 (95.27%) | 163/402 | 152/168 |
| after | 409 | 383/401 (95.51%) | 163/401 | 152/168 |

The only changed finding is that false positive. Held-out findings are
unchanged (30/32, gate PASS).

**Fixed: methods called on an inline construction.** `await new
C().m()` was not dispatched, although `const c = new C(); await c.m()` was
(and the supported-patterns reference lists both): the receiver was
evaluated before its `new` expression had been lowered, so its instance
was unknown. The construction is now lowered first, once. No dev or
held-out finding changes; `a_method_called_on_an_inline_construction_is_dispatched_once`
covers it.

**Tried and not shipped: SDK clients stored on `this`.** midday's
KP02 (`send-invoice-email.ts`) sends email through
`notifications.create`, whose `Notifications` class holds an
`EmailService` in a private field, which holds a Resend client in another.
Recognizing clients assigned to instance fields, dispatching
`this.field.m()` on a field that always holds one project class, and
modelling private members (`#x`) recovered KP02 but added 12 findings. A
separate blind labeler classified them as 5 intended-risk and 7
false-positive, none true-positive. Four of the false positives come from
`options?.sendEmail ?? false` with no options argument (no email is
sent), which value analysis does not prune; three report the single-send
branch that runs only with attachments. That would lower dev precision,
so the change was set aside until optional-chaining and nullish defaults
on parameters prune callee branches.

**Inventoried limits (not fixed):**

* zero's KP02–KP05: `db` is a Cloudflare Durable Object RPC stub
  (`scopedRpc<DbRpcDO>`) whose methods forward to a second Durable Object
  that runs Drizzle. Its methods are reported as unmodelled database
  methods (MW9001). Follow-up: DEV-179.
* `options?.x ?? default` on a parameter does not decide callee branches,
  even for an explicit `{ x: false }` argument. (Fixed on 2026-11-13, see
  below.)

## Optional arguments decide callee branches (2026-11-13, DEV-166)

Record: [`reports/eval/2026-11-13-optional-arguments.json`](../../reports/eval/2026-11-13-optional-arguments.json).
The target came from the dev-set false positives of the parked SDK-field
experiment above, never from held-out findings.

**Changed.** A call now decides a callee's branch on a read of a
parameter, not only on a destructured one: a `const` bound once to
`p.x`, `p?.x` or either `?? literal`, or such a read used directly as a
condition ([value analysis](../architecture/value-analysis.md#path-facts-and-loops)).
An omitted argument is `undefined`, a property of `null` or `undefined`
is `undefined`, and `?? literal` replaces both. When the parameter was
passed to other code before the read, only an omitted or `undefined`
argument decides it, because that code may have changed the object's
properties but cannot change the binding. Regression:
`fixtures/projects/optional_argument_branch` (omitted, `undefined`, an
explicit `{ purge: false }` read directly and after the object was handed
to other code, a forwarding helper, a reassigned parameter, a
caller-chosen value).

No dev or held-out finding changes (dev 383/401, held-out 30/32, gate
PASS): no dev path reaches such a read through a call that leaves the
argument out.

**SDK clients stored on `this`, tried again: still not shipped.** With
this change, the parked field-client patch adds 8 dev findings instead of
12. The four `options?.sendEmail ?? false` false positives
(`inbox-matching-notifications.ts:95,118`) are gone. The other 8 are the
same findings the blind labeler classified: 5 intended-risk and 3 false
positives. Those 3 report Resend's single `emails.send` branch, which runs only when
`emailPayloads.some((p) => p.attachments …)` holds. That depends on the contents of an array
built by `Promise.all(...map(...))`, which value analysis does not track.
Dev hazard precision would go from 383/401 (95.51%) to 388/409 (94.87%),
so the patch stays parked. Follow-ups: array-content tracking (DEV-177),
then field clients (DEV-178).

## Nested higher-order helpers (2026-11-13, DEV-166)

Record: [`reports/eval/2026-11-13-nested-wrappers.json`](../../reports/eval/2026-11-13-nested-wrappers.json).
The case came from an adversarial probe of wrapper shapes the DEV-166 scope
lists, not from held-out findings.

**Fixed.** A write reached through a helper that runs a callback, nested
in another such helper (`withSpan("x", () => withRetry(() =>
db.insert(t)))`, or `withRetry` in itself), was silently lost. Two causes:
the same helper with a different callback counted as recursion, and each
callback run used a level of `--max-call-depth`, so two helpers already
needed four. The expansion stack is now keyed by function and callback
arguments, and running a callback parameter uses no level; the stack still
cuts real cycles and every helper call still uses a level, so expansion
ends. Regression: `fixtures/projects/higher_order_wrappers` (nested,
mixed and three-deep helpers, a transaction helper inside a tracing one,
and a recursive helper). The baseline binary reported none of its findings
except the recursive one.

| development set (all five) | hazards | hazard precision | bug precision | recall |
| --- | ---: | --- | --- | --- |
| before | 409 | 383/401 (95.51%) | 163/401 | 152/168 |
| after | 413 | 387/405 (95.56%) | 164/405 | 152/168 |

The four new findings are all dittofeed code reached through `withSpan`.
A separate blind labeler classified them: 1 true-positive
(`broadcasts/activities.ts:196`, messages sent before their delivery
events are written; the retry guard depends on those events) and 3
intended-risk (`computePropertiesIncremental.ts:3753`, at-least-once
HubSpot signals; `computeProperties.ts:132` and `:133`, per-step period
rows that each step resumes from, cross-record). Held-out findings are
unchanged (30/32, gate PASS). Midday analysis time is unchanged (0.5 s).

**Inventoried, not fixed:** effects in callbacks a builtin defers
(`setTimeout(cb)`, `queueMicrotask(cb)`) and in promise continuations
(`p.then(cb)`) are not placed on any path; the call itself is a modeled
builtin, so not even an MW9001 note marks them. Deferred timer callbacks
stay out by design ("never part of the current sequence",
`fixtures/safe/deferred_callback.ts`).

**Tried and not shipped: promise continuations (2026-11-13).** Lowering an
awaited `p.then(cb)` as `p` then `cb`, `p.catch(cb)` as `try { p } catch
{ cb }` and `p.finally(cb)` as `try { p } finally { cb }`, with an inline
`cb` lowered in place, added 4 dev findings (held-out unchanged). A
separate blind labeler classified them as 1 true-positive (kan
`card.ts:620`, through a rethrowing `.catch`) and 3 false positives, all
midday code reached through `.catch(() => {})`: an infeasible path after
an early return (`bot/runtime.ts:194`), mutually exclusive branches
(`customers.ts:102`) and a best-effort `lastSeenAt` write with nothing
depending on it (`bot/runtime.ts:214`). Dev hazard precision would go from
387/405 to 388/409, so it was not shipped. Skipping only empty `.catch`
handlers would keep the true positive and drop the three, but their causes
are not the handler, so that rule would be fitted to these labels.
Re-measured on 2026-11-13 at `2e3ba8f`, after the cardinality guard and
factory-closure changes, the patch adds the same four findings: dev hazard
precision would go from 400/418 to 401/422, and held-out is unchanged.
The infeasible path at `runtime.ts:194` needs per-path literal properties
of returned objects (`consumeResolvedConversation` returns
`{ ...resolved, consumed: true }`), which Midwrite does not infer. That
work is DEV-175. The continuation work itself is DEV-176, which keeps the
rebased patch.

## Calls through factory results (2026-11-13)

Record: [`reports/eval/2026-11-13-returned-functions.json`](../../reports/eval/2026-11-13-returned-functions.json).
The target came from the development inventory of call-returned functions
and an adversarial probe, not from held-out findings.

**Fixed within a bounded subset.** A local `const f = make(); await f()`
or `await make()()` now follows one known returned function. A plain
synchronous factory must be proven inert; async factories must first be
awaited. Imported factories and inert forwarding helpers preserve function
identity across module numbering. Possible implicit returns and unions of
functions do not choose a target. Captured variables remain unknown, and
module-level factory bindings are not evaluated.

Regression: `fixtures/projects/returned_functions` covers imported and
forwarding factories with a preceding module that changes the function
offsets. `tests/returned_functions.rs` in the analyzer covers stored and
immediate calls, callback returns, async factories, ambiguous and implicit
returns, argument binding, unawaited effects and lazy builders, captures
and recursion. The initial stored-factory regression failed before the
change. This is fixture evidence, not a new real-application bug.

The five dev projects have exactly the same 413 finding IDs as the nested
wrapper baseline above: hazard precision 387/405, bug precision 164/405,
recall 152/168, 8 unclear and no unlabeled findings. All four held-out
projects are unchanged: 32 hazards, precision 30/32, gate PASS. No labels,
frozen inputs or configurations changed, and no corpus code was executed.
**There is no measured real-project recall gain from this change.**

## Lexical captures and module factory bindings (2026-11-13)

Record: [`reports/eval/2026-11-13-closure-environments.json`](../../reports/eval/2026-11-13-closure-environments.json).
This extends the returned-function work above, using first-party probes
and development-set coverage checks. No held-out finding drove a fix.

**Supported.** Immutable captured locals and parameters, callback values,
primitive parameter defaults and recognized database/transaction handles
travel with the closure. Capture slots follow its own parameters; callback
environments distinguish memo contexts. Callback-use summaries propagate
through captured callbacks and higher-order helper composition. Stable
module `const` factory bindings, aliases and named/default exports resolve
through inert initializer chains, including imports and barrels. Resolved
exports are analyzed as entries even without a project-local caller.

Regression: `tests/returned_functions.rs` in the analyzer and
`fixtures/projects/returned_functions` cover these cases, arrays,
per-creation isolation, transaction aliases, missing/explicit arguments,
mutation, initialization cycles (including a cycle in an unused factory
argument), temporal dead zones, async initialization and recursion.
The stable module alias in `private_transaction_helper/aliased.ts` now
resolves at its transaction call; its standalone helper warning remains.
Recursive closure composition in `higher_order_wrappers` now also carries
an explicit MW9001 limitation when the expansion bound is reached.

| set | before | after | interpretation |
| --- | ---: | ---: | --- |
| development hazards | 413 | 422 | all 413 old finding IDs retained; 9 new, unclassified |
| development harness recall (includes positives outside the audit) | 152/168 | 152/168 | unchanged; no measured recall gain |
| held-out hazards | 32 | 32 | findings unchanged; hazard precision 30/32, gate PASS |

The nine new development findings are one in midday (a batch update reached
through a captured handle), six in Kan (two captured batch writes and four
export-entry contexts), and two in Dittofeed (`sendMessages` and
`sendMessageV2`, both factory-built exports). An exported entry context can
overlap an already reported inner callback; these are not nine confirmed
or necessarily independent bugs. The existing labeled development score
remains 387/405, but excludes the nine new findings and is not a complete
precision estimate for the new analyzer. Labels, configurations and frozen
inputs were unchanged, and no corpus application code was executed.

Validation used an isolated source snapshot because unrelated Zod work was
in progress in the shared checkout. The snapshot included this change and
the preceding returned-function change; the shared checkout also passed a
workspace compile check after both changes were present. Mutable capture
contents, effectful/unlinked initializers and general module execution
remain outside this support.

## Review of the nine closure findings (2026-11-13)

Records: [per-finding source review](../../reports/eval/2026-11-13-closure-findings-review.json),
[same-binary labeled scoring](../../reports/eval/2026-11-13-closure-findings-labeled.json).
The user requested classification of the nine previously unlabeled
development findings. This is a source review in the implementation chat,
not a blind labeler, independent human review or runtime reproduction.

| finding | project / window | classification | overlap |
| --- | --- | --- | --- |
| `4fda8babb4a80988` | midday: account base values then transaction batches | intended-risk | retried recomputation; assignments overwrite the same derived values |
| `d758901597e4584a` | Kan: subscription entitlement then linked workspace plan | true-positive | same inner-handler window as `574be59f041f0e7e` |
| `d5803fc413673fa3` | Kan: workspace downgrade then member pausing | true-positive | same cancellation window as `83e6e8979513e69f` |
| `930f362854dc67c5` | Kan: slot creation then removed-slot deactivation | **false-positive** | growth and shrink are mutually exclusive |
| `cef2b1b6c27c48e9` | Kan: linked-slot insert then remainder-slot insert | true-positive | same helper window as `11727d753731af8a` |
| `305530861885c933` | Kan: imported cards then activity rows | true-positive | same window as `2aadc5212a97547b` |
| `ecaea63b0bab9693` | Kan: imported board then labels | true-positive | same window as `f322f5de643c442a` |
| `933e380a6fe7be7d` | Dittofeed: broadcast delivery then event batch | true-positive | same window as `4b0058e6a225b154`; older labels disagree |
| `5e0ac5ccb0740877` | Dittofeed: journey delivery then tracking event | intended-risk | same retry-aware activity window as `ece472399ebf7efb` / `0a33bc81d8b3fa98` |

**The false-positive path is impossible.** In the partner webhook, let
`m = existing.length` and `n = newCount`. `preferKeep` concatenates the
truthy and falsy `referenceId` partitions of `existing`, so its length is
exactly `m`. `createAndLinkSlots` requires `n > m`; removed-slot updates
require `preferKeep.slice(n).length > 0`, hence `n < m`. These cannot both
hold. The same argument applies to the migrate branch. A source-derived
cardinality sanity check also passed all 99 combinations of pinned tier
sizes (1, 2, 4) and existing counts 0–32; it did not execute project code.

The other eight claimed windows exist in source. Midday's worker retries
the same overwrite-style recomputation, so temporary stale derived values
are an accepted retried-job risk. Kan's import path has no outer rollback
or resume: `allSettled` and an import-status update do not repair committed
partial boards, and a rerun allocates fresh IDs. Its webhook writes also
have no shared transaction or durable compensation established by this
source; a possible provider redelivery is not a verified recovery contract.

The two Dittofeed judgments are kept explicit: broadcast filtering promises
to omit delivered recipients, but depends on the missing delivery events,
so a retry can resend. The journey exposes per-node retry limits and a
final-attempt skipped result; its existing at-least-once interpretation is
retained. Stable event IDs do **not** establish provider-side deduplication
or exactly-once delivery. Existing auditor/labeler disagreements remain in
the review queue and require human product-semantics review.

All nine labels were appended to the dev label files. Existing finding
labels and known positives were left unchanged. Seven new findings overlap
already reported windows: these are not six new independent bugs. Two
older Kan true-positive labels (`9303f5935fcd8c8a`, `e34fc613ab846935`)
appear to share the same impossible growth/shrink path; they were queued
for a separate correction rather than silently changed in this review.

The same binary still reports 422 dev hazards, now with zero unlabeled:
170 true-positive, 225 intended-risk, 19 false-positive and 8 unclear.
Hazard precision is 395/414 (95.41%); bug precision is 170/414 (41.06%).
The harness recall changes mechanically from 152/168 to 153/169 because
the newly labeled broadcast export is another TP outside the audit span;
every audited known-positive match is unchanged. This is not evidence of
a new independent bug or better audited recall. Held-out findings and
frozen files are unchanged (30/32 hazard precision, gate PASS).

## Cardinality guard false-positive fix (2026-11-13)

Records: [corrected-binary scoring](../../reports/eval/2026-11-13-cardinality-guards.json),
[newly selected feasible-pair review](../../reports/eval/2026-11-13-cardinality-guards-review.json).
The user requested a fix for the newly classified impossible-path finding
`930f362854dc67c5`, with regression coverage.

**Fixed generically.** The source prepass recognizes complementary pure
filters on an unchanged native array, records a length upper bound and
correlates a nonempty `slice(count)` with the growth comparison. Existing
boolean path facts eliminate the contradiction, including inside inlined
helpers and factory-built exports. No project name, fingerprint or
application-specific suppression is used by the analyzer. The proof does
not assume dense arrays: sparse partitions can be shorter than the source.
Its native-array/scalar model, bounds and invalidation rules are documented
in [value analysis](../architecture/value-analysis.md#array-cardinality-guards).

The reduced first-party fixture
`fixtures/safe/complementary_partition.ts` reported MW1004 at line 13
with the previous binary and is clean with the fix. Seventeen targeted tests
in `crates/midwrite-analyzer/tests/cardinality.rs` cover the exclusion and
equivalent guards, immutable aliases, non-complementary filters, different
indices, mutation and escape, getters/coercion hooks, array callbacks,
sparse arrays, reassigned counts and `length >= 0`. Feasible write pairs
remain reported. Core/analyzer regression tests, CLI tests, clippy, the
release build, formatting and documentation links passed in the validation
snapshot; the shared checkout also passed a workspace compile check.

The requested diagnostic and its two previously flagged inner-handler
equivalents (`9303f5935fcd8c8a`, `e34fc613ab846935`) disappear. Their
historical labels are preserved and become stale in this run. Removing
impossible paths also changes which feasible pair the detector selects and
lets it reach further migration branches within its path budget: seven
diagnostics become visible. Each was source-reviewed as a TP candidate,
not independently human-validated or reproduced. Unlike the old pair,
their first operation is a retained-workspace or retained-subscription
update; those can coexist with either growth or removal. See the review
record for concrete count witnesses and source lines.

| measure | before | after |
| --- | ---: | ---: |
| dev hazards | 422 | 426 (3 impossible pairs removed, 7 feasible pairs exposed) |
| dev unlabeled | 0 | 0 |
| dev hazard precision | 395/414 (95.41%) | 400/418 (95.69%) |
| dev bug precision | 170/414 | 175/418 |
| dev harness recall | 153/169 | 153/169 |
| held-out hazard precision | 30/32 | 30/32, gate PASS |

Every audited known-positive match and held-out finding ID is unchanged.
No corpus code was executed, no frozen inputs were edited, and these
additional contexts/branches are not seven confirmed independent bugs.

## Returned-object properties across helpers (2026-11-13, DEV-175)

Record: [`reports/eval/2026-11-13-returned-object-properties.json`](../../reports/eval/2026-11-13-returned-object-properties.json),
binary built from `91903bb` plus this change, scored with the sets of `91903bb`. The target came from the
parked promise-continuation patch's dev false positive at midday
`apps/api/src/bot/runtime.ts:194`, not from a held-out result.

**What changed.** A property of the object a function returns already
decided a caller's branch when every `return` was a plain object literal
(`fixtures/vulnerable/return_value_correlation.ts`). Four gaps kept midday's
link-code path from being excluded:

* a returned object with a spread (`{ ...resolved, consumed: true }`) gave
  no property facts. Properties written after the last spread now do;
  anything before a spread stays unknown;
* `return f(...)` passed none of `f`'s return facts on. The returned call is
  now bound to the function's own returned value, so the facts reach the
  caller's caller;
* a call of an inert helper (no effects) was an empty scope. When its
  result is bound, the facts that **every** `return` of the helper implies
  are added as one return that holds on all paths. A helper that sets the
  property on some paths only adds nothing, and no paths are added;
* a binding referenced as `!x`, `if (x)`, `x ? :` or `x == null` was not
  "member stable", and neither was one passed to other code later. Pure
  tests now count as reads. Outside loops and callbacks, a property test
  gets a fact when every earlier reference only reads or tests the binding,
  even if the object escapes afterwards. A method call or a write through
  the binding anywhere still disables it.

Fixtures: [`fixtures/safe/returned_object_properties.ts`](../../fixtures/safe/returned_object_properties.ts)
(spread, helper, returned call, cast, truthiness test, escape after the
test) and [`fixtures/vulnerable/returned_object_properties_unknown.ts`](../../fixtures/vulnerable/returned_object_properties_unknown.ts)
(a spread after the property, the property on one helper path only, a write
or an exposure before the test, a test in a loop). All of these remain
reported.

**Scores.** No finding changes on dev or held-out: dev hazard precision is
400/418 and held-out is 30/32, with the gate passing. On its own, the
change only removes paths that no current finding uses.

**The parked continuation patch on top (DEV-176).** It still adds four dev
findings. The targeted pair at `runtime.ts:194` (`21a77df7f0cd12c3`, whose
first write is `createOrUpdatePlatformIdentity`) is gone. In its place a
pair with the same second write appears (`47d042c897a2541f`). Its first
write is `consumePlatformLinkToken`'s `UPDATE ... RETURNING`, on the path
where the token is `null`, which is the path where the update matched no row.
Midwrite does not model "an empty `RETURNING` result means nothing was
written", so the continuation patch stays parked. The new pair is unlabeled:
it was not given a label here.

## Writes whose RETURNING row is missing (2026-11-13, DEV-180)

Record: [`reports/eval/2026-11-13-empty-returning-writes.json`](../../reports/eval/2026-11-13-empty-returning-writes.json),
binary built from `dcad6fb` plus this change, scored with the sets of
`dcad6fb`. The target was the dev pair that replaced the DEV-175 target
under the parked continuation patch (midday `47d042c897a2541f`), not a
held-out result.

**What changed.** `UPDATE ... RETURNING` and `DELETE ... RETURNING` return
exactly the rows they changed. An insert that skips conflicts
(`onConflictDoNothing`) returns nothing when it skipped. When such a
write's first row is bound (`const [row] = await
db.update(t)...returning()`), the write records `row is truthy` as a fact
that holds whenever it changed something. A trace whose facts contradict it
(`if (!row) ...`) gets no commit from that write. When every `return` after
the write returns that row (`return row`, `return row ?? null`) and the
function cannot fall off its end, the same holds for the function's
result. A caller that tests its binding of the result (`if (token)`)
decides the write. The rule does not apply in these cases:

* an upsert (`onConflictDoUpdate`), which returns rows it did not insert;
* a binding inside a loop or a callback;
* an operation that is not awaited where it runs, or that occurs more than
  once on the trace.

In those cases one binding stands for several executions. The first
version had no such guard and dropped two midday true positives in
`Promise.all(validInvites.map(...))`, where one element's empty insert
said nothing about another element's write.

Fixtures: [`fixtures/safe/empty_returning_write.ts`](../../fixtures/safe/empty_returning_write.ts)
and [`fixtures/vulnerable/returning_write_still_reported.ts`](../../fixtures/vulnerable/returning_write_still_reported.ts).
The second covers:

* a present row;
* an untested result;
* an upsert;
* a helper that may return `null` after writing;
* an array callback.

**Scores.**

* **midday.** Three false positives and one intended risk disappear, at
  `invoice.ts:552`, `invoice.ts:705`, `rest/routers/invoices.ts:563` and
  `process-attachment.ts:114`. On each path, the first "write" was an
  update that matched no row. At the same lines, other pairs become the
  representative finding: three are intended-risk and one is a
  false-positive (`5cded3e81f9a7b4f`, a delete that only runs for an
  argument this call does not pass).
* **petdex**, held-out at `dcad6fb`. False positive `5c207dae68e2a103` is
  replaced at the same line by false positive `135c580767456c38-2`.

A separate blind AI labeler, which saw only the source, labeled the five
new findings. The held-out label was added with a refreeze that records
the reason. No existing label changed.

| measure | before | after |
| --- | ---: | ---: |
| dev hazards | 426 | 426 |
| dev hazard precision | 400/418 | 402/418 |
| dev bug precision | 175/418 | 175/418 |
| dev recall | 153/169 | 153/169 |
| held-out hazard precision | 30/32 | 30/32, gate PASS |

**The parked continuation patch on top (DEV-176).** The pair at
`runtime.ts:194` is gone. The patch now adds three dev findings:

* kan `card.ts:620`, blind-labeled true-positive;
* midday `runtime.ts:214`, a best-effort `lastSeenAt` write;
* midday `customers.ts:102`, an enrichment cleanup in a `catch`.

The earlier blind labeler called the two midday findings false positives.
Their causes are not analyzer facts, so the patch stays parked until those
two labels are checked by the DEV-162 human review.

## Field dispatch and class contracts (2026-11-13, DEV-179)

Record: [`reports/eval/2026-11-13-class-contracts.json`](../../reports/eval/2026-11-13-class-contracts.json),
binary built from `360fe07` plus this change, scored with the sets of
`360fe07`. The target was zero's audited KP02–KP05 (dev), not a held-out
result.

**What changed.**

* **Field dispatch.** `this.f.m()` and `this.#f.m()` dispatch to the class
  the field always holds. The field must be initialized with `new X()`, or
  assigned once with `new X()` at the top of the constructor, and must be
  `private`, `readonly` or `#private`. A public field, or a field assigned
  again elsewhere, is not followed.
* **`classContracts`** in the configuration declare what the analysis
  cannot see: the class a field holds (a constructor parameter property),
  and the functions whose awaited result is an instance of a class (a
  `Proxy` or RPC stub). See
  [configuration](../reference/configuration.md#class-contracts).
* **Instance receivers.** A binding that holds a declared or constructed
  instance is dispatched to its class, even when it is named like a
  database receiver (`db`).

Fixtures: [`fixtures/vulnerable/class_field_dispatch.ts`](../../fixtures/vulnerable/class_field_dispatch.ts)
and `fixtures/class_contracts/rpc_stub`, which is checked by
`crates/midwrite-analyzer/tests/class_contracts.rs` with and without the
contract.

**zero.** [`corpora/configs/zero.json`](../../corpora/configs/zero.json)
now declares the database Durable Object chain: `getZeroDB` → `DbRpcDO`,
whose `mainDo` field holds `ZeroDB`, which writes through `this.db`.

* KP02–KP05 are recovered, and zero recall goes from 4/10 to 8/10.
* Eight findings appear. A separate blind AI labeler, which saw only the
  source, labeled them 5 intended-risk and 3 false-positive. The
  intended-risk ones are best-effort cleanups and revocations, and a
  subscription that a cron re-enqueues. The false positives are a branch
  that cannot run after the delete, and two cases where a null default
  connection is a supported state.
* The labeler noted three borderline cases: `15b8fc69066c8199` could be
  true-positive, and `f5bd749ae025608e` / `a27a27f3a651d800` could be
  intended-risk.
* One intended-risk finding at `auth.ts:257` (`6372758b463e75d6`) is
  replaced by a pair at the same line.

The code alone changes no dev or held-out finding. Every difference comes
from the zero contract. The user chose to ship the contract despite the
lower dev precision. Unlike the parked field-client patch (DEV-178), it
recovers four audited windows.

| measure | before | code only | with the zero contract |
| --- | ---: | ---: | ---: |
| dev hazards | 426 | 426 | 433 |
| dev hazard precision | 402/418 (96.2%) | 402/418 | 406/425 (95.5%) |
| dev bug precision | 175/418 | 175/418 | 175/425 |
| dev recall | 153/169 | 153/169 | 157/169 |
| held-out hazard precision | 30/32 | 30/32 | 30/32, gate PASS |

## SDK clients in class fields and private methods (2026-11-13, DEV-178)

Record: [`reports/eval/2026-11-13-field-sdk-clients.json`](../../reports/eval/2026-11-13-field-sdk-clients.json),
binary built from `1996288` plus this change, scored with the sets of
`1996288`. The target was midday's audited KP02 (dev), a miss.

**What changed.**

* **SDK clients in fields.** A field that always holds an instance of a
  known SDK package's client class is that client. "Always holds" uses the
  rules of the DEV-179 field dispatch: `private client = new Resend(...)`
  set once in the constructor. So `this.client.emails.send(...)` is a
  built-in SDK effect.
* **Private methods.** `#name()` methods are recorded with their class.
  `this.#name()` dispatches to them; no subclass can override them.

Together with the DEV-179 field dispatch, this follows midday's
notification chain:

* `new Notifications(db).create(...)`;
* the private `#createInternal`;
* `this.#emailService.sendBulk(...)`;
* `EmailService`, whose `private client` is a Resend client.

Fixture: [`fixtures/vulnerable/sdk_client_field.ts`](../../fixtures/vulnerable/sdk_client_field.ts).

**midday.** KP02 is recovered. Eight findings appear: at each of four
callers, the email goes out before a database update. Each caller yields
two findings, one for Resend's per-payload `emails.send` branch and one
for its `batch.send` branch. A separate blind AI labeler labeled them:

* **4 true-positive.** The invoice email before `updateInvoice` in both
  branches. The invoice-paid and invoice-overdue owners' emails before the
  provider batch update.
* **1 intended-risk.** An upcoming-invoice reminder whose
  notify-then-mark order is explained in the source.
* **3 false-positive.** The per-payload branch for callers that never pass
  attachments. Deciding that branch needs array-content tracking
  (DEV-177).

The user chose to ship with these labels.

| measure | before | after |
| --- | ---: | ---: |
| dev hazards | 433 | 441 |
| dev hazard precision | 406/425 (95.5%) | 411/433 (94.9%) |
| dev bug precision | 175/425 | 179/433 |
| dev recall | 157/169 | 160/171 (the new true positives join the denominator) |
| held-out hazard precision | 30/32 | 30/32, gate PASS |

## Promise continuations (2026-11-13, DEV-176)

Record: [`reports/eval/2026-11-13-promise-continuations.json`](../../reports/eval/2026-11-13-promise-continuations.json),
binary built from `8b2697f` plus this change, scored with the sets of
`8b2697f`. This ships the patch that was parked in DEV-166, rebased onto
the analyzer changes made since.

**What changed.** The following callbacks now run in the caller's
sequence:

* `await p.then(cb)` runs `p`, then `cb`;
* `await p.catch(cb)` is `try { p } catch { cb }`;
* `await p.finally(cb)` is `try { p } finally { cb }`.

This holds when the call is awaited, returned from an `async` function or
awaited concurrently, and `cb` is an inline function. The callback is
lowered in place, so its captures (an id, a transaction handle) stay
known. Timer callbacks (`setTimeout`) are still deferred by design.

Fixture: [`fixtures/vulnerable/promise_continuations.ts`](../../fixtures/vulnerable/promise_continuations.ts).

**Why it ships now.** DEV-175 (returned-object properties) and DEV-180
(empty `RETURNING`) removed the infeasible pair at midday
`runtime.ts:194`. Three dev findings remain:

* **kan `card.ts:620`, true-positive.** A label link commits in its own
  transaction before the activity insert.
* **midday `runtime.ts:214`, intended-risk.** A best-effort `lastSeenAt`
  heartbeat before an inbox upload.
* **midday `customers.ts:102`, false-positive.** The new-customer path
  inserts a fresh row, so the tag delete cannot run. That is an analyzer
  gap in its own right, not one caused by the continuation.

Two new blind AI labelers labeled each finding independently. For the
two midday findings, which had an earlier blind label (both
false-positive), the label is the majority of the three. That rule was
fixed before the new labels were seen. This is AI adjudication, not the
DEV-162 human review.

| measure | before | after |
| --- | ---: | ---: |
| dev hazards | 441 | 444 |
| dev hazard precision | 411/433 (94.9%) | 413/436 (94.7%) |
| dev bug precision | 179/433 | 180/436 |
| dev recall | 160/171 | 161/172 |
| held-out hazard precision | 30/32 | 30/32, gate PASS |

## Blind AI re-review of the labels (2026-11-13, DEV-162)

The DEV-162 sample's 109 tier-1 items were re-reviewed by independent
blind AI agents in two passes. The scope of DEV-162 was changed by the
project owner: no human review. 23 labels changed where two reviewers
agreed against them, and 3 disagreements kept their labels. Procedure,
records and per-item changes:
[human review § Blind AI review](human-review.md#blind-ai-review-2026-11-13).
Record: [`reports/eval/2026-11-13-ai-review-rescore.json`](../../reports/eval/2026-11-13-ai-review-rescore.json).

| measure | before | after |
| --- | ---: | ---: |
| dev hazard precision | 413/436 (94.7%) | 421/443 (95.0%) |
| dev bug precision | 180/436 | 181/443 |
| held-out hazard precision | 30/32 | 28/30, gate PASS |
| held-out bug precision | 3/32 | 1/30 |
| held-out recall | 8/19 | 7/18 |

The held-out changes come from label corrections. No analyzer change is
involved, and the analyzer was not tuned. All labels remain AI-written.

## Constructor-set handle fields (2026-11-14, DEV-170)

The analyzer now follows a handle that the constructor stores in a field
(`constructor(private readonly db: Database)`, or a sealed field assigned
once from a constructor parameter). Each call site binds it to the handle
the receiver was constructed with.

Fixtures:

* `fixtures/vulnerable/constructor_handle_fields.ts`
* `fixtures/safe/constructor_transaction_field.ts`
* `fixtures/unsupported/constructor_field_mutations.ts`
* `fixtures/projects/constructor_handle/`

**Dev-set impact: none.** All sets give the same 476 findings, with the
same IDs, classifications and limitation counts as
[`2026-11-13-ai-review-rescore.json`](../../reports/eval/2026-11-13-ai-review-rescore.json):

| measure | result |
| --- | ---: |
| dev hazard precision | 421/443 |
| dev recall | 161/172 |
| held-out hazard precision | 28/30, gate PASS |

Two corpus classes have the pattern. Neither writes through the field:

* midday's `EmailService` (`constructor(private db: Database)`) only
  passes `this.db` to a read;
* zero's `Conversations` (`constructor(private readonly sql: Sql)`) issues
  SQL tags, not Drizzle builders.

**Found but not changed: unconfigured parameter handles.** A helper that
writes twice on an *unconfigured* parameter (`async function both(h) {
await h.update(…); await h.insert(…) }`), called with `db`, is reported
by neither side. The helper alone treats the parameter as unknown, and
the caller leaves a pair inside one call to the callee. Constructor-set
fields avoid this through `SummaryOrigin::bound_at`. Applying the same
rule to parameters would change dev findings, so it is a separate change.

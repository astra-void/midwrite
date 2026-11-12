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

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

Reserve: empty. All former held-out projects moved to dev before the
2026-11-12 coverage fixes below; there is currently no independent held-out
score. New independent projects must be audited and frozen before the
held-out gate can be measured again. Every project uses Drizzle + PostgreSQL. The first
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


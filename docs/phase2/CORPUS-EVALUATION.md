# Corpus evaluation

How CrashProof Phase 2 behaves on TypeScript applications that were not
written for it. Everything here is measured or manually reviewed as stated;
nothing was executed against the target applications, and no target source
file was modified.

## Corpus

| Project | Category | Origin | Revision | Files | Functions |
| --- | --- | --- | --- | --- | --- |
| saas-starter | Next.js + Drizzle + Stripe | github.com/nextjs/saas-starter | `6e33e58b1e553a41fe22e6b941a7229a002de361` | 41 | 156 |
| midday | large monorepo: API, workers, Drizzle, BullMQ/trigger.dev jobs, Resend, Stripe | github.com/midday-ai/midday | `51587319f26a0ffaa9dfccab1920373cb65689b7` | 2022 | 14827 |
| school-ledger | Next.js + Drizzle (no external effects) | local private checkout | `4b97f342` (2 locally modified files) | 462 | 6391 |
| unnamed-ai-project | Next.js + Drizzle + `fetch` | local private checkout | `39fcfc7c` (2 locally modified files) | 104 | 648 |

The two public projects are pinned in `corpora/manifest.json` and fetched by
`corpora/fetch.sh`; the two local ones (`corpora/manifest.local.json`) are
applications on the maintainer's machine and cannot be reproduced by others.
"Independent" means they were not written for CrashProof; school-ledger and
unnamed-ai-project belong to the same developer who requested this work.

Each project's configuration (`corpora/configs/`) was written from its
database and client modules before any result was looked at, and was not
changed afterwards. In particular no `exclude` patterns were added, so test
and seed files are analyzed.

## Procedure

```bash
corpora/fetch.sh
cargo run --release -p crashproof-corpus -- run \
  --manifest corpora/manifest.json --manifest corpora/manifest.local.json \
  --output reports/phase2/corpus-phase2.json                 # default policy
cargo run --release -p crashproof-corpus -- run ... --policy strict \
  --output reports/phase2/corpus-phase2-strict.json
cargo run --release -p crashproof-corpus -- precision \
  --report reports/phase2/corpus-phase2.json --labels corpora/labels
```

Two runs of the same command produce byte-identical reports (checked with
`cmp`; also with 1 and 8 threads).

## Measured statistics (Phase 2, default `recommended` policy)

| | saas-starter | midday | school-ledger | unnamed-ai-project |
| --- | --- | --- | --- | --- |
| Functions with recognized effects | 28 | 1030 | 526 | 41 |
| Same-file calls | 21 | 349 | 1560 | 23 |
| Cross-file calls resolved | 33 | 1262 | 3203 | 11 |
| Cross-file calls unresolved | 0 | 71 | 5 | 1 |
| Package calls (not followed) | 8 | 70 | 19 | 20 |
| Calls replaced by a summary | 40 | 1150 | 1310 | 23 |
| Functions truncated (`--max-paths 512`) | 0 | 10 | 12 | 0 |
| Paths pruned as contradictory | 0 | 112 | 759 | 0 |
| Paths merged after calls | 62 | 5771 | 34581 | 4 |
| CP1001 / CP1002 / CP1003 | 1 / 0 / 0 | 40 / 9 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| CP1004 / CP1005 | 11 / 3 | 116 / 15 | 123 / 0 | 8 / 0 |
| CP9001 | 6 | 89 | 194 | 9 |
| Warnings (fail CI at `--fail-on warning`) | 1 | 49 | 0 | 0 |

"Resolved" means the import was linked to a function declaration; whether
its summary was used is the "replaced by a summary" row (a callee without a
recognized operation, for example, is linked but not used).

Where the findings are (all of school-ledger's CP1004 are in tests and seed
scripts; its application code uses transactions consistently and has no
configured external effect):

| Project | Application code | Tests, seeds and scripts |
| --- | --- | --- |
| saas-starter | CP1004 ×9, CP9001 ×6 | CP1001 ×1, CP1004 ×2, CP1005 ×3 |
| midday | CP1001 ×40, CP1002 ×9, CP1004 ×105, CP1005 ×15, CP9001 ×87 | CP1004 ×11, CP9001 ×2 |
| school-ledger | CP9001 ×5 | CP1004 ×123, CP9001 ×189 |
| unnamed-ai-project | CP1004 ×8, CP9001 ×9 | — |

(Classified by path: `test`, `tests`, `__tests__`, `e2e`, `.test.`, `.spec.`,
`scripts/`, `seed`.)

## Phase 1 versus Phase 2

Same projects and configurations; Phase 1 is the saved binary
(`.corpora/bin/crashproof-phase1`), whose semantics correspond to
`--policy strict`. Matching is by (file, line, code)
(`scripts/compare-phase1.py`, output in
`reports/phase2/phase1-vs-phase2-strict.txt`).

| Project | Phase 1 diagnostics | Phase 2 strict | Phase 1 warnings | Phase 2 strict warnings | Phase 2 default warnings |
| --- | --- | --- | --- | --- | --- |
| saas-starter | 25 | 23 | 18 | 15 | 1 |
| midday | 164 | 344 | 84 | 180 | 49 |
| school-ledger | 60 | 880 | 10 | 123 | 0 |
| unnamed-ai-project | 19 | 21 | 5 | 8 | 0 |

What disappeared, and why (every Phase 1 diagnostic missing in Phase 2 was
inspected):

* **17 CP1004 merged** (3 in saas-starter, 14 in midday): the same write was
  reported once per preceding commit (four times at saas-starter
  `app/(login)/actions.ts:210`); Phase 2 reports it once and lists the
  alternatives.
* **2 CP1004 removed as false positives** (midday
  `packages/db/src/queries/teams.ts:280`): `createSystemCategoriesForTeam(tx,
  ...)` is called inside `db.transaction` with the transaction handle (the
  source comments say "atomic"), but Phase 1 analyzed the helper's `db`
  parameter as the root handle and reported a write bypassing the
  transaction. Phase 2 binds the parameter to `tx`.
* **2 CP1005 became CP1001**: an unknown `updateInvoice(...)` call is now
  resolved, so the pair is a commit and an effect, not two effects.
* **42 CP9001 resolved** (3 of them replaced by a CP1004 at the same line):
  unknown calls that are now summarized, for example school-ledger helpers
  called with `tx` inside transactions (`lockActingAdmin(tx, actor)` and
  similar, 17 in `src/`), and midday query functions imported from the
  `@midday/db` workspace package.

What appeared: midday's cross-resource findings (CP1001 ×40, CP1002 ×9)
exist only because calls into `@midday/db/queries` (a workspace package) and
other modules are now followed; Phase 1 reported no CP1001/CP1002 there.
school-ledger's CP1004 and CP9001 growth is almost entirely in integration
tests, which call several services that each commit their own transaction.

## Manual review

Labels are in `corpora/labels/<project>.json`, one entry per reviewed
finding, with a classification and a note. The reviewer was the Phase 2
implementer (an AI agent), reading the target source code; no maintainer of
these projects confirmed anything, and no finding was reproduced at runtime.
"True positive" therefore means "a potential hazard that the source code does
not appear to handle", not a confirmed bug.

Selection:

* every CP1001 and CP1002 of every project (51);
* every CP1004 and CP1005 of saas-starter and unnamed-ai-project (22);
* a deterministic sample of midday CP1004 (every 8th finding in report
  order, 15) and CP1005 (every 3rd, 5);
* a deterministic sample of school-ledger CP1004 (every 12th, 11).

CP9001 is a limitation, not a claim, and was not labeled.

Classifications: `true-positive` (the window exists and nothing in the code
handles it), `intended-risk` (the window exists but the code accepts it on
purpose: best-effort notifications in `try`/`catch` or fire-and-forget,
operations inside worker jobs that are retried, orderings explained by a
source comment, seed and test code), `false-positive` (the claimed partial
state is not real), `unclear` (could not be decided from the source).

Results (`reports/phase2/precision.txt`), with 95% Wilson intervals; "hazard
precision" counts true positives and intended risks as correct (the reported
window exists), "bug precision" counts only true positives:

| Code | Reviewed | TP | Intended | FP | Unclear | Hazard precision | Bug precision |
| --- | --- | --- | --- | --- | --- | --- | --- |
| CP1001 | 41 | 13 | 24 | 1 | 3 | 0.97 (0.87–1.00) | 0.34 (0.21–0.50) |
| CP1002 | 9 | 2 | 4 | 2 | 1 | 0.75 (0.41–0.93) | 0.25 (0.07–0.59) |
| CP1004 | 45 | 12 | 30 | 1 | 2 | 0.98 (0.88–1.00) | 0.28 (0.17–0.43) |
| CP1005 | 8 | 0 | 7 | 1 | 0 | 0.88 (0.53–0.98) | 0.00 (0.00–0.32) |
| All | 103 | 27 | 65 | 5 | 6 | 0.95 | 0.28 |

Per project and code, see `reports/phase2/precision.txt`. Sample sizes are
small; the intervals are wide and the CP1004/CP1005 numbers for midday and
school-ledger come from samples, not from every finding.

No recall is claimed: there is no labeled set of known hazards in these
projects.

### What the true positives look like

All of these were found through cross-module summaries (the database write
is inside an imported query function):

* midday: an invoice is set to `unpaid` (meaning sent) by `updateInvoice(...)`
  before the `generate-invoice` job is enqueued, in four entry points; a crash
  in between leaves an invoice marked sent that is never generated or
  emailed.
* midday, `apps/worker/src/processors/invoices/schedule-invoice.ts:100`: the
  scheduled job sets the status to `unpaid` before queueing generation; the
  job's own guard (`if (invoice.status !== "scheduled") return`) makes a
  retry after a crash skip the invoice for good.
* midday, `apps/api/src/trpc/routers/bank-connections.ts:70`: the bank
  connection row is deleted before the job that revokes the provider
  connection is triggered; after a crash the data needed to revoke it is
  gone.
* midday: status fields set to `pending` before the job that would move them
  on is enqueued (customer enrichment, document reprocessing): a crash leaves
  them stuck.
* saas-starter sign-up: user, team and membership are inserted without a
  transaction; a failure after the user insert blocks a retry ("user exists").
* unnamed-ai-project: a conversation is inserted before its system and
  greeting messages; deletion removes messages before the conversation.

### What the false positives look like

* Two adjacent operations that are unrelated: an "uploaded" notification
  followed by a processing update; a write in one loop and an enqueue for
  other records in a later loop (3 findings).
* A pair whose second write cannot run because of database state
  (`getOrCreate` after the row was just created) (1).
* An external effect followed by an unrelated bookkeeping update (1).

### What the intended risks look like

Most non-bug findings (65 intended risks) are real windows the code
accepts. Approximate breakdown from the review notes: notifications and
emails in `try`/`catch` or explicitly fire-and-forget (about 20), test or
seed code (18), steps inside BullMQ worker jobs that are retried as a whole
(about 10), leftover states that are harmless or completed by retrying the
request (about 10), and orderings explained in comments (about 6; "Trigger
cleanup job BEFORE deleting team", "Queue jobs AFTER transaction commits ...
can be manually processed via the dashboard").

## Unsupported patterns seen

* **Handles from calls** (`getDb().transaction(...)`, `getDb().insert(...)`):
  school-ledger obtains its database this way everywhere. Writes are only
  seen where a recognized handle (`tx`, a `db` parameter) is used. (Addressed
  by configurable call paths in the follow-up; see `FOLLOW-UP.md`.)
* **Workspace packages and aliases** were the dominant obstacle before
  workspace-package resolution was added: 609 of midday's calls went to
  `@midday/db/...`. After the change, 70 package calls remain (third-party
  packages and workspace packages whose entry points are not analyzed).
* **Unresolved imports** (midday 71): mostly exports that are not function
  declarations (`export const x = wrap(...)`), class methods behind
  imports, and modules that resolve only through features outside the
  subset.
* **Methods** (`this.processor.process()`, service objects) and callbacks
  (`items.map(async ...)`, `Promise.allSettled(batch.map(...))`) are not
  followed.
* **Truncation**: 22 functions across the corpus exceed 512 paths (mostly
  integration tests and large worker processors); their findings are
  partial.

## Corrections made to CrashProof during the evaluation

All were changes to CrashProof, never to the targets, and each is covered by
tests:

1. Workspace-package resolution (`package.json` `name` and `exports`), after
   seeing 609 unresolved `@midday/db` calls.
2. Reads no longer fork paths or appear in traces, and traces leaving an
   inlined call are merged when equivalent: truncated functions in
   school-ledger dropped from 129 to 12 once cross-module summaries made test
   functions large.
3. Unknown calls inside one inlined call are reported once per call, and
   (default policy) once per function: school-ledger CP9001 went from 770 to
   194.
4. Diagnostics that share their primary operation are merged (same callee
   operation reached on different paths).
5. A `fireAndForget` transaction no longer yields `high` confidence.

The configurations were not tuned to improve the numbers.

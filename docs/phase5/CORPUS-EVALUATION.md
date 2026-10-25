# Corpus evaluation

Pinned corpus and configurations unchanged (saas-starter `6e33e58b`, midday
`51587319`, school-ledger `4b97f34`, unnamed-ai-project `39fcfc7`). Phase 4
binary vs Phase 5 release binary, same machine, same session. Raw data:
`reports/phase5/corpus/` (reports, `compare` input, `attribution.txt`,
`trace-transitions.txt`, `trace-coverage-*.txt`, `relations-vs-labels.txt`).
Private projects: counts only; per-call details stay under `.corpora/private/`
(git-ignored).

## 1. Diagnostics

| Project | Phase 4 | Phase 5 | Warnings | Added / removed |
| --- | --- | --- | --- | --- |
| saas-starter | 21 (CP1001 1, CP1004 11, CP1005 3, CP9001 6) | 23 (1, 14, 3, 5) | 1 → 1 | +5 / −3 |
| midday | 269 (CP1001 40, CP1002 9, CP1004 116, CP1005 15, CP9001 89) | 282 (43, 9, 122, 18, 90) | 49 → 52 | +29 / −16 |
| school-ledger | 317 (CP1004 123, CP9001 194) | 332 (131, 201) | 0 → 0 | +17 / −2 |
| unnamed-ai-project | 17 | 17 | 0 → 0 | 0 |

`--policy strict` and the evidence-cited midday contracts variant were
re-run (`corpus-phase5-strict.json`, `corpus-phase5-midday-contracts.json`);
the Phase 4 baselines of both reproduced byte-identically first. No
Phase 4 CP1001–CP1005 finding disappeared anywhere: all 21 removed findings
are CP9001 (unknown calls that were resolved or moved).

## 2. Every changed diagnostic

Mechanisms come from `scripts/phase5-attribute.py` (IR diff of the
function) and were then checked by reading the IR and the source.

### saas-starter (+5 / −3)

`logActivity(...)` is a `Promise.all` element in four server actions.
Phase 4 never summarized concurrent calls, so it was an unknown call
(CP9001 at `actions.ts:211, 282, 354` — removed). Summarized, its
`db.insert(activityLogs)` is visible: **CP1004 info** at the same three
lines (concurrent, low confidence), and two CP9001 move to the next unknown
calls after that insert (`setSession(createdUser)` at 212,
`cookies()` via `createCheckoutSession` at 97).

### midday (+29 / −16)

| Change | Finding(s) | Mechanism and explanation |
| --- | --- | --- |
| resolved | − CP9001 `webhooks/polar/index.ts:193`; − `inbox-matching.ts:168`; − `jobs/.../match-transactions-bidirectional.ts:104`; − `jobs/.../batch-process-matching.ts:42` | concurrent calls `getTeamById`, `getTeamOwnerContact`, `getTeamCalibration`, `getInboxById`, `getTransactionById` (read-only queries in `Promise.all`) are summarized; their traces became complete (§3) |
| moved | − CP9001 `worker/.../match-transactions-bidirectional.ts:115`, − `worker/.../batch-process-matching.ts:55` → + same lines | `triggerMatchingNotification` is now inlined past its resolved reads; the unknown call is its `notifications.create(…)` (third-party) |
| moved | − `process-document.ts:75` → + `:76` | `withTimeout` proven inert; its awaited argument `supabase.storage.from("vault").download(fileName)` is the unknown (argument placeholder) |
| moved | − CP9001 `Promise.all(…)` at `documents.ts:120`, `inbox.ts:106`, `invoice-recurring.ts:559, 629`, `transactions.ts:1817, 1914`, `update-account-base-currency.ts:93` → + per-element callback at `121`/`109`/`560`/`630`/`1818`/`1915`/`93` | `Promise.all(xs.map(cb))` is a loop over `cb`; where `cb` is not summarizable (it calls unknown code, or uses a captured transaction handle) the unknown is now the callback |
| new hazard | + **CP1001** `documents.ts:121` | `updateDocuments` commits, then `Promise.all(supportedDocuments.map(item => triggerJob("process-document", …)))` (array callback); Phase 4 only saw an unknown `Promise.all` |
| new hazard | + **CP1001** `team.ts:358` | `createTeamInvites` inserts invites inside `Promise.all(validInvites.map(async …))` — invisible in Phase 4 — and the caller then runs `tasks.trigger("invite-team-members", …)` |
| new hazard | + **CP1005** `webhooks/inbox/utils.ts:168`, `trpc/routers/inbox.ts:167`, `sync-scheduler.ts:254` | unawaited `xs.map(x => triggerJob("process-attachment", …))` (fire-and-forget callback loop) followed by `triggerJob("notification", …)` |
| new hazard (info) | + CP1004 `export-transactions.ts:281, 425`, `sync-attachments.ts:292` | `this.initializeProvider(…)` resolved by method dispatch; its `db.update(apps)` commits independently of later writes |
| moved | − CP9001 `export-transactions.ts:443` → + `:176`; − `sync-attachments.ts:320` → + `:198` | same dispatch: an earlier commit now precedes earlier unknown calls; `recommended` reports one CP9001 per function (grouped), located at the first |
| new unknown | + CP9001 `user.ts:79` | `deleteUser(db, …)` in `Promise.all` summarized: its `db.delete(teams)` precedes `supabaseAdmin.auth.admin.deleteUser(...)` |
| new unknown | + CP9001 `embeddings.ts:190`, `utils.ts:157`, `update-base-currency.ts:68` | effects in now-modeled array callbacks precede an unknown (`Promise.allSettled(promises)` over a non-literal collection, `Promise.all(jobPromises)`, `this.updateProgress`) |
| partial coverage | + CP1001 `jobs/.../match-transactions-bidirectional.ts:218`, + 3 × CP1004 `:210` | `run` exceeds `--max-paths` (512) in both versions; Phase 5 merges more equivalent traces (resolved concurrent calls), so the analyzed prefix reaches lines 210–218. Not a new fact: a different part of a truncated function |
| truncation | + CP9001 `batch-process-matching.ts:16` (worker and jobs) | more summarized calls push these functions over 512 paths (reported, partial) |

Severity: the three new CP1001 and three new CP1005 are warnings/infos as
before under `recommended` (warnings 49 → 52). None was reviewed by a
human; they join the review queue.

### school-ledger (private, +17 / −2)

By mechanism (a finding can show several): array callback 8 CP1004 + 9
CP9001, inert calls, concurrent calls, argument placeholders 7 + 7, return
facts; 4 CP9001 for path truncation; 2 CP9001 removed (resolved). All
additions are in test-support and integration-test files that call
provisioning helpers in `Promise.all`/`map` callbacks. Details are kept
locally.

## 3. Incomplete traces

Matched by finding fingerprint (`trace-transitions.txt`):

| Project | Phase 4 incomplete | → complete | → still incomplete | New traces (new findings) |
| --- | --- | --- | --- | --- |
| midday | 47 | **6** | 41 | 12 (2 complete, 7 approximated, 3 incomplete) |
| school-ledger | 93 | 0 | 93 | 8 (all incomplete) |
| saas-starter | 0 | — | — | 3 approximated |
| unnamed-ai-project | 0 | — | — | 0 |

Totals: incomplete traces 140 → 145, complete 178 → 186, approximated 8 → 18
(new concurrent windows are approximations by definition). The definition
of *complete* is unchanged.

The six midday traces that became complete (all through concurrent-call
summaries of read-only queries, which removed the only unknown steps from
their windows): CP1001 `polar/index.ts:198`; 3 × CP1004 `inbox-matching.ts:177`;
CP1001 `jobs/.../batch-process-matching.ts:42`; CP1001
`jobs/.../match-transactions-bidirectional.ts:104`.

Of the 41 still-incomplete midday traces, 14 now name a different, more
precise unknown: the wrapper `withTimeout(…)` replaced by the SDK call it
awaits (`supabase.storage.from("vault").download(fileName)`,
`…createSignedUrl(…)`), `getTeamCalibration(…)` replaced by the raw SQL it
reaches (`tx.execute(…)`), the second IIFE in `process-attachment.ts`
resolved (the first remains: it awaits a Supabase call through `withTimeout`).

**Unknown calls in windows by category (midday, occurrences):**

| Category | Phase 4 | Phase 5 |
| --- | --- | --- |
| resolved function without recognized effects (not inert) | 24 | 30 |
| method call on a value (mostly SDK clients) | 21 | 26 |
| third-party package | 1 | 10 |
| call through an expression (IIFE) | 10 | 5 |
| plain call of an unresolved binding (concurrent elements) | 13 | 1 |
| method call on `this` | 5 | 0 (now "resolved function…": `BaseProcessor.updateProgress` calls BullMQ) |
| resolved function beyond the depth bound | 7 | 5 |
| raw SQL | 0 | 4 |

Phase 5 moved uncertainty to the operations that are actually unknown
(SDK and package calls, raw SQL); it did not mark them known. Making them
known requires configuration (`externalEffects`) — evidence-backed SDK
recognition was not added.

### Why school-ledger did not improve

Its unknown steps are dominated by effectful helpers reached more than
`--max-call-depth` (3) calls deep (155 occurrences: audit-record helpers)
and test-support helpers with unknown calls (162). Neither is a missing
technique: the helpers perform writes, so inertness cannot resolve them.
Measured with `commitrift trace --max-call-depth N`:

| depth | school-ledger findings | traces (complete/approx./incomplete) | time (1 thread) |
| --- | --- | --- | --- |
| 3 (default) | 332 | 131 (30/0/101) | 0.37 s |
| 4 | 544 | 228 (43/13/172) | 0.47 s |
| 6 | 593 | 251 (60/14/177) | 0.56 s |

Raising the bound is cheap but changes the finding set by +79 %; midday
changes by +3 findings at depth ≥ 4. The default stays 3 until such a change
can be reviewed.

## 4. Relationships

Value relations per project (findings with two operations): saas-starter
6 provenShared / 5 possiblyRelated / 4 noRelationEstablished / 3 unknown;
midday 15 / 17 / 21 / 139; school-ledger 1 / 1 / 59 / 70; unnamed 3 / 4 /
0 / 1. The status does not separate reviewed true positives from intended
risks or false positives ([`EFFECT-RELATIONSHIPS.md`](EFFECT-RELATIONSHIPS.md)),
so it is descriptive only.

## 5. Review

No independent human review happened in Phase 5 (Phase 4 prepared 12 midday
packets; still 0 reviews). No automated reviewer was used as a substitute.
The new warnings (3 × CP1001, 3 × CP1005 in midday) and the changed CP1004
infos should be added to the next review round; their review packets can be
generated with `commitrift-corpus review-packet --finding <id>`.
No external repository was executed.

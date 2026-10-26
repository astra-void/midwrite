# Corpus evaluation (Phase 8)

## Static analysis: no change in any output

The pinned corpus — saas-starter (rev 6e33e58), midday, and the two local
projects school-ledger and unnamed-ai-project — was analyzed with the
Phase 7 and the Phase 8 binaries: `commitrift check` in json, sarif and
text, and `commitrift trace` in json, at call depths 3, 6 and 12, in the
default (shared) and the compact and legacy path modes. **All 144 outputs
are byte-identical** (`reports/phase8/corpus-identity.txt`). Therefore,
compared with Phase 7:

| | change |
| --- | --- |
| CP1001–CP9001, CP7001–CP7103 | none |
| finding fingerprints, severity, confidence | none |
| trace completeness, unknown operations | none |
| path truncation, pruned/merged paths | none |
| function summaries, `callsSummarized` | none (kept exact by the counter replay) |
| call-depth behavior | none; the default depth stays 3 |

Findings by depth are the Phase 7 numbers (school-ledger 332 at depth 3,
544 at depth 4, 593 from depth 6 on; midday 282 → 285; saas-starter 23;
unnamed-ai-project 17). Phase 8 made deeper analysis cheaper, not
different; deeper analysis still needs a reviewed policy decision.

## Model-checking corpus

The 14 Phase 6 scenarios and the 3 Phase 7 scenarios (including the
**original** saas-starter `inviteTeamMember` extraction): identical reports
(`reports/phase8/extraction-identity.txt`).

saas-starter: its Drizzle schema is now read statically
(`commitrift schema .corpora/saas-starter --file lib/db/schema.ts`): it
confirms there is no unique constraint on `invitations(team_id, email)`,
the missing constraint behind the Phase 6/7 finding. Its migrations and its
code were **not executed** (not authorized); the finding remains a
model counterexample confirmed only for model SQL on PostgreSQL. No
upstream maintainer was contacted.

## First-party recovery findings

The static analyzer on `fixtures/phase8/shop-app` (with its
`commitrift.json` naming the four external seams) reports CP1001/CP1002 in
`payOrder`, `payOrderIdempotent`, `shipOrderDualWrite`, `relayOutbox`,
`sendReceipt`, `sendReceiptOnce`, `reserveStock`, and CP1004 informational
findings. It flags the failure **windows** — identically for the vulnerable
and the idempotent checkout, because idempotency keys and provider
contracts are outside its view. The crash/recovery runs decide what the
windows mean under declared recovery semantics: double charge vs one charge;
lost vs at-least-once event; duplicate vs at-most-once receipt
([`ORIGINAL-CODE-RECOVERY.md`](ORIGINAL-CODE-RECOVERY.md)). This is
evidence about the fixtures, which were written to contain these patterns —
not about external applications.

## Human review

There is still **no independent human review** of any finding. The
Phase 4 blind-review workflow was used to prepare packets for the
first-party recovery findings: `commitrift-corpus review-packet
--manifest corpora/manifest.phase8-first-party.json --project
phase8-shop-app --sample 20` wrote 10 blind packets and an empty form to
`reports/phase8/review-packets/` (diagnostic, failure sequence and
source — no label or verdict). They are unreviewed. No AI judgment is
presented as a review.

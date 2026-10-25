# Corpus evaluation

Projects: saas-starter (6e33e58), midday (5158731), school-ledger (4b97f34,
private), unnamed-ai-project (39fcfc7, private) — the Phase 2–6 corpus.

## Ordinary analysis is unchanged

`scripts/phase7-identity.sh` runs the Phase 6 binary and the Phase 7
binary on each project: `commitrift check` in json, sarif and text, and
`commitrift trace` (json), at depths 3 and 6, with compact summaries (the
new default) and with `COMMITRIFT_PATHS=legacy`.
**64 of 64 outputs byte-identical** (`reports/phase7/corpus-identity.txt`).
Diagnostics, fingerprints, traces, unknown calls, truncation and the
`prunedPaths`/`mergedPaths` statistics are therefore unchanged; CP1001–CP9001
meanings are untouched. Baseline formats, replay manifests and Phase 6
model-check scenarios load unchanged.

Phase 6 model-check scenarios, Phase 7 vs Phase 6 binary
(`reports/phase7/model-check-phase6-identity.txt`, with
`--validate-postgres`): verdicts, states, transitions, counterexamples and
PostgreSQL validation identical for all 16; one report differs only in the
`visitedBytes` memory estimate (21 006 → 21 238 bytes, `inventory-row-lock`)
because the thread state now carries a (empty) `try`-handler stack.

## Compact summaries on the corpus

Legacy vs compact path enumeration, every function analysis compared event
by event: **22 022 analyses at each of depths 3, 6 and 8, 0 differences**
([`COMPACT-SUMMARIES.md`](COMPACT-SUMMARIES.md)). Findings by depth
(identical in both modes): midday 282 / 285 / 285 / 285 / 285 and
school-ledger 332 / 544 / 594 / 593 / 593 at depths 3 / 4 / 5 / 6 / 8;
saas-starter 23 and unnamed-ai-project 17 at every depth. The extra
school-ledger findings at depth ≥ 4 are the Phase 5/6 observation and are
**not reviewed**; the default depth stays 3.

## Real-world source fidelity: saas-starter `inviteTeamMember`

Phase 6 needed an *adapted isolated reproduction* (body moved out of the
wrapper, membership JOIN and activity log removed, explicit ids). Phase 7
extracts the **original** function
(`fixtures/phase7/scenarios/saas-starter-original-invite.json`, selector
`app/(login)/actions.ts#inviteTeamMember/arg[1]`):

| | Phase 6 | Phase 7 |
| --- | --- | --- |
| entry | rewritten named function | the original callback at `app/(login)/actions.ts@401:3` (`<anonymous@401:3>` in `check`) |
| wrapper | removed | `validatedActionWithUser` resolved through `@/lib/auth/middleware` (tsconfig paths), `returned-function-returns-promise`, guards `await getUser()`, `if (!user) throw`, `if (!result.success) return` |
| `getUserWithTeam` (LEFT JOIN + projection, `result[0]`) | not supported | extracted (`lib/db/queries.ts`) |
| membership check (LEFT JOIN) | omitted | extracted |
| invitation id | explicit | generated (serial) |
| `logActivity` (const object, enum member, `ipAddress \|\| ''`) | omitted | extracted |
| unsupported operations | — | 0 (5 represented, 4 assumed) |
| counterexample | yes (adapted code) | yes (original code), 10 transitions |
| PostgreSQL | model SQL confirmed | model SQL confirmed (evidence level `postgres-confirmed-schedule`) |
| original code executed | no | **no** — not authorized; saas-starter uses `postgres-js`, for which no coordinator adapter exists |

Declared assumptions: the callback is awaited, the session belongs to
user 1, validation succeeded, two submits run concurrently at READ
COMMITTED, initial rows as declared, timestamps not modeled.

**Status of the finding: unverified at the original-code level.** The
same structural pattern *was* reproduced end to end in the first-party
application `fixtures/phase7/team-app` (wrapped Server Action, same
queries, real Drizzle, PostgreSQL): see
[`DIFFERENTIAL-VERIFICATION.md`](DIFFERENTIAL-VERIFICATION.md). That is a
first-party reproduction of the pattern, **not** evidence about saas-starter's
runtime behavior. No human review and no maintainer contact took place.

Evidence packet (source locations, assumptions, actor model,
counterexample, PostgreSQL observations, invariant, uncertainties):
`reports/phase7/model-check/saas-starter-original-invite.{json,txt}` and
this section.

# Phase 6 corpus evaluation

## 1. Regression of ordinary static analysis

The default analysis policy was not changed. The same four projects as in
Phase 5 (saas-starter `6e33e58`, midday `5158731`, school-ledger
`4b97f34`, unnamed-ai-project `39fcfc7`), with the Phase 5 binary
(preserved before Phase 6) and the Phase 6 binary:

| Project | `check` JSON report | `trace` JSON report | Diagnostics | Traces: complete / approximated / incomplete |
| --- | --- | --- | --- | --- |
| saas-starter | byte-identical (90 294 B) | byte-identical | 23 | 14 / 4 / 0 |
| midday | byte-identical (1 384 280 B) | byte-identical | 282 | 134 / 14 / 44 |
| school-ledger | byte-identical (2 359 249 B) | byte-identical | 332 | 30 / 0 / 101 |
| unnamed-ai-project | byte-identical (57 989 B) | byte-identical | 17 | 8 / 0 / 0 |

Diagnostic counts, fingerprints, confidence, unknown calls, complete and
incomplete traces, and path truncation are therefore unchanged (they are
all in these reports). Raw data: `reports/phase6/perf/bench-phase{5,6}-r*.*.json`,
`reports/phase6/corpus-trace-identity.txt`. Wall time and RSS:
[`PERFORMANCE.md`](PERFORMANCE.md).

## 2. Candidate concurrency scenarios

Model checking is not run over a corpus automatically: a scenario must name
the actors, their arguments and an invariant. Candidates were looked for by
reading source for operations that two requests can plausibly run at the
same time on the same data (check-then-act, read-modify-write, claim of a
shared resource). Possible concurrency is **not** evidence that it happens
in production; every scenario states its actor assumptions.

### saas-starter: duplicate pending invitation (check-then-act)

`app/(login)/actions.ts`, `inviteTeamMember` (rev `6e33e58`): looks for a
pending invitation for `(email, teamId)`, and inserts one if there is none.
`lib/db/schema.ts` declares no unique constraint on those columns.

1. **Extraction of the original**: not possible. The action is an
   anonymous callback passed to `validatedActionWithUser(schema, async
   (data, _, user) => …)`; the extractor lowers named functions only, and
   guessing what an arbitrary wrapper does (validation, authentication)
   would be a heuristic. Reported as `function … was not found`.
2. **Adapted isolated reproduction**
   (`fixtures/concurrency/corpus/saas-starter-invite/src/invite.ts`, every
   change listed in its header): the check and the insert copied as
   written; the wrapper replaced by explicit parameters; the membership
   check (a join) and activity log omitted; an explicit `id` instead of the
   `serial` default. Supporting this exposed an unsupported idiom that the
   extractor now lowers exactly: `const rows = await db.select()…limit(1)`
   followed by `rows.length` (only `length` is modeled; without `.limit(1)`
   it stays unknown).
3. **Model**: two concurrent submissions by the same owner (double submit),
   READ COMMITTED. `COUNTEREXAMPLE_FOUND` in 4 transitions: both
   existence checks see no row, both insert.
4. **PostgreSQL**: `postgres-confirmed-schedule` — all 4 statement outcomes
   matched and both pending invitations are committed; the invariant reads
   only committed rows.
5. **Mitigation, hypothetical**: with a declared `UNIQUE(team_id, email)`
   (not in saas-starter's schema) the second insert fails with 23505:
   `NO_COUNTEREXAMPLE_WITHIN_BOUNDS`.

Evidence level reached: **2. adapted isolated reproduction** (first-party
re-write of the operation, confirmed on PostgreSQL). Not reached: 3.
reproduction in the original application (the original was not executed —
CommitRift never executes third-party repositories), 4. maintainer
confirmation. Whether two invitations for the same email are harmful in
saas-starter (for example, whether sign-up uses the first or every
invitation) has not been assessed.

### Other projects (not searched)

midday, school-ledger and unnamed-ai-project were **not** searched for
candidates in Phase 6. What is known from Phase 5 measurements only: most
unknown operations in midday's failure windows are Supabase, Slack,
Trigger.dev and BullMQ SDK calls, and school-ledger's effectful helpers
often lie beyond the call-depth bound. Operations of those kinds are
outside the extractor's subset and would make a scenario `UNSUPPORTED`;
whether particular functions fall inside the subset has not been assessed.

## 3. Review status

No new concurrency finding has an independent human review; the Phase 2–5
review infrastructure (`commitrift-corpus review-packet` /
`review-check`) is unchanged. Needs independent assessment:

* the saas-starter duplicate-invitation scenario (whether concurrent double
  submission occurs and whether duplicates matter in that application).

No external adoption, bug report or maintainer confirmation is claimed.

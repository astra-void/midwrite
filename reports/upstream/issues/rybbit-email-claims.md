A process restart between the durable claim and the provider call can mark an email as already sent even though it was never submitted to Resend. The catch/rollback handles rejected calls, but cannot run if the process dies.

Reviewed revision: `4288204761ed2fc801fcaa0b3bbfacd1b8cc3936`.

Source:
- [`sendBundle`](https://github.com/rybbit-io/rybbit/blob/4288204761ed2fc801fcaa0b3bbfacd1b8cc3936/server/src/services/lifecycleEmails/lifecycleEmailService.ts#L282-L324) commits `lifecycleEmailLog` rows before `build(claimed)` and `sendLifecycleEmail`. An existing key is treated as already sent.
- [`TrialGraceEmails.sendDue`](https://github.com/rybbit-io/rybbit/blob/4288204761ed2fc801fcaa0b3bbfacd1b8cc3936/server/src/services/trialGrace/trialGraceEmails.ts#L95-L160) has the same ordering: an existing owner/stage key is filtered out, and the claim commits before `sendTrialGraceEmail`.
- The [log schema](https://github.com/rybbit-io/rybbit/blob/4288204761ed2fc801fcaa0b3bbfacd1b8cc3936/server/src/db/postgres/schema.ts#L915-L930) has a unique `(userId, emailKey)` and no pending/delivered state or claim expiry.

Failure sequence to reproduce in a disposable development instance:
1. Make one lifecycle key or trial-grace owner/stage eligible with no existing log row.
2. Pause immediately after the `.insert(lifecycleEmailLog)...returning()` resolves and before the send is invoked, then terminate the process without graceful cleanup.
3. Restart and let the cron process the same eligible item.
4. The log row survives. Lifecycle processing drops the claimed key / returns when nothing is inserted; trial-grace processing excludes that owner/stage. Resend was never called for it.

Expected: a claimed-but-unsent item remains recoverable after restart. Actual source path: the durable claim is indistinguishable from a delivered email. This suppresses that lifecycle key or trial-grace stage; a later stage being eligible does not deliver the missed stage.

A durable pending/delivered record with a lease/reclaim path (or an outbox relay) would preserve recovery while retaining the stable Resend idempotency key. Moving the insert after the send alone reintroduces concurrent duplicate sends. Resend idempotency cannot recover a request that was never made.


The same pattern also appears in [usage-limit notifications](https://github.com/rybbit-io/rybbit/blob/4288204761ed2fc801fcaa0b3bbfacd1b8cc3936/server/src/services/usageService.ts#L438-L520): `overMonthlyLimit` and `approachingLimitNotifiedPeriodStart` are persisted before delivery. A normal rejected send is caught and logged without restoring the flags. The next run sees `wasOverLimit` / `alreadyNotifiedApproaching` and skips that notification (for approaching-limit, for the remainder of the same month). A one-shot rejected email is enough to exercise this path; it does not require a process crash. This is the same durable-marker-before-delivery problem, with no pending delivery state.

Found with [Midwrite](https://github.com/astra-void/midwrite), an experimental static analyzer for partial-failure windows in TypeScript backends. I checked the reported operations and recovery path against the source. This is an AI-assisted source review, not a production observation or an executed fault-injection test; the steps above describe the proposed reproduction.

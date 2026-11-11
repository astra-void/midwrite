For a new-workspace `checkout.session.completed`, workspace creation and subscription creation commit separately. If the subscription insert fails after the workspace has committed, redelivery finds the workspace and skips both operations, acknowledging the event without repairing the missing subscription.

Reviewed revision: `1cc2f332131d736c5e262654bd279e3beb95c93d`; the webhook file was also checked at current HEAD `84afd92f9a07248c3eff43de23475d575f550a2e` and is unchanged.

Source:
- [Webhook existence check and both creates](https://github.com/kanbn/kan/blob/84afd92f9a07248c3eff43de23475d575f550a2e/apps/web/src/pages/api/stripe/webhook.ts#L64-L95).
- [`workspaceRepo.create`](https://github.com/kanbn/kan/blob/1cc2f332131d736c5e262654bd279e3beb95c93d/packages/db/src/repository/workspace.repo.ts#L64-L133) writes the workspace, system roles, and owner membership using the passed root database handle. The webhook does not wrap this and `subscriptionRepo.create` in one transaction.

Failure sequence to reproduce with a test database and signed test event:
1. Deliver a checkout event whose metadata selects the new-workspace branch and whose `workspacePublicId` does not exist.
2. Let `workspaceRepo.create` succeed, then inject a one-shot rejection from `subscriptionRepo.create` (or terminate the process between the calls).
3. Deliver the same event again after removing the injected failure.
4. `getByPublicId` now returns the workspace, so `if (!existing)` is false. No subscription is created, and the handler returns `{ received: true }` with HTTP 200.

Expected: redelivery completes the subscription record, or all related database writes roll back together. Actual source path: a paid workspace remains without its subscription record, and the retry guard prevents repair. The `customer.subscription.updated` branch also skips work when it cannot find a subscription row.

A single database transaction using its transaction handle for both repository calls, plus an idempotent subscription upsert/check independent of workspace existence, would address the failure and retry behavior. It would also protect against partial role/member creation inside workspace creation.

Found with [Midwrite](https://github.com/astra-void/midwrite), an experimental static analyzer for partial-failure windows in TypeScript backends. I checked the reported operations and recovery path against the source. This is an AI-assisted source review, not a production observation or an executed fault-injection test; the steps above describe the proposed reproduction.

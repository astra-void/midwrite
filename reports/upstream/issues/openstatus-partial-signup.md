The auth adapter's `createUser` helper independently commits the user, initial workspace, and owner membership. A failure during workspace/membership creation leaves a persisted user even though signup failed. A later lookup can find that user, without rerunning the helper that was supposed to provision their workspace.

Reviewed revision: `36288495c8544d4498986be7d72c37ee7e666837` (Turso/libSQL via Drizzle; no PostgreSQL model assumptions are needed).

Source:
- [`createUser`](https://github.com/openstatusHQ/openstatus/blob/36288495c8544d4498986be7d72c37ee7e666837/apps/dashboard/src/lib/auth/helpers.ts#L39-L86) inserts the user, then workspace, then `usersToWorkspaces`, each through the root `db`.
- [Adapter](https://github.com/openstatusHQ/openstatus/blob/36288495c8544d4498986be7d72c37ee7e666837/apps/dashboard/src/lib/auth/adapter.ts#L30-L45) calls this helper and returns the user only after all of those writes complete.

Failure sequence to reproduce against a disposable libSQL database:
1. Call the helper with a previously unused email address.
2. Let user creation succeed and inject a one-shot error in workspace creation; alternatively allow both inserts and reject the owner-membership insert.
3. After the helper rejects, inspect the three tables.
4. The first case leaves a user with no initial workspace. The second leaves both user and workspace, with no linking owner membership. `getUserByEmail` can already return the persisted user.

Expected: provisioning a new user and their initial owned workspace is atomic, or incomplete provisioning is explicitly retried. Actual source path: signup can fail after durable user creation, without rolling it back. This is about the database failure path, not evidence of an observed production incident.

Wrapping these three writes in a transaction and using its handle for the slug check and each write would prevent this half-provisioned state; a separate reconciliation path could also repair existing incomplete accounts.

Found with [Midwrite](https://github.com/astra-void/midwrite), an experimental static analyzer for partial-failure windows in TypeScript backends. I checked the reported operations and recovery path against the source. This is an AI-assisted source review, not a production observation or an executed fault-injection test; the steps above describe the proposed reproduction.

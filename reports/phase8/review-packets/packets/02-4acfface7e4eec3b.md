# Review packet 2 — finding `4acfface7e4eec3b`

Project `phase8-shop-app` at revision `working tree`. Classify this finding from the source; you are not shown any earlier verdict.

**CP1002 external effect followed by an independent database commit** in `sendReceiptOnce` at `src/app/jobs.ts:43`

`sendReceiptEmail({ orderId: job.orderId })` (email) is performed before `db.update(jobs)` commits; a failure in between leaves an external effect without the database change

## Failure sequence (static analysis)

- 1. `sendReceiptEmail({ orderId: job.orderId })` (43:9) performs an external effect (email); other systems can observe it as soon as it succeeds.
- 2. The process crashes, or the database operation fails, after `sendReceiptEmail({ orderId: job.orderId })` and before `db.update(jobs)` commits.
- 3. `db.update(jobs)` never commits.
- 4. Result: other systems observe the external effect, but the database change it implies never commits.

## Failure trace

```text
tr-7fc1981300921811 CP1002 4acfface7e4eec3b in `sendReceiptOnce` (src/app/jobs.ts:43:9) [STATIC_CANDIDATE; Complete]
    0. dbRead      Completed   db.select() @ 31:23
    1. branch      Taken       if `!job || job.status === "done"` takes the `else` path @ 32:3
    2. dbWrite     Committed   db.insert(jobEffects) @ 35:25
    3. branch      Taken       if `claimed.length === 0` takes the `else` path @ 40:3
    4. external    Performed   sendReceiptEmail({ orderId: job.orderId }) @ 43:9 <- completes first
       ---- failure injection boundary (after-external-effect / before-db-write) ----
    5. dbWrite     NotReached  db.update(jobs) @ 44:9 <- not yet performed
    6. return      NotReached  return @ 45:3
  simulated: If the process dies right after step 4: durable or visible: `db.insert(jobEffects)` (step 2), `sendReceiptEmail({ orderId: job.orderId })` (step 4); never performed: `db.update(jobs)` (step 5).
```

## Source

`src/app/jobs.ts:43`

```ts
    31 |   const [job] = await db.select().from(jobs).where(eq(jobs.id, input.jobId)).limit(1);
    32 |   if (!job || job.status === "done") {
    33 |     return { skipped: true };
    34 |   }
    35 |   const claimed = await db
    36 |     .insert(jobEffects)
    37 |     .values({ jobId: job.id, effect: "receipt" })
    38 |     .onConflictDoNothing()
    39 |     .returning();
    40 |   if (claimed.length === 0) {
    41 |     return { skipped: true };
    42 |   }
>   43 |   await sendReceiptEmail({ orderId: job.orderId });
    44 |   await db.update(jobs).set({ status: "done" }).where(eq(jobs.id, job.id));
    45 |   return { sent: true };
    46 | }
```

`src/app/jobs.ts:44`

```ts
    32 |   if (!job || job.status === "done") {
    33 |     return { skipped: true };
    34 |   }
    35 |   const claimed = await db
    36 |     .insert(jobEffects)
    37 |     .values({ jobId: job.id, effect: "receipt" })
    38 |     .onConflictDoNothing()
    39 |     .returning();
    40 |   if (claimed.length === 0) {
    41 |     return { skipped: true };
    42 |   }
    43 |   await sendReceiptEmail({ orderId: job.orderId });
>   44 |   await db.update(jobs).set({ status: "done" }).where(eq(jobs.id, job.id));
    45 |   return { sent: true };
    46 | }
```

## Questions

1. Can the failure window occur in production (is the path executable, are the operations related)?
2. If it occurs, what state is left, and does anything repair it (retries, redelivery, reconciliation, a user-visible repair action)? Cite where.
3. Classification: confirmed-bug, intended-risk, false-positive or unclear.

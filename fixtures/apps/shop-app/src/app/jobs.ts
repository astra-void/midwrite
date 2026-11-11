// Job worker: an at-least-once queue delivers `{ jobId }`; the queue
// redelivers a job whose acknowledgement it did not receive (the worker
// returning acknowledges it).
import { eq } from "drizzle-orm";

import { db } from "../lib/db/drizzle.js";
import { jobEffects, jobs } from "../lib/db/schema.js";
import { sendReceiptEmail } from "../lib/messaging/bus.js";

export interface JobDelivery {
  jobId: number;
}

/** Sends the receipt and marks the job done. No idempotency. */
export async function sendReceipt(input: JobDelivery) {
  const [job] = await db.select().from(jobs).where(eq(jobs.id, input.jobId)).limit(1);
  if (!job || job.status === "done") {
    return { skipped: true };
  }
  await sendReceiptEmail({ orderId: job.orderId });
  await db.update(jobs).set({ status: "done" }).where(eq(jobs.id, job.id));
  return { sent: true };
}

/**
 * Claims the effect first: a row in `job_effects`, unique per (job, effect),
 * inserted with ON CONFLICT DO NOTHING. Only the worker whose insert
 * succeeded sends the e-mail.
 */
export async function sendReceiptOnce(input: JobDelivery) {
  const [job] = await db.select().from(jobs).where(eq(jobs.id, input.jobId)).limit(1);
  if (!job || job.status === "done") {
    return { skipped: true };
  }
  const claimed = await db
    .insert(jobEffects)
    .values({ jobId: job.id, effect: "receipt" })
    .onConflictDoNothing()
    .returning();
  if (claimed.length === 0) {
    return { skipped: true };
  }
  await sendReceiptEmail({ orderId: job.orderId });
  await db.update(jobs).set({ status: "done" }).where(eq(jobs.id, job.id));
  return { sent: true };
}

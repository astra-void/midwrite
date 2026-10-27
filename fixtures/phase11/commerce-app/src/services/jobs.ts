// Refund worker: the queue delivers `{ jobId }` at least once and redelivers
// a job whose acknowledgement it did not receive.
import { eq } from "drizzle-orm";

import { db } from "../db/client.js";
import { jobEffects, refundJobs } from "../db/schema.js";
import { sendPayout } from "./payments.js";

export interface RefundDelivery {
  jobId: number;
}

/** Pays the refund out and marks the job done. */
export async function processRefund(delivery: RefundDelivery) {
  const [job] = await db
    .select()
    .from(refundJobs)
    .where(eq(refundJobs.id, delivery.jobId))
    .limit(1);
  if (!job || job.status === "done") {
    return { skipped: true };
  }
  await sendPayout({ orderId: job.orderId, amount: job.amount });
  await db.update(refundJobs).set({ status: "done" }).where(eq(refundJobs.id, job.id));
  return { refunded: job.amount };
}

/** Claims the payout with a unique effect row before paying it out. */
export async function processRefundOnce(delivery: RefundDelivery) {
  const [job] = await db
    .select()
    .from(refundJobs)
    .where(eq(refundJobs.id, delivery.jobId))
    .limit(1);
  if (!job || job.status === "done") {
    return { skipped: true };
  }
  const claimed = await db
    .insert(jobEffects)
    .values({ jobId: job.id, effect: "payout" })
    .onConflictDoNothing()
    .returning();
  if (claimed.length === 0) {
    return { skipped: true };
  }
  await sendPayout({ orderId: job.orderId, amount: job.amount });
  await db.update(refundJobs).set({ status: "done" }).where(eq(refundJobs.id, job.id));
  return { refunded: job.amount };
}

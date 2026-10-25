import { eq } from "drizzle-orm";
import { db } from "./db";
import { jobs } from "./schema";
import { publishEvent } from "./events";

// A worker claims a job, publishes its result, then marks it done. The
// claim is a plain read: two workers can both see the job as pending.
export async function processJob(jobId: string) {
  const job = await db.query.jobs.findFirst({ where: eq(jobs.id, jobId) });
  if (!job || job.status !== "pending") {
    return;
  }
  await db.update(jobs).set({ status: "processing" }).where(eq(jobs.id, jobId));
  await publishEvent("job.processed", jobId);
  await db.update(jobs).set({ status: "done" }).where(eq(jobs.id, jobId));
}

import { eq } from "drizzle-orm";

import { db } from "../db.js";
import { notifications, processedJobs, users } from "../schema/index.js";

export type WelcomeJob = { id: number; userId: string };

// E24: the queue consumer for "welcome" jobs.
export async function handleWelcomeJob(job: WelcomeJob) {
  await db.insert(notifications).values({ userId: job.userId, kind: "welcome", jobId: job.id });
  await db.update(users).set({ welcomedAt: new Date() }).where(eq(users.id, job.userId));
}

// E25: the same, recording the job id in the same transaction first.
export async function handleWelcomeJobOnce(job: WelcomeJob) {
  await db.transaction(async (tx) => {
    const [first] = await tx
      .insert(processedJobs)
      .values({ jobId: job.id })
      .onConflictDoNothing()
      .returning({ jobId: processedJobs.jobId });
    if (!first) return;
    await tx.insert(notifications).values({ userId: job.userId, kind: "welcome", jobId: job.id });
    await tx.update(users).set({ welcomedAt: new Date() }).where(eq(users.id, job.userId));
  });
}

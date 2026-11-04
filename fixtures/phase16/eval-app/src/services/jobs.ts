import { and, eq } from "drizzle-orm";

import { db } from "../db.js";
import { jobRuns, jobs } from "../schema/index.js";

// P16-E19: a worker claims a queued job and records the run it starts.
export async function claimJob(jobId: number, workerId: string) {
  const claimable = and(eq(jobs.id, jobId), eq(jobs.status, "queued"));
  const [job] = await db.select({ id: jobs.id, kind: jobs.kind }).from(jobs).where(claimable).limit(1);
  if (!job) return null;

  await db.update(jobs).set({ status: "running", workerId }).where(eq(jobs.id, job.id));
  const [run] = await db.insert(jobRuns).values({ jobId: job.id, workerId }).returning({ id: jobRuns.id });
  return run ? { runId: run.id, kind: job.kind } : null;
}

// P16-E20: the same; the hoisted predicate is the UPDATE's WHERE clause.
export async function claimJobAtomic(jobId: number, workerId: string) {
  const claimable = and(eq(jobs.id, jobId), eq(jobs.status, "queued"));
  const [job] = await db
    .update(jobs)
    .set({ status: "running", workerId })
    .where(claimable)
    .returning({ id: jobs.id, kind: jobs.kind });
  if (!job) return null;

  const [run] = await db.insert(jobRuns).values({ jobId: job.id, workerId }).returning({ id: jobRuns.id });
  return run ? { runId: run.id, kind: job.kind } : null;
}

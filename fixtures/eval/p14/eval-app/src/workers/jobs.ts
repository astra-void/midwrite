import { and, eq, isNull, lte, or } from "drizzle-orm";

import { db } from "../db.js";
import { jobRuns, jobs } from "../schema/index.js";

// How long a worker holds a job once it claimed it.
const LEASE_MS = 60_000;

// P14-E26: a worker claims a job at the scheduler tick `now`: the job must
// not be done and its lease must be free or expired. Returns the lease end.
export async function claimJob(jobId: number, workerId: string, now: Date) {
  const [job] = await db.select().from(jobs).where(eq(jobs.id, jobId)).limit(1);
  if (!job || job.doneAt !== null) return null;
  if (job.lockedUntil !== null && job.lockedUntil > now) return null;
  const lockedUntil = new Date(now.getTime() + LEASE_MS);
  await db.update(jobs).set({ lockedBy: workerId, lockedUntil }).where(eq(jobs.id, jobId));
  await db.insert(jobRuns).values({ jobId, workerId, startedAt: now });
  return lockedUntil;
}

// P14-E27: the lease test goes into the UPDATE's WHERE clause; the run is
// recorded only by the worker whose UPDATE matched.
export async function claimJobAtomic(jobId: number, workerId: string, now: Date) {
  const lockedUntil = new Date(now.getTime() + LEASE_MS);
  const [claimed] = await db
    .update(jobs)
    .set({ lockedBy: workerId, lockedUntil })
    .where(and(eq(jobs.id, jobId), isNull(jobs.doneAt), or(isNull(jobs.lockedUntil), lte(jobs.lockedUntil, now))))
    .returning({ id: jobs.id });
  if (!claimed) return null;
  await db.insert(jobRuns).values({ jobId, workerId, startedAt: now });
  return lockedUntil;
}

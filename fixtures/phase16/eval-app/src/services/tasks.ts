import { asc, eq, inArray } from "drizzle-orm";

import { db } from "../db.js";
import { taskClaims, tasks } from "../schema/index.js";

function batchSize(limit: number) {
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 100) throw new RangeError("limit must be 1..100");
  return limit;
}

// P16-E25: a worker claims up to `limit` ready tasks.
export async function claimBatchUnlocked(workerId: string, limit = 10) {
  const size = batchSize(limit);
  return db.transaction(async (tx) => {
    const ready = await tx
      .select({ id: tasks.id })
      .from(tasks)
      .where(eq(tasks.status, "ready"))
      .orderBy(asc(tasks.id))
      .limit(size);
    if (ready.length === 0) return [];
    const ids = ready.map((task) => task.id);
    await tx.update(tasks).set({ status: "claimed" }).where(inArray(tasks.id, ids));
    await tx.insert(taskClaims).values(ids.map((taskId) => ({ taskId, workerId })));
    return ids;
  });
}

// P16-E26: the same, locking the batch with FOR UPDATE SKIP LOCKED.
export async function claimBatch(workerId: string, limit = 10) {
  const size = batchSize(limit);
  return db.transaction(async (tx) => {
    const ready = await tx
      .select({ id: tasks.id })
      .from(tasks)
      .where(eq(tasks.status, "ready"))
      .orderBy(asc(tasks.id))
      .limit(size)
      .for("update", { skipLocked: true });
    if (ready.length === 0) return [];
    const ids = ready.map((task) => task.id);
    await tx.update(tasks).set({ status: "claimed" }).where(inArray(tasks.id, ids));
    await tx.insert(taskClaims).values(ids.map((taskId) => ({ taskId, workerId })));
    return ids;
  });
}

// P16-E27: the same, locking the batch with FOR UPDATE NOWAIT (a worker that
// meets a locked task fails with 55P03 and polls again later).
export async function claimBatchNoWait(workerId: string, limit = 10) {
  const size = batchSize(limit);
  return db.transaction(async (tx) => {
    const ready = await tx
      .select({ id: tasks.id })
      .from(tasks)
      .where(eq(tasks.status, "ready"))
      .orderBy(asc(tasks.id))
      .limit(size)
      .for("update", { noWait: true });
    if (ready.length === 0) return [];
    const ids = ready.map((task) => task.id);
    await tx.update(tasks).set({ status: "claimed" }).where(inArray(tasks.id, ids));
    await tx.insert(taskClaims).values(ids.map((taskId) => ({ taskId, workerId })));
    return ids;
  });
}

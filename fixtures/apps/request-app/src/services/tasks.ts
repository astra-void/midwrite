import { and, eq, isNull } from "drizzle-orm";

import { db } from "../db/client.js";
import { tasks } from "../db/schema.js";

// Read the assignee, then assign unconditionally: two concurrent callers
// can both be told they claimed the task.
export async function claimTask(taskId: number, who: string) {
  const [task] = await db.select().from(tasks).where(eq(tasks.id, taskId)).limit(1);
  if (!task) throw new Error("no such task");
  if (task.assignee !== null) return false;
  await db.update(tasks).set({ assignee: who }).where(eq(tasks.id, taskId));
  return true;
}

// The conditional UPDATE decides: its row count is the answer.
export async function claimTaskAtomic(taskId: number, who: string) {
  const rows = await db
    .update(tasks)
    .set({ assignee: who })
    .where(and(eq(tasks.id, taskId), isNull(tasks.assignee)))
    .returning();
  return rows.length > 0;
}

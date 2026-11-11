import { and, eq, isNull } from "drizzle-orm";

import { db } from "../db.js";
import { taskRepo } from "../repositories/index.js";
import { tasks } from "../schema/index.js";

// E2: a duplicate title is checked, not enforced.
export async function addTask(projectId: string, title: string) {
  const existing = await taskRepo.findByTitle(db, projectId, title);
  if (existing) return existing.id;
  const [created] = await db.insert(tasks).values({ projectId, title }).returning({ id: tasks.id });
  return created?.id;
}

// E3: read the assignee, then assign unconditionally.
export async function claimTask(taskId: number, who: string) {
  const [task] = await db.select().from(tasks).where(eq(tasks.id, taskId)).limit(1);
  if (!task) throw new Error("no task");
  if (task.assignee !== null) return false;
  await db.update(tasks).set({ assignee: who }).where(eq(tasks.id, taskId));
  return true;
}

// E4: a conditional update decides.
export async function claimTaskAtomic(taskId: number, who: string) {
  const rows = await db
    .update(tasks)
    .set({ assignee: who })
    .where(and(eq(tasks.id, taskId), isNull(tasks.assignee)))
    .returning();
  return rows.length > 0;
}

// E13 (unsupported): a loop over an input array.
export async function importTasks(projectId: string, titles: string[]) {
  for (const title of titles) {
    await db.insert(tasks).values({ projectId, title });
  }
}

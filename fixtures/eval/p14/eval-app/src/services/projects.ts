import { and, eq } from "drizzle-orm";

import { db } from "../db.js";
import { isForeignKeyViolation } from "../lib/pg-errors.js";
import { priorities, projects, stages, tasks } from "../schema/index.js";

// P14-E23: adds a task to an existing project (tasks.project_id references
// projects.id, ON DELETE NO ACTION). A project purged meanwhile is reported
// to the caller, not thrown.
export async function addTask(projectId: number, title: string) {
  const [project] = await db.select({ id: projects.id }).from(projects).where(eq(projects.id, projectId)).limit(1);
  if (!project) return { ok: false as const, reason: "no-project" };
  try {
    const [task] = await db.insert(tasks).values({ projectId, title }).returning({ id: tasks.id });
    return { ok: true as const, id: task?.id };
  } catch (error) {
    if (isForeignKeyViolation(error)) return { ok: false as const, reason: "no-project" };
    throw error;
  }
}

// Deletes a project and everything in it.
export async function purgeProject(projectId: number) {
  await db.transaction(async (tx) => {
    await tx.delete(tasks).where(eq(tasks.projectId, projectId));
    await tx.delete(stages).where(eq(stages.projectId, projectId));
    await tx.delete(priorities).where(eq(priorities.projectId, projectId));
    await tx.delete(projects).where(eq(projects.id, projectId));
  });
}

// P14-E24: adds a task unless the project is archived (soft-deleted).
export async function addTaskIfActive(projectId: number, title: string) {
  const [project] = await db
    .select({ id: projects.id, archivedAt: projects.archivedAt })
    .from(projects)
    .where(eq(projects.id, projectId))
    .limit(1);
  if (!project || project.archivedAt !== null) throw new Error("project is archived");
  const [task] = await db.insert(tasks).values({ projectId, title }).returning({ id: tasks.id });
  return task?.id;
}

// P14-E25: the same, holding the project row FOR SHARE for the check and the
// insert.
export async function addTaskIfActiveLocked(projectId: number, title: string) {
  return db.transaction(async (tx) => {
    const [project] = await tx
      .select({ id: projects.id, archivedAt: projects.archivedAt })
      .from(projects)
      .where(eq(projects.id, projectId))
      .limit(1)
      .for("share");
    if (!project || project.archivedAt !== null) throw new Error("project is archived");
    const [task] = await tx.insert(tasks).values({ projectId, title }).returning({ id: tasks.id });
    return task?.id;
  });
}

// Archives (soft-deletes) a project and cancels its open tasks.
export async function archiveProject(projectId: number, now: Date) {
  await db.transaction(async (tx) => {
    await tx.update(projects).set({ archivedAt: now }).where(eq(projects.id, projectId));
    await tx
      .update(tasks)
      .set({ status: "cancelled" })
      .where(and(eq(tasks.projectId, projectId), eq(tasks.status, "open")));
  });
}

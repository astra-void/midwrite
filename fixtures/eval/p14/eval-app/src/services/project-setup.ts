import { db } from "../db.js";
import { priorities, stages } from "../schema/index.js";

// P14-E08: POST /projects/:id/setup creates the default board columns in one
// multi-row INSERT. The client resubmits when it gets no response.
export async function createDefaultStages(projectId: number) {
  await db.insert(stages).values([
    { projectId, name: "todo", position: 0 },
    { projectId, name: "doing", position: 1 },
    { projectId, name: "done", position: 2 },
  ]);
  return { stages: 3 };
}

// P14-E09: POST /projects/:id/priorities creates the default priority levels
// in one multi-row INSERT; UNIQUE(project_id, name) with ON CONFLICT DO
// NOTHING. The client resubmits when it gets no response.
export async function createDefaultPriorities(projectId: number) {
  const created = await db
    .insert(priorities)
    .values([
      { projectId, name: "low", rank: 0 },
      { projectId, name: "high", rank: 1 },
    ])
    .onConflictDoNothing({ target: [priorities.projectId, priorities.name] })
    .returning({ id: priorities.id });
  return { priorities: created.length };
}

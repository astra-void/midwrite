import { and, eq, isNull } from "drizzle-orm";
import { z } from "zod";

import { db } from "../db.js";
import { labels, tags } from "../schema/index.js";

// groupId may be left out: the label is then ungrouped.
const CreateLabelInput = z.strictObject({
  workspaceId: z.number().int().positive(),
  name: z.string().trim().min(1).max(32),
  groupId: z.number().int().positive().optional(),
});

// groupId may be left out: the tag is then ungrouped (group 0).
const CreateTagInput = z.strictObject({
  workspaceId: z.number().int().positive(),
  name: z.string().trim().min(1).max(32),
  groupId: z.number().int().nonnegative().default(0),
});

// P16-E03: returns the id of the label with this name in this group (or
// ungrouped), creating it if there is none.
export async function createLabel(raw: unknown) {
  const input = CreateLabelInput.parse(raw);
  const groupId = input.groupId ?? null;
  const inGroup = groupId === null ? isNull(labels.groupId) : eq(labels.groupId, groupId);
  const sameLabel = and(eq(labels.workspaceId, input.workspaceId), inGroup, eq(labels.name, input.name));

  const [existing] = await db.select({ id: labels.id }).from(labels).where(sameLabel).limit(1);
  if (existing) return existing.id;

  const [created] = await db
    .insert(labels)
    .values({ workspaceId: input.workspaceId, groupId, name: input.name })
    .onConflictDoNothing()
    .returning({ id: labels.id });
  if (created) return created.id;

  // Lost a race with a concurrent request: return the winner's label.
  const [winner] = await db.select({ id: labels.id }).from(labels).where(sameLabel).limit(1);
  if (!winner) throw new Error("label vanished");
  return winner.id;
}

// P16-E04: the same for tags.
export async function createTag(raw: unknown) {
  const input = CreateTagInput.parse(raw);
  const sameTag = and(
    eq(tags.workspaceId, input.workspaceId),
    eq(tags.groupId, input.groupId),
    eq(tags.name, input.name),
  );

  const [existing] = await db.select({ id: tags.id }).from(tags).where(sameTag).limit(1);
  if (existing) return existing.id;

  const [created] = await db
    .insert(tags)
    .values({ workspaceId: input.workspaceId, groupId: input.groupId, name: input.name })
    .onConflictDoNothing()
    .returning({ id: tags.id });
  if (created) return created.id;

  const [winner] = await db.select({ id: tags.id }).from(tags).where(sameTag).limit(1);
  if (!winner) throw new Error("tag vanished");
  return winner.id;
}

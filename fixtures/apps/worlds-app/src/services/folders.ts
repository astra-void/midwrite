import { and, eq, isNull } from "drizzle-orm";

import { db } from "../db/client.js";
import { folders, labels } from "../db/schema.js";

// Creates a folder under `parentId`, or at the root when it is null, unless
// one with this name is already there. Defect: at the root the unique key
// does not cover the duplicate (NULLS DISTINCT), so two concurrent calls
// both insert.
export async function createFolder(ownerId: number, parentId: number | null, name: string) {
  const [existing] = await db
    .select({ id: folders.id })
    .from(folders)
    .where(
      and(
        eq(folders.ownerId, ownerId),
        parentId === null ? isNull(folders.parentId) : eq(folders.parentId, parentId),
        eq(folders.name, name),
      ),
    )
    .limit(1);
  if (existing) return existing.id;
  const [created] = await db
    .insert(folders)
    .values({ ownerId, parentId, name })
    .onConflictDoNothing()
    .returning({ id: folders.id });
  return created?.id ?? null;
}

// The same for labels, whose unique key is NULLS NOT DISTINCT: safe.
export async function createLabel(ownerId: number, parentId: number | null, name: string) {
  const [existing] = await db
    .select({ id: labels.id })
    .from(labels)
    .where(
      and(
        eq(labels.ownerId, ownerId),
        parentId === null ? isNull(labels.parentId) : eq(labels.parentId, parentId),
        eq(labels.name, name),
      ),
    )
    .limit(1);
  if (existing) return existing.id;
  const [created] = await db
    .insert(labels)
    .values({ ownerId, parentId, name })
    .onConflictDoNothing()
    .returning({ id: labels.id });
  return created?.id ?? null;
}

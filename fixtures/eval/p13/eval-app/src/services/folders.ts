import { and, eq, isNull } from "drizzle-orm";

import { db } from "../db.js";
import { folders } from "../schema/index.js";

// E26: looks for a folder with this name under the parent (or at the root),
// creates it if there is none. UNIQUE(owner_id, parent_id, name).
export async function createFolder(ownerId: string, parentId: number | null, name: string) {
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
  const [created] = await db.insert(folders).values({ ownerId, parentId, name }).returning({ id: folders.id });
  return created?.id;
}

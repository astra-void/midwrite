import { and, eq, isNull } from "drizzle-orm";

import { db } from "../db.js";
import { collections, workspaces } from "../schema/index.js";

function siblingNamed(workspaceId: number, parentId: number | null, name: string) {
  const sameParent = parentId === null ? isNull(collections.parentId) : eq(collections.parentId, parentId);
  return and(eq(collections.workspaceId, workspaceId), sameParent, eq(collections.name, name));
}

// P16-E21: returns the id of the collection with this name under this
// parent (null: the workspace root), creating it if there is none.
export async function createCollection(workspaceId: number, parentId: number | null, name: string) {
  const sibling = siblingNamed(workspaceId, parentId, name);
  const [existing] = await db.select({ id: collections.id }).from(collections).where(sibling).limit(1);
  if (existing) return existing.id;

  const [created] = await db
    .insert(collections)
    .values({ workspaceId, parentId, name })
    .returning({ id: collections.id });
  if (!created) throw new Error("insert returned no row");
  return created.id;
}

// P16-E22: the same, serialized per workspace by locking the workspace row.
export async function createCollectionLocked(workspaceId: number, parentId: number | null, name: string) {
  return db.transaction(async (tx) => {
    const [workspace] = await tx
      .select({ id: workspaces.id })
      .from(workspaces)
      .where(eq(workspaces.id, workspaceId))
      .for("update");
    if (!workspace) throw new Error(`unknown workspace ${workspaceId}`);

    const sibling = siblingNamed(workspaceId, parentId, name);
    const [existing] = await tx.select({ id: collections.id }).from(collections).where(sibling).limit(1);
    if (existing) return existing.id;

    const [created] = await tx
      .insert(collections)
      .values({ workspaceId, parentId, name })
      .returning({ id: collections.id });
    if (!created) throw new Error("insert returned no row");
    return created.id;
  });
}

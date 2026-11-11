import { eq } from "drizzle-orm";

import { db } from "../db.js";
import { collections } from "../schema/index.js";

// P16-E40: archives a collection and, recursively, every collection below
// it (the tree has no depth limit). Returns the number of rows archived.
export async function archiveCollectionTree(collectionId: number): Promise<number> {
  const children = await db
    .select({ id: collections.id })
    .from(collections)
    .where(eq(collections.parentId, collectionId));

  let archived = 0;
  for (const child of children) {
    archived += await archiveCollectionTree(child.id);
  }

  const updated = await db
    .update(collections)
    .set({ archived: true })
    .where(eq(collections.id, collectionId))
    .returning({ id: collections.id });
  return archived + updated.length;
}

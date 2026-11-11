import { and, eq, isNull } from "drizzle-orm";

import { db } from "../db.js";
import { documents, spaces } from "../schema.js";

// The folder condition, as a project helper returning a Drizzle predicate.
function inFolderOf(folderId: number | null) {
  return folderId === null ? isNull(documents.folderId) : eq(documents.folderId, folderId);
}

// Creates a document in a folder, or at the root of the space when folderId
// is null, unless one with this title is already there. The predicate is
// built in locals before the query. Defect: at the root the unique key does
// not cover the duplicate (NULLS DISTINCT), so two concurrent calls both
// insert.
export async function createDocument(spaceId: number, folderId: number | null, title: string) {
  const inFolder = folderId === null ? isNull(documents.folderId) : eq(documents.folderId, folderId);
  const sameTitle = and(eq(documents.spaceId, spaceId), inFolder, eq(documents.title, title));
  const [existing] = await db.select({ id: documents.id }).from(documents).where(sameTitle).limit(1);
  if (existing) return existing.id;
  const [created] = await db
    .insert(documents)
    .values({ spaceId, folderId, title })
    .onConflictDoNothing()
    .returning({ id: documents.id });
  return created?.id ?? null;
}

// The same check and insert, serialized by a lock on the space row: the
// second call waits, then sees the first call's document.
export async function createDocumentLocked(spaceId: number, folderId: number | null, title: string) {
  return db.transaction(async (tx) => {
    const [space] = await tx.select().from(spaces).where(eq(spaces.id, spaceId)).limit(1).for("update");
    if (!space) throw new Error("no space");
    const where = and(eq(documents.spaceId, spaceId), inFolderOf(folderId), eq(documents.title, title));
    const [existing] = await tx.select({ id: documents.id }).from(documents).where(where).limit(1);
    if (existing) return existing.id;
    const [created] = await tx
      .insert(documents)
      .values({ spaceId, folderId, title })
      .onConflictDoNothing()
      .returning({ id: documents.id });
    return created?.id ?? null;
  });
}

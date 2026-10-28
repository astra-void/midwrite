import { and, eq, isNull } from "drizzle-orm";

import { db } from "../db.js";
import { documents, pages } from "../schema/index.js";

// P15-E06: creates a document in a folder, or at the root of the space when
// folderId is null, unless one with the same title is already there.
export async function createDocument(spaceId: number, folderId: number | null, title: string) {
  const inFolder = folderId === null ? isNull(documents.folderId) : eq(documents.folderId, folderId);
  const [existing] = await db
    .select({ id: documents.id })
    .from(documents)
    .where(and(eq(documents.spaceId, spaceId), inFolder, eq(documents.title, title)))
    .limit(1);
  if (existing) return existing.id;
  const [created] = await db
    .insert(documents)
    .values({ spaceId, folderId, title })
    .onConflictDoNothing()
    .returning({ id: documents.id });
  return created?.id ?? null;
}

// P15-E07: the same for pages (UNIQUE ... NULLS NOT DISTINCT).
export async function createPage(spaceId: number, folderId: number | null, title: string) {
  const inFolder = folderId === null ? isNull(pages.folderId) : eq(pages.folderId, folderId);
  const [existing] = await db
    .select({ id: pages.id })
    .from(pages)
    .where(and(eq(pages.spaceId, spaceId), inFolder, eq(pages.title, title)))
    .limit(1);
  if (existing) return existing.id;
  const [created] = await db
    .insert(pages)
    .values({ spaceId, folderId, title })
    .onConflictDoNothing()
    .returning({ id: pages.id });
  return created?.id ?? null;
}

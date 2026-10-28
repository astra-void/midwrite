import { and, eq, inArray, isNull, not, notInArray, or } from "drizzle-orm";

import { db } from "../db/client.js";
import { comments } from "../db/schema.js";

// Hide the visible comments among `ids` (inArray).
export async function hideMany(ids: number[]) {
  await db
    .update(comments)
    .set({ status: "hidden" })
    .where(and(inArray(comments.id, ids), or(isNull(comments.status), eq(comments.status, "visible"))));
}

// Remove the replies of a parent (or the top-level comments) except `keep`.
export async function prune(parentId: number | null, keep: number[]) {
  await db
    .delete(comments)
    .where(
      and(
        parentId === null ? isNull(comments.parentId) : eq(comments.parentId, parentId),
        notInArray(comments.id, keep),
      ),
    );
}

// Count comments that are not explicitly hidden (NULL status counts).
export async function visibleCount() {
  const rows = await db
    .select()
    .from(comments)
    .where(not(eq(comments.status, "hidden")));
  return rows.length;
}

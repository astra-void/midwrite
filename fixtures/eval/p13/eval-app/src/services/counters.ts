import { sql } from "drizzle-orm";

import { db } from "../db.js";
import { handles, pageCounters } from "../schema/index.js";

// E22: counts a page view with one upsert.
export async function recordView(path: string) {
  await db
    .insert(pageCounters)
    .values({ path, hits: 1 })
    .onConflictDoUpdate({
      target: pageCounters.path,
      set: { hits: sql`${pageCounters.hits} + 1` },
    });
}

// E23: reserves a public handle with one upsert and reports whether the
// caller got it.
export async function reserveHandle(userId: string, handle: string) {
  const rows = await db
    .insert(handles)
    .values({ handle, userId })
    .onConflictDoUpdate({ target: handles.handle, set: { updatedAt: new Date() } })
    .returning({ handle: handles.handle });
  return { reserved: rows.length > 0 };
}

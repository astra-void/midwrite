import { isNull, sql } from "drizzle-orm";

import { db } from "../db/client.js";
import { handles, views } from "../db/schema.js";

// INSERT ... ON CONFLICT (id) DO UPDATE SET hits = views.hits + 1.
export async function recordView(id: number) {
  await db
    .insert(views)
    .values({ id, hits: 1 })
    .onConflictDoUpdate({ target: views.id, set: { hits: sql`${views.hits} + 1` } });
}

// Takes the handle over: every caller is told it got the handle.
export async function reserveHandle(handle: string, owner: string) {
  const [row] = await db
    .insert(handles)
    .values({ handle, owner })
    .onConflictDoUpdate({ target: handles.handle, set: { owner: sql`excluded.owner` } })
    .returning();
  return row !== undefined;
}

// Only a free handle is taken: the conditional DO UPDATE decides.
export async function reserveHandleIfFree(handle: string, owner: string) {
  const rows = await db
    .insert(handles)
    .values({ handle, owner })
    .onConflictDoUpdate({
      target: handles.handle,
      set: { owner: sql`excluded.owner` },
      setWhere: isNull(handles.owner),
    })
    .returning();
  return rows.length > 0;
}

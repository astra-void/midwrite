import { eq } from "drizzle-orm";

import { db } from "../db/client.js";
import { handles } from "../db/schema.js";

// `Ann`/`Lee` and `ann`/`lee` reserve the same handle: check, then insert.
export async function reserveHandle(id: number, first: string, last: string) {
  const handle = `${first.trim()}.${last.trim()}`.toLowerCase();
  const [taken] = await db.select().from(handles).where(eq(handles.handle, handle)).limit(1);
  if (taken) return null;
  await db.insert(handles).values({ id, handle });
  return handle;
}

// Without normalization the two spellings are different handles.
export async function reserveHandleRaw(id: number, first: string, last: string) {
  const handle = first + "." + last;
  const [taken] = await db.select().from(handles).where(eq(handles.handle, handle)).limit(1);
  if (taken) return null;
  await db.insert(handles).values({ id, handle });
  return handle;
}

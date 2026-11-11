import { eq } from "drizzle-orm";

import { db } from "../db.js";
import { notes } from "../schema/index.js";

// P15-E05: archives a note unless it is already archived.
export async function archiveNote(noteId: number) {
  const [note] = await db.select({ archived: notes.archived }).from(notes).where(eq(notes.id, noteId)).limit(1);
  if (!note || note.archived) return;
  await db.update(notes).set({ archived: true }).where(eq(notes.id, noteId));
}

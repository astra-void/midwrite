// @expect none
import { eq } from "drizzle-orm";
import { db } from "@/db";
import { attachments, inbox } from "@/db/schema";

type CloseOptions = { purge?: boolean };

// `options` is handed to other code first, then read into a `const` with a
// default. Only an absent `options` decides the branch: the other code may
// change `purge` on an object it was given.
export async function close(database: typeof db, id: string, options?: CloseOptions) {
  audit(options);
  const purge = options?.purge ?? false;
  if (purge) {
    await database.delete(attachments).where(eq(attachments.inboxId, id));
  }
  await database.update(inbox).set({ status: "closed" }).where(eq(inbox.id, id));
}

// Read in the condition itself, and nothing else touches `options`: a
// literal `purge` decides the branch too.
export async function closeDirect(database: typeof db, id: string, options?: CloseOptions) {
  if (options?.purge) {
    await database.delete(attachments).where(eq(attachments.inboxId, id));
  }
  await database.update(inbox).set({ status: "closed" }).where(eq(inbox.id, id));
}

// Reassigned: never decided.
export async function closeReassigned(database: typeof db, id: string, options?: CloseOptions) {
  options = options ?? { purge: true };
  const purge = options?.purge ?? false;
  if (purge) {
    await database.delete(attachments).where(eq(attachments.inboxId, id));
  }
  await database.update(inbox).set({ status: "closed" }).where(eq(inbox.id, id));
}

// Forwards its own optional parameter.
export async function closeVia(id: string, options?: CloseOptions) {
  await close(db, id, options);
}

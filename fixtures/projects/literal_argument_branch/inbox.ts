// @expect none
import { eq } from "drizzle-orm";
import { db } from "@/db";
import { attachments, inbox, suggestions } from "@/db/schema";

type UpdateInboxParams = {
  id: string;
  teamId: string;
  status?: "deleted" | "pending" | "analyzing";
};

// Deletes the inbox item's attachments and suggestions only when the
// status written is "deleted".
export async function updateInbox(database: typeof db, params: UpdateInboxParams) {
  const { id, teamId, ...data } = params;

  if (data.status === "deleted") {
    await database.delete(attachments).where(eq(attachments.inboxId, id));
    await database.delete(suggestions).where(eq(suggestions.inboxId, id));
  }

  await database.update(inbox).set(data).where(eq(inbox.id, id));
}

// The same deletes, but `params` is handed to other code before it is
// destructured: that code may change `status`, so no call decides it.
export async function updateInboxAfterAudit(database: typeof db, params: UpdateInboxParams) {
  audit(params);
  const { id, ...data } = params;

  if (data.status === "deleted") {
    await database.delete(attachments).where(eq(attachments.inboxId, id));
  }

  await database.update(inbox).set(data).where(eq(inbox.id, id));
}

// A primitive parameter: decided by a literal argument too.
export async function archive(database: typeof db, id: string, purge: boolean) {
  if (purge) {
    await database.delete(attachments).where(eq(attachments.inboxId, id));
  }
  await database.update(inbox).set({ status: "archived" }).where(eq(inbox.id, id));
}

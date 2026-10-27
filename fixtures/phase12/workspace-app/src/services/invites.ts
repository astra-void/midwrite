import { db } from "../db/client.js";
import { invites } from "../db/schema/index.js";
import { invitesRepository } from "../repositories/index.js";

// One pending invite per address and workspace: checked in a transaction
// and enforced by a unique index.
export async function inviteToWorkspace(workspaceId: string, email: string) {
  return db.transaction(async (tx) => {
    const pending = await invitesRepository.findPending(tx, workspaceId, email);
    if (pending) return pending.id;
    const [invite] = await tx
      .insert(invites)
      .values({ workspaceId, email })
      .returning({ id: invites.id });
    if (!invite) throw new Error("invite insert failed");
    return invite.id;
  });
}

import { eq } from "drizzle-orm";

import { replicatedDb } from "../db.js";
import { invites } from "../schema.js";

// G3 (defect against a declared read-your-writes promise): creates an
// invitation (database-generated id) on the primary, reads it back through
// the replica set and returns the id it read. The replica may not have
// replayed the INSERT yet: the caller is told no id.
export async function createInvite(teamId: number, email: string) {
  const [created] = await replicatedDb
    .insert(invites)
    .values({ teamId, email })
    .returning({ id: invites.id });
  if (!created) {
    throw new Error("invite not created");
  }
  const [row] = await replicatedDb
    .select({ id: invites.id })
    .from(invites)
    .where(eq(invites.id, created.id))
    .limit(1);
  return { id: row?.id ?? null };
}

// G3-safe: the read-back goes to the primary (`$primary`).
export async function createInvitePrimary(teamId: number, email: string) {
  const [created] = await replicatedDb
    .insert(invites)
    .values({ teamId, email })
    .returning({ id: invites.id });
  if (!created) {
    throw new Error("invite not created");
  }
  const [row] = await replicatedDb.$primary
    .select({ id: invites.id })
    .from(invites)
    .where(eq(invites.id, created.id))
    .limit(1);
  return { id: row?.id ?? null };
}

import { and, eq } from "drizzle-orm";

import { db } from "../db/client.js";
import { invitations, seatInvites } from "../db/schema.js";

export async function findPendingInvitation(orgId: number, email: string) {
  const [existing] = await db
    .select()
    .from(invitations)
    .where(
      and(
        eq(invitations.orgId, orgId),
        eq(invitations.email, email),
        eq(invitations.status, "pending"),
      ),
    )
    .limit(1);
  return existing;
}

export async function createInvitation(orgId: number, email: string, role: string, invitedBy: number) {
  await db.insert(invitations).values({ orgId, email, role, invitedBy, status: "pending" });
}

export async function findSeatInvite(orgId: number, email: string) {
  const [existing] = await db
    .select()
    .from(seatInvites)
    .where(and(eq(seatInvites.orgId, orgId), eq(seatInvites.email, email)))
    .limit(1);
  return existing;
}

export async function createSeatInvite(orgId: number, email: string) {
  await db.insert(seatInvites).values({ orgId, email });
}

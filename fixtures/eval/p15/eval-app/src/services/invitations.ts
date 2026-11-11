import { and, eq } from "drizzle-orm";

import { db } from "../db.js";
import { invitations, teamInvites } from "../schema/index.js";

// P15-E08: invites an email address to a team of the organization, or to the
// organization itself when no team is given.
export async function inviteMember(orgId: number, email: string, teamId?: number | null) {
  if (teamId == null) {
    const [existing] = await db
      .select({ id: invitations.id })
      .from(invitations)
      .where(and(eq(invitations.orgId, orgId), eq(invitations.email, email)))
      .limit(1);
    if (existing) return existing.id;
    const [created] = await db.insert(invitations).values({ orgId, email }).returning({ id: invitations.id });
    return created?.id ?? null;
  }
  await db.insert(teamInvites).values({ teamId, email }).onConflictDoNothing();
  return null;
}

// P15-E09: invites an email address to a team; the team is required.
export async function inviteToTeam(orgId: number, email: string, teamId: number) {
  return inviteMember(orgId, email, teamId);
}

"use server";

import { and, eq } from "drizzle-orm";

import { validatedActionWithUser } from "../lib/auth/middleware.js";
import { db } from "../lib/db/drizzle.js";
import { getUserWithTeam } from "../lib/db/queries.js";
import {
  ActivityType,
  activityLogs,
  invitations,
  teamMembers,
  users,
  type NewActivityLog,
} from "../lib/db/schema.js";
import { email, object, oneOf, positiveInteger } from "../lib/validation.js";

async function logActivity(
  teamId: number | null | undefined,
  userId: number,
  type: ActivityType,
  ipAddress?: string,
) {
  if (teamId === null || teamId === undefined) {
    return;
  }
  const newActivity: NewActivityLog = {
    teamId,
    userId,
    action: type,
    ipAddress: ipAddress || "",
  };
  await db.insert(activityLogs).values(newActivity);
}

const inviteTeamMemberSchema = object({
  email: email("Invalid email address"),
  role: oneOf(["member", "owner"] as const),
});

export const inviteTeamMember = validatedActionWithUser(
  inviteTeamMemberSchema,
  async (data, _, user) => {
    const { email, role } = data;
    const userWithTeam = await getUserWithTeam(user.id);

    if (!userWithTeam?.teamId) {
      return { error: "User is not part of a team" };
    }

    const existingMember = await db
      .select()
      .from(users)
      .leftJoin(teamMembers, eq(users.id, teamMembers.userId))
      .where(and(eq(users.email, email), eq(teamMembers.teamId, userWithTeam.teamId)))
      .limit(1);

    if (existingMember.length > 0) {
      return { error: "User is already a member of this team" };
    }

    // Check if there's an existing invitation
    const existingInvitation = await db
      .select()
      .from(invitations)
      .where(
        and(
          eq(invitations.email, email),
          eq(invitations.teamId, userWithTeam.teamId),
          eq(invitations.status, "pending"),
        ),
      )
      .limit(1);

    if (existingInvitation.length > 0) {
      return { error: "An invitation has already been sent to this email" };
    }

    // Create a new invitation
    await db.insert(invitations).values({
      teamId: userWithTeam.teamId,
      email,
      role,
      invitedBy: user.id,
      status: "pending",
    });

    await logActivity(userWithTeam.teamId, user.id, ActivityType.INVITE_TEAM_MEMBER);

    return { success: "Invitation sent successfully" };
  },
);

const acceptInvitationSchema = object({
  invitationId: positiveInteger("Invalid invitation"),
});

export const acceptInvitation = validatedActionWithUser(
  acceptInvitationSchema,
  async (data, _, user) => {
    const { invitationId } = data;

    return db.transaction(async (tx) => {
      const [invitation] = await tx
        .select()
        .from(invitations)
        .where(and(eq(invitations.id, invitationId), eq(invitations.status, "pending")))
        .limit(1);

      if (!invitation) {
        return { error: "Invitation not found or already accepted" };
      }

      await tx.insert(teamMembers).values({
        userId: user.id,
        teamId: invitation.teamId,
        role: invitation.role,
      });

      await tx
        .update(invitations)
        .set({ status: "accepted" })
        .where(eq(invitations.id, invitation.id));

      return { success: "Invitation accepted" };
    });
  },
);

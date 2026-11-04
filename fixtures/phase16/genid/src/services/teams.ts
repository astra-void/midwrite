import { and, eq } from "drizzle-orm";

import { db } from "../db.js";
import { users, usersOnTeam } from "../schema.js";

// The shape of midday's team-membership queries (`hasTeamAccess`,
// `leaveTeam`, `deleteTeamMember`): what the Phase 16 contract templates for
// midday's two team-membership write skews are validated against. Whether a
// team must keep an owner, or a user's current team must be one of theirs,
// is a business rule the code does not state; it is declared only by a
// contract the developer chooses. (midday passes its replica-set database as
// a parameter; here the primary is imported, which reads the same way.)
export const hasTeamAccess = async (teamId: string, userId: string): Promise<boolean> => {
  const result = await db
    .select({ teamId: usersOnTeam.teamId })
    .from(usersOnTeam)
    .where(and(eq(usersOnTeam.teamId, teamId), eq(usersOnTeam.userId, userId)))
    .limit(1);

  return result.length > 0;
};

type LeaveTeamParams = {
  userId: string;
  teamId: string;
};

export async function leaveTeam(params: LeaveTeamParams) {
  const hasAccess = await hasTeamAccess(params.teamId, params.userId);
  if (!hasAccess) {
    throw new Error("User is not a member of this team");
  }

  await db
    .update(users)
    .set({ teamId: null })
    .where(and(eq(users.id, params.userId), eq(users.teamId, params.teamId)));

  await db
    .delete(usersOnTeam)
    .where(and(eq(usersOnTeam.teamId, params.teamId), eq(usersOnTeam.userId, params.userId)));

  return true;
}

type DeleteTeamMemberParams = {
  userId: string;
  teamId: string;
};

export async function deleteTeamMember(params: DeleteTeamMemberParams) {
  const hasAccess = await hasTeamAccess(params.teamId, params.userId);
  if (!hasAccess) {
    throw new Error("User is not a member of this team");
  }

  await db
    .delete(usersOnTeam)
    .where(and(eq(usersOnTeam.userId, params.userId), eq(usersOnTeam.teamId, params.teamId)));

  return true;
}

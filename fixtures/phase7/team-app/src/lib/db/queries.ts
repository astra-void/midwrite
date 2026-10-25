import { and, eq, isNull } from "drizzle-orm";

import { currentSession } from "../auth/session.js";
import { db } from "./drizzle.js";
import { teamMembers, users } from "./schema.js";

export async function getUser() {
  const session = currentSession();
  if (!session) {
    return null;
  }

  const user = await db
    .select()
    .from(users)
    .where(and(eq(users.id, session.userId), isNull(users.deletedAt)))
    .limit(1);

  if (user.length === 0) {
    return null;
  }

  return user[0];
}

export async function getUserWithTeam(userId: number) {
  const result = await db
    .select({
      user: users,
      teamId: teamMembers.teamId,
    })
    .from(users)
    .leftJoin(teamMembers, eq(users.id, teamMembers.userId))
    .where(eq(users.id, userId))
    .limit(1);

  return result[0];
}

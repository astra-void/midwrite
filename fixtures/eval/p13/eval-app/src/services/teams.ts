import { eq, sql } from "drizzle-orm";

import { db } from "../db.js";
import { teamRepo } from "../repositories/index.js";
import { teamMembers, teams, users } from "../schema/index.js";

// E01 / E03: looks for the owner's team, creates one if there is none.
export async function createTeam(ownerId: string, name: string) {
  const existing = await teamRepo.findByOwner(ownerId);
  if (existing) return existing.id;
  const created = await teamRepo.create(ownerId, name);
  return created.id;
}

// E02: the same check while holding the owner's row FOR UPDATE.
export async function createTeamLocked(ownerId: string, name: string) {
  return db.transaction(async (tx) => {
    const [owner] = await tx.select().from(users).where(eq(users.id, ownerId)).limit(1).for("update");
    if (!owner) throw new Error("unknown owner");
    const [existing] = await tx.select().from(teams).where(eq(teams.ownerId, ownerId)).limit(1);
    if (existing) return existing.id;
    const [created] = await tx.insert(teams).values({ ownerId, name }).returning({ id: teams.id });
    if (!created) throw new Error("team insert failed");
    return created.id;
  });
}

// E04: the same check while holding the owner's row FOR SHARE.
export async function createTeamShared(ownerId: string, name: string) {
  return db.transaction(async (tx) => {
    const [owner] = await tx.select().from(users).where(eq(users.id, ownerId)).limit(1).for("share");
    if (!owner) throw new Error("unknown owner");
    const [existing] = await tx.select().from(teams).where(eq(teams.ownerId, ownerId)).limit(1);
    if (existing) return existing.id;
    const [created] = await tx.insert(teams).values({ ownerId, name }).returning({ id: teams.id });
    if (!created) throw new Error("team insert failed");
    return created.id;
  });
}

// E05: adds the member, then locks the team row to bump its member count.
export async function addMember(teamId: number, userId: string) {
  await db.transaction(async (tx) => {
    await tx.insert(teamMembers).values({ teamId, userId });
    const [team] = await tx.select().from(teams).where(eq(teams.id, teamId)).limit(1).for("update");
    if (!team) throw new Error("unknown team");
    await tx
      .update(teams)
      .set({ memberCount: team.memberCount + 1 })
      .where(eq(teams.id, teamId));
  });
}

// E06: adds the member, then increments the member count in the database.
export async function addMemberCounted(teamId: number, userId: string) {
  await db.transaction(async (tx) => {
    await tx.insert(teamMembers).values({ teamId, userId });
    await tx
      .update(teams)
      .set({ memberCount: sql`${teams.memberCount} + 1` })
      .where(eq(teams.id, teamId));
  });
}

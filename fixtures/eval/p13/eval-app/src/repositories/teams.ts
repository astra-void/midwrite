import { eq } from "drizzle-orm";

import { db } from "../db.js";
import { teams } from "../schema/index.js";

export const teamRepo = {
  async findByOwner(ownerId: string) {
    const [found] = await db.select().from(teams).where(eq(teams.ownerId, ownerId)).limit(1);
    return found;
  },

  async create(ownerId: string, name: string) {
    const [created] = await db.insert(teams).values({ ownerId, name }).returning({ id: teams.id });
    if (!created) throw new Error("team insert failed");
    return created;
  },
};

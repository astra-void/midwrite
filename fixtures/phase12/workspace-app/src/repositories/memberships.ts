import { and, eq } from "drizzle-orm";

import { db, type Database } from "../db/client.js";
import { memberships } from "../db/schema/index.js";

// A class-based repository; the handle is injected (the pool, or a
// transaction).
export class MembershipRepository {
  constructor(private readonly database: Database) {}

  async add(workspaceId: string, userId: string, role: string) {
    await this.database.insert(memberships).values({ workspaceId, userId, role });
  }

  async find(workspaceId: string, userId: string) {
    const [found] = await this.database
      .select()
      .from(memberships)
      .where(and(eq(memberships.workspaceId, workspaceId), eq(memberships.userId, userId)))
      .limit(1);
    return found;
  }
}

export const membershipRepository = new MembershipRepository(db);

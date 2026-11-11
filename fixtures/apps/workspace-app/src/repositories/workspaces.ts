import { eq } from "drizzle-orm";

import { db } from "../db/client.js";
import { workspaces } from "../db/schema/index.js";

export const workspacesRepository = {
  async findByOwner(ownerId: string) {
    const [found] = await db
      .select()
      .from(workspaces)
      .where(eq(workspaces.ownerId, ownerId))
      .limit(1);
    return found;
  },

  async create(name: string, ownerId: string) {
    const [created] = await db
      .insert(workspaces)
      .values({ name, ownerId })
      .returning({ id: workspaces.id });
    return created;
  },

  // The caller chooses the identifier (a request id): a resubmitted request
  // inserts the same key again.
  async createWithId(id: string, name: string, ownerId: string) {
    const [created] = await db
      .insert(workspaces)
      .values({ id, name, ownerId })
      .returning({ id: workspaces.id });
    return created;
  },
};

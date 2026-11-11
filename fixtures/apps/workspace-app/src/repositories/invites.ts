import { and, eq } from "drizzle-orm";

import type { Database } from "../db/client.js";
import { invites } from "../db/schema/index.js";

type Executor = Parameters<Parameters<Database["transaction"]>[0]>[0];

export const invitesRepository = {
  // Runs on the caller's transaction.
  async findPending(tx: Executor, workspaceId: string, email: string) {
    const [found] = await tx
      .select()
      .from(invites)
      .where(
        and(
          eq(invites.workspaceId, workspaceId),
          eq(invites.email, email),
          eq(invites.status, "pending"),
        ),
      )
      .limit(1);
    return found;
  },
};

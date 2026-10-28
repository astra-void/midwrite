import { and, eq } from "drizzle-orm";

import { db } from "../db.js";
import { apiClients } from "../schema/index.js";

// P14-E35: registers a named API client of an account, unless it exists.
// UNIQUE(account_id, name).
export async function registerApiClient(accountId: string, name: string) {
  const [existing] = await db
    .select({ id: apiClients.id })
    .from(apiClients)
    .where(and(eq(apiClients.accountId, accountId), eq(apiClients.name, name)))
    .limit(1);
  if (existing) return existing.id;
  const [created] = await db.insert(apiClients).values({ accountId, name }).returning({ id: apiClients.id });
  if (!created) throw new Error("api client insert failed");
  return created.id;
}

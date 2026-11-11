import { and, eq } from "drizzle-orm";

import { db } from "../db.js";
import { subscriptions } from "../schema/index.js";

// P14-E36: activates a subscription unless the account has an active one.
// A partial unique index allows one row with status 'active' per account.
export async function activateSubscription(accountId: string, plan: string) {
  const [active] = await db
    .select({ id: subscriptions.id })
    .from(subscriptions)
    .where(and(eq(subscriptions.accountId, accountId), eq(subscriptions.status, "active")))
    .limit(1);
  if (active) return active.id;
  const [created] = await db
    .insert(subscriptions)
    .values({ accountId, plan, status: "active" })
    .returning({ id: subscriptions.id });
  if (!created) throw new Error("subscription insert failed");
  return created.id;
}

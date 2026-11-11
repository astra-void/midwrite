import { eq } from "drizzle-orm";

import { replicatedDb } from "../db.js";
import { accounts } from "../schema/index.js";

// P14-E21: saves the display name, reads it back through the replica set and
// tells the profile page whether the change was saved.
export async function saveDisplayName(accountId: string, displayName: string) {
  await replicatedDb.update(accounts).set({ displayName }).where(eq(accounts.id, accountId));
  const [current] = await replicatedDb
    .select({ displayName: accounts.displayName })
    .from(accounts)
    .where(eq(accounts.id, accountId))
    .limit(1);
  return current?.displayName === displayName;
}

// P14-E22: saves the timezone and returns the stored timezone, read back
// through the replica set, for the page to show.
export async function saveTimezone(accountId: string, timezone: string) {
  await replicatedDb.update(accounts).set({ timezone }).where(eq(accounts.id, accountId));
  const [current] = await replicatedDb
    .select({ timezone: accounts.timezone })
    .from(accounts)
    .where(eq(accounts.id, accountId))
    .limit(1);
  return current?.timezone;
}

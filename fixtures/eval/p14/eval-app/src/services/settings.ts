import { eq, sql } from "drizzle-orm";

import { db } from "../db.js";
import { accounts, type AccountSettings } from "../schema/index.js";

export type SettingsPatch = Partial<Omit<AccountSettings, "plan">>;

// P14-E29: reads the settings document, merges the patch in the application,
// writes the whole document back.
export async function updateSettings(accountId: string, patch: SettingsPatch) {
  const [account] = await db
    .select({ settings: accounts.settings })
    .from(accounts)
    .where(eq(accounts.id, accountId))
    .limit(1);
  if (!account) throw new Error("unknown account");
  const next: AccountSettings = { ...account.settings, ...patch };
  await db.update(accounts).set({ settings: next }).where(eq(accounts.id, accountId));
  return next;
}

// P14-E30: merges the patch in the database (jsonb || jsonb) in one UPDATE.
export async function updateSettingsAtomic(accountId: string, patch: SettingsPatch) {
  const updated = await db
    .update(accounts)
    .set({ settings: sql`${accounts.settings} || ${JSON.stringify(patch)}::jsonb` })
    .where(eq(accounts.id, accountId))
    .returning({ id: accounts.id });
  return updated.length > 0;
}

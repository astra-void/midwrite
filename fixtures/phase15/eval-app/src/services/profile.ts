import { and, eq, ne, sql } from "drizzle-orm";

import { db, replicatedDb } from "../db.js";
import { profiles, type Theme } from "../schema/index.js";

// P15-E01: saves the nickname, reads it back through the replica set and
// tells the caller whether the change was saved.
export async function saveNickname(accountId: string, nickname: string) {
  await replicatedDb.update(profiles).set({ nickname }).where(eq(profiles.accountId, accountId));
  const [current] = await replicatedDb
    .select({ nickname: profiles.nickname })
    .from(profiles)
    .where(eq(profiles.accountId, accountId))
    .limit(1);
  if (!current) throw new Error("no profile");
  return current.nickname === nickname;
}

// P15-E02: the same, with the write and the read-back in one transaction.
export async function saveNicknameInTransaction(accountId: string, nickname: string) {
  return replicatedDb.transaction(async (tx) => {
    await tx.update(profiles).set({ nickname }).where(eq(profiles.accountId, accountId));
    const [current] = await tx
      .select({ nickname: profiles.nickname })
      .from(profiles)
      .where(eq(profiles.accountId, accountId))
      .limit(1);
    if (!current) throw new Error("no profile");
    return current.nickname === nickname;
  });
}

// P15-E03: switches the theme and counts the change, unless the profile
// already has the requested theme.
export async function setTheme(accountId: string, theme: Theme) {
  const [profile] = await db
    .select({ theme: profiles.theme })
    .from(profiles)
    .where(eq(profiles.accountId, accountId))
    .limit(1);
  if (!profile) throw new Error("no profile");
  if (profile.theme === theme) return false;
  await db
    .update(profiles)
    .set({ theme, themeChanges: sql`${profiles.themeChanges} + 1` })
    .where(eq(profiles.accountId, accountId));
  return true;
}

// P15-E04: the same decision taken by one conditional UPDATE.
export async function setThemeAtomic(accountId: string, theme: Theme) {
  const changed = await db
    .update(profiles)
    .set({ theme, themeChanges: sql`${profiles.themeChanges} + 1` })
    .where(and(eq(profiles.accountId, accountId), ne(profiles.theme, theme)))
    .returning({ accountId: profiles.accountId });
  return changed.length > 0;
}

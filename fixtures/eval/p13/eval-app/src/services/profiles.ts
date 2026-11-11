import { eq } from "drizzle-orm";

import { replicatedDb } from "../db.js";
import { users } from "../schema/index.js";

// E16: renames, then reads the profile back for the response.
export async function renameAndRead(userId: string, name: string) {
  await replicatedDb.update(users).set({ name }).where(eq(users.id, userId));
  const [user] = await replicatedDb.select({ name: users.name }).from(users).where(eq(users.id, userId)).limit(1);
  return user?.name;
}

// E17: the same, reading back from the primary.
export async function renameAndReadPrimary(userId: string, name: string) {
  await replicatedDb.update(users).set({ name }).where(eq(users.id, userId));
  const [user] = await replicatedDb.$primary
    .select({ name: users.name })
    .from(users)
    .where(eq(users.id, userId))
    .limit(1);
  return user?.name;
}

// E18: the same inside a transaction of the replica set.
export async function renameInTransaction(userId: string, name: string) {
  return replicatedDb.transaction(async (tx) => {
    await tx.update(users).set({ name }).where(eq(users.id, userId));
    const [user] = await tx.select({ name: users.name }).from(users).where(eq(users.id, userId)).limit(1);
    return user?.name;
  });
}

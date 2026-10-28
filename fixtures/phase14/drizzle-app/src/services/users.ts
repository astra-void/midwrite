import { and, eq, isNull } from "drizzle-orm";

import { db } from "../db/client.js";
import { counters, settings, users } from "../db/schema.js";

// A job handler: welcome a user once.
export async function welcome(userId: number) {
  const [user] = await db.select().from(users).where(eq(users.id, userId));
  if (!user || user.welcomedAt !== null) return false;
  await db.update(users).set({ welcomedAt: new Date() }).where(eq(users.id, userId));
  return true;
}

// The same with the check in the statement.
export async function welcomeOnce(userId: number) {
  const rows = await db
    .update(users)
    .set({ welcomedAt: new Date() })
    .where(and(eq(users.id, userId), isNull(users.welcomedAt)))
    .returning();
  return rows.length > 0;
}

// An upsert that stamps the counter.
export async function touch(name: string) {
  await db
    .insert(counters)
    .values({ name, hits: 1, updatedAt: new Date() })
    .onConflictDoUpdate({ target: counters.name, set: { hits: 2, updatedAt: new Date() } });
}

// Settings stored as jsonb and read back.
export async function saveSettings(userId: number, theme: string) {
  await db.insert(settings).values({ userId, prefs: { theme, beta: true } });
  const [row] = await db.select().from(settings).where(eq(settings.userId, userId));
  return row;
}

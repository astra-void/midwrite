import { and, eq } from "drizzle-orm";

import { db } from "../db.js";
import { newRefreshToken } from "../lib/ids.js";
import { sessions, tokenRotations } from "../schema/index.js";

export type Refreshed = { sessionId: string; refreshToken: string };

// P16-E32: exchanges a refresh token for a freshly generated one (refresh
// token rotation). Returns null for an unknown token.
export async function refreshSession(presentedToken: string): Promise<Refreshed | null> {
  const [session] = await db
    .select({ id: sessions.id })
    .from(sessions)
    .where(eq(sessions.refreshToken, presentedToken))
    .limit(1);
  if (!session) return null;

  const nextToken = newRefreshToken();
  await db.update(sessions).set({ refreshToken: nextToken }).where(eq(sessions.id, session.id));
  await db
    .insert(tokenRotations)
    .values({ sessionId: session.id, previousToken: presentedToken, issuedToken: nextToken });
  return { sessionId: session.id, refreshToken: nextToken };
}

// P16-E33: the same; the UPDATE replaces the token only if it is still the
// presented one.
export async function refreshSessionChecked(presentedToken: string): Promise<Refreshed | null> {
  const [session] = await db
    .select({ id: sessions.id })
    .from(sessions)
    .where(eq(sessions.refreshToken, presentedToken))
    .limit(1);
  if (!session) return null;

  const nextToken = newRefreshToken();
  const rotated = await db
    .update(sessions)
    .set({ refreshToken: nextToken })
    .where(and(eq(sessions.id, session.id), eq(sessions.refreshToken, presentedToken)))
    .returning({ id: sessions.id });
  if (rotated.length === 0) return null;

  await db
    .insert(tokenRotations)
    .values({ sessionId: session.id, previousToken: presentedToken, issuedToken: nextToken });
  return { sessionId: session.id, refreshToken: nextToken };
}

// A write whose `RETURNING` row is bound changed nothing on a path where the
// row is missing: `UPDATE ... RETURNING` and `DELETE ... RETURNING` return
// exactly the rows they changed, and an insert that skips conflicts returns
// nothing when it skipped. The write before each `!row` arm, or before the
// caller's test of a helper that returns the row (`return token ?? null`),
// commits nothing there, so no path commits two changes.
// @expect none
import { db } from "@/db";
import { tokens, attempts, links } from "@/db/schema";
import { and, eq, isNull } from "drizzle-orm";

export async function claimOrLog(id: string) {
  const [row] = await db
    .update(tokens)
    .set({ used: 1 })
    .where(and(eq(tokens.id, id), isNull(tokens.usedAt)))
    .returning();
  if (!row) {
    await db.insert(attempts).values({ id });
  }
}

async function consume(code: string) {
  const [token] = await db.update(tokens).set({ used: 1 }).where(eq(tokens.code, code)).returning();
  return token ?? null;
}

export async function redeem(code: string) {
  const token = await consume(code);
  if (token) {
    return token.id;
  }
  await db.insert(attempts).values({ code });
  return null;
}

export async function linkOnce(id: string) {
  const [row] = await db.insert(links).values({ id }).onConflictDoNothing().returning();
  if (!row) {
    await db.insert(attempts).values({ id });
  }
}

export async function removeOrLog(id: string) {
  const [gone] = await db.delete(links).where(eq(links.id, id)).returning();
  if (!gone) {
    await db.insert(attempts).values({ id });
  }
}

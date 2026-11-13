// Writes with a `RETURNING` row that still commit: the row is present on
// the path, the result is never tested, an upsert returns rows it did not
// insert, a helper may return null although it wrote, and an array
// callback writes once per element (one element's empty result says
// nothing about another's).
// @expect MW1004 line=21 confidence=medium function=claimThenGrant
// @expect MW1004 line=26 confidence=high function=untested
// @expect MW1004 line=36 confidence=medium function=upsert
// @expect MW1004 line=49 confidence=medium function=helperMayHideWrite
// @expect MW1004 line=55 confidence=low function=perElement
// @expect MW1004 line=58 confidence=medium function=perElement::<anonymous@55:13>
import { db } from "@/db";
import { tokens, grants, attempts } from "@/db/schema";
import { eq } from "drizzle-orm";

export async function claimThenGrant(id: string) {
  const [row] = await db.update(tokens).set({ used: 1 }).where(eq(tokens.id, id)).returning();
  if (!row) {
    return;
  }
  await db.insert(grants).values({ id: row.id });
}

export async function untested(id: string) {
  const [row] = await db.update(tokens).set({ used: 1 }).where(eq(tokens.id, id)).returning();
  await db.insert(grants).values({ id, ref: row?.id });
}

export async function upsert(id: string) {
  const [row] = await db
    .insert(tokens)
    .values({ id })
    .onConflictDoUpdate({ target: tokens.id, set: { used: 1 } })
    .returning();
  if (!row) {
    await db.insert(attempts).values({ id });
  }
}

async function consumeUnlessStrict(code: string, strict: boolean) {
  const [token] = await db.update(tokens).set({ used: 1 }).where(eq(tokens.code, code)).returning();
  if (strict) return null;
  return token ?? null;
}

export async function helperMayHideWrite(code: string, strict: boolean) {
  const token = await consumeUnlessStrict(code, strict);
  if (!token) {
    await db.insert(attempts).values({ code });
  }
}

export async function perElement(ids: string[]) {
  await Promise.all(
    ids.map(async (id) => {
      const [row] = await db.insert(grants).values({ id }).onConflictDoNothing().returning();
      if (!row) return null;
      await db.insert(attempts).values({ id });
      return row;
    }),
  );
}

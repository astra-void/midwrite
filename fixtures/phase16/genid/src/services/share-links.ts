import { and, eq, isNull } from "drizzle-orm";
import { nanoid } from "nanoid";

import { db } from "../db.js";
import { shareLinks } from "../schema.js";

// G1 (defect): returns the link's token, generating it on first use (the
// shape of midday's `toggleCustomerPortal`: read, keep the stored token or
// generate one, write it back). Two concurrent first calls both read no
// token, both generate one and both write; the last write wins, and the
// other caller is told a token that is not stored (its link does not work).
export async function ensureShareToken(linkId: number) {
  const [link] = await db
    .select({ token: shareLinks.token })
    .from(shareLinks)
    .where(eq(shareLinks.id, linkId))
    .limit(1);
  if (!link) {
    throw new Error("link not found");
  }
  const token = link.token ? link.token : nanoid(21);
  await db.update(shareLinks).set({ token }).where(eq(shareLinks.id, linkId));
  return { token };
}

// G1-safe: one conditional UPDATE stores a token only while none is stored
// (the first caller's wins; a token, once stored, is never replaced), then
// every caller reads the stored token back from the primary and returns it.
export async function ensureShareTokenAtomic(linkId: number) {
  const candidate = nanoid(21);
  await db
    .update(shareLinks)
    .set({ token: candidate })
    .where(and(eq(shareLinks.id, linkId), isNull(shareLinks.token)));
  const [link] = await db
    .select({ token: shareLinks.token })
    .from(shareLinks)
    .where(eq(shareLinks.id, linkId))
    .limit(1);
  if (!link) {
    throw new Error("link not found");
  }
  return { token: link.token };
}

import { eq } from "drizzle-orm";
import { db } from "./db";
import { redemptions } from "./schema";

async function alreadyRedeemed(code: string) {
  const row = await db.query.redemptions.findFirst({
    where: eq(redemptions.code, code),
  });
  return row !== undefined;
}

async function redeem(code: string, id: string) {
  await db.insert(redemptions).values({ id, code });
}

// A check-then-act race on a shared redemption code.
export async function redeemCode(code: string, id: string) {
  if (!(await alreadyRedeemed(code))) {
    await redeem(code, id);
  }
}

// The atomic alternative: let the database's primary key arbitrate.
export async function redeemCodeAtomically(code: string) {
  await db.insert(redemptions).values({ id: code, code }).onConflictDoNothing();
}

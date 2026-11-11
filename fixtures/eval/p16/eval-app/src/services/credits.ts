import { eq, sql } from "drizzle-orm";

import { db } from "../db.js";
import { isConnectionError } from "../lib/pg-errors.js";
import { creditBalances, creditEntries, creditRequests } from "../schema/index.js";

const MAX_ATTEMPTS = 4;

// P16-E11: grants credits to an account: one transaction raises the balance
// and records the grant. Retried while the connection fails.
export async function grantCredits(accountId: string, amount: number, requestId: string) {
  let attempt = 0;
  while (attempt < MAX_ATTEMPTS) {
    attempt += 1;
    try {
      await db.transaction(async (tx) => {
        await tx
          .update(creditBalances)
          .set({ credits: sql`${creditBalances.credits} + ${amount}` })
          .where(eq(creditBalances.accountId, accountId));
        await tx.insert(creditEntries).values({ accountId, requestId, amount });
      });
      return;
    } catch (error) {
      if (!isConnectionError(error) || attempt >= MAX_ATTEMPTS) throw error;
    }
  }
}

// P16-E12: the same; the transaction first claims the request id.
export async function grantCreditsOnce(accountId: string, amount: number, requestId: string) {
  let attempt = 0;
  while (attempt < MAX_ATTEMPTS) {
    attempt += 1;
    try {
      await db.transaction(async (tx) => {
        const claimed = await tx
          .insert(creditRequests)
          .values({ requestId, accountId })
          .onConflictDoNothing()
          .returning({ requestId: creditRequests.requestId });
        if (claimed.length === 0) return;
        await tx
          .update(creditBalances)
          .set({ credits: sql`${creditBalances.credits} + ${amount}` })
          .where(eq(creditBalances.accountId, accountId));
        await tx.insert(creditEntries).values({ accountId, requestId, amount });
      });
      return;
    } catch (error) {
      if (!isConnectionError(error) || attempt >= MAX_ATTEMPTS) throw error;
    }
  }
}

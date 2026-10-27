import { and, eq, gte, sql } from "drizzle-orm";

import { db } from "../db.js";
import { taskRepo } from "../repositories/index.js";
import { wallets } from "../schema/index.js";

// E5: read, check, write back a computed balance.
export async function spendCredits(accountId: string, amount: number) {
  const wallet = await taskRepo.wallet(db, accountId);
  if (!wallet || wallet.balance < amount) return false;
  await db
    .update(wallets)
    .set({ balance: wallet.balance - amount })
    .where(eq(wallets.accountId, accountId));
  return true;
}

// E6: the database checks and decrements.
export async function spendCreditsAtomic(accountId: string, amount: number) {
  const rows = await db
    .update(wallets)
    .set({ balance: sql`${wallets.balance} - ${amount}` })
    .where(and(eq(wallets.accountId, accountId), gte(wallets.balance, amount)))
    .returning();
  return rows.length > 0;
}

// E8: under a row lock in a transaction, through a repository that takes
// the transaction handle.
export async function transferCredits(from: string, to: string, amount: number) {
  return db.transaction(async (tx) => {
    const source = await taskRepo.lockedWallet(tx, from);
    if (!source || source.balance < amount) return false;
    await tx.update(wallets).set({ balance: source.balance - amount }).where(eq(wallets.accountId, from));
    await tx
      .update(wallets)
      .set({ balance: sql`${wallets.balance} + ${amount}` })
      .where(eq(wallets.accountId, to));
    return true;
  });
}

// E9: the same without the lock.
export async function transferCreditsUnlocked(from: string, to: string, amount: number) {
  return db.transaction(async (tx) => {
    const source = await taskRepo.wallet(tx, from);
    if (!source || source.balance < amount) return false;
    await tx.update(wallets).set({ balance: source.balance - amount }).where(eq(wallets.accountId, from));
    await tx
      .update(wallets)
      .set({ balance: sql`${wallets.balance} + ${amount}` })
      .where(eq(wallets.accountId, to));
    return true;
  });
}

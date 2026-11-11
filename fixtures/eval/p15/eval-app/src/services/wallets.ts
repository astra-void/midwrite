import { eq, sql } from "drizzle-orm";

import { db } from "../db.js";
import { wallets } from "../schema/index.js";

// P15-E10: moves an amount between two wallets: locks the source wallet,
// then the destination wallet, then updates both.
export async function transfer(fromId: number, toId: number, amount: number) {
  await db.transaction(async (tx) => {
    const [from] = await tx.select({ id: wallets.id }).from(wallets).where(eq(wallets.id, fromId)).for("update");
    const [to] = await tx.select({ id: wallets.id }).from(wallets).where(eq(wallets.id, toId)).for("update");
    if (!from || !to) throw new Error("no wallet");
    await tx
      .update(wallets)
      .set({ balance: sql`${wallets.balance} - ${amount}` })
      .where(eq(wallets.id, fromId));
    await tx
      .update(wallets)
      .set({ balance: sql`${wallets.balance} + ${amount}` })
      .where(eq(wallets.id, toId));
  });
}

// P15-E11: the same, locking the wallet with the smaller id first.
export async function transferOrdered(fromId: number, toId: number, amount: number) {
  const firstId = fromId < toId ? fromId : toId;
  const secondId = fromId < toId ? toId : fromId;
  await db.transaction(async (tx) => {
    const [first] = await tx.select({ id: wallets.id }).from(wallets).where(eq(wallets.id, firstId)).for("update");
    const [second] = await tx.select({ id: wallets.id }).from(wallets).where(eq(wallets.id, secondId)).for("update");
    if (!first || !second) throw new Error("no wallet");
    await tx
      .update(wallets)
      .set({ balance: sql`${wallets.balance} - ${amount}` })
      .where(eq(wallets.id, fromId));
    await tx
      .update(wallets)
      .set({ balance: sql`${wallets.balance} + ${amount}` })
      .where(eq(wallets.id, toId));
  });
}

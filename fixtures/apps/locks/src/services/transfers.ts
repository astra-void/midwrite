import { eq, inArray, sql } from "drizzle-orm";

import { db } from "../db.js";
import { accounts } from "../schema.js";

// Locks the source account, then the destination account, each with its
// own statement (argument order). Two crossed transfers can each hold one
// account and wait for the other: PostgreSQL aborts one with 40P01 and that
// transfer fails.
export async function transfer(fromId: number, toId: number, amount: number) {
  await db.transaction(async (tx) => {
    const [from] = await tx.select().from(accounts).where(eq(accounts.id, fromId)).for("update");
    const [to] = await tx.select().from(accounts).where(eq(accounts.id, toId)).for("update");
    if (!from || !to) throw new Error("no account");
    await tx
      .update(accounts)
      .set({ balance: sql`${accounts.balance} - ${amount}` })
      .where(eq(accounts.id, fromId));
    await tx
      .update(accounts)
      .set({ balance: sql`${accounts.balance} + ${amount}` })
      .where(eq(accounts.id, toId));
  });
}

// Locks both accounts with one statement ordered by the key: every transfer
// takes its locks in the same order, so no wait cycle forms.
export async function transferLocked(fromId: number, toId: number, amount: number) {
  if (fromId === toId) throw new Error("same account");
  await db.transaction(async (tx) => {
    const locked = await tx
      .select()
      .from(accounts)
      .where(inArray(accounts.id, [fromId, toId]))
      .orderBy(accounts.id)
      .for("update");
    if (locked.length !== 2) throw new Error("no account");
    await tx
      .update(accounts)
      .set({ balance: sql`${accounts.balance} - ${amount}` })
      .where(eq(accounts.id, fromId));
    await tx
      .update(accounts)
      .set({ balance: sql`${accounts.balance} + ${amount}` })
      .where(eq(accounts.id, toId));
  });
}

// The same without `.orderBy`: PostgreSQL locks in its plan's order (key
// order for an index scan, physical order for a bitmap or sequential
// scan); the model assumes key order and says so.
export async function transferLockedUnordered(fromId: number, toId: number, amount: number) {
  if (fromId === toId) throw new Error("same account");
  await db.transaction(async (tx) => {
    const locked = await tx
      .select()
      .from(accounts)
      .where(inArray(accounts.id, [fromId, toId]))
      .for("update");
    if (locked.length !== 2) throw new Error("no account");
    await tx
      .update(accounts)
      .set({ balance: sql`${accounts.balance} - ${amount}` })
      .where(eq(accounts.id, fromId));
    await tx
      .update(accounts)
      .set({ balance: sql`${accounts.balance} + ${amount}` })
      .where(eq(accounts.id, toId));
  });
}

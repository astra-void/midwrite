import { asc, eq, inArray, sql } from "drizzle-orm";

import { db } from "../db.js";
import { assertPositiveAmount, InsufficientFundsError } from "../lib/pg-errors.js";
import { ledgerAccounts, ledgerTransfers } from "../schema/index.js";

function assertTransfer(fromId: number, toId: number, amount: number) {
  assertPositiveAmount(amount);
  if (fromId === toId) throw new RangeError("cannot transfer to the same account");
}

// P16-E23: moves an amount between two ledger accounts in one transaction,
// refusing to overdraw the source.
export async function transferFundsUnlocked(fromId: number, toId: number, amount: number) {
  assertTransfer(fromId, toId, amount);
  const ids = [fromId, toId].sort((a, b) => a - b);
  return db.transaction(async (tx) => {
    const rows = await tx
      .select({ id: ledgerAccounts.id, balance: ledgerAccounts.balance })
      .from(ledgerAccounts)
      .where(inArray(ledgerAccounts.id, ids))
      .orderBy(asc(ledgerAccounts.id));
    const from = rows.find((row) => row.id === fromId);
    const to = rows.find((row) => row.id === toId);
    if (!from || !to) throw new Error("unknown ledger account");
    if (from.balance < amount) throw new InsufficientFundsError(fromId);

    await tx
      .update(ledgerAccounts)
      .set({ balance: sql`${ledgerAccounts.balance} - ${amount}` })
      .where(eq(ledgerAccounts.id, fromId));
    await tx
      .update(ledgerAccounts)
      .set({ balance: sql`${ledgerAccounts.balance} + ${amount}` })
      .where(eq(ledgerAccounts.id, toId));
    const [transfer] = await tx
      .insert(ledgerTransfers)
      .values({ fromId, toId, amount })
      .returning({ id: ledgerTransfers.id });
    return transfer?.id ?? null;
  });
}

// P16-E24: the same, locking both accounts in ascending id order with one
// SELECT ... FOR UPDATE before the balance check.
export async function transferFunds(fromId: number, toId: number, amount: number) {
  assertTransfer(fromId, toId, amount);
  const ids = [fromId, toId].sort((a, b) => a - b);
  return db.transaction(async (tx) => {
    const rows = await tx
      .select({ id: ledgerAccounts.id, balance: ledgerAccounts.balance })
      .from(ledgerAccounts)
      .where(inArray(ledgerAccounts.id, ids))
      .orderBy(asc(ledgerAccounts.id))
      .for("update");
    const from = rows.find((row) => row.id === fromId);
    const to = rows.find((row) => row.id === toId);
    if (!from || !to) throw new Error("unknown ledger account");
    if (from.balance < amount) throw new InsufficientFundsError(fromId);

    await tx
      .update(ledgerAccounts)
      .set({ balance: sql`${ledgerAccounts.balance} - ${amount}` })
      .where(eq(ledgerAccounts.id, fromId));
    await tx
      .update(ledgerAccounts)
      .set({ balance: sql`${ledgerAccounts.balance} + ${amount}` })
      .where(eq(ledgerAccounts.id, toId));
    const [transfer] = await tx
      .insert(ledgerTransfers)
      .values({ fromId, toId, amount })
      .returning({ id: ledgerTransfers.id });
    return transfer?.id ?? null;
  });
}

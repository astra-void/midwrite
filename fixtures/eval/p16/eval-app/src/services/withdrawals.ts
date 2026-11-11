import { eq, sql } from "drizzle-orm";

import { db } from "../db.js";
import {
  assertPositiveAmount,
  backoffMs,
  delay,
  InsufficientFundsError,
  isSerializationFailure,
} from "../lib/pg-errors.js";
import { outboxEvents, wallets } from "../schema/index.js";

const MAX_ATTEMPTS = 4;

// P16-E15: debits a wallet and queues a "withdrawal.requested" event for the
// payout service through the outbox. SERIALIZABLE, retried on 40001.
export async function requestWithdrawal(walletId: number, amount: number, requestId: string) {
  assertPositiveAmount(amount);
  for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
    try {
      await db.transaction(
        async (tx) => {
          const [wallet] = await tx
            .select({ balance: wallets.balance })
            .from(wallets)
            .where(eq(wallets.id, walletId))
            .limit(1);
          if (!wallet) throw new Error(`unknown wallet ${walletId}`);
          if (wallet.balance < amount) throw new InsufficientFundsError(walletId);
          await db.insert(outboxEvents).values({ requestId, topic: "withdrawal.requested", walletId, amount });
          await tx
            .update(wallets)
            .set({ balance: sql`${wallets.balance} - ${amount}` })
            .where(eq(wallets.id, walletId));
        },
        { isolationLevel: "serializable" },
      );
      return;
    } catch (error) {
      if (isSerializationFailure(error) && attempt + 1 < MAX_ATTEMPTS) {
        await delay(backoffMs(attempt));
        continue;
      }
      throw error;
    }
  }
}

// P16-E16: the same, with the outbox row written on the transaction.
export async function requestWithdrawalTx(walletId: number, amount: number, requestId: string) {
  assertPositiveAmount(amount);
  for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
    try {
      await db.transaction(
        async (tx) => {
          const [wallet] = await tx
            .select({ balance: wallets.balance })
            .from(wallets)
            .where(eq(wallets.id, walletId))
            .limit(1);
          if (!wallet) throw new Error(`unknown wallet ${walletId}`);
          if (wallet.balance < amount) throw new InsufficientFundsError(walletId);
          await tx.insert(outboxEvents).values({ requestId, topic: "withdrawal.requested", walletId, amount });
          await tx
            .update(wallets)
            .set({ balance: sql`${wallets.balance} - ${amount}` })
            .where(eq(wallets.id, walletId));
        },
        { isolationLevel: "serializable" },
      );
      return;
    } catch (error) {
      if (isSerializationFailure(error) && attempt + 1 < MAX_ATTEMPTS) {
        await delay(backoffMs(attempt));
        continue;
      }
      throw error;
    }
  }
}

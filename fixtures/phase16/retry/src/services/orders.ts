import { eq } from "drizzle-orm";

import { db } from "../db/client.js";
import { accounts, orders, payments } from "../db/schema.js";

// Attempts of a database call before its error reaches the caller.
const MAX_ATTEMPTS = 3;

// Records an order, running the insert again when it fails. Defect: when
// the insert committed on PostgreSQL but its acknowledgment was lost (a
// broken connection), the driver reports an error and the next attempt
// inserts a second row for the same request.
export async function placeOrder(requestId: string, customerId: number, amount: number) {
  for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
    try {
      await db.insert(orders).values({ requestId, customerId, amount });
      return "placed";
    } catch (error) {
      if (attempt === MAX_ATTEMPTS - 1) throw error;
    }
  }
  return "not placed";
}

// The same retry loop around an idempotent insert (in a transaction, whose
// COMMIT acknowledgment can be lost the same way): every attempt reuses the
// caller's idempotency key, and UNIQUE(idempotency_key) with ON CONFLICT DO
// NOTHING makes a repeated attempt change nothing. Safe.
export async function recordPayment(idempotencyKey: string, orderId: number, amount: number) {
  for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
    try {
      await db.transaction(async (tx) => {
        await tx.insert(payments).values({ idempotencyKey, orderId, amount }).onConflictDoNothing();
      });
      return "recorded";
    } catch (error) {
      if (attempt === MAX_ATTEMPTS - 1) throw error;
    }
  }
  return "not recorded";
}

// Whether a caught error is PostgreSQL's serialization failure. drizzle-orm
// 0.45 wraps the driver's error in `DrizzleQueryError`: the SQLSTATE is the
// `code` of its `cause`.
function isSerializationFailure(error: unknown): boolean {
  const cause = (error as { cause?: { code?: string } }).cause;
  return cause?.code === "40001";
}

// Withdraws from an account in a SERIALIZABLE transaction, running the
// whole transaction again when PostgreSQL reports a serialization failure
// (SQLSTATE 40001); any other error reaches the caller. Safe: a retried
// attempt reads the balance again.
export async function withdraw(accountId: number, amount: number) {
  let attempt = 0;
  while (attempt < MAX_ATTEMPTS) {
    attempt++;
    try {
      return await db.transaction(
        async (tx) => {
          const [account] = await tx
            .select({ balance: accounts.balance })
            .from(accounts)
            .where(eq(accounts.id, accountId))
            .limit(1);
          if (!account || account.balance < amount) throw new Error("insufficient funds");
          await tx
            .update(accounts)
            .set({ balance: account.balance - amount })
            .where(eq(accounts.id, accountId));
          return account.balance - amount;
        },
        { isolationLevel: "serializable" },
      );
    } catch (error) {
      if (!isSerializationFailure(error) || attempt === MAX_ATTEMPTS) throw error;
    }
  }
  throw new Error("withdrawal not completed");
}

// The same loop testing the caught error's own `code`. Defect: drizzle-orm
// 0.45 wraps the driver's error in `DrizzleQueryError`, whose `code` is
// undefined, so a serialization failure is never retried and reaches the
// caller.
export async function withdrawOnCode(accountId: number, amount: number) {
  let attempt = 0;
  while (attempt < MAX_ATTEMPTS) {
    attempt++;
    try {
      return await db.transaction(
        async (tx) => {
          const [account] = await tx
            .select({ balance: accounts.balance })
            .from(accounts)
            .where(eq(accounts.id, accountId))
            .limit(1);
          if (!account || account.balance < amount) throw new Error("insufficient funds");
          await tx
            .update(accounts)
            .set({ balance: account.balance - amount })
            .where(eq(accounts.id, accountId));
          return account.balance - amount;
        },
        { isolationLevel: "serializable" },
      );
    } catch (error) {
      if ((error as { code?: string }).code !== "40001" || attempt === MAX_ATTEMPTS) throw error;
    }
  }
  throw new Error("withdrawal not completed");
}

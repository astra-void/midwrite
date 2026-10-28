import { and, eq } from "drizzle-orm";

import { db } from "../db/client.js";
import { orders, payments } from "../db/schema.js";
import { withRetry } from "../lib/retry.js";

// Records the payment and marks the order paid, retried on any error.
export async function settlePayment(orderId: number, amount: number): Promise<void> {
  await withRetry(() =>
    db.transaction(async (tx) => {
      await tx.insert(payments).values({ orderId, amount });
      await tx.update(orders).set({ status: "paid" }).where(eq(orders.id, orderId));
    }),
  );
}

// The pending -> paid transition gates the payment, retried on any error.
export async function settlePaymentOnce(orderId: number, amount: number): Promise<boolean> {
  return withRetry(() =>
    db.transaction(async (tx) => {
      const claimed = await tx
        .update(orders)
        .set({ status: "paid" })
        .where(and(eq(orders.id, orderId), eq(orders.status, "pending")))
        .returning({ id: orders.id });
      if (claimed.length === 0) return false;
      await tx.insert(payments).values({ orderId, amount });
      return true;
    }),
  );
}

import { and, eq } from "drizzle-orm";

import { db } from "../db.js";
import { withRetry } from "../lib/retry.js";
import { orderRepo } from "../repositories/index.js";
import { orders, payments } from "../schema/index.js";

// E12: POST /checkout. One INSERT; the response carries the new order id.
export async function placeOrder(customerId: string, cartId: string, total: number) {
  const created = await orderRepo.insert({ customerId, cartId, total });
  if (!created) throw new Error("order insert failed");
  return created.id;
}

// E13: the same with the client's checkout id as the order's key.
export async function placeOrderIdempotent(checkoutId: string, customerId: string, cartId: string, total: number) {
  const created = await orderRepo.insertWithId(checkoutId, { customerId, cartId, total });
  if (created) return created.id;
  const existing = await orderRepo.findById(checkoutId);
  if (!existing) throw new Error("order insert failed");
  return existing.id;
}

// E14: records the payment and marks the order paid, retried on any error.
export async function settleOrder(orderId: string, amount: number) {
  await withRetry(() =>
    db.transaction(async (tx) => {
      await tx.insert(payments).values({ orderId, amount });
      await tx.update(orders).set({ status: "paid" }).where(eq(orders.id, orderId));
    }),
  );
}

// E15: the pending -> paid transition gates the payment, retried on any error.
export async function settleOrderOnce(orderId: string, amount: number) {
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

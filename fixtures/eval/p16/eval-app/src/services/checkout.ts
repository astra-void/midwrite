import { eq } from "drizzle-orm";

import { db } from "../db.js";
import { newPaymentIntentId } from "../lib/ids.js";
import { orders, paymentIntents } from "../schema/index.js";

// P16-E30: returns the order's payment intent id, creating the intent (with
// a freshly generated id) on the first call. The checkout page passes the
// returned id to the payment form.
export async function ensurePaymentIntent(orderId: number): Promise<string> {
  const [order] = await db
    .select({ totalCents: orders.totalCents, paymentIntentId: orders.paymentIntentId })
    .from(orders)
    .where(eq(orders.id, orderId))
    .limit(1);
  if (!order) throw new Error(`unknown order ${orderId}`);
  if (order.paymentIntentId) return order.paymentIntentId;

  const intentId = newPaymentIntentId();
  await db.insert(paymentIntents).values({ id: intentId, orderId, amountCents: order.totalCents });
  await db.update(orders).set({ paymentIntentId: intentId }).where(eq(orders.id, orderId));
  return intentId;
}

// P16-E31: the same, in one transaction that locks the order row first.
export async function ensurePaymentIntentLocked(orderId: number): Promise<string> {
  return db.transaction(async (tx) => {
    const [order] = await tx
      .select({ totalCents: orders.totalCents, paymentIntentId: orders.paymentIntentId })
      .from(orders)
      .where(eq(orders.id, orderId))
      .for("update");
    if (!order) throw new Error(`unknown order ${orderId}`);
    if (order.paymentIntentId) return order.paymentIntentId;

    const intentId = newPaymentIntentId();
    await tx.insert(paymentIntents).values({ id: intentId, orderId, amountCents: order.totalCents });
    await tx.update(orders).set({ paymentIntentId: intentId }).where(eq(orders.id, orderId));
    return intentId;
  });
}

// Checkout: charge the order's amount through the payment provider and mark
// the order paid. A client that gets no response (the server crashed, the
// connection dropped) submits the same request again.
import { eq } from "drizzle-orm";

import { db } from "../lib/db/drizzle.js";
import { idempotencyKeys, orders, paymentAttempts } from "../lib/db/schema.js";
import { chargeCard } from "../lib/payments/provider.js";

export interface PayRequest {
  requestId: string;
  orderId: number;
}

/** Records the attempt, charges, marks the order paid. No idempotency. */
export async function payOrder(input: PayRequest) {
  const [order] = await db.select().from(orders).where(eq(orders.id, input.orderId)).limit(1);
  if (!order) {
    throw new Error("no such order");
  }
  if (order.status === "paid") {
    return { status: "already-paid" };
  }
  await db.insert(paymentAttempts).values({ orderId: order.id, amount: order.amount });
  await chargeCard({ orderId: order.id, amount: order.amount });
  await db.update(orders).set({ status: "paid" }).where(eq(orders.id, order.id));
  return { status: "paid" };
}

/**
 * The same checkout with an idempotency key: the client's request id is
 * recorded (primary key) and sent to the provider, which deduplicates
 * charges by key (the provider's contract).
 */
export async function payOrderIdempotent(input: PayRequest) {
  const [order] = await db.select().from(orders).where(eq(orders.id, input.orderId)).limit(1);
  if (!order) {
    throw new Error("no such order");
  }
  if (order.status === "paid") {
    return { status: "already-paid" };
  }
  await db
    .insert(idempotencyKeys)
    .values({ key: input.requestId, orderId: order.id })
    .onConflictDoNothing();
  await chargeCard({ orderId: order.id, amount: order.amount }, input.requestId);
  await db.update(orders).set({ status: "paid" }).where(eq(orders.id, order.id));
  await db
    .update(idempotencyKeys)
    .set({ status: "completed" })
    .where(eq(idempotencyKeys.key, input.requestId));
  return { status: "paid" };
}

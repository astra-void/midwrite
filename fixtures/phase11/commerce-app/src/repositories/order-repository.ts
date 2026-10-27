import { eq } from "drizzle-orm";

import { db } from "../db/client.js";
import { charges, orders } from "../db/schema.js";

export async function getOrder(orderId: number) {
  const [order] = await db.select().from(orders).where(eq(orders.id, orderId)).limit(1);
  return order;
}

export async function recordCharge(orderId: number, amount: number) {
  await db.insert(charges).values({ orderId, amount });
}

export async function markPaid(orderId: number) {
  await db.update(orders).set({ status: "paid" }).where(eq(orders.id, orderId));
}

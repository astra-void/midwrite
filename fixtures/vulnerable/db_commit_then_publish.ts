// Example A: a database commit followed by an event publication.
// @expect CP1001 line=12 confidence=high function=completeOrder
import { eq } from "drizzle-orm";
import { db } from "@/db";
import { orders } from "@/db/schema";
import { queue } from "@/lib/queue";

export async function completeOrder(orderId: string) {
  await db.update(orders)
    .set({ status: "paid" })
    .where(eq(orders.id, orderId));
  await queue.publish("order.completed", { orderId });
}

// A publication that is started but not awaited.
// @expect MW1001 line=8 confidence=medium
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function completeOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  queue.publish("order.completed", { id }).catch(console.error);
}

// Example B: an external effect before the database commit.
// @expect MW1002 line=7 confidence=high
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function completeOrder(id: string) {
  await queue.publish("order.completed", { id });
  await db.update(orders).set({ status: "paid" });
}

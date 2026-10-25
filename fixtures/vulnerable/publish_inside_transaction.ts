// Example D: an external publication inside a database transaction.
// @expect CP1003 line=9 confidence=high
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function completeOrder(id: string) {
  await db.transaction(async (tx) => {
    await tx.update(orders).set({ status: "paid" });
    await queue.publish("order.completed", { id });
  });
}

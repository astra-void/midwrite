// A query builder stored in a variable executes when it is awaited.
// @expect CP1001 line=11 confidence=high
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function completeOrder(id: string) {
  const markPaid = db.update(orders).set({ status: "paid" });
  // Nothing has executed yet.
  await markPaid;
  // The write has committed now.
  await queue.publish("order.completed", { id });
}

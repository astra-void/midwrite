// Operations started together with Promise.all have no defined completion order.
// @expect MW1001 line=9 confidence=low
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function completeOrder(id: string) {
  await Promise.all([
    db.update(orders).set({ status: "paid" }),
    queue.publish("order.completed", { id }),
  ]);
}

// Differential test: a commit, then a publication, in order.
// @expect MW1001 line=7 confidence=high function=completeOrder
import { db, orders, queue } from "./runtime.ts";

export async function completeOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  await queue.publish("order.completed", { id });
}

export async function main() {
  await completeOrder("1");
}

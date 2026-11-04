// The publication only happens on one branch: the hazard depends on a condition.
// @expect MW1001 line=9 confidence=medium
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function updateStatus(id: string, status: string) {
  await db.update(orders).set({ status });
  if (status === "paid") {
    await queue.publish("order.paid", { id });
  }
}

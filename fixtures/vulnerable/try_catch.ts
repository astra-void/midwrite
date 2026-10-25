// Effects inside try/catch: the handler may compensate for thrown errors, but a
// process crash bypasses it, so the hazard is still reported (with a note).
// @expect CP1001 line=10 confidence=high
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function completeOrder(id: string) {
  try {
    await db.update(orders).set({ status: "paid" });
    await queue.publish("order.completed", { id });
  } catch (error) {
    console.error("completion failed", error);
    throw error;
  }
}

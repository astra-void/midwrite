// Nested functions are analyzed as separate units: defining `finish` runs
// nothing, so `scheduleCompletion` has no hazard of its own, but `finish` does.
// @expect MW1001 line=11 confidence=high function=finish
import { db } from "@/db";
import { orders } from "@/db/schema";

export function scheduleCompletion(id: string, later: (fn: () => Promise<void>) => void) {
  async function finish() {
    await db.update(orders).set({ status: "paid" });
    // crash window
    await queue.publish("order.completed", { id });
  }
  later(finish);
}

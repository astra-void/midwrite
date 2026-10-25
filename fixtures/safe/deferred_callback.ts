// Phase 5: a callback passed to `setTimeout` runs later, outside the current
// sequence; its publication is not serialized after the commit (it is
// analyzed as its own function, where nothing precedes it).
// @expect none
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function completeOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  setTimeout(async () => {
    await queue.publish("order.reminder", { id });
  }, 60_000);
}

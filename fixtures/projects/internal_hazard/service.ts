// The hazard between the write and the publish is inside this function and
// is reported here, not again at every caller.
// @expect CP1001 line=8 confidence=high function=complete
import { db } from "@/db";

export async function complete(id: string) {
  await db.update(orders).set({ status: "paid" });
  await queue.publish("order.paid", { id });
}

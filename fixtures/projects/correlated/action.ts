// The write and the publish are guarded by contradictory conditions on the
// same unchanged parameter: no path performs both.
// @expect none
import { db } from "@/db";

export async function settle(id: string, offline: boolean) {
  if (offline) {
    await db.update(orders).set({ status: "queued" });
  }
  if (!offline) {
    await queue.publish("order.settle", { id });
  }
}

// Phase 5: resolved calls inside `Promise.all` are summarized too. The
// helper's publication runs concurrently with the other element, after the
// commit: CP1001 with low confidence (completion order undetermined); the
// unknown `audit.log(id)` element stays an unknown call (CP9001). An
// immediately invoked async arrow is resolved to its own body.
// @expect CP1001 line=18 confidence=low function=completeOrder
// @expect CP1001 line=23 confidence=high function=shipOrder
// @expect CP9001 line=18 function=completeOrder
import { db } from "@/db";
import { orders } from "@/db/schema";

async function notify(id: string) {
  await queue.publish("order.completed", { id });
}

export async function completeOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  await Promise.all([notify(id), audit.log(id)]);
}

export async function shipOrder(id: string) {
  await db.update(orders).set({ status: "shipped" });
  await (async () => {
    await queue.publish("order.shipped", { id });
  })();
}

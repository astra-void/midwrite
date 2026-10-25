// Calls of functions declared in the same file are analyzed through the
// callee's summary: `notify` publishes on every path, so the commit followed
// by the awaited call is a hazard with high confidence. The diagnostic points
// at the call; the publish keeps its own location in `effects`, and the
// evidence says it comes from the summary of `notify`.
// @expect CP1001 line=17 confidence=high function=completeOrder
import { db } from "@/db";
import { orders } from "@/db/schema";

async function notify(id: string) {
  await queue.publish("order.completed", { id });
}

export async function completeOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  // `notify` publishes: a crash here leaves a paid order without an event.
  await notify(id);
}

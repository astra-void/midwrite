// A hazard entirely inside a helper is reported once, for the helper itself,
// not again for every caller.
// @expect CP1001 line=10 confidence=high function=completeOrder
import { db } from "@/db";
import { orders } from "@/db/schema";

async function completeOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  // crash window
  await queue.publish("order.completed", { id });
}

export async function handleWebhook(id: string) {
  await completeOrder(id);
}

export async function handleRetry(id: string) {
  await completeOrder(id);
}

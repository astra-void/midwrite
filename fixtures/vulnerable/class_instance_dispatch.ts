// Phase 5: a method called on an object created by `new Notifier()` in the
// same function dispatches exactly to `Notifier.notify` (the receiver's
// class is known, overrides are irrelevant).
// @expect CP1001 line=17 confidence=high function=completeOrder
import { db } from "@/db";
import { orders } from "@/db/schema";

class Notifier {
  async notify(id: string) {
    await queue.publish("order.completed", { id });
  }
}

export async function completeOrder(id: string) {
  const notifier = new Notifier();
  await db.update(orders).set({ status: "paid" });
  await notifier.notify(id);
}

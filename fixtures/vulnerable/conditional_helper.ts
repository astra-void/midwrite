// The callee performs its effect only on some paths (a guard clause or a
// condition), so a hazard through its summary is reported with medium
// confidence, never high.
// @expect CP1001 line=23 confidence=medium function=completeOrder
// @expect CP1002 line=28 confidence=medium function=refundOrder
import { db } from "@/db";
import { orders } from "@/db/schema";

async function notifyIfSubscribed(id: string, subscribed: boolean) {
  if (!subscribed) {
    return;
  }
  await queue.publish("order.completed", { id });
}

const announceRefund = async (id: string, amount: number) => {
  if (amount > 0) await queue.publish("order.refunded", { id, amount });
};

export async function completeOrder(id: string, subscribed: boolean) {
  await db.update(orders).set({ status: "paid" });
  // Only some paths of `notifyIfSubscribed` publish.
  await notifyIfSubscribed(id, subscribed);
}

export async function refundOrder(id: string, amount: number) {
  // The refund may be announced before the status change commits.
  await announceRefund(id, amount);
  await db.update(orders).set({ status: "refunded" });
}

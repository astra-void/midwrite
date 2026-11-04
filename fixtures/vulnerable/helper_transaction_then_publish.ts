// A helper that commits a transaction, followed by an external effect in the
// caller; and an `async` function that returns a helper call, which the
// caller awaits by returning it.
// @expect MW1001 line=22 confidence=high function=completeOrder
// @expect MW1001 line=27 confidence=high function=shipOrder
import { db } from "@/db";
import { orders, outbox } from "@/db/schema";

async function markPaid(id: string) {
  await db.transaction(async (tx) => {
    await tx.update(orders).set({ status: "paid" });
    await tx.insert(outbox).values({ topic: "order.paid", payload: { id } });
  });
}

async function announceShipment(id: string) {
  await mailer.send({ template: "shipped", id });
}

export async function completeOrder(id: string) {
  await markPaid(id);
  await queue.publish("order.completed", { id });
}

export async function shipOrder(id: string) {
  await db.update(orders).set({ status: "shipped" });
  return announceShipment(id);
}

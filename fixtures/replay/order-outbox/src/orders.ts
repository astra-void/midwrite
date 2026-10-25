// The transactional-outbox version of the order flow: the status change and
// the intent to publish commit atomically in one transaction. Publication is
// left to the relay (relay.ts).
import { db, eq } from "../../shared/db.ts";
import { orders, outbox } from "../../shared/schema.ts";

export async function completeOrder(orderId: string) {
  await db.transaction(async (tx) => {
    await tx.update(orders).set({ status: "paid" }).where(eq(orders.id, orderId));
    await tx.insert(outbox).values({ topic: "order.completed", payload: { orderId } });
  });
}

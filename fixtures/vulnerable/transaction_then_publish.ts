// A committed transaction followed by a publication: the transaction and the
// event are still not atomic.
// @expect CP1001 line=12 confidence=high
import { db } from "@/db";
import { orders, payments } from "@/db/schema";

export async function settle(id: string, amount: number) {
  await db.transaction(async (tx) => {
    await tx.update(orders).set({ status: "paid" });
    await tx.insert(payments).values({ orderId: id, amount });
  });
  await queue.publish("order.settled", { id });
}

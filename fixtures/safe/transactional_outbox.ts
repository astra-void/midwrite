// Example C: the transactional outbox pattern. Both writes commit atomically.
// CommitRift only establishes that they share a transaction; it does not check
// that an outbox relay exists.
// @expect none
import { db } from "@/db";
import { orders, outbox } from "@/db/schema";

export async function completeOrder(id: string) {
  await db.transaction(async (tx) => {
    await tx.update(orders).set({ status: "paid" });

    await tx.insert(outbox).values({
      topic: "order.completed",
      payload: { id },
    });
  });
}

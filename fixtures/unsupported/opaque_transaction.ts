// The transaction callback is defined elsewhere, so its body cannot be
// attributed to the transaction.
// @expect MW9001 line=13
import { db } from "@/db";
import { orders } from "@/db/schema";

async function markPaid(tx: typeof db) {
  await tx.update(orders).set({ status: "paid" });
}

export async function completeOrder(id: string) {
  // The writes inside `markPaid` are invisible here.
  await db.transaction(markPaid);
  await queue.publish("order.completed", { id });
}

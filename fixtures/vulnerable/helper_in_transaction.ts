// Helpers called inside a transaction callback: one publishes before the
// transaction commits (CP1003), the other writes on `db` instead of the
// transaction handle, so its write commits on its own (CP1004). Both
// diagnostics point at the call inside the callback.
// @expect CP1003 line=23 confidence=high function=completeOrder
// @expect CP1004 line=30 confidence=high function=recordPayment
import { db } from "@/db";
import { orders, payments } from "@/db/schema";

async function announce(id: string) {
  await queue.publish("order.completed", { id });
}

const logPayment = async (id: string, amount: number) => {
  // Issued on `db`: not part of the caller's transaction.
  await db.insert(payments).values({ orderId: id, amount });
};

export async function completeOrder(id: string) {
  await db.transaction(async (tx) => {
    await tx.update(orders).set({ status: "paid" });
    // The event is visible while the transaction may still roll back.
    await announce(id);
  });
}

export async function recordPayment(id: string, amount: number) {
  await db.transaction(async (tx) => {
    await tx.update(orders).set({ status: "paid" });
    await logPayment(id, amount);
  });
}

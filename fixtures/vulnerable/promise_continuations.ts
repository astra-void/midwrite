// `.then`, `.catch` and `.finally` callbacks of an awaited promise run in
// sequence with it: `p` then `cb`, `try { p } catch { cb }`, `try { p }
// finally { cb }`. The callback is lowered in place, so what it captures
// (an id, a transaction handle) stays known.
// @expect MW1002 line=14 confidence=medium function=chargeThenSave
// @expect MW1001 line=22 confidence=high function=updateThenPublish
// @expect MW1001 line=29 confidence=medium function=recordFailedCharge
// @expect MW1004 line=30 confidence=low function=recordFailedCharge
import { db } from "@/db";
import { events, orders } from "@/db/schema";

// The write runs after the payment.
export async function chargeThenSave(id: string) {
  await stripe.charges.create({ id }).then(() => db.insert(orders).values({ id }));
}

// The publish runs after the update, whether or not it succeeds.
export async function updateThenPublish(id: string) {
  await db
    .update(orders)
    .set({ status: "paid" })
    .finally(() => queue.publish("order.paid", { id }));
}

// The write runs only when the payment fails: it compensates, after the
// earlier insert.
export async function recordFailedCharge(id: string) {
  await db.insert(events).values({ id });
  await stripe.charges.create({ id }).catch(async () => {
    await db.update(orders).set({ status: "failed" });
  });
}

// A transaction handle captured by the callback: both writes are in the
// transaction.
export async function insideTransaction(id: string) {
  await db.transaction(async (tx) => {
    await tx
      .insert(orders)
      .values({ id })
      .then(() => tx.insert(events).values({ id }));
  });
}

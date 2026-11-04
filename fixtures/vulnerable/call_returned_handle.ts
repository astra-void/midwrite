// The database handle is obtained from a call: fixtures/midwrite.json lists
// `getDb()` imported from "@/server/db". Direct chains, `const` aliases and
// arguments are recognized; a call with arguments or from another module is not.
// @expect MW1001 line=12 confidence=high function=markPaid
// @expect MW1003 line=19 confidence=high function=checkout
// @expect MW1004 line=26 confidence=high function=recordPayment
import { getDb } from "@/server/db";
import { getDb as otherDb } from "@/legacy/db";

export async function markPaid(id: string) {
  await getDb().update(orders).set({ status: "paid" });
  await queue.publish("order.paid", { id });
}

export async function checkout(id: string) {
  const db = getDb();
  await db.transaction(async (tx) => {
    await tx.update(orders).set({ status: "paid" });
    await queue.publish("order.paid", { id });
  });
}

export async function recordPayment(id: string) {
  await getDb().transaction(async (tx) => {
    await tx.insert(payments).values({ id });
    await getDb().insert(audit).values({ id });
  });
}

export async function notRecognized(id: string) {
  await otherDb().update(orders).set({ id });
  await getDb("replica").update(orders).set({ id });
  await queue.publish("x", { id });
}

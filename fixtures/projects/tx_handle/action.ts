// The helpers receive the transaction handle, so their writes join the
// caller's transaction: no MW1004. The publish inside the callback is MW1003.
// @expect MW1003 line=12 confidence=high function=checkout
import { db } from "@/db";
import { saveOrder, audit } from "./repository";

export async function checkout(id: string) {
  await db.transaction(async (tx) => {
    await saveOrder(tx, id);
    // `audit`'s parameter is named like the configured receiver `db`.
    await audit(tx, id);
    await queue.publish("order.paid", { id });
  });
}

export async function checkoutOutbox(id: string) {
  await db.transaction(async (tx) => {
    await saveOrder(tx, id);
    await audit(tx, id);
  });
}

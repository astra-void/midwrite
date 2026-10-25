// A nested transaction (savepoint) on the transaction handle commits with the
// outer transaction.
// @expect none
import { db } from "@/db";
import { orders, payments } from "@/db/schema";

export async function settle(id: string) {
  await db.transaction(async (tx) => {
    await tx.update(orders).set({ status: "paid" });
    await tx.transaction(async (savepoint) => {
      await savepoint.insert(payments).values({ orderId: id });
    });
  });
}

// A write inside the transaction callback that uses `db` instead of `tx`
// commits on its own, outside the transaction.
// @expect MW1004 line=10 confidence=high
import { db } from "@/db";
import { auditLog, orders } from "@/db/schema";

export async function cancelOrder(id: string) {
  await db.transaction(async (tx) => {
    await tx.update(orders).set({ status: "cancelled" });
    await db.insert(auditLog).values({ action: "cancel", id });
  });
}

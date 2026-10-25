// Writes on whatever handle the caller passes. Analyzed on its own, `tx` is
// an unconfigured parameter, so nothing is claimed here.
// @expect none
import type { Transaction } from "@/db";
import { orders, payments } from "@/db/schema";

export async function saveOrder(tx: Transaction, id: string) {
  await tx.update(orders).set({ status: "paid" });
  await tx.insert(payments).values({ id });
}

export async function audit(db: Database, id: string) {
  await db.insert(auditLog).values({ id });
}

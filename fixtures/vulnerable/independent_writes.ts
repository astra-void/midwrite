// Two writes to different tables without a shared transaction.
// @expect MW1004 line=8 confidence=high
import { db } from "@/db";
import { orders, payments } from "@/db/schema";

export async function recordPayment(orderId: string, amount: number) {
  await db.update(orders).set({ status: "paid" });
  await db.insert(payments).values({ orderId, amount });
}

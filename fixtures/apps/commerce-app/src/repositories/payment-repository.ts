import { db } from "../db/client.js";
import { payments, settledPayments } from "../db/schema.js";

export async function insertPayment(orderId: number, reference: string, amount: number) {
  await db.insert(payments).values({ orderId, reference, amount });
}

export async function insertSettledPayment(orderId: number, reference: string, amount: number) {
  await db.insert(settledPayments).values({ orderId, reference, amount }).onConflictDoNothing();
}

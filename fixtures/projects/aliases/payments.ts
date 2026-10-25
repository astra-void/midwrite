// @expect none
import { db } from "@/db";
import { payments } from "@/db/schema";

export async function recordPayment(id: string) {
  await db.insert(payments).values({ id });
}

export default async function refund(id: string) {
  await db.delete(payments);
}

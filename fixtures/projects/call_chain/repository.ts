// @expect none
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function updateOrder(id: string, status: string) {
  await db.update(orders).set({ status });
}

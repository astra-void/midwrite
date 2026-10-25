// @expect none
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function closeOrder(id: string) {
  await db.update(orders).set({ status: "closed" });
}

// The transaction handle is destructured; writes cannot be attributed to it.
// @expect MW9001 line=7
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function completeOrder(id: string) {
  await db.transaction(async ({ update }) => {
    await update(orders).set({ status: "paid", id });
  });
}

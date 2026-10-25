// @expect none
import { db } from "@/db";

export async function markShipped(id: string) {
  await db.update(orders).set({ status: "shipped" });
}

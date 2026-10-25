// A service that commits a write. Its own analysis finds nothing.
// @expect none
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function markPaid(id: string) {
  await db.update(orders).set({ status: "paid" });
}

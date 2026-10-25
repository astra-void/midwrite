// Example F: an unknown call after a database commit. CommitRift does not
// claim it is an external effect; it reports an analysis limitation.
// @expect CP9001 line=10
import { db } from "@/db";
import { orders } from "@/db/schema";
import { unknownService } from "@/lib/unknown";

export async function completeOrder(orderId: string) {
  await db.update(orders).set({ status: "paid" });
  await unknownService.process(orderId);
}

// Writes inside a loop followed by a publication. Only zero or one iteration
// is modeled, so confidence is medium.
// @expect MW1001 line=12 confidence=medium
import { db } from "@/db";
import { lineItems } from "@/db/schema";

export async function importItems(orderId: string, items: { sku: string }[]) {
  for (const item of items) {
    await db.insert(lineItems).values({ orderId, sku: item.sku });
  }
  // Each iteration commits separately; a crash mid-way leaves a partial import.
  await queue.publish("order.items-imported", { orderId });
}

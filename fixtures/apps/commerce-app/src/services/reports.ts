// Read-only and out-of-scope code the discovery must not turn into races.
import { eq } from "drizzle-orm";

import { db } from "../db/client.js";
import { orders, products } from "../db/schema.js";

/** Read-only. */
export async function stockLevel(productId: number) {
  const [product] = await db.select().from(products).where(eq(products.id, productId)).limit(1);
  return product ? product.stock : 0;
}

/** Writes through a query the extractor cannot read. */
export async function archiveOrder(orderId: number) {
  await db.execute(`update orders set status = 'archived' where id = ${orderId}`);
}

/** Calls a client nothing declares. */
export async function notifyCustomer(orderId: number, client: { send(id: number): Promise<void> }) {
  const [order] = await db.select().from(orders).where(eq(orders.id, orderId)).limit(1);
  if (order) {
    await client.send(order.id);
  }
}

/** Pure helper. */
export function formatAmount(amount: number) {
  return `${amount / 100}`;
}

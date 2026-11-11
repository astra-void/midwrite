import { and, eq, gte, sql } from "drizzle-orm";

import { db } from "../db/client.js";
import { products, reservations } from "../db/schema.js";

export async function getProduct(productId: number) {
  const [product] = await db.select().from(products).where(eq(products.id, productId)).limit(1);
  return product;
}

export async function setStock(productId: number, stock: number) {
  await db.update(products).set({ stock }).where(eq(products.id, productId));
}

export async function addReservation(productId: number, quantity: number) {
  await db.insert(reservations).values({ productId, quantity });
}

// One statement: the decrement happens only while enough stock is left;
// the returned rows say whether it did.
export async function takeStock(productId: number, quantity: number) {
  return db
    .update(products)
    .set({ stock: sql`${products.stock} - ${quantity}` })
    .where(and(eq(products.id, productId), gte(products.stock, quantity)))
    .returning();
}

import { and, eq, gt, sql } from "drizzle-orm";
import { db } from "./db";
import { orders, products } from "./schema";

// The classic read-modify-write purchase: check the stock, then write back
// a value computed from what was read.
export async function purchase(productId: string, orderId: string) {
  const product = await db.query.products.findFirst({
    where: eq(products.id, productId),
  });

  if (!product || product.stock <= 0) {
    throw new Error("Out of stock");
  }

  await db
    .update(products)
    .set({ stock: product.stock - 1 })
    .where(eq(products.id, productId));

  await db.insert(orders).values({ id: orderId, product: productId });
}

// The same read-modify-write inside a transaction (default isolation).
export async function purchaseInTransaction(productId: string, orderId: string) {
  await db.transaction(async (tx) => {
    const product = await tx.query.products.findFirst({
      where: eq(products.id, productId),
    });
    if (!product || product.stock <= 0) {
      throw new Error("Out of stock");
    }
    await tx
      .update(products)
      .set({ stock: product.stock - 1 })
      .where(eq(products.id, productId));
    await tx.insert(orders).values({ id: orderId, product: productId });
  });
}

// Atomic alternative: a conditional UPDATE decides, the database arbitrates.
export async function purchaseAtomically(productId: string, orderId: string) {
  const updated = await db
    .update(products)
    .set({ stock: sql`${products.stock} - 1` })
    .where(and(eq(products.id, productId), gt(products.stock, 0)))
    .returning();

  if (updated.length === 0) {
    throw new Error("Out of stock");
  }

  await db.insert(orders).values({ id: orderId, product: productId });
}

// Row-locking alternative: read the row with FOR UPDATE inside a transaction.
export async function purchaseWithRowLock(productId: string, orderId: string) {
  await db.transaction(async (tx) => {
    const [product] = await tx
      .select()
      .from(products)
      .where(eq(products.id, productId))
      .limit(1)
      .for("update");
    if (!product || product.stock <= 0) {
      throw new Error("Out of stock");
    }
    await tx
      .update(products)
      .set({ stock: product.stock - 1 })
      .where(eq(products.id, productId));
    await tx.insert(orders).values({ id: orderId, product: productId });
  });
}

// Stock reservations and transfers between warehouse bins.
import { eq, sql } from "drizzle-orm";

import { db } from "../db/client.js";
import { bins, products, reservations } from "../db/schema.js";
import {
  addReservation,
  getProduct,
  setStock,
  takeStock,
} from "../repositories/inventory-repository.js";

export interface ReserveRequest {
  productId: number;
  quantity: number;
}

/** Reads the stock, checks it, writes the new count. */
export async function reserveItem(input: ReserveRequest) {
  const product = await getProduct(input.productId);
  if (!product) {
    throw new Error("no such product");
  }
  if (product.stock < input.quantity) {
    throw new Error("insufficient stock");
  }
  await setStock(product.id, product.stock - input.quantity);
  await addReservation(product.id, input.quantity);
  return { reserved: input.quantity };
}

/** A conditional decrement in one statement; the affected rows are checked. */
export async function reserveItemAtomic(input: ReserveRequest) {
  const taken = await takeStock(input.productId, input.quantity);
  if (taken.length === 0) {
    throw new Error("insufficient stock");
  }
  await addReservation(input.productId, input.quantity);
  return { reserved: input.quantity };
}

/** Reads the row FOR UPDATE inside a transaction before writing it. */
export async function reserveItemLocked(input: ReserveRequest) {
  return db.transaction(async (tx) => {
    const [product] = await tx
      .select()
      .from(products)
      .where(eq(products.id, input.productId))
      .for("update")
      .limit(1);
    if (!product || product.stock < input.quantity) {
      throw new Error("insufficient stock");
    }
    await tx
      .update(products)
      .set({ stock: product.stock - input.quantity })
      .where(eq(products.id, product.id));
    await tx.insert(reservations).values({ productId: product.id, quantity: input.quantity });
    return { reserved: input.quantity };
  });
}

export interface TransferRequest {
  from: number;
  to: number;
  quantity: number;
}

/** Moves stock between two bins, updating the source bin first. */
export async function transferStock(input: TransferRequest) {
  await db.transaction(async (tx) => {
    await tx
      .update(bins)
      .set({ quantity: sql`${bins.quantity} - ${input.quantity}` })
      .where(eq(bins.id, input.from));
    await tx
      .update(bins)
      .set({ quantity: sql`${bins.quantity} + ${input.quantity}` })
      .where(eq(bins.id, input.to));
  });
  return { moved: input.quantity };
}

/** The same transfer, touching the lower bin id first. */
export async function transferStockOrdered(input: TransferRequest) {
  await db.transaction(async (tx) => {
    if (input.from < input.to) {
      await tx
        .update(bins)
        .set({ quantity: sql`${bins.quantity} - ${input.quantity}` })
        .where(eq(bins.id, input.from));
      await tx
        .update(bins)
        .set({ quantity: sql`${bins.quantity} + ${input.quantity}` })
        .where(eq(bins.id, input.to));
    } else {
      await tx
        .update(bins)
        .set({ quantity: sql`${bins.quantity} + ${input.quantity}` })
        .where(eq(bins.id, input.to));
      await tx
        .update(bins)
        .set({ quantity: sql`${bins.quantity} - ${input.quantity}` })
        .where(eq(bins.id, input.from));
    }
  });
  return { moved: input.quantity };
}

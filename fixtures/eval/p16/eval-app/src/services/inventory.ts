import { eq, sql } from "drizzle-orm";

import { db } from "../db.js";
import { stockLevels, reservations } from "../schema/index.js";

const MAX_ATTEMPTS = 5;

export class OutOfStockError extends Error {
  constructor(readonly sku: string) {
    super(`not enough stock for ${sku}`);
  }
}

function assertQuantity(quantity: number) {
  if (!Number.isSafeInteger(quantity) || quantity <= 0) throw new RangeError("quantity must be a positive integer");
}

// P16-E13: reserves stock for an order; the transaction is retried when
// PostgreSQL reports a serialization failure.
export async function reserveStock(orderId: number, sku: string, quantity: number) {
  assertQuantity(quantity);
  for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
    try {
      return await db.transaction(async (tx) => {
        const [level] = await tx
          .select({ available: stockLevels.available })
          .from(stockLevels)
          .where(eq(stockLevels.sku, sku))
          .limit(1);
        if (!level) throw new Error(`unknown sku ${sku}`);
        if (level.available < quantity) throw new OutOfStockError(sku);
        await tx
          .update(stockLevels)
          .set({ available: sql`${stockLevels.available} - ${quantity}` })
          .where(eq(stockLevels.sku, sku));
        const [reservation] = await tx
          .insert(reservations)
          .values({ sku, orderId, quantity })
          .returning({ id: reservations.id });
        return reservation?.id ?? null;
      });
    } catch (e) {
      if ((e as { code?: string } | null)?.code === "40001") continue;
      throw e;
    }
  }
  throw new Error("could not reserve stock: too much contention");
}

// P16-E14: the same, at SERIALIZABLE.
export async function reserveStockSerializable(orderId: number, sku: string, quantity: number) {
  assertQuantity(quantity);
  for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
    try {
      return await db.transaction(
        async (tx) => {
          const [level] = await tx
            .select({ available: stockLevels.available })
            .from(stockLevels)
            .where(eq(stockLevels.sku, sku))
            .limit(1);
          if (!level) throw new Error(`unknown sku ${sku}`);
          if (level.available < quantity) throw new OutOfStockError(sku);
          await tx
            .update(stockLevels)
            .set({ available: sql`${stockLevels.available} - ${quantity}` })
            .where(eq(stockLevels.sku, sku));
          const [reservation] = await tx
            .insert(reservations)
            .values({ sku, orderId, quantity })
            .returning({ id: reservations.id });
          return reservation?.id ?? null;
        },
        { isolationLevel: "serializable" },
      );
    } catch (e) {
      if ((e as { code?: string } | null)?.code === "40001") continue;
      throw e;
    }
  }
  throw new Error("could not reserve stock: too much contention");
}

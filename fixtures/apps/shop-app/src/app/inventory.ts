// Stock reservations under REPEATABLE READ: a concurrent reservation that
// commits first makes PostgreSQL abort the other with SQLSTATE 40001, and
// `withTransactionRetry` runs the work again in a fresh transaction.
import { eq } from "drizzle-orm";

import { products, reservations } from "../lib/db/schema.js";
import { withTransactionRetry } from "../lib/db/retry.js";
import { notifyWarehouse } from "../lib/messaging/bus.js";

export interface ReserveRequest {
  productId: number;
  quantity: number;
}

/** Notifies the warehouse from inside the transaction. */
export async function reserveStock(input: ReserveRequest) {
  return withTransactionRetry(
    async (tx) => {
      const [product] = await tx
        .select()
        .from(products)
        .where(eq(products.id, input.productId))
        .limit(1);
      if (!product || product.stock < input.quantity) {
        throw new Error("insufficient stock");
      }
      await notifyWarehouse({ productId: product.id, quantity: input.quantity });
      await tx
        .update(products)
        .set({ stock: product.stock - input.quantity })
        .where(eq(products.id, product.id));
      await tx.insert(reservations).values({ productId: product.id, quantity: input.quantity });
      return { reserved: input.quantity };
    },
    { attempts: 3, isolationLevel: "repeatable read" },
  );
}

/** Notifies the warehouse once the reservation has committed. */
export async function reserveStockNotifyAfterCommit(input: ReserveRequest) {
  const result = await withTransactionRetry(
    async (tx) => {
      const [product] = await tx
        .select()
        .from(products)
        .where(eq(products.id, input.productId))
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
    },
    { attempts: 3, isolationLevel: "repeatable read" },
  );
  await notifyWarehouse({ productId: input.productId, quantity: input.quantity });
  return result;
}

/** The same reservation without a retry: a serialization failure is final. */
export async function reserveStockOnce(input: ReserveRequest) {
  return withTransactionRetry(
    async (tx) => {
      const [product] = await tx
        .select()
        .from(products)
        .where(eq(products.id, input.productId))
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
    },
    { attempts: 1, isolationLevel: "repeatable read" },
  );
}

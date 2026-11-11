import { eq } from "drizzle-orm";

import { db } from "../db.js";
import { orders } from "../schema/index.js";

export type NewOrder = { customerId: string; cartId: string; total: number };

export const orderRepo = {
  async insert(order: NewOrder) {
    const [created] = await db.insert(orders).values(order).returning({ id: orders.id });
    return created;
  },

  // The caller chooses the order's key.
  async insertWithId(id: string, order: NewOrder) {
    const [created] = await db
      .insert(orders)
      .values({ id, ...order })
      .onConflictDoNothing({ target: orders.id })
      .returning({ id: orders.id });
    return created;
  },

  async findById(id: string) {
    const [found] = await db.select().from(orders).where(eq(orders.id, id)).limit(1);
    return found;
  },
};

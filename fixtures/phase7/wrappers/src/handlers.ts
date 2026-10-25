import { eq } from "drizzle-orm";

import { db } from "./db";
import { action, authed, ignore, immediate, later, store } from "./lib/wrappers";
import { audit, orders } from "./schema";

// Identical callback text, different functions.
export const cancelA = action(async (id: number) => {
  await db.update(orders).set({ status: "cancelled" }).where(eq(orders.id, id));
});
export const cancelB = action(async (id: number) => {
  await db.update(orders).set({ status: "cancelled" }).where(eq(orders.id, id));
});

// Nested higher-order functions: the callback is guarded by `authed`.
export const shipOrder = action(
  authed(async (id: number, user) => {
    await db.update(orders).set({ status: "shipped" }).where(eq(orders.id, id));
    await db.insert(audit).values({ orderId: id });
    return user.id;
  }),
);

export const bootTask = immediate(async (reason: string) => {
  await db.insert(audit).values({ orderId: 0 });
  return reason;
});

export const stored = store(async (id: number) => {
  await db.delete(orders).where(eq(orders.id, id));
});

export const neverRuns = ignore(async (id: number) => {
  await db.delete(orders).where(eq(orders.id, id));
});

export const background = later(async (id: number) => {
  await db.insert(audit).values({ orderId: id });
});

// A callback factory: the returned function closes over `next`.
export function makeTransition(next: string) {
  return async (id: number) => {
    await db.update(orders).set({ status: next }).where(eq(orders.id, id));
  };
}

// A tRPC-style procedure (the builder is a package API: its invocation of
// the resolver is not visible in analyzed source).
declare const procedure: {
  mutation<R>(resolver: (opts: { input: { id: number } }) => Promise<R>): unknown;
};
export const router = {
  archive: procedure.mutation(async ({ input }) => {
    await db.update(orders).set({ status: "archived" }).where(eq(orders.id, input.id));
  }),
};

export class OrderService {
  async place(id: number) {
    await db.insert(orders).values({ id, status: "new" });
  }
  static async purge(id: number) {
    await db.delete(orders).where(eq(orders.id, id));
  }
}

export const objectHandlers = {
  async reopen(id: number) {
    await db.update(orders).set({ status: "open" }).where(eq(orders.id, id));
  },
};

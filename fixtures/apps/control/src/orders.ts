import { eq } from "drizzle-orm";

import { db } from "./db";
import { failures, orders } from "./schema";

declare function chargePayment(orderId: number, payment: { amount: number }): Promise<void>;
declare function releaseResource(orderId: number): Promise<void>;

export async function processOrder(orderId: number) {
  try {
    await chargePayment(orderId, { amount: 10 });
    await db.insert(orders).values({ id: orderId, status: "paid" });
  } catch (error) {
    await db.insert(failures).values({ orderId });
  } finally {
    await releaseResource(orderId);
  }
}

async function findOrder(orderId: number) {
  try {
    const [order] = await db.select().from(orders).where(eq(orders.id, orderId)).limit(1);
    return order;
  } catch {
    return null;
  }
}

/** The helper's handler must not catch the caller's later failure. */
export async function requireOrder(orderId: number) {
  const order = await findOrder(orderId);
  if (!order) {
    throw new Error("no such order");
  }
  return order;
}

export async function inspectsError(orderId: number) {
  try {
    await db.insert(orders).values({ id: orderId, status: "new" });
  } catch (error) {
    if (error.code === "23505") {
      return "duplicate";
    }
  }
}

export async function tryInTransaction(orderId: number) {
  await db.transaction(async (tx) => {
    try {
      await tx.insert(orders).values({ id: orderId, status: "new" });
    } catch {
      await tx.insert(failures).values({ orderId });
    }
  });
}

export async function returnThroughFinally(orderId: number) {
  try {
    return await db.insert(orders).values({ id: orderId, status: "new" });
  } finally {
    await releaseResource(orderId);
  }
}

export async function setStatus(orderId: number, action: string) {
  switch (action) {
    case "pay":
      await db.update(orders).set({ status: "paid" }).where(eq(orders.id, orderId));
      break;
    case "cancel":
    case "void":
      await db.update(orders).set({ status: "cancelled" }).where(eq(orders.id, orderId));
      break;
    default:
      throw new Error("unknown action");
  }
}

export async function touchAll(orderId: number) {
  for (const status of ["a", "b", "c", "d"]) {
    if (status === "b") {
      continue;
    }
    await db.update(orders).set({ status }).where(eq(orders.id, orderId));
    if (status === "c") break;
  }
}

export async function deleteEach(ids: number[]) {
  for (const id of ids) {
    await db.delete(orders).where(eq(orders.id, id));
  }
}

// The vulnerable order flow: the status change commits on its own, then the
// event is published. A failure in between leaves a paid order whose
// `order.completed` event never exists.
import { db, eq } from "../../shared/db.ts";
import { queue } from "../../shared/queue.ts";
import { orders } from "../../shared/schema.ts";

export async function completeOrder(orderId: string) {
  await db.update(orders).set({ status: "paid" }).where(eq(orders.id, orderId));

  await queue.publish("order.completed", { orderId });
}

// The same flow with an in-process compensation: if publishing throws, the
// status change is reverted. This handles exceptions, but a process crash
// never reaches the catch block.
export async function completeOrderWithCompensation(orderId: string) {
  await db.update(orders).set({ status: "paid" }).where(eq(orders.id, orderId));
  try {
    await queue.publish("order.completed", { orderId });
  } catch (error) {
    await db.update(orders).set({ status: "pending" }).where(eq(orders.id, orderId));
    throw error;
  }
}

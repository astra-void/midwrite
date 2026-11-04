// Order service on Drizzle ORM + node-postgres. Each function is one of the
// Phase 4 replay scenarios (docs/phase4/NODE-DRIZZLE.md).
import { eq } from "drizzle-orm";

import { db } from "./database.js";
import { queue } from "./queue.js";
import { orders, outbox, payments, shipments } from "./schema.js";

// Scenario A — vulnerable dual write (MW1001): the status change commits on
// its own, then the event is published. A crash in between leaves a paid
// order whose `order.completed` event never exists.
export async function completeOrder(orderId: string) {
  await db.update(orders).set({ status: "paid" }).where(eq(orders.id, orderId));

  await queue.publish("order.completed", { orderId });
}

// Scenario B — both writes in one Drizzle transaction: a failure before the
// commit rolls both back.
export async function recordPayment(orderId: string, amountCents: number) {
  await db.transaction(async (tx) => {
    await tx.insert(payments).values({ orderId, amountCents });
    await tx.update(orders).set({ status: "paid" }).where(eq(orders.id, orderId));
  });
}

// Scenario C — external effect inside a transaction (MW1003): the event is
// published before the transaction commits. If the transaction then rolls
// back (an exception, a crash, or a failing COMMIT), the event is out.
export async function shipOrder(orderId: string, trackingNumber: string) {
  await db.transaction(async (tx) => {
    await tx.update(orders).set({ status: "shipped" }).where(eq(orders.id, orderId));
    await queue.publish("order.shipped", { orderId, trackingNumber }, { id: `order.shipped:${orderId}` });
    await tx.insert(shipments).values({ orderId, trackingNumber });
  });
}

// Scenario D — transactional outbox: the state change and the intent to
// publish commit atomically; relay.ts publishes the intent later.
export async function confirmOrder(orderId: string) {
  await db.transaction(async (tx) => {
    await tx.update(orders).set({ status: "confirmed" }).where(eq(orders.id, orderId));
    await tx.insert(outbox).values({
      id: `order.confirmed:${orderId}`,
      topic: "order.confirmed",
      payload: { orderId },
    });
  });
}

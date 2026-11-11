// Shipping: mark an order shipped and tell other services about it.
import { eq } from "drizzle-orm";

import { db } from "../lib/db/drizzle.js";
import { orders, outbox } from "../lib/db/schema.js";
import { publishEvent } from "../lib/messaging/bus.js";

export interface ShipRequest {
  orderId: number;
}

/** Dual write: commit the state change, then publish. */
export async function shipOrderDualWrite(input: ShipRequest) {
  await db.update(orders).set({ status: "shipped" }).where(eq(orders.id, input.orderId));
  await publishEvent({ topic: "order.shipped", orderId: input.orderId });
  return { status: "shipped" };
}

/** Transactional outbox: the event is stored in the same transaction. */
export async function shipOrderWithOutbox(input: ShipRequest) {
  await db.transaction(async (tx) => {
    await tx.update(orders).set({ status: "shipped" }).where(eq(orders.id, input.orderId));
    await tx.insert(outbox).values({ topic: "order.shipped", orderId: input.orderId });
  });
  return { status: "shipped" };
}

/**
 * One pass of the outbox relay: publish the oldest unpublished event, then
 * mark it published. The relay process is restarted by its supervisor when
 * it dies; the outbox row id is the message id consumers deduplicate by.
 */
export async function relayOutbox() {
  const [event] = await db
    .select()
    .from(outbox)
    .where(eq(outbox.published, false))
    .orderBy(outbox.id)
    .limit(1);
  if (!event) {
    return { published: 0 };
  }
  await publishEvent({ topic: event.topic, orderId: event.orderId }, event.id);
  await db.update(outbox).set({ published: true }).where(eq(outbox.id, event.id));
  return { published: 1 };
}

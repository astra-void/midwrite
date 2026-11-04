// Phase 5: `forEach` runs an async callback per element without awaiting
// it (fire-and-forget, medium); `Promise.all(xs.map(cb))` runs the
// callbacks concurrently (low). Neither is treated as a sequential loop.
// @expect MW1001 line=11 confidence=medium function=publishEach
// @expect MW1001 line=19 confidence=low function=publishAll
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function publishEach(ids: string[]) {
  await db.update(orders).set({ status: "paid" });
  ids.forEach(async (id) => {
    await queue.publish("order.paid", { id });
  });
}

export async function publishAll(ids: string[]) {
  await db.update(orders).set({ status: "paid" });
  await Promise.all(
    ids.map(async (id) => {
      await queue.publish("order.paid", { id });
    }),
  );
}

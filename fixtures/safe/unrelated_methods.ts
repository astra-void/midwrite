// Methods named like database or queue operations on unrelated objects.
// @expect none
import { db } from "@/db";
import { orders } from "@/db/schema";

const cache = new Map<string, string>();

export async function refresh(id: string, events: { publish(topic: string): Promise<void> }) {
  cache.delete(id);
  await events.publish("order.refreshed"); // `events` is not a configured receiver
  const queue = { publish: async (_topic: string) => {} };
  await queue.publish("local"); // a local object literal that shadows `queue`
  await db.update(orders).set({ refreshedAt: new Date() });
}

export async function shadowedDb(key: string) {
  const db = new Map<string, number>(); // shadows the database import
  await db.delete(key);
  await queue.publish("cache.cleared", { key });
}

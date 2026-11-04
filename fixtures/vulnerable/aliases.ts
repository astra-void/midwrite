// Renamed imports and local `const` aliases of recognized receivers are followed.
// @expect MW1001 line=11 confidence=high
import { db as database } from "@/db";
import { orders } from "@/db/schema";

const bus = queue;

export async function completeOrder(id: string) {
  const conn = database;
  await conn.update(orders).set({ status: "paid" });
  await bus.publish("order.completed", { id });
}

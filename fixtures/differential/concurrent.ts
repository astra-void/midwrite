// Differential test: two publications started together after the commit.
// Statically their completion order is undetermined (low confidence); at
// runtime the slower one completes last.
// @expect MW1001 line=14 confidence=low function=completeOrder
// @expect MW1005 line=14 confidence=low function=completeOrder
import { db, orders, queue } from "./runtime.ts";

async function announce(topic: string) {
  await queue.publish(topic, {});
}

export async function completeOrder(_id: string) {
  await db.update(orders).set({ status: "paid" });
  await Promise.all([announce("order.completed.slow"), announce("order.audit")]);
}

export async function main() {
  await completeOrder("1");
}

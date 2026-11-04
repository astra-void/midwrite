// Phase 5: `execute` awaits its parameter; at this call it is the inline
// callback, whose publication runs inside `execute` after the commit.
// `withRetry` runs a named function the same way.
// @expect MW1001 line=19 confidence=high function=completeOrder
// @expect MW1001 line=30 confidence=high function=refundOrder
import { db } from "@/db";
import { orders } from "@/db/schema";

async function execute(action: () => Promise<void>) {
  await action();
}

async function withRetry(operation: () => Promise<void>) {
  await operation();
}

export async function completeOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  await execute(async () => {
    await queue.publish("order.completed", { id });
  });
}

async function publishRefund() {
  await queue.publish("order.refunded", {});
}

export async function refundOrder(id: string) {
  await db.update(orders).set({ status: "refunded" });
  await withRetry(publishRefund);
}

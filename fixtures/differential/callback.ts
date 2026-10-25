// Differential test: the publication runs in a callback awaited by a
// wrapper, after the commit.
// @expect CP1001 line=12 confidence=high function=completeOrder
import { db, orders, queue } from "./runtime.ts";

async function execute(action: () => Promise<void>) {
  await action();
}

export async function completeOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  await execute(async () => {
    await queue.publish("order.completed", { id });
  });
}

export async function main() {
  await completeOrder("1");
}

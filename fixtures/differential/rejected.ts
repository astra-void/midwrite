// Differential test: on one path the helper throws after its commit, so the
// caller's publication never runs (the exception propagates out of the
// caller); on the other the publication follows the commit (MW1001, with
// the helper's branch as a path condition). The runtime run takes the
// throwing path.
// @expect MW1001 line=18 confidence=medium function=completeOrder
import { db, orders, queue } from "./runtime.ts";

async function save(id: string) {
  await db.update(orders).set({ status: "paid" });
  if (id === "reject") {
    throw new Error("rejected");
  }
}

export async function completeOrder(id: string) {
  await save(id);
  await queue.publish("order.completed", { id });
}

export async function main() {
  await completeOrder("reject").catch(() => {});
}

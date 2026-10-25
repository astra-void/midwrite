// Known false negative: only awaited (or `async`-returned) calls of same-file
// helpers are summarized. A call that is neither awaited nor returned is not
// followed, so the publish inside `notify` is not paired with the commit.
// @expect none
import { db } from "@/db";
import { orders } from "@/db/schema";

async function notify(id: string) {
  await queue.publish("order.completed", { id });
}

export async function completeOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  notify(id);
}

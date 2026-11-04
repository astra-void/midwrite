// Summaries are applied transitively (up to three calls deep): `completeOrder`
// reaches the publish through `finish` and `notify`. Two calls of the same
// helper are two independent external effects.
// @expect MW1001 line=21 confidence=high function=completeOrder
// @expect MW1005 line=26 confidence=high function=notifyBoth
import { db } from "@/db";
import { orders } from "@/db/schema";

function notify(id: string) {
  return queue.publish("order.completed", { id });
}

async function finish(id: string) {
  await notify(id);
}

export async function completeOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  // `finish` awaits `notify`, which returns the publish.
  // crash window
  await finish(id);
}

export async function notifyBoth(buyer: string, seller: string) {
  await finish(buyer);
  await finish(seller);
}

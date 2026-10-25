// The write and the publication are on mutually exclusive branches.
// @expect none
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function settle(id: string, offline: boolean) {
  if (offline) {
    await db.update(orders).set({ status: "queued" });
  } else {
    await queue.publish("order.settle", { id });
  }
}

export async function settleTernary(id: string, offline: boolean) {
  await (offline
    ? db.update(orders).set({ status: "queued" })
    : queue.publish("order.settle", { id }));
}

// Helpers whose summaries contain no commit or external effect add nothing:
// a read-only helper after a commit, and an `async` return of a helper that
// performs no recognized operation.
// @expect none
import { db } from "@/db";
import { orders } from "@/db/schema";

async function loadOrder(id: string) {
  return db.query.orders.findFirst({ where: (o, { eq }) => eq(o.id, id) });
}

function toDto(id: string) {
  return { id, status: "paid" };
}

export async function completeOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  const order = await loadOrder(id);
  return toDto(order?.id ?? id);
}

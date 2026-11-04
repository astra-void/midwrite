// An early return guards the whole sequence; once the write commits, the
// publication always follows.
// @expect MW1001 line=14 confidence=high
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function completeOrder(id: string) {
  const order = await db.query.orders.findFirst({ where: (o, { eq }) => eq(o.id, id) });
  if (!order) {
    return;
  }

  await db.update(orders).set({ status: "paid" });
  await queue.publish("order.completed", { id });
}

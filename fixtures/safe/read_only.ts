// Reads never commit anything, so they cannot leave partial state.
// @expect none
import { eq } from "drizzle-orm";
import { db } from "@/db";
import { orders, users } from "@/db/schema";

export async function getOrderSummary(id: string) {
  const order = await db.select().from(orders).where(eq(orders.id, id));
  const user = await db.query.users.findFirst({ where: eq(users.id, order[0].userId) });
  const count = await db.$count(orders);
  await queue.publish("order.viewed", { id });
  return { order, user, count };
}

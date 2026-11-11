// Anonymous callback positions must not become part of finding identity.
// @expect MW1001 line=7 confidence=high
import { db } from "@/db";

register(async (id: string) => {
  await db.update(orders).set({ status: "paid" });
  await queue.publish("order.paid", { id });
});

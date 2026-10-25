// Each path performs at most one independent effect.
// @expect none
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function handle(id: string, mode: "db" | "event") {
  if (mode === "db") {
    await db.update(orders).set({ status: "paid" });
    return;
  }
  await queue.publish("order.paid", { id });
}

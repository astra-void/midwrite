// An object service: its function properties are summarized at calls.
// @expect none
import { db } from "@/db";
import { orders } from "@/db/schema";

async function closeOrder(id: string) {
  await db.update(orders).set({ status: "closed" });
}

export const orderService = {
  async markPaid(id: string) {
    await db.update(orders).set({ status: "paid" });
  },
  close: closeOrder,
  label: "orders",
};

// Methods of classes and object literals are analyzed; names are qualified.
// @expect MW1001 line=10 confidence=high function=OrderService.complete
// @expect MW1002 line=17 confidence=high function=refund
import { db } from "@/db";
import { orders } from "@/db/schema";

export class OrderService {
  async complete(id: string) {
    await db.update(orders).set({ status: "paid" });
    await queue.publish("order.completed", { id });
  }
}

export const handlers = {
  async refund(id: string) {
    // The refund event is visible before the status change commits.
    await queue.publish("order.refunded", { id });
    await db.update(orders).set({ status: "refunded" });
  },
};

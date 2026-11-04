// Differential test: the operations run in methods reached through `this`.
// @expect MW1001 line=8 confidence=high function=OrderService.complete
import { db, orders, queue } from "./runtime.ts";

export class OrderService {
  async complete(id: string) {
    await this.save(id);
    await this.publish(id);
  }

  async save(_id: string) {
    await db.update(orders).set({ status: "paid" });
  }

  async publish(id: string) {
    await queue.publish("order.completed", { id });
  }
}

export async function main() {
  await new OrderService().complete("1");
}

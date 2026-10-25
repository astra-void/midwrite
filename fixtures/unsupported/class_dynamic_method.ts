// Phase 5: the method is replaced on the instance (`this.notify = ...`), so
// dispatch is dynamic and the call is not resolved.
// @expect CP9001 line=16 function=Service.complete
import { db } from "@/db";
import { orders } from "@/db/schema";

export class Service {
  constructor(fast: boolean) {
    if (fast) {
      this.notify = async () => {};
    }
  }

  async complete(id: string) {
    await db.update(orders).set({ status: "paid" });
    await this.notify(id);
  }

  async notify(id: string) {
    await queue.publish("order.completed", { id });
  }
}

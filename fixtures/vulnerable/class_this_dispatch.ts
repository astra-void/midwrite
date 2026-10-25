// Phase 5: `this.method()` is resolved through the receiver's class. No
// subclass in the analyzed files overrides `save` or `publish`, so both
// calls dispatch to `OrderService`'s own methods; the commit and the
// publication they perform are traced through them. (The receivers are
// configured member paths, so confidence is capped at medium.)
// @expect CP1001 line=17 confidence=medium function=OrderService.complete
import { orders } from "@/db/schema";

export class OrderService {
  constructor(
    private readonly db: Database,
    private readonly queue: Queue,
  ) {}

  async complete(id: string) {
    await this.save(id);
    await this.publish(id);
  }

  async save(id: string) {
    await this.db.update(orders).set({ status: "paid" });
  }

  async publish(id: string) {
    await this.queue.publish("order.completed", { id });
  }
}

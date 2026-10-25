// A transactional outbox through a configured member receiver: both writes use
// the transaction handle, so they commit atomically. Nothing to report.
// @expect none
import { orders, outbox } from "@/db/schema";

export class OrderService {
  constructor(private readonly db: any) {}

  async complete(id: string) {
    await this.db.transaction(async (tx) => {
      await tx.update(orders).set({ status: "paid" });
      await tx.insert(outbox).values({ topic: "order.completed", payload: { id } });
    });
  }
}

// Receivers reached through members are analyzed when the configuration names
// their path: fixtures/commitrift.json lists `this.db`, `ctx.db` and
// `this.queue.publish`. The evidence says the match is member-path based, and
// confidence is capped at medium because only the names along the path are
// checked (Phase 2; Phase 1 reported these as high).
// @expect CP1001 line=18 confidence=medium function=OrderService.complete
// @expect CP1003 line=25 confidence=medium function=OrderService.completeInTransaction
// @expect CP1004 line=33 confidence=medium function=OrderService.audit
// @expect CP1001 line=44 confidence=medium function=completeOrderProcedure
import { orders, auditLog } from "@/db/schema";

export class OrderService {
  constructor(private readonly db: any, private readonly queue: any) {}

  async complete(id: string) {
    await this.db.update(orders).set({ status: "paid" });
    // crash window
    await this.queue.publish("order.completed", { id });
  }

  async completeInTransaction(id: string) {
    await this.db.transaction(async (tx) => {
      await tx.update(orders).set({ status: "paid" });
      // visible before the transaction commits
      await this.queue.publish("order.completed", { id });
    });
  }

  async audit(id: string) {
    await this.db.transaction(async (tx) => {
      await tx.update(orders).set({ status: "audited" });
      // `this.db`, not `tx`: commits on its own
      await this.db.insert(auditLog).values({ id });
    });
  }
}

// A tRPC-style procedure: the database handle lives on the request context.
export async function completeOrderProcedure({ ctx, input }: { ctx: any; input: { id: string } }) {
  await ctx.db
    .update(orders)
    .set({ status: "paid" });
  // crash window
  await queue.publish("order.completed", { id: input.id });
}

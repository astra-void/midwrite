// Member receivers whose path is not configured are reported, never guessed:
// `this.repo.db` and `deps.queue` resemble the configured `db` and `queue`, but
// only the exact configured paths (`this.db`, `ctx.db`, `this.queue`) match.
// @expect CP9001 line=14
// @expect CP9001 line=15
// @expect CP9001 line=19
import { orders } from "@/db/schema";

export class OrderService {
  constructor(private readonly repo: any) {}

  async complete(id: string, deps: any) {
    // Neither call is analyzed.
    await this.repo.db.update(orders).set({ status: "paid" });
    await deps.queue.publish("order.completed", { id });
  }

  async notify(id: string) {
    await this.mailer.send({ to: id });
  }
}

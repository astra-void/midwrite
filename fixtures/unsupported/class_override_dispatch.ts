// Phase 5: `this.notify()` inside `Base` may run `Base.notify` or the
// override in `Loud`: several targets, so the call is not resolved and
// stays an unknown call after the commit (never guessed by name).
// @expect MW9001 line=11 function=Base.complete
import { db } from "@/db";
import { orders } from "@/db/schema";

class Base {
  async complete(id: string) {
    await db.update(orders).set({ status: "paid" });
    await this.notify(id);
  }

  async notify(id: string) {
    await mailer.send({ id });
  }
}

export class Loud extends Base {
  async notify(id: string) {
    await queue.publish("order.completed", { id });
  }
}

// Methods of the imported object service are followed; a property that is
// not a function of the module, and methods of other objects, stay unknown.
// @expect MW1001 line=11 confidence=high function=pay
// @expect MW1001 line=16 confidence=high function=close
// @expect MW9001 line=21 function=other
import { orderService } from "./services/orders";
import { cache } from "./services/cache";

export async function pay(id: string) {
  await orderService.markPaid(id);
  await queue.publish("order.paid", { id });
}

export async function close(id: string) {
  await orderService.close(id);
  await queue.publish("order.closed", { id });
}

export async function other(id: string) {
  await db.update(orders).set({ id });
  await orderService.refund(id);
  await cache.clear();
}

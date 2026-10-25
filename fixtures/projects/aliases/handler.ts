// Named import with an alias, and a default import.
// @expect CP1001 line=9 confidence=high function=pay
// @expect CP1002 line=13 confidence=high function=cancel
import { recordPayment as record } from "./payments";
import undo from "./payments";

export async function pay(id: string) {
  await record(id);
  await queue.publish("payment.recorded", { id });
}

export async function cancel(id: string) {
  await queue.publish("payment.refunding", { id });
  await undo(id);
}

// The write inside the imported `markPaid` commits before the publish.
// @expect CP1001 line=9 confidence=high function=checkout
import { markPaid } from "../services/order";
import { queue } from "@/lib/queue";

export async function checkout(id: string) {
  await markPaid(id);
  // A crash here leaves the order paid without the event.
  await queue.publish("order.paid", { id });
}

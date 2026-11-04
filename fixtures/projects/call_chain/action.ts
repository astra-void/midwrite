// Action -> service -> repository -> database write, then a publish.
// @expect MW1001 line=7 confidence=high function=completeAction
import { completeOrder } from "./service";

export async function completeAction(id: string) {
  await completeOrder(id);
  await queue.publish("order.completed", { id });
}

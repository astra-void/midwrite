// `@app/lib/orders` is resolved through the tsconfig `paths` alias.
// @expect MW1001 line=7 confidence=high function=POST
import { markShipped } from "@app/lib/orders";

export async function POST(id: string) {
  await markShipped(id);
  await queue.publish("order.shipped", { id });
}

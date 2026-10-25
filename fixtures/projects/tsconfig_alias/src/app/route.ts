// `@app/lib/orders` is resolved through the tsconfig `paths` alias.
// @expect CP1001 line=7 confidence=high function=POST
import { markShipped } from "@app/lib/orders";

export async function POST(id: string) {
  await markShipped(id);
  await queue.publish("order.shipped", { id });
}

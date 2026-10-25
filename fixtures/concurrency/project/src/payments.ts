import { db } from "./db";
import { payments } from "./schema";
import { chargeProvider } from "./events";

// A payment request: charge the card, then record it. The request id is
// passed to the provider as an idempotency key (argument 0 of
// chargeProvider). Whether that key actually deduplicates is NOT visible in
// this code: the scenario must say who enforces it (channels.*.dedup).
export async function pay(requestId: string) {
  await chargeProvider(requestId, 10);
  await db.insert(payments).values({ id: requestId, request: requestId, amount: 10 });
}

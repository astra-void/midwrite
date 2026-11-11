import { sql } from "drizzle-orm";

import { db } from "../db.js";
import { assertPositiveAmount, backoffMs, delay, isConnectionError } from "../lib/pg-errors.js";
import { paymentReceipts, payments } from "../schema/index.js";

const MAX_ATTEMPTS = 3;

// P16-E09: records a card payment for an order; retried up to three times
// when the connection fails.
export async function recordPayment(orderId: number, amountCents: number, requestId: string) {
  assertPositiveAmount(amountCents);
  for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
    try {
      const [payment] = await db
        .insert(payments)
        .values({ orderId, requestId, amountCents })
        .returning({ id: payments.id });
      if (!payment) throw new Error("insert returned no row");
      return payment.id;
    } catch (error) {
      if (!isConnectionError(error) || attempt === MAX_ATTEMPTS - 1) throw error;
      await delay(backoffMs(attempt));
    }
  }
  throw new Error("unreachable");
}

// P16-E10: the same, written as an upsert keyed by the request id.
export async function recordPaymentIdempotent(orderId: number, amountCents: number, requestId: string) {
  assertPositiveAmount(amountCents);
  for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
    try {
      const [receipt] = await db
        .insert(paymentReceipts)
        .values({ orderId, requestId, amountCents })
        .onConflictDoUpdate({
          target: paymentReceipts.requestId,
          set: { amountCents: sql`excluded.amount_cents` },
        })
        .returning({ id: paymentReceipts.id });
      if (!receipt) throw new Error("upsert returned no row");
      return receipt.id;
    } catch (error) {
      if (!isConnectionError(error) || attempt === MAX_ATTEMPTS - 1) throw error;
      await delay(backoffMs(attempt));
    }
  }
  throw new Error("unreachable");
}

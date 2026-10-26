// Recording a payment. A client that gets no answer (the server crashed, the
// connection dropped while the COMMIT was in flight) submits the same request
// again, so each function must cope with a COMMIT whose outcome it never
// learned. They differ in how.
import { eq } from "drizzle-orm";

import { db } from "../lib/db/drizzle.js";
import { ledger, orders, payments } from "../lib/db/schema.js";
import { publishConfirmation } from "../lib/messaging/confirmations.js";

export interface PayInput {
  requestId: string;
  orderId: number;
  amount: number;
}

/** Blind: every submission records a payment, then confirms. */
export async function recordPayment(input: PayInput) {
  await db.transaction(async (tx) => {
    await tx.insert(payments).values({
      requestId: input.requestId,
      orderId: input.orderId,
      amount: input.amount,
    });
  });
  await publishConfirmation({ orderId: input.orderId, requestId: input.requestId });
  return { status: "recorded" };
}

/**
 * Idempotency key: the database allows one ledger row per request id, and a
 * repeated insert does nothing.
 */
export async function recordPaymentIdempotent(input: PayInput) {
  await db.transaction(async (tx) => {
    await tx
      .insert(ledger)
      .values({ requestId: input.requestId, orderId: input.orderId, amount: input.amount })
      .onConflictDoNothing();
  });
  await publishConfirmation({ orderId: input.orderId, requestId: input.requestId });
  return { status: "recorded" };
}

/**
 * Reconciliation: look for the request in the table first and record the
 * payment only when it is not there. The read happens before the insert,
 * and nothing makes the two atomic.
 */
export async function recordPaymentReconciled(input: PayInput) {
  const [existing] = await db
    .select()
    .from(payments)
    .where(eq(payments.requestId, input.requestId))
    .limit(1);
  if (existing) {
    return { status: "already-recorded" };
  }
  await db.transaction(async (tx) => {
    await tx.insert(payments).values({
      requestId: input.requestId,
      orderId: input.orderId,
      amount: input.amount,
    });
  });
  return { status: "recorded" };
}

/**
 * Outside a transaction: record the payment in the ledger, then mark the
 * order paid. Each write commits by itself, so the acknowledgment of the
 * second one can be lost while the write itself is durable.
 */
export async function settleOrder(input: PayInput) {
  await db
    .insert(ledger)
    .values({ requestId: input.requestId, orderId: input.orderId, amount: input.amount })
    .onConflictDoNothing();
  await db.update(orders).set({ status: "paid" }).where(eq(orders.id, input.orderId));
  return { status: "settled" };
}

/** Blind autocommit: every submission records a payment. */
export async function recordPaymentAutocommit(input: PayInput) {
  await db.insert(payments).values({
    requestId: input.requestId,
    orderId: input.orderId,
    amount: input.amount,
  });
  return { status: "recorded" };
}

// Checkout: charge the order's amount and mark the order paid. A client that
// gets no response submits the same request again.
import { getOrder, markPaid, recordCharge } from "../repositories/order-repository.js";
import { chargeCard } from "../services/payments.js";

export interface CheckoutRequest {
  requestId: string;
  orderId: number;
}

/** Charges, then records the charge and marks the order paid. */
export async function checkout(input: CheckoutRequest) {
  const order = await getOrder(input.orderId);
  if (!order) {
    throw new Error("no such order");
  }
  if (order.status === "paid") {
    return { status: "already-paid" };
  }
  await chargeCard({ orderId: order.id, amount: order.amount });
  await recordCharge(order.id, order.amount);
  await markPaid(order.id);
  return { status: "paid" };
}

/** The same checkout, sending the client's request id as idempotency key. */
export async function checkoutIdempotent(input: CheckoutRequest) {
  const order = await getOrder(input.orderId);
  if (!order) {
    throw new Error("no such order");
  }
  if (order.status === "paid") {
    return { status: "already-paid" };
  }
  await chargeCard({ orderId: order.id, amount: order.amount }, input.requestId);
  await recordCharge(order.id, order.amount);
  await markPaid(order.id);
  return { status: "paid" };
}

// Two configured external effects in sequence.
// @expect CP1005 line=9 confidence=high
import { queue } from "@/lib/queue";
import { mailer } from "@/lib/mailer";

export async function notifyShipment(orderId: string, email: string) {
  await queue.publish("order.shipped", { orderId });
  // A crash here leaves the event published but the email unsent.
  await mailer.send({ to: email, subject: "Your order has shipped" });
}

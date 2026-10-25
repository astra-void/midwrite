// A payment before the database commit, and an email after it.
// `stripe.charges` is a member-path match (only the names are checked), so
// the CP1002 finding that relies on it is capped at medium confidence.
// @expect CP1002 line=11 confidence=medium
// @expect CP1001 line=13 confidence=high
import { db } from "@/db";
import { orders } from "@/db/schema";
import { sendEmail } from "@/lib/email";

export async function checkout(orderId: string, amountCents: number, email: string) {
  await stripe.charges.create({ amount: amountCents, currency: "usd" });
  await db.update(orders).set({ status: "paid" });
  await sendEmail(email, "Payment received");
}

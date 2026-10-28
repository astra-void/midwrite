import { eq } from "drizzle-orm";

import { replicatedDb } from "../db.js";
import { invoices, postings } from "../schema/index.js";

// P14-E16: called by the billing cron for each due invoice. Reads the invoice
// through the replica set; if it is still a draft, posts it to the ledger and
// marks it final in one transaction (on the primary).
export async function finalizeInvoice(invoiceId: string) {
  const [invoice] = await replicatedDb.select().from(invoices).where(eq(invoices.id, invoiceId)).limit(1);
  if (!invoice || invoice.status !== "draft") return false;
  await replicatedDb.transaction(async (tx) => {
    await tx.insert(postings).values({ invoiceId, amount: invoice.total });
    await tx.update(invoices).set({ status: "final" }).where(eq(invoices.id, invoiceId));
  });
  return true;
}

// P14-E17: the same, with the check read from the primary.
export async function finalizeInvoicePrimary(invoiceId: string) {
  const [invoice] = await replicatedDb.$primary.select().from(invoices).where(eq(invoices.id, invoiceId)).limit(1);
  if (!invoice || invoice.status !== "draft") return false;
  await replicatedDb.transaction(async (tx) => {
    await tx.insert(postings).values({ invoiceId, amount: invoice.total });
    await tx.update(invoices).set({ status: "final" }).where(eq(invoices.id, invoiceId));
  });
  return true;
}

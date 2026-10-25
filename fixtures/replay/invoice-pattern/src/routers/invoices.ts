// Shape of the "create" branch of midday's REST invoice router.
import { db } from "../../../shared/db.ts";
import { triggerJob } from "../jobs.ts";
import { updateInvoice } from "../queries/invoices.ts";

export async function finalizeInvoice(input: { id: string; deliveryType: string }) {
  await updateInvoice(db, { id: input.id, status: "unpaid" });

  await triggerJob(
    "generate-invoice",
    { invoiceId: input.id, deliveryType: input.deliveryType },
    "invoices",
  );
}

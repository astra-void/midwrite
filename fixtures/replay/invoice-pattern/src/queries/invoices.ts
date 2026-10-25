// Shape of midday's packages/db/src/queries/invoices.ts `updateInvoice`:
// a query helper that writes on whatever database handle it is given.
import { eq } from "../../../shared/db.ts";
import { invoices } from "../schema.ts";

type Database = { update: (table: typeof invoices) => any };

export async function updateInvoice(
  db: Database,
  params: { id: string; status: string },
) {
  const { id, ...rest } = params;
  await db.update(invoices).set(rest).where(eq(invoices.id, id));
}

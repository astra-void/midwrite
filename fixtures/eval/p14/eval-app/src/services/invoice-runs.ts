import { eq } from "drizzle-orm";

import { db } from "../db.js";
import { invoiceRuns } from "../schema/index.js";

// P14-E05: schedules the invoice run of an account for a period ("2026-10"),
// unless one is already scheduled. invoice_runs.run_key is not unique.
export async function scheduleInvoiceRun(accountId: string, period: string) {
  if (!period.startsWith("20") || period.length !== 7) throw new Error(`bad period: ${period}`);
  const runKey = `invoice:${accountId}:${period}`;
  const [existing] = await db
    .select({ id: invoiceRuns.id })
    .from(invoiceRuns)
    .where(eq(invoiceRuns.runKey, runKey))
    .limit(1);
  if (existing) return existing.id;
  const [created] = await db
    .insert(invoiceRuns)
    .values({ accountId, runKey, period })
    .returning({ id: invoiceRuns.id });
  if (!created) throw new Error("invoice run insert failed");
  return created.id;
}

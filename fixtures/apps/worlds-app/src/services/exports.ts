import { and, eq, gt, sql } from "drizzle-orm";

import { db } from "../db/client.js";
import { exportJobs, quotas } from "../db/schema.js";

// Starts an export if the account has exports left. Defect: the quota is
// tested on the value read, then decremented; with one export left two
// concurrent calls both pass the test and the quota ends at -1.
export async function startExport(accountId: number) {
  const [quota] = await db
    .select({ remaining: quotas.remaining })
    .from(quotas)
    .where(eq(quotas.accountId, accountId))
    .limit(1);
  if (!quota || quota.remaining <= 0) throw new Error("export quota exhausted");
  await db
    .update(quotas)
    .set({ remaining: sql`${quotas.remaining} - 1` })
    .where(eq(quotas.accountId, accountId));
  const [job] = await db.insert(exportJobs).values({ accountId }).returning({ id: exportJobs.id });
  return job?.id ?? null;
}

// The same with the test in the UPDATE's WHERE clause: safe.
export async function startExportAtomic(accountId: number) {
  const taken = await db
    .update(quotas)
    .set({ remaining: sql`${quotas.remaining} - 1` })
    .where(and(eq(quotas.accountId, accountId), gt(quotas.remaining, 0)))
    .returning({ accountId: quotas.accountId });
  if (taken.length === 0) throw new Error("export quota exhausted");
  const [job] = await db.insert(exportJobs).values({ accountId }).returning({ id: exportJobs.id });
  return job?.id ?? null;
}

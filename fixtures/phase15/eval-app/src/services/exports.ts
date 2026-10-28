import { and, eq, gt, sql } from "drizzle-orm";

import { db } from "../db.js";
import { exportJobs, quotas } from "../schema/index.js";

// P15-E12: starts an export if the account has exports left this period.
export async function startExport(accountId: string, format: string) {
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
  const [job] = await db.insert(exportJobs).values({ accountId, format }).returning({ id: exportJobs.id });
  return job?.id;
}

// P15-E13: the same; the quota test is in the UPDATE's WHERE, and the job is
// created in the same transaction.
export async function startExportAtomic(accountId: string, format: string) {
  return db.transaction(async (tx) => {
    const taken = await tx
      .update(quotas)
      .set({ remaining: sql`${quotas.remaining} - 1` })
      .where(and(eq(quotas.accountId, accountId), gt(quotas.remaining, 0)))
      .returning({ accountId: quotas.accountId });
    if (taken.length === 0) throw new Error("export quota exhausted");
    const [job] = await tx.insert(exportJobs).values({ accountId, format }).returning({ id: exportJobs.id });
    return job?.id;
  });
}

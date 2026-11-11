import { eq, sql } from "drizzle-orm";

import { db } from "../db.js";
import { usage } from "../schema/index.js";

// P14-E31: adds metered units: reads the counter and writes the new total in
// one (READ COMMITTED) transaction.
export async function recordUsage(accountId: string, units: number) {
  await db.transaction(async (tx) => {
    const [row] = await tx.select({ used: usage.used }).from(usage).where(eq(usage.accountId, accountId)).limit(1);
    if (!row) throw new Error("no usage row");
    await tx
      .update(usage)
      .set({ used: row.used + units })
      .where(eq(usage.accountId, accountId));
  });
}

// P14-E32: the increment is computed by the database.
export async function recordUsageAtomic(accountId: string, units: number) {
  const updated = await db
    .update(usage)
    .set({ used: sql`${usage.used} + ${units}` })
    .where(eq(usage.accountId, accountId))
    .returning({ accountId: usage.accountId });
  if (updated.length === 0) throw new Error("no usage row");
}

// P14-E33: the read and the write of P14-E31 in a REPEATABLE READ transaction.
export async function recordUsageRepeatable(accountId: string, units: number) {
  await db.transaction(
    async (tx) => {
      const [row] = await tx.select({ used: usage.used }).from(usage).where(eq(usage.accountId, accountId)).limit(1);
      if (!row) throw new Error("no usage row");
      await tx
        .update(usage)
        .set({ used: row.used + units })
        .where(eq(usage.accountId, accountId));
    },
    { isolationLevel: "repeatable read" },
  );
}

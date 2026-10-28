import { eq } from "drizzle-orm";

import { db } from "../../../../db.js";
import { withAuth } from "../../../../lib/auth.js";
import { accounts, trials } from "../../../../schema/index.js";

// P14-E15: POST /api/v2/trials, the same with the account row locked for the
// check.
export const POST = withAuth(async (ctx) =>
  db.transaction(async (tx) => {
    const [account] = await tx
      .select({ id: accounts.id })
      .from(accounts)
      .where(eq(accounts.id, ctx.accountId))
      .limit(1)
      .for("update");
    if (!account) throw new Error("unknown account");
    const [existing] = await tx
      .select({ id: trials.id })
      .from(trials)
      .where(eq(trials.accountId, ctx.accountId))
      .limit(1);
    if (existing) return { started: false, trialId: existing.id };
    const [trial] = await tx.insert(trials).values({ accountId: ctx.accountId, plan: "pro" }).returning({ id: trials.id });
    return { started: true, trialId: trial?.id };
  }),
);

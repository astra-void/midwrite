import { eq } from "drizzle-orm";

import { db } from "../../../db.js";
import { withAuth } from "../../../lib/auth.js";
import { trials } from "../../../schema/index.js";

// P14-E14: POST /api/trials starts the signed-in account's free trial.
export const POST = withAuth(async (ctx) => {
  const [existing] = await db
    .select({ id: trials.id })
    .from(trials)
    .where(eq(trials.accountId, ctx.accountId))
    .limit(1);
  if (existing) return { started: false, trialId: existing.id };
  const [trial] = await db.insert(trials).values({ accountId: ctx.accountId, plan: "pro" }).returning({ id: trials.id });
  return { started: true, trialId: trial?.id };
});

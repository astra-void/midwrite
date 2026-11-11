import { eq } from "drizzle-orm";

import { db } from "../db.js";
import { accounts, onboardingCalls } from "../schema/index.js";

// P14-E28: pro accounts may book one onboarding call. onboarding_calls has
// no unique constraint.
export async function requestOnboardingCall(accountId: string) {
  const [account] = await db
    .select({ settings: accounts.settings })
    .from(accounts)
    .where(eq(accounts.id, accountId))
    .limit(1);
  if (!account) throw new Error("unknown account");
  if (account.settings.plan === "pro") {
    const [existing] = await db
      .select({ id: onboardingCalls.id })
      .from(onboardingCalls)
      .where(eq(onboardingCalls.accountId, accountId))
      .limit(1);
    if (existing) return { scheduled: true, callId: existing.id };
    const [call] = await db.insert(onboardingCalls).values({ accountId }).returning({ id: onboardingCalls.id });
    return { scheduled: true, callId: call?.id };
  }
  return { scheduled: false };
}

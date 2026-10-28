import { db } from "../db.js";
import { withRetry } from "../lib/retry.js";
import { credits } from "../schema/index.js";

const SIGNUP_CREDIT = 500;

// P14-E12: grants the signup credit, retried on a dropped connection or a
// timeout.
export async function grantSignupCredit(accountId: string) {
  await withRetry(async () => {
    await db.insert(credits).values({ accountId, reason: "signup", amount: SIGNUP_CREDIT });
  });
}

// P14-E13: the same, with an idempotency key (credits.grant_key is unique).
export async function grantSignupCreditOnce(accountId: string) {
  await withRetry(async () => {
    await db
      .insert(credits)
      .values({ accountId, reason: "signup", amount: SIGNUP_CREDIT, grantKey: `signup:${accountId}` })
      .onConflictDoNothing({ target: credits.grantKey });
  });
}

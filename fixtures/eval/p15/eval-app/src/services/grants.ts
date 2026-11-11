import { db } from "../db.js";
import { withRetry } from "../lib/retry.js";
import { grants } from "../schema/index.js";

// P15-E24: grants a credit, with an optional idempotency key, retried on a
// dropped connection or a timeout.
export async function grantCredit(accountId: string, reason: string, amount: number, grantKey?: string) {
  await withRetry(async () => {
    await db
      .insert(grants)
      .values({ accountId, reason, amount, grantKey: grantKey ?? null })
      .onConflictDoNothing({ target: grants.grantKey });
  });
}

// P15-E25: the same; the idempotency key is required.
export async function grantCreditKeyed(accountId: string, reason: string, amount: number, grantKey: string) {
  await withRetry(async () => {
    await db
      .insert(grants)
      .values({ accountId, reason, amount, grantKey })
      .onConflictDoNothing({ target: grants.grantKey });
  });
}

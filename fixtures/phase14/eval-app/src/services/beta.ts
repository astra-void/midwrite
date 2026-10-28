import { eq } from "drizzle-orm";

import { RELEASE } from "../config.js";
import { db } from "../db.js";
import { betaSignups, waitlist } from "../schema/index.js";

// P14-E07: beta builds enroll the account at once; stable builds put it on
// the waitlist (one row per account).
export async function joinBetaProgram(accountId: string) {
  const [channel] = RELEASE.split("/");
  if (channel === "beta") {
    const [enrolled] = await db
      .select({ id: betaSignups.id })
      .from(betaSignups)
      .where(eq(betaSignups.accountId, accountId))
      .limit(1);
    if (!enrolled) await db.insert(betaSignups).values({ accountId });
    return { status: "enrolled" as const };
  }
  await db.insert(waitlist).values({ accountId }).onConflictDoNothing({ target: waitlist.accountId });
  return { status: "waitlisted" as const };
}

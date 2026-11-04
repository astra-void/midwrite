import { eq } from "drizzle-orm";

import { readScaled } from "../db.js";
import { digestPreferences, type DigestFrequency } from "../schema/index.js";

// P16-E34: saves the digest frequency and tells the settings page whether
// the saved value is now in effect (the page shows "saved" or "not saved,
// try again").
export async function saveDigestFrequency(accountId: string, frequency: DigestFrequency): Promise<boolean> {
  await readScaled
    .update(digestPreferences)
    .set({ frequency })
    .where(eq(digestPreferences.accountId, accountId));
  const [current] = await readScaled
    .select({ frequency: digestPreferences.frequency })
    .from(digestPreferences)
    .where(eq(digestPreferences.accountId, accountId))
    .limit(1);
  if (!current) throw new Error("no digest preferences");
  return current.frequency === frequency;
}

// P16-E35: the same, with the write and the read-back in one transaction.
export async function saveDigestFrequencyInTransaction(
  accountId: string,
  frequency: DigestFrequency,
): Promise<boolean> {
  return readScaled.transaction(async (tx) => {
    await tx.update(digestPreferences).set({ frequency }).where(eq(digestPreferences.accountId, accountId));
    const [current] = await tx
      .select({ frequency: digestPreferences.frequency })
      .from(digestPreferences)
      .where(eq(digestPreferences.accountId, accountId))
      .limit(1);
    if (!current) throw new Error("no digest preferences");
    return current.frequency === frequency;
  });
}

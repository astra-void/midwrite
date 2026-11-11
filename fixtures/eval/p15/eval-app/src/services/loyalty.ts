import { eq } from "drizzle-orm";

import { db } from "../db.js";
import { ledger, processedEvents } from "../schema/index.js";

// P15-E26: applies a loyalty event from the message bus (at-least-once
// delivery), unless its event id was already processed.
export async function applyLoyaltyEvent(accountId: string, eventId: string, points: number) {
  const [seen] = await db
    .select({ eventId: processedEvents.eventId })
    .from(processedEvents)
    .where(eq(processedEvents.eventId, eventId))
    .limit(1);
  if (seen) return;
  await db.insert(ledger).values({ accountId, eventId, points });
  await db.insert(processedEvents).values({ eventId }).onConflictDoNothing();
}

// P15-E27: records the event id first, in the same transaction as the ledger
// row.
export async function applyLoyaltyEventOnce(accountId: string, eventId: string, points: number) {
  await db.transaction(async (tx) => {
    const claimed = await tx
      .insert(processedEvents)
      .values({ eventId })
      .onConflictDoNothing()
      .returning({ eventId: processedEvents.eventId });
    if (claimed.length === 0) return;
    await tx.insert(ledger).values({ accountId, eventId, points });
  });
}

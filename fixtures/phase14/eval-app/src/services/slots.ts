import { and, eq, isNull } from "drizzle-orm";

import { db } from "../db.js";
import { slots } from "../schema/index.js";

// P14-E18: reads the slot, reserves it if it is free, tells the caller
// whether it got it.
export async function reserveSlot(slotId: number, accountId: string) {
  const [slot] = await db.select().from(slots).where(eq(slots.id, slotId)).limit(1);
  if (!slot || slot.reservedBy !== null) return false;
  await db.update(slots).set({ reservedBy: accountId }).where(eq(slots.id, slotId));
  return true;
}

// P14-E19: one conditional UPDATE ... RETURNING decides.
export async function reserveSlotAtomic(slotId: number, accountId: string) {
  const reserved = await db
    .update(slots)
    .set({ reservedBy: accountId })
    .where(and(eq(slots.id, slotId), isNull(slots.reservedBy)))
    .returning({ id: slots.id });
  return reserved.length > 0;
}

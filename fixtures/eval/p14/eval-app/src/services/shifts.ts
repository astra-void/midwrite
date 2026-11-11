import { eq } from "drizzle-orm";

import { db } from "../db.js";
import { shifts } from "../schema/index.js";

// P14-E20: reads the shift, gives it to the worker if nobody has it, tells
// the worker whether it got it.
export async function claimShift(shiftId: number, workerId: string) {
  const [shift] = await db.select().from(shifts).where(eq(shifts.id, shiftId)).limit(1);
  if (!shift || shift.workerId !== null) return false;
  await db.update(shifts).set({ workerId }).where(eq(shifts.id, shiftId));
  return true;
}
